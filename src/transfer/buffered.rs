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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FileProgressMode {
    Detailed,
    AggregateOnly,
}

#[derive(Clone, Copy)]
struct BufferedCopyMode {
    preserve_sparse: bool,
    progress: FileProgressMode,
}

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
    copy_file_buffered_with_progress(
        src,
        dst,
        state,
        limiter,
        resume,
        checkpoints,
        BufferedCopyMode {
            preserve_sparse,
            progress: FileProgressMode::Detailed,
        },
    )
}

pub(super) fn copy_file_buffered_aggregate_with_limiter(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
    preserve_sparse: bool,
) -> std::io::Result<u64> {
    copy_file_buffered_with_progress(
        src,
        dst,
        state,
        limiter,
        None,
        None,
        BufferedCopyMode {
            preserve_sparse,
            progress: FileProgressMode::AggregateOnly,
        },
    )
}

fn copy_file_buffered_with_progress(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
    resume: Option<&ResumeCheckpoint>,
    checkpoints: Option<&mut dyn CheckpointSink>,
    mode: BufferedCopyMode,
) -> std::io::Result<u64> {
    let retain_partial = checkpoints.is_some();
    let result = if mode.preserve_sparse && resume.is_none() && super::sparse::is_sparse_file(src) {
        // Sparse copies publish no checkpoints, so their full-length,
        // hole-filled partial is never resumable and is cleaned up below.
        super::sparse::copy_file_sparse_with_progress(src, dst, state, limiter, mode.progress)
    } else {
        copy_file_buffered_inner(src, dst, state, limiter, resume, checkpoints, mode.progress)
    };
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
    progress_mode: FileProgressMode,
) -> std::io::Result<u64> {
    // A missing or unreadable source reports its own error, not a bogus
    // "invalid checkpoint" from comparing the offset against size 0.
    let file_size = src.metadata()?.len();
    let (offset, mut content_hasher) = validate_prefix_resume(src, dst, resume, file_size)?;

    {
        let mut s = crate::lock_util::recover(state);
        if progress_mode == FileProgressMode::Detailed {
            s.begin_file(src, file_size);
            s.current_file_copied = offset;
        }
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

        // The throttle sleep can last many seconds per chunk on a slow limit,
        // so it watches the cancel flag itself.
        if limiter
            .consume(n, || crate::lock_util::recover(state).cancelled)
            .is_err()
        {
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

        {
            let mut s = crate::lock_util::recover(state);
            s.copied_bytes = s.copied_bytes.saturating_add(n as u64);
            if progress_mode == FileProgressMode::Detailed {
                s.current_file_copied = s.current_file_copied.saturating_add(n as u64);
            }
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
    // Apply the source's modification time and permissions only after the
    // contents are complete, so a concurrent reader never sees a partial file
    // already wearing its final (possibly executable) mode. Resume binds the
    // partial by file id, not mtime, so the stamp cannot void a checkpoint.
    if let Ok(meta) = src.metadata() {
        if let Ok(modified) = meta.modified() {
            let _ = writer.get_ref().set_modified(modified);
        }
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
    let Some(checkpoints) = checkpoints.as_deref_mut() else {
        return Ok(());
    };
    // A durable checkpoint must never name bytes that are still only in the
    // OS cache. Tree children pass no sink and cannot be resumed separately;
    // their enclosing Verified/Versioned receipt syncs the staged tree once.
    writer.get_ref().sync_data()?;
    checkpoints.publish(ResumeCheckpoint::observe(
        src,
        dst,
        offset,
        CheckpointLayout::Prefix,
        *content_hasher.clone().finalize().as_bytes(),
    )?)
}

/// Name the tree entry an error came from, so a failure deep in a folder
/// copy is not reported against the top folder. The kind (which drives
/// failure classification) and the OS error text are kept; a cancel passes
/// through untouched.
pub(super) fn with_path(error: std::io::Error, path: &Path) -> std::io::Error {
    if error.kind() == std::io::ErrorKind::Interrupted {
        return error;
    }
    std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

/// Refusal for a tree entry that is neither file, folder nor symlink.
/// Opening a FIFO blocks until a writer appears, beyond the reach of cancel,
/// and devices and sockets have no bytes to copy.
pub(super) fn special_file_error(path: &Path) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!(
            "{}: special file (FIFO, socket or device) cannot be copied",
            path.display()
        ),
    )
}

/// Apply source folder modes, deepest first, once every child exists (a
/// read-only folder would otherwise refuse its own contents). Best effort,
/// like file modes: a target that rejects chmod (some FAT/SMB mounts) must
/// not fail a copy whose data all landed.
pub(super) fn restore_dir_permissions(modes: Vec<(PathBuf, std::fs::Permissions)>) {
    for (path, mode) in modes.into_iter().rev() {
        let _ = std::fs::set_permissions(path, mode);
    }
}

/// Recursively copy a directory with progress (buffered strategy).
/// Returns total bytes copied. Symlinks follow `symlink_policy`; followed
/// links are checked for cycles against the folders currently being walked.
pub(super) fn copy_dir_buffered_with_limiter(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
    preserve_sparse: bool,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
) -> std::io::Result<u64> {
    use crate::filesystem_policy::SymlinkPolicy;

    enum Task {
        Visit(PathBuf, PathBuf),
        ExitDirectory(PathBuf),
    }

    // Only a followed link can lead back into the walk, so other policies
    // skip the per-folder canonicalize.
    let follow = symlink_policy == SymlinkPolicy::Follow;
    let mut tasks = vec![Task::Visit(src.to_path_buf(), dst.to_path_buf())];
    let mut ancestors = std::collections::HashSet::new();
    let mut modes = Vec::new();
    let mut copied = 0u64;
    while let Some(task) = tasks.pop() {
        crate::transfer::check_cancelled(state)?;
        match task {
            Task::ExitDirectory(canonical) => {
                ancestors.remove(&canonical);
            }
            Task::Visit(source, destination) => {
                let metadata =
                    std::fs::symlink_metadata(&source).map_err(|e| with_path(e, &source))?;
                if metadata.file_type().is_symlink() {
                    match symlink_policy {
                        SymlinkPolicy::Preserve => {
                            crate::fs_util::copy_symlink(&source, &destination)
                                .map_err(|e| with_path(e, &source))?;
                        }
                        SymlinkPolicy::Skip => {}
                        SymlinkPolicy::Follow => {
                            let target = std::fs::canonicalize(&source)
                                .map_err(|e| with_path(e, &source))?;
                            tasks.push(Task::Visit(target, destination));
                        }
                    }
                    continue;
                }
                if metadata.is_file() {
                    let bytes = copy_file_buffered_with_limiter(
                        &source,
                        &destination,
                        state,
                        limiter,
                        None,
                        None,
                        preserve_sparse,
                    )
                    .map_err(|e| with_path(e, &source))?;
                    copied = copied.saturating_add(bytes);
                    continue;
                }
                if !metadata.is_dir() {
                    return Err(special_file_error(&source));
                }
                let canonical = if follow {
                    let canonical =
                        std::fs::canonicalize(&source).map_err(|e| with_path(e, &source))?;
                    if !ancestors.insert(canonical.clone()) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("symlink cycle detected at {}", source.display()),
                        ));
                    }
                    Some(canonical)
                } else {
                    None
                };
                std::fs::create_dir(&destination).map_err(|e| with_path(e, &destination))?;
                modes.push((destination.clone(), metadata.permissions()));
                let mut children = std::fs::read_dir(&source)
                    .and_then(|entries| entries.collect::<Result<Vec<_>, _>>())
                    .map_err(|e| with_path(e, &source))?;
                children.sort_by_key(|entry| entry.file_name());
                if let Some(canonical) = canonical {
                    tasks.push(Task::ExitDirectory(canonical));
                }
                for entry in children.into_iter().rev() {
                    tasks.push(Task::Visit(
                        entry.path(),
                        destination.join(entry.file_name()),
                    ));
                }
            }
        }
    }
    restore_dir_permissions(modes);
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

    #[test]
    fn buffered_copy_keeps_the_source_modification_time() {
        let tmp = TempDir::new();
        let src = tmp.file("src.txt", "dated");
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        std::fs::File::options()
            .write(true)
            .open(&src)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let dst = tmp.path().join("dst.txt");
        copy_file_buffered_with_limiter(&src, &dst, &state(), &mut unlimited(), None, None, false)
            .unwrap();
        assert_eq!(std::fs::metadata(&dst).unwrap().modified().unwrap(), old);
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "dated");
    }

    #[test]
    fn cancel_interrupts_a_bandwidth_throttle_sleep() {
        let tmp = TempDir::new();
        let src = tmp.path().join("src.bin");
        // 256 KiB at the 64 KiB/s floor is a four-second throttle sleep.
        std::fs::write(&src, vec![7u8; 256 * 1024]).unwrap();
        let dst = tmp.path().join("dst.bin");
        let progress = state();
        let mut limiter =
            crate::transfer_tuning::BandwidthLimiter::new(crate::transfer_tuning::VolumeRule {
                max_bytes_per_second: Some(64 * 1024),
                ..Default::default()
            });
        let canceller = {
            let progress = Arc::clone(&progress);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                crate::lock_util::recover(&progress).request_cancel();
            })
        };
        let started = std::time::Instant::now();

        let error =
            copy_file_buffered_with_limiter(&src, &dst, &progress, &mut limiter, None, None, false)
                .unwrap_err();

        canceller.join().unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "cancel waited {:?}",
            started.elapsed()
        );
        assert!(!dst.exists());
    }

    #[cfg(unix)]
    #[test]
    fn failed_sparse_copy_removes_its_full_length_partial() {
        let tmp = TempDir::new();
        let src = tmp.path().join("sparse.bin");
        std::fs::File::create(&src)
            .unwrap()
            .set_len(8 * 1024 * 1024)
            .unwrap();
        assert!(super::super::sparse::is_sparse_file(&src));
        let dst = tmp.path().join("dst.bin");
        let progress = state();
        crate::lock_util::recover(&progress).cancelled = true;

        let error = copy_file_buffered_with_limiter(
            &src,
            &dst,
            &progress,
            &mut unlimited(),
            None,
            None,
            true,
        )
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert!(!dst.exists());
    }

    #[cfg(unix)]
    fn mkfifo(path: &Path) {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
    }

    fn copy_tree(
        src: &Path,
        dst: &Path,
        policy: crate::filesystem_policy::SymlinkPolicy,
    ) -> std::io::Result<u64> {
        copy_dir_buffered_with_limiter(src, dst, &state(), &mut unlimited(), false, policy)
    }

    #[cfg(unix)]
    #[test]
    fn sequential_tree_copy_restores_folder_modes() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new();
        let src = tmp.dir("src");
        let nested = tmp.dir("src/nested");
        tmp.file("src/nested/f.txt", "abc");
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o700)).unwrap();
        let dst = tmp.path().join("dst");

        for policy in [
            crate::filesystem_policy::SymlinkPolicy::Preserve,
            crate::filesystem_policy::SymlinkPolicy::Follow,
        ] {
            let _ = std::fs::remove_dir_all(&dst);
            assert_eq!(copy_tree(&src, &dst, policy).unwrap(), 3);
            let mode = std::fs::metadata(dst.join("nested"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700, "{policy:?}");
            assert_eq!(
                std::fs::read_to_string(dst.join("nested/f.txt")).unwrap(),
                "abc"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn sequential_tree_copy_refuses_fifos_instead_of_blocking() {
        let tmp = TempDir::new();
        let src = tmp.dir("src");
        tmp.file("src/a.txt", "a");
        mkfifo(&src.join("pipe"));
        let error = copy_tree(
            &src,
            &tmp.path().join("dst"),
            crate::filesystem_policy::SymlinkPolicy::Preserve,
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("pipe"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn sequential_tree_copy_errors_name_the_failing_child() {
        let tmp = TempDir::new();
        let src = tmp.dir("src");
        tmp.file("src/deep/inner/file.txt", "x");
        // A dangling link deep in the tree fails when followed.
        std::os::unix::fs::symlink(tmp.path().join("gone"), src.join("deep/inner/dangling"))
            .unwrap();
        let error = copy_tree(
            &src,
            &tmp.path().join("dst"),
            crate::filesystem_policy::SymlinkPolicy::Follow,
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(error.to_string().contains("deep/inner/dangling"), "{error}");
    }

    #[test]
    fn with_path_keeps_kind_and_passes_cancel_through() {
        let error = with_path(
            std::io::Error::from(std::io::ErrorKind::StorageFull),
            Path::new("/a/b.txt"),
        );
        assert_eq!(error.kind(), std::io::ErrorKind::StorageFull);
        assert!(error.to_string().starts_with("/a/b.txt: "), "{error}");
        let cancel = with_path(crate::transfer::cancelled_error(), Path::new("/a"));
        assert_eq!(cancel.to_string(), "cancelled");
    }
}
