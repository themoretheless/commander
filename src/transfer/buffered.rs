//! Buffered byte-copy primitives used by the staging backends.
//!
//! Owns single-file prefix-checkpointed copy, sequential directory walks, and
//! symlink recreation. Parallel tree orchestration lives in [`super::parallel_tree`].

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::backend::CheckpointSink;
use super::{CheckpointLayout, ResumeCheckpoint, TransferState, prefix_digest};

pub(super) const COPY_BUF_SIZE: usize = 1024 * 1024; // 1 MB buffer
/// Smallest per-file copy buffer, so a tree of tiny files does not allocate
/// and zero a full [`COPY_BUF_SIZE`] buffer per file.
const MIN_COPY_BUF_SIZE: usize = 64 * 1024;
const CHECKPOINT_INTERVAL: u64 = 16 * 1024 * 1024;

/// Copy a single file with progress reporting (buffered strategy).
/// Removes the partial destination file on any failure.
/// Returns the bytes now in the destination (resume offset included).
pub(super) fn copy_file_buffered_with_limiter(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
    resume: Option<&ResumeCheckpoint>,
    checkpoints: Option<&mut dyn CheckpointSink>,
    preserve_sparse: bool,
) -> std::io::Result<u64> {
    if preserve_sparse && resume.is_none() && super::sparse::is_sparse_file(src) {
        return super::sparse::copy_file_sparse(src, dst, state, limiter);
    }
    let retain_partial = checkpoints.is_some();
    let result = copy_file_buffered_inner(src, dst, state, limiter, resume, checkpoints);
    if let Err(ref e) = result {
        // Clean up our own partial write, but never delete a destination that
        // was already there (AlreadyExists means create_new refused to clobber).
        if !retain_partial && e.kind() != std::io::ErrorKind::AlreadyExists {
            let _ = std::fs::remove_file(dst);
        }
    }
    result
}

/// Check a prefix checkpoint against the source and staging file.
///
/// Returns the verified resume offset and a hasher already fed the first
/// `offset` source bytes (a fresh hasher at offset 0 without a checkpoint).
/// Any mismatch fails closed with `InvalidData` so the caller restarts.
fn validate_prefix_resume(
    src: &Path,
    dst: &Path,
    resume: Option<&ResumeCheckpoint>,
    file_size: u64,
) -> std::io::Result<(u64, blake3::Hasher)> {
    let mut content_hasher = blake3::Hasher::new();
    let Some(checkpoint) = resume else {
        return Ok((0, content_hasher));
    };
    let offset = checkpoint.offset;
    if offset > file_size
        || checkpoint.staging != dst
        || checkpoint.layout != CheckpointLayout::Prefix
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid transfer checkpoint",
        ));
    }
    let expected = checkpoint.content_digest.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Prefix checkpoint has no content proof",
        )
    })?;
    crate::verified_hash::prefix_into(src, offset, &mut content_hasher)?;
    if *content_hasher.clone().finalize().as_bytes() != expected
        || prefix_digest(dst, offset)? != expected
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Prefix checkpoint content proof does not match source and staging",
        ));
    }
    Ok((offset, content_hasher))
}

fn copy_file_buffered_inner(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
    resume: Option<&ResumeCheckpoint>,
    mut checkpoints: Option<&mut dyn CheckpointSink>,
) -> std::io::Result<u64> {
    // A missing or unreadable source reports its own error, not a bogus
    // "invalid checkpoint" from comparing the offset against size 0.
    let file_size = src.metadata()?.len();
    let (offset, mut content_hasher) = validate_prefix_resume(src, dst, resume, file_size)?;

    {
        let mut s = crate::lock_util::recover(state);
        s.begin_file(src, file_size);
        s.current_file_copied = offset;
        s.copied_bytes = s.copied_bytes.saturating_add(offset);
    }

    // Reads go straight into `buf`, so no BufReader; size the buffer to the
    // file so small files do not each zero a full megabyte.
    let buf_len = usize::try_from(file_size.saturating_sub(offset))
        .unwrap_or(COPY_BUF_SIZE)
        .clamp(MIN_COPY_BUF_SIZE, COPY_BUF_SIZE);
    let mut reader = std::fs::File::open(src)?;
    reader.seek(SeekFrom::Start(offset))?;
    // create_new (O_EXCL): the caller always targets a path that should not
    // exist yet, so refuse to truncate a file that races into being.
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    let mut dst_file = if resume.is_some() {
        options.open(dst)?
    } else {
        options.create_new(true).open(dst)?
    };
    let staging_len = dst_file.metadata()?.len();
    if staging_len < offset {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "checkpoint staging is shorter than its verified offset",
        ));
    }
    if resume.is_some() && staging_len > offset {
        dst_file.set_len(offset)?;
    }
    dst_file.seek(SeekFrom::Start(offset))?;
    let mut writer = std::io::BufWriter::with_capacity(buf_len, dst_file);

    let mut buf = vec![0u8; buf_len];
    let mut copied = offset;
    let mut next_checkpoint = offset.saturating_add(CHECKPOINT_INTERVAL);

    loop {
        // Single mutex acquisition per iteration — O(1) instead of O(2)
        // Acquire once and use for both cancellation check and progress tracking
        let cancelled = {
            let s = crate::lock_util::recover(state);
            let was_cancelled = s.cancelled;
            drop(s);
            was_cancelled
        };

        if cancelled {
            persist_checkpoint(
                src,
                dst,
                copied,
                &mut writer,
                &content_hasher,
                &mut checkpoints,
            )?;
            return Err(crate::transfer::cancelled_error());
        }

        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        content_hasher.update(&buf[..n]);
        copied = copied.saturating_add(n as u64);

        if limiter.consume(n, || false).is_err() {
            persist_checkpoint(
                src,
                dst,
                copied,
                &mut writer,
                &content_hasher,
                &mut checkpoints,
            )?;
            return Err(std::io::ErrorKind::Other.into());
        }

        {
            let mut s = crate::lock_util::recover(state);
            s.copied_bytes = s.copied_bytes.saturating_add(n as u64);
            s.current_file_copied = s.current_file_copied.saturating_add(n as u64);
            s.maybe_sample();
        }
        if copied >= next_checkpoint {
            persist_checkpoint(
                src,
                dst,
                copied,
                &mut writer,
                &content_hasher,
                &mut checkpoints,
            )?;
            next_checkpoint = copied.saturating_add(CHECKPOINT_INTERVAL);
        }
    }
    persist_checkpoint(
        src,
        dst,
        copied,
        &mut writer,
        &content_hasher,
        &mut checkpoints,
    )?;
    // Apply the source's permissions only after the contents are complete, so
    // a concurrent reader never sees a partial file already wearing its final
    // (possibly executable) mode.
    if let Ok(meta) = src.metadata() {
        let _ = std::fs::set_permissions(dst, meta.permissions());
    }
    // What actually landed, so a source that changed size mid-copy cannot
    // skew the caller's byte accounting.
    Ok(copied)
}

fn persist_checkpoint(
    src: &Path,
    dst: &Path,
    offset: u64,
    writer: &mut std::io::BufWriter<std::fs::File>,
    content_hasher: &blake3::Hasher,
    checkpoints: &mut Option<&mut dyn CheckpointSink>,
) -> std::io::Result<()> {
    writer.flush()?;
    writer.get_ref().sync_data()?;
    let Some(checkpoints) = checkpoints.as_deref_mut() else {
        return Ok(());
    };
    checkpoints.publish(ResumeCheckpoint::observe(
        src,
        dst,
        offset,
        CheckpointLayout::Prefix,
        *content_hasher.clone().finalize().as_bytes(),
    )?)
}

/// Recursively copy a directory with progress (buffered strategy).
/// Returns total bytes copied. Symlinks are recreated as links rather than
/// followed, so a link pointing back into the tree cannot cause infinite
/// recursion.
pub(super) fn copy_dir_buffered_with_limiter(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
    preserve_sparse: bool,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
) -> std::io::Result<u64> {
    enum Task {
        Visit(PathBuf, PathBuf),
        ExitDirectory(PathBuf),
    }

    let mut tasks = vec![Task::Visit(src.to_path_buf(), dst.to_path_buf())];
    let mut ancestors = std::collections::HashSet::new();
    let mut copied = 0u64;
    while let Some(task) = tasks.pop() {
        crate::transfer::check_cancelled(state)?;
        match task {
            Task::ExitDirectory(canonical) => {
                ancestors.remove(&canonical);
            }
            Task::Visit(source, destination) => {
                let metadata = std::fs::symlink_metadata(&source)?;
                if metadata.file_type().is_symlink() {
                    match symlink_policy {
                        crate::filesystem_policy::SymlinkPolicy::Preserve => {
                            crate::fs_util::copy_symlink(&source, &destination)?;
                        }
                        crate::filesystem_policy::SymlinkPolicy::Skip => {}
                        crate::filesystem_policy::SymlinkPolicy::Follow => {
                            tasks.push(Task::Visit(std::fs::canonicalize(&source)?, destination));
                        }
                    }
                    continue;
                }
                if !metadata.is_dir() {
                    copied += copy_file_buffered_with_limiter(
                        &source,
                        &destination,
                        state,
                        limiter,
                        None,
                        None,
                        preserve_sparse,
                    )?;
                    continue;
                }
                let canonical = std::fs::canonicalize(&source)?;
                if !ancestors.insert(canonical.clone()) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("symlink cycle detected at {}", source.display()),
                    ));
                }
                std::fs::create_dir(&destination)?;
                let mut children = std::fs::read_dir(&source)?.collect::<Result<Vec<_>, _>>()?;
                children.sort_by_key(|entry| entry.file_name());
                tasks.push(Task::ExitDirectory(canonical));
                for entry in children.into_iter().rev() {
                    tasks.push(Task::Visit(
                        entry.path(),
                        destination.join(entry.file_name()),
                    ));
                }
            }
        }
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use crate::transfer::TransferProgress;
    use std::sync::{Arc, Mutex};

    fn state() -> TransferState {
        Arc::new(Mutex::new(TransferProgress::new(0, 1)))
    }

    fn unlimited() -> crate::transfer_tuning::BandwidthLimiter {
        crate::transfer_tuning::BandwidthLimiter::new(Default::default())
    }

    fn prefix_checkpoint(src: &Path, dst: &Path, offset: u64) -> ResumeCheckpoint {
        ResumeCheckpoint::observe(
            src,
            dst,
            offset,
            CheckpointLayout::Prefix,
            prefix_digest(src, offset).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn prefix_resume_accepts_a_matching_checkpoint() {
        let tmp = TempDir::new();
        let src = tmp.file("src.bin", "0123456789");
        let dst = tmp.file("staging.bin", "01234");
        let checkpoint = prefix_checkpoint(&src, &dst, 5);
        let (offset, hasher) = validate_prefix_resume(&src, &dst, Some(&checkpoint), 10).unwrap();
        assert_eq!(offset, 5);
        assert_eq!(
            *hasher.finalize().as_bytes(),
            checkpoint.content_digest.unwrap()
        );
        assert_eq!(validate_prefix_resume(&src, &dst, None, 10).unwrap().0, 0);
    }

    #[test]
    fn prefix_resume_rejects_every_mismatch_as_invalid_data() {
        let tmp = TempDir::new();
        let src = tmp.file("src.bin", "0123456789");
        let dst = tmp.file("staging.bin", "01234");
        let good = prefix_checkpoint(&src, &dst, 5);
        let reject = |checkpoint: &ResumeCheckpoint, size: u64| {
            validate_prefix_resume(&src, &dst, Some(checkpoint), size)
                .unwrap_err()
                .kind()
        };

        // Offset past the current source size.
        assert_eq!(reject(&good, 4), std::io::ErrorKind::InvalidData);
        // Checkpoint for another staging path.
        let mut other = good.clone();
        other.staging = tmp.path().join("other.bin");
        assert_eq!(reject(&other, 10), std::io::ErrorKind::InvalidData);
        // Delta layout handed to the prefix copier.
        let mut delta = good.clone();
        delta.layout = CheckpointLayout::DeltaFixed;
        assert_eq!(reject(&delta, 10), std::io::ErrorKind::InvalidData);
        // No content proof.
        let mut unproven = good.clone();
        unproven.content_digest = None;
        assert_eq!(reject(&unproven, 10), std::io::ErrorKind::InvalidData);
        // Staging prefix no longer matches the proof.
        std::fs::write(&dst, "0123X").unwrap();
        assert_eq!(reject(&good, 10), std::io::ErrorKind::InvalidData);
        // Source prefix no longer matches the proof.
        std::fs::write(&dst, "01234").unwrap();
        std::fs::write(&src, "X123456789").unwrap();
        assert_eq!(reject(&good, 10), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn resume_with_a_missing_source_reports_not_found() {
        let tmp = TempDir::new();
        let src = tmp.file("src.bin", "0123456789");
        let dst = tmp.file("staging.bin", "01234");
        let checkpoint = prefix_checkpoint(&src, &dst, 5);
        std::fs::remove_file(&src).unwrap();
        let error = copy_file_buffered_with_limiter(
            &src,
            &dst,
            &state(),
            &mut unlimited(),
            Some(&checkpoint),
            None,
            false,
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn buffered_copy_reports_per_file_progress_and_exact_bytes() {
        let tmp = TempDir::new();
        let bytes: Vec<u8> = (0..(COPY_BUF_SIZE + 777))
            .map(|i| (i % 253) as u8)
            .collect();
        let src = tmp.path().join("big.bin");
        std::fs::write(&src, &bytes).unwrap();
        let small = tmp.file("small.txt", "tiny");
        let progress = state();

        let dst = tmp.path().join("big.copy");
        let copied = copy_file_buffered_with_limiter(
            &src,
            &dst,
            &progress,
            &mut unlimited(),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(copied, bytes.len() as u64);
        assert_eq!(std::fs::read(&dst).unwrap(), bytes);
        {
            let s = crate::lock_util::recover(&progress);
            assert_eq!(s.current_file, "big.bin");
            assert_eq!(s.current_file_size, bytes.len() as u64);
            assert_eq!(s.current_file_copied, bytes.len() as u64);
            assert_eq!(s.copied_bytes, bytes.len() as u64);
        }

        let small_dst = tmp.path().join("small.copy");
        let copied = copy_file_buffered_with_limiter(
            &small,
            &small_dst,
            &progress,
            &mut unlimited(),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(copied, 4);
        assert_eq!(std::fs::read_to_string(&small_dst).unwrap(), "tiny");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn buffered_copy_returns_the_bytes_written_not_the_stat_size() {
        // procfs reports size 0 but has content, like a file that grew after
        // it was measured.
        let tmp = TempDir::new();
        let dst = tmp.path().join("status");
        let copied = copy_file_buffered_with_limiter(
            Path::new("/proc/self/status"),
            &dst,
            &state(),
            &mut unlimited(),
            None,
            None,
            false,
        )
        .unwrap();
        assert!(copied > 0);
        assert_eq!(copied, std::fs::metadata(&dst).unwrap().len());
    }
}
