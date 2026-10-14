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
pub(super) const fn is_sparse_file(_path: &Path) -> bool {
    false
}

/// Granularity of hole preservation: an all-zero block of this size is
/// skipped instead of written, leaving a hole in the destination.
#[cfg(unix)]
const HOLE_BLOCK: usize = 4 * 1024;

/// Copy `src` to `dst`, leaving all-zero blocks as holes.
///
/// Every byte of the source is read, so correctness never depends on
/// `SEEK_DATA`/`SEEK_HOLE`, which APFS can report inaccurately for recently
/// written files; reading a real hole costs no disk I/O.
#[cfg(unix)]
pub(super) fn copy_file_sparse(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
) -> std::io::Result<u64> {
    let mut reader = std::fs::File::open(src)?;
    let file_size = reader.metadata()?.len();
    let mut writer = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)?;
    writer.set_len(file_size)?;
    {
        crate::lock_util::recover(state).begin_file(src, file_size);
    }
    let mut offset = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUF_SIZE];
    while offset < file_size {
        crate::transfer::check_cancelled(state)?;
        let wanted = (file_size - offset).min(buffer.len() as u64) as usize;
        let read = reader.read(&mut buffer[..wanted])?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "sparse source ended early",
            ));
        }
        for (index, block) in buffer[..read].chunks(HOLE_BLOCK).enumerate() {
            if block.iter().all(|&byte| byte == 0) {
                continue;
            }
            writer.seek(SeekFrom::Start(offset + (index * HOLE_BLOCK) as u64))?;
            writer.write_all(block)?;
        }
        limiter.consume(read, || crate::lock_util::recover(state).cancelled)?;
        offset = offset.saturating_add(read as u64);
        let mut progress = crate::lock_util::recover(state);
        progress.current_file_copied = offset;
        progress.copied_bytes = progress.copied_bytes.saturating_add(read as u64);
        progress.maybe_sample();
    }
    writer.sync_data()?;
    if let Ok(metadata) = src.metadata() {
        std::fs::set_permissions(dst, metadata.permissions())?;
    }
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
