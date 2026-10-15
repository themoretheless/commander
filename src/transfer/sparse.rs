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
                format!("sparse source ended early: {}", src.display()),
            ));
        }
        // One seek + write per run of consecutive data blocks, not per block.
        let mut run: Option<(usize, usize)> = None;
        for (index, block) in buffer[..read].chunks(HOLE_BLOCK).enumerate() {
            let start = index * HOLE_BLOCK;
            if block.iter().all(|&byte| byte == 0) {
                if let Some((from, to)) = run.take() {
                    write_run(&mut writer, offset, from, &buffer[from..to])?;
                }
                continue;
            }
            let end = start + block.len();
            run = Some(run.map_or((start, end), |(from, _)| (from, end)));
        }
        if let Some((from, to)) = run {
            write_run(&mut writer, offset, from, &buffer[from..to])?;
        }
        limiter.consume(read, || crate::lock_util::recover(state).cancelled)?;
        offset = offset.saturating_add(read as u64);
        let mut progress = crate::lock_util::recover(state);
        progress.current_file_copied = offset;
        progress.copied_bytes = progress.copied_bytes.saturating_add(read as u64);
        progress.maybe_sample();
    }
    writer.sync_data()?;
    // Best effort, like the buffered copy: a refused chmod must not fail a
    // copy whose bytes all landed.
    if let Ok(metadata) = src.metadata() {
        let _ = std::fs::set_permissions(dst, metadata.permissions());
    }
    Ok(file_size)
}

/// Write one run of data blocks at `chunk_offset + from` in the destination.
#[cfg(unix)]
fn write_run(
    writer: &mut std::fs::File,
    chunk_offset: u64,
    from: usize,
    bytes: &[u8],
) -> std::io::Result<()> {
    writer.seek(SeekFrom::Start(chunk_offset.saturating_add(from as u64)))?;
    writer.write_all(bytes)
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use crate::transfer::TransferProgress;
    use std::sync::{Arc, Mutex};

    #[test]
    fn sparse_copy_keeps_mixed_runs_byte_exact_and_holey() {
        use std::os::unix::fs::MetadataExt;

        let tmp = TempDir::new();
        let src = tmp.path().join("mixed.bin");
        let size = 4 * COPY_BUF_SIZE as u64;
        let mut file = std::fs::File::create(&src).unwrap();
        file.set_len(size).unwrap();
        // Dense runs that start and end off block boundaries, one spanning a
        // read-buffer boundary, plus a lone byte in the last block.
        let dense: Vec<u8> = (0..(5 * HOLE_BLOCK + 123))
            .map(|i| (i % 251 + 1) as u8)
            .collect();
        for at in [
            77_u64,
            COPY_BUF_SIZE as u64 - 1000,
            3 * COPY_BUF_SIZE as u64 + 9,
        ] {
            file.seek(SeekFrom::Start(at)).unwrap();
            file.write_all(&dense).unwrap();
        }
        file.seek(SeekFrom::Start(size - 1)).unwrap();
        file.write_all(&[9]).unwrap();
        file.sync_all().unwrap();
        assert!(is_sparse_file(&src));
        let dst = tmp.path().join("copy.bin");
        let state = Arc::new(Mutex::new(TransferProgress::new(size, 1)));
        let mut limiter = crate::transfer_tuning::BandwidthLimiter::new(Default::default());

        assert_eq!(
            copy_file_sparse(&src, &dst, &state, &mut limiter).unwrap(),
            size
        );

        assert_eq!(std::fs::read(&dst).unwrap(), std::fs::read(&src).unwrap());
        let (src_blocks, dst_blocks) = (
            src.metadata().unwrap().blocks(),
            dst.metadata().unwrap().blocks(),
        );
        assert!(
            dst_blocks <= src_blocks.saturating_add(16),
            "{dst_blocks} > {src_blocks}"
        );
    }
}
