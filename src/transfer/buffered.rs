//! Buffered byte-copy primitives used by the staging backends.
//!
//! Owns single-file prefix-checkpointed copy, sequential directory walks, and
//! symlink recreation. Parallel tree orchestration lives in [`super::parallel_tree`].

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::backend::CheckpointSink;
use super::{CheckpointLayout, ResumeCheckpoint, TransferState, prefix_digest};
use crate::path_identity::PathIdentity;

pub(super) const COPY_BUF_SIZE: usize = 1024 * 1024; // 1 MB buffer
const CHECKPOINT_INTERVAL: u64 = 16 * 1024 * 1024;

/// Copy a single file with progress reporting (buffered strategy).
/// Removes the partial destination file on any failure.
/// Returns the file size on success.
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

fn copy_file_buffered_inner(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
    resume: Option<&ResumeCheckpoint>,
    mut checkpoints: Option<&mut dyn CheckpointSink>,
) -> std::io::Result<u64> {
    let file_size = src.metadata().map(|m| m.len()).unwrap_or(0);
    let offset = resume.map_or(0, |checkpoint| checkpoint.offset);
    if offset > file_size
        || resume.is_some_and(|checkpoint| {
            checkpoint.staging != dst || checkpoint.layout != CheckpointLayout::Prefix
        })
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid transfer checkpoint",
        ));
    }
    let mut content_hasher = blake3::Hasher::new();
    if let Some(checkpoint) = resume {
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
    }

    // Init per-file progress
    {
        let mut s = crate::lock_util::recover(state);
        s.current_file = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        s.current_file_size = file_size;
        s.current_file_copied = offset;
        s.copied_bytes = s.copied_bytes.saturating_add(offset);
    }

    let mut source = std::fs::File::open(src)?;
    source.seek(SeekFrom::Start(offset))?;
    let mut reader = std::io::BufReader::with_capacity(COPY_BUF_SIZE, source);
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
    let mut writer = std::io::BufWriter::with_capacity(COPY_BUF_SIZE, dst_file);

    let mut buf = vec![0u8; COPY_BUF_SIZE];
    let mut copied = offset;
    let mut next_checkpoint = offset.saturating_add(CHECKPOINT_INTERVAL);

    loop {
        {
            let s = crate::lock_util::recover(state);
            if s.cancelled {
                drop(s);
                persist_checkpoint(
                    src,
                    dst,
                    copied,
                    &mut writer,
                    &content_hasher,
                    &mut checkpoints,
                )?;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
        }

        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        content_hasher.update(&buf[..n]);
        copied = copied.saturating_add(n as u64);
        if let Err(error) = limiter.consume(n, || crate::lock_util::recover(state).cancelled) {
            persist_checkpoint(
                src,
                dst,
                copied,
                &mut writer,
                &content_hasher,
                &mut checkpoints,
            )?;
            return Err(error);
        }

        {
            let mut s = crate::lock_util::recover(state);
            s.copied_bytes += n as u64;
            s.current_file_copied += n as u64;
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
    Ok(file_size)
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
    let source = PathIdentity::observe_deep(src)?;
    let partial = PathIdentity::observe_deep(dst)?;
    let checkpoint = ResumeCheckpoint {
        staging: dst.to_path_buf(),
        offset,
        source,
        partial,
        layout: CheckpointLayout::Prefix,
        content_digest: Some(*content_hasher.clone().finalize().as_bytes()),
    };
    checkpoints.publish(checkpoint)
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
        if crate::lock_util::recover(state).cancelled {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }
        match task {
            Task::ExitDirectory(canonical) => {
                ancestors.remove(&canonical);
            }
            Task::Visit(source, destination) => {
                let metadata = std::fs::symlink_metadata(&source)?;
                if metadata.file_type().is_symlink() {
                    match symlink_policy {
                        crate::filesystem_policy::SymlinkPolicy::Preserve => {
                            copy_symlink(&source, &destination)?;
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

/// Recreate a symlink at `dst` pointing at the same target as `src`.
#[cfg(unix)]
pub(super) fn copy_symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    let target = std::fs::read_link(src)?;
    std::os::unix::fs::symlink(target, dst)
}

#[cfg(not(unix))]
pub(super) fn copy_symlink(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "preserving symlinks is not supported on this platform",
    ))
}
