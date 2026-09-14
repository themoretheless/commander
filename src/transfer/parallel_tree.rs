//! Parallel buffered directory tree copy.
//!
//! Streams the source walk: destination directories are created as discovered,
//! and leaf file jobs are fed through a bounded queue so workers start copying
//! before the scan finishes. Symlink policy is Preserve-only; other policies
//! use the sequential buffered walk.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use rayon::prelude::*;

use super::TransferState;
use super::buffered::{copy_file_buffered_with_limiter, copy_symlink};

/// Bound the scan→worker queue so a deep tree cannot materialize every leaf
/// path pair in memory before the first copy runs.
fn file_job_queue_bound(workers: usize) -> usize {
    workers.saturating_mul(8).max(8)
}

pub(super) fn copy_dir_buffered_parallel(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    workers: usize,
    preserve_sparse: bool,
) -> std::io::Result<u64> {
    let workers = workers.max(1);
    let pool = directory_copy_pool(workers)?;
    {
        let mut progress = crate::lock_util::recover(state);
        progress.active_workers = workers;
        progress.peak_workers = progress.peak_workers.max(workers);
    }

    let (tx, rx) =
        std::sync::mpsc::sync_channel::<(PathBuf, PathBuf)>(file_job_queue_bound(workers));
    let permissions = Arc::new(Mutex::new(Vec::new()));
    let scan_error = Arc::new(Mutex::new(None::<std::io::Error>));

    let scan_state = Arc::clone(state);
    let scan_permissions = Arc::clone(&permissions);
    let scan_error_slot = Arc::clone(&scan_error);
    let scan_src = src.to_path_buf();
    let scan_dst = dst.to_path_buf();
    let scan = thread::Builder::new()
        .name("commander-copy-scan".into())
        .spawn(move || {
            let outcome =
                stream_buffered_tree(&scan_src, &scan_dst, &scan_state, &tx, &scan_permissions);
            // Drop the sender so the worker iterator ends once the scan finishes.
            drop(tx);
            if let Err(error) = outcome {
                let mut slot = crate::lock_util::recover(&scan_error_slot);
                if slot.is_none() {
                    *slot = Some(error);
                }
            }
        })
        .map_err(std::io::Error::other)?;

    // Mutex so rayon::par_bridge can pull jobs from multiple worker threads.
    // Intake is serialized; the buffered copy work still runs in parallel.
    let rx = Mutex::new(rx);
    let copied = AtomicU64::new(0);
    let copy_result = pool.install(|| {
        std::iter::from_fn(|| crate::lock_util::recover(&rx).recv().ok())
            .par_bridge()
            .try_for_each(|(source, destination)| {
                if crate::lock_util::recover(state).cancelled {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "cancelled while copying the directory tree",
                    ));
                }
                let mut limiter = crate::transfer_tuning::BandwidthLimiter::new(Default::default());
                let bytes = copy_file_buffered_with_limiter(
                    &source,
                    &destination,
                    state,
                    &mut limiter,
                    None,
                    None,
                    preserve_sparse,
                )?;
                copied.fetch_add(bytes, Ordering::Relaxed);
                Ok(())
            })
    });

    // Drop the receiver so a scan thread blocked on a full queue can exit
    // after workers stop early (copy failure / cancel).
    drop(rx);
    scan.join()
        .map_err(|_| std::io::Error::other("directory copy scan thread panicked"))?;

    crate::lock_util::recover(state).active_workers = 1;

    copy_result?;
    if let Some(error) = crate::lock_util::recover(&scan_error).take() {
        return Err(error);
    }

    let modes = std::mem::take(&mut *crate::lock_util::recover(&permissions));
    for (path, mode) in modes.into_iter().rev() {
        std::fs::set_permissions(path, mode)?;
    }
    Ok(copied.load(Ordering::Relaxed))
}

/// Walk `src`, create destination directories / preserve symlinks as discovered,
/// and enqueue leaf file jobs without building a complete leaf vector first.
fn stream_buffered_tree(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    tx: &std::sync::mpsc::SyncSender<(PathBuf, PathBuf)>,
    permissions: &Mutex<Vec<(PathBuf, std::fs::Permissions)>>,
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
            // Bounded send: back-pressures the scan so queued path pairs stay
            // O(workers), not O(tree).
            if tx.send((source, destination)).is_err() {
                // Workers disconnected after a copy failure or cancel.
                return Ok(());
            }
            continue;
        }
        std::fs::create_dir(&destination)?;
        crate::lock_util::recover(permissions).push((destination.clone(), metadata.permissions()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_job_queue_bound_scales_with_workers_and_stays_small() {
        assert_eq!(file_job_queue_bound(1), 8);
        assert_eq!(file_job_queue_bound(2), 16);
        assert_eq!(file_job_queue_bound(8), 64);
        // Never grows with tree size — only with worker count.
        assert!(file_job_queue_bound(32) < 1_000);
    }
}
