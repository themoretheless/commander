//! Sparse-file byte-copy primitives.
//!
//! Detects holey files and copies data extents without materializing holes.
//! Buffered and parallel directory paths call into this module when sparse
//! preservation is enabled.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::TransferState;
use super::buffered::COPY_BUF_SIZE;

#[cfg(unix)]
pub(super) fn is_sparse_file(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    path.metadata().is_ok_and(|metadata| {
        metadata.is_file()
            && metadata.len() > 0
            && metadata.blocks().saturating_mul(512) < metadata.len()
    })
}

#[cfg(not(unix))]
pub(super) fn is_sparse_file(_path: &Path) -> bool {
    false
}

#[cfg(unix)]
pub(super) fn copy_file_sparse(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
) -> std::io::Result<u64> {
    use std::os::fd::AsRawFd;

    let mut reader = std::fs::File::open(src)?;
    let file_size = reader.metadata()?.len();
    let mut writer = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)?;
    writer.set_len(file_size)?;
    {
        let mut progress = crate::lock_util::recover(state);
        progress.current_file = src
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        progress.current_file_size = file_size;
        progress.current_file_copied = 0;
    }
    let mut cursor = 0_u64;
    let mut reported = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUF_SIZE];
    while cursor < file_size {
        if crate::lock_util::recover(state).cancelled {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }
        let data = unsafe {
            libc::lseek(
                reader.as_raw_fd(),
                cursor.try_into().map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "file offset overflow")
                })?,
                libc::SEEK_DATA,
            )
        };
        if data < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENXIO) {
                break;
            }
            return Err(error);
        }
        let hole = unsafe { libc::lseek(reader.as_raw_fd(), data, libc::SEEK_HOLE) };
        if hole < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let data = data as u64;
        let hole = (hole as u64).min(file_size);
        reader.seek(SeekFrom::Start(data))?;
        writer.seek(SeekFrom::Start(data))?;
        let mut extent_offset = data;
        while extent_offset < hole {
            let wanted = (hole - extent_offset).min(buffer.len() as u64) as usize;
            let read = reader.read(&mut buffer[..wanted])?;
            if read == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "sparse extent ended early",
                ));
            }
            writer.write_all(&buffer[..read])?;
            limiter.consume(read, || crate::lock_util::recover(state).cancelled)?;
            extent_offset = extent_offset.saturating_add(read as u64);
            let advanced = extent_offset.saturating_sub(reported);
            reported = extent_offset;
            let mut progress = crate::lock_util::recover(state);
            progress.current_file_copied = extent_offset;
            progress.copied_bytes = progress.copied_bytes.saturating_add(advanced);
            progress.maybe_sample();
        }
        cursor = hole;
    }
    writer.sync_data()?;
    if let Ok(metadata) = src.metadata() {
        std::fs::set_permissions(dst, metadata.permissions())?;
    }
    let mut progress = crate::lock_util::recover(state);
    progress.copied_bytes = progress
        .copied_bytes
        .saturating_add(file_size.saturating_sub(reported));
    progress.current_file_copied = file_size;
    Ok(file_size)
}

#[cfg(not(unix))]
pub(super) fn copy_file_sparse(
    _src: &Path,
    _dst: &Path,
    _state: &TransferState,
    _limiter: &mut crate::transfer_tuning::BandwidthLimiter,
) -> std::io::Result<u64> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "sparse copy is unavailable",
    ))
}
