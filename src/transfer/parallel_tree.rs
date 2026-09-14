//! Parallel buffered directory tree copy.
//!
//! Scans the source tree first, then copies leaf files on a sized rayon pool.
//! Symlink policy is Preserve-only; other policies use sequential buffered walk.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use super::TransferState;
use super::buffered::{copy_file_buffered_with_limiter, copy_symlink};

pub(super) fn copy_dir_buffered_parallel(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    workers: usize,
    preserve_sparse: bool,
) -> std::io::Result<u64> {
    let mut files = Vec::new();
    let mut permissions = Vec::new();
    prepare_buffered_tree(src, dst, state, &mut files, &mut permissions)?;
    let workers = workers.max(1).min(files.len().max(1));
    let pool = directory_copy_pool(workers)?;
    {
        let mut progress = crate::lock_util::recover(state);
        progress.active_workers = workers;
        progress.peak_workers = progress.peak_workers.max(workers);
    }
    let results = pool.install(|| {
        files
            .par_iter()
            .map(|(source, destination)| {
                let mut limiter = crate::transfer_tuning::BandwidthLimiter::new(Default::default());
                copy_file_buffered_with_limiter(
                    source,
                    destination,
                    state,
                    &mut limiter,
                    None,
                    None,
                    preserve_sparse,
                )
            })
            .collect::<Vec<_>>()
    });
    crate::lock_util::recover(state).active_workers = 1;
    let copied = results
        .into_iter()
        .try_fold(0_u64, |total, result| result.map(|bytes| total + bytes))?;
    for (path, mode) in permissions.into_iter().rev() {
        std::fs::set_permissions(path, mode)?;
    }
    Ok(copied)
}

fn prepare_buffered_tree(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    files: &mut Vec<(PathBuf, PathBuf)>,
    permissions: &mut Vec<(PathBuf, std::fs::Permissions)>,
) -> std::io::Result<()> {
    let mut tasks = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((source, destination)) = tasks.pop() {
        if crate::lock_util::recover(state).cancelled {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled while scanning the directory copy",
            ));
        }
        let metadata = std::fs::symlink_metadata(&source)?;
        if metadata.file_type().is_symlink() {
            copy_symlink(&source, &destination)?;
            continue;
        }
        if !metadata.is_dir() {
            files.push((source, destination));
            continue;
        }
        std::fs::create_dir(&destination)?;
        permissions.push((destination.clone(), metadata.permissions()));
        let mut children = std::fs::read_dir(&source)?.collect::<Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for entry in children.into_iter().rev() {
            tasks.push((entry.path(), destination.join(entry.file_name())));
        }
    }
    Ok(())
}

fn directory_copy_pool(workers: usize) -> std::io::Result<Arc<rayon::ThreadPool>> {
    static POOLS: std::sync::OnceLock<
        Mutex<std::collections::HashMap<usize, Arc<rayon::ThreadPool>>>,
    > = std::sync::OnceLock::new();
    let pools = POOLS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some(pool) = crate::lock_util::recover(pools).get(&workers).cloned() {
        return Ok(pool);
    }
    let candidate = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .thread_name(|index| format!("commander-copy-{index}"))
            .build()
            .map_err(std::io::Error::other)?,
    );
    let mut pools = crate::lock_util::recover(pools);
    Ok(Arc::clone(
        pools.entry(workers).or_insert_with(|| candidate),
    ))
}
