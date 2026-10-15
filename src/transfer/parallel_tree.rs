//! Parallel buffered directory tree copy.
//!
//! Streams the source walk: destination directories are created as discovered,
//! and leaf file jobs are fed through a bounded queue so workers start copying
//! before the scan finishes. Symlink policy is Preserve-only; other policies
//! use the sequential buffered walk. Each copy runs its own scoped workers,
//! so one stalled transfer cannot hold threads another transfer waits for.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use super::TransferState;
use super::buffered::{
    copy_file_buffered_with_limiter, restore_dir_permissions, special_file_error, with_path,
};
use crate::fs_util::copy_symlink;

/// Bound the scan→worker queue so a deep tree cannot materialize every leaf
/// path pair in memory before the first copy runs.
fn file_job_queue_bound(workers: usize) -> usize {
    workers.saturating_mul(8).max(8)
}

/// Test-only fault injection: a worker asked to copy this exact source fails.
/// Tests run as root, so permission tricks cannot make one copy fail.
#[cfg(test)]
static FAIL_WORKER_COPY_OF: Mutex<Option<PathBuf>> = Mutex::new(None);

pub(super) fn copy_dir_buffered_parallel(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    workers: usize,
    preserve_sparse: bool,
) -> std::io::Result<u64> {
    let workers = workers.max(1);
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

    // Mutex so several workers can pull from one receiver. Intake is
    // serialized; the buffered copy work still runs in parallel.
    let rx = Mutex::new(rx);
    let copied = AtomicU64::new(0);
    let stop = AtomicBool::new(false);
    let first_error = Mutex::new(None::<std::io::Error>);
    let fail = |error: std::io::Error| {
        stop.store(true, Ordering::Relaxed);
        let mut slot = crate::lock_util::recover(&first_error);
        if slot.is_none() {
            *slot = Some(error);
        }
    };
    let copy_job = |source: &Path, destination: &Path| -> std::io::Result<()> {
        if crate::lock_util::recover(state).cancelled {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled while copying the directory tree",
            ));
        }
        #[cfg(test)]
        if crate::lock_util::recover(&FAIL_WORKER_COPY_OF).as_deref() == Some(source) {
            return Err(with_path(
                std::io::Error::other("injected worker copy failure"),
                source,
            ));
        }
        let mut limiter = crate::transfer_tuning::BandwidthLimiter::new(Default::default());
        let bytes = copy_file_buffered_with_limiter(
            source,
            destination,
            state,
            &mut limiter,
            None,
            None,
            preserve_sparse,
        )
        .map_err(|e| with_path(e, source))?;
        copied.fetch_add(bytes, Ordering::Relaxed);
        Ok(())
    };
    thread::scope(|scope| {
        for index in 0..workers {
            let spawned = thread::Builder::new()
                .name(format!("commander-copy-{index}"))
                .spawn_scoped(scope, || {
                    while !stop.load(Ordering::Relaxed) {
                        let Ok((source, destination)) = crate::lock_util::recover(&rx).recv()
                        else {
                            break;
                        };
                        // Another worker may have failed while this one waited.
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        if let Err(error) = copy_job(&source, &destination) {
                            fail(error);
                        }
                    }
                });
            if let Err(error) = spawned {
                fail(error);
                break;
            }
        }
    });
    let copy_result = crate::lock_util::recover(&first_error)
        .take()
        .map_or(Ok(()), Err);

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
    restore_dir_permissions(modes);
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
        let metadata = std::fs::symlink_metadata(&source).map_err(|e| with_path(e, &source))?;
        if metadata.file_type().is_symlink() {
            copy_symlink(&source, &destination).map_err(|e| with_path(e, &source))?;
            continue;
        }
        if metadata.is_file() {
            // Bounded send: back-pressures the scan so queued path pairs stay
            // O(workers), not O(tree).
            if tx.send((source, destination)).is_err() {
                // Workers disconnected after a copy failure or cancel.
                return Ok(());
            }
            continue;
        }
        if !metadata.is_dir() {
            return Err(special_file_error(&source));
        }
        std::fs::create_dir(&destination).map_err(|e| with_path(e, &destination))?;
        crate::lock_util::recover(permissions).push((destination.clone(), metadata.permissions()));
        let mut children = std::fs::read_dir(&source)
            .and_then(|entries| entries.collect::<Result<Vec<_>, _>>())
            .map_err(|e| with_path(e, &source))?;
        children.sort_by_key(|entry| entry.file_name());
        for entry in children.into_iter().rev() {
            tasks.push((entry.path(), destination.join(entry.file_name())));
        }
    }
    Ok(())
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

    use crate::testutil::TempDir;
    use crate::transfer::TransferProgress;

    fn progress() -> TransferState {
        Arc::new(Mutex::new(TransferProgress::new(0, 1)))
    }

    #[cfg(unix)]
    #[test]
    fn scan_refuses_a_fifo_after_queueing_earlier_files_without_hanging() {
        use std::os::unix::ffi::OsStrExt;

        let tmp = TempDir::new();
        let src = tmp.dir("src");
        // More files than the queue holds, scanned before the FIFO in "z".
        for index in 0..(file_job_queue_bound(2) * 3) {
            tmp.file(&format!("src/a/{index:03}.txt"), "x");
        }
        tmp.dir("src/z");
        let fifo = std::ffi::CString::new(src.join("z/pipe").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);

        let error =
            copy_dir_buffered_parallel(&src, &tmp.path().join("dst"), &progress(), 2, false)
                .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("z/pipe"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn parallel_tree_copy_restores_folder_modes() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new();
        let src = tmp.dir("src");
        let nested = tmp.dir("src/nested");
        tmp.file("src/nested/f.txt", "abc");
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o700)).unwrap();
        let dst = tmp.path().join("dst");

        assert_eq!(
            copy_dir_buffered_parallel(&src, &dst, &progress(), 2, false).unwrap(),
            3
        );
        let mode = std::fs::metadata(dst.join("nested"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn scan_errors_name_the_path_they_failed_on() {
        let tmp = TempDir::new();
        let src = tmp.dir("src");
        tmp.file("src/a.txt", "a");
        // The destination root already exists, so the scan's create fails.
        let dst = tmp.dir("taken");
        let error = copy_dir_buffered_parallel(&src, &dst, &progress(), 2, false).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(error.to_string().contains("taken"), "{error}");
    }

    #[test]
    fn a_stalled_tree_copy_does_not_block_another_with_the_same_worker_count() {
        let tmp = TempDir::new();
        let src_a = tmp.dir("a");
        for index in 0..300 {
            tmp.file(&format!("a/{index:03}.txt"), "x");
        }
        let src_b = tmp.dir("b");
        tmp.file("b/one.txt", "1");
        let (dst_a, dst_b) = (tmp.path().join("dst-a"), tmp.path().join("dst-b"));
        let state_a = progress();
        crate::lock_util::recover(&state_a).active_workers = 0;

        let copy_a = {
            let (state_a, src_a, dst_a) = (Arc::clone(&state_a), src_a.clone(), dst_a.clone());
            thread::spawn(move || copy_dir_buffered_parallel(&src_a, &dst_a, &state_a, 2, false))
        };
        // Stall copy A mid-tree: its scan and workers all wait on its progress.
        let stalled = loop {
            let guard = crate::lock_util::recover(&state_a);
            if guard.active_workers == 2 {
                break guard;
            }
            drop(guard);
            thread::yield_now();
        };
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let copy_b = thread::spawn(move || {
            let result = copy_dir_buffered_parallel(&src_b, &dst_b, &progress(), 2, false);
            let _ = done_tx.send(result.map_err(|e| e.to_string()));
        });

        let finished = done_rx.recv_timeout(std::time::Duration::from_secs(5));
        drop(stalled);
        copy_b.join().unwrap();
        assert_eq!(copy_a.join().unwrap().unwrap(), 300);
        assert_eq!(finished.expect("copy B waited for copy A").unwrap(), 1);
    }

    #[test]
    fn a_worker_copy_failure_stops_the_tree_copy_without_hanging() {
        let tmp = TempDir::new();
        let src = tmp.dir("src");
        let total = file_job_queue_bound(2) * 20;
        for index in 0..total {
            tmp.file(&format!("src/{index:04}.txt"), "x");
        }
        let failing = src.join("0000.txt");
        *crate::lock_util::recover(&FAIL_WORKER_COPY_OF) = Some(failing.clone());
        let dst = tmp.path().join("dst");
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let copy = {
            let (src, dst) = (src.clone(), dst.clone());
            thread::spawn(move || {
                let result = copy_dir_buffered_parallel(&src, &dst, &progress(), 2, false);
                let _ = done_tx.send(result);
            })
        };

        let result = done_rx.recv_timeout(std::time::Duration::from_secs(10));
        *crate::lock_util::recover(&FAIL_WORKER_COPY_OF) = None;
        copy.join().unwrap();
        let error = result
            .expect("tree copy hung after a worker failed")
            .unwrap_err();

        let message = error.to_string();
        assert!(
            message.contains("injected worker copy failure"),
            "{message}"
        );
        assert!(message.contains("0000.txt"), "{message}");
        // The first error stops the other worker and the scan: the queue
        // never drains the whole tree.
        let copied = std::fs::read_dir(&dst).map_or(0, |entries| entries.count());
        assert!(copied < total, "copied {copied} of {total}");
    }

    #[test]
    fn cancel_mid_tree_stops_workers_and_scan_without_hanging() {
        let tmp = TempDir::new();
        let src = tmp.dir("src");
        // Far more files than the queue holds, so the scan is still feeding
        // workers (or blocked on the full queue) when the cancel lands.
        for index in 0..(file_job_queue_bound(2) * 20) {
            tmp.file(&format!("src/{index:04}.txt"), "x");
        }
        let dst = tmp.path().join("dst");
        let state = progress();
        crate::lock_util::recover(&state).active_workers = 0;
        let copy = {
            let (state, src, dst) = (Arc::clone(&state), src.clone(), dst.clone());
            thread::spawn(move || copy_dir_buffered_parallel(&src, &dst, &state, 2, false))
        };
        loop {
            let mut guard = crate::lock_util::recover(&state);
            if guard.active_workers == 2 {
                guard.request_cancel();
                break;
            }
            drop(guard);
            thread::yield_now();
        }

        let error = copy.join().unwrap().unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(crate::lock_util::recover(&state).active_workers, 1);
    }
}
