//! Filesystem watcher and poll logic (notify + dirty flags for reload/sizes).
//! SRP: the "react to FS changes without blocking UI" concern extracted.
//! Uses notify crate. Deep events only dirty sizes (with debounce).
//! Extracted to make the main panel state smaller (like in other FMs with separate FS observers).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, RecursiveMode, Watcher};

use super::PanelState;

/// Check if fs watcher flagged a change; if so, refresh.
/// Returns `true` when the directory listing was re-read.
/// Deep events (below the watched dir) only recompute directory
/// sizes, debounced so event floods during transfers don't thrash.
pub fn poll_fs_changes(panel: &mut PanelState) -> bool {
    const SIZES_DEBOUNCE: Duration = Duration::from_millis(500);

    let reload = panel
        .needs_refresh
        .swap(false, std::sync::atomic::Ordering::Relaxed);
    if reload {
        panel.reload_entries();
        let retry = panel.compute_dir_sizes(true);
        panel
            .sizes_dirty
            .store(retry, std::sync::atomic::Ordering::Relaxed);
        panel.last_sizes_recompute = Some(std::time::Instant::now());
        return true;
    }

    if panel.sizes_dirty.load(std::sync::atomic::Ordering::Relaxed) {
        let due = panel
            .last_sizes_recompute
            .is_none_or(|t| t.elapsed() >= SIZES_DEBOUNCE);
        if due {
            panel.last_sizes_recompute = Some(std::time::Instant::now());
            // Keep the dirty flag when some dir is still in its walk
            // cooldown: a later poll picks it up.
            let retry = panel.compute_dir_sizes(false);
            panel
                .sizes_dirty
                .store(retry, std::sync::atomic::Ordering::Relaxed);
        } else if let Some(wake) = &panel.notify {
            // Poll again on a later frame once the debounce expires.
            wake();
        }
    }
    false
}

pub fn start_watcher(panel: &mut PanelState) {
    use notify::{Event, RecursiveMode, Watcher};

    // Skip if already watching this path
    if panel.watched_path.as_ref() == Some(&panel.current_path) {
        return;
    }

    // Drop old watcher
    panel.watcher = None;
    panel.watched_path = None;

    let flag = Arc::clone(&panel.needs_refresh);
    let sizes_flag = Arc::clone(&panel.sizes_dirty);
    let wake = panel.notify.clone();
    let watched = panel.current_path().clone();

    let watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        if let Ok(event) = res {
            // A change anywhere under a cached directory makes its
            // size stale, even though its own mtime doesn't move.
            let mut direct = event.paths.is_empty();
            for p in &event.paths {
                super::invalidate_size_cache(p);
                if p == &watched || p.parent() == Some(watched.as_path()) {
                    direct = true;
                }
            }
            if direct {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            } else {
                sizes_flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            if let Some(wake) = &wake {
                wake();
            }
        }
    });

    if let Ok(mut w) = watcher {
        // Recursive: deep events never reload the listing, but they
        // must invalidate cached sizes (see the callback above).
        let _ = w.watch(&panel.current_path, RecursiveMode::Recursive);
        panel.watched_path = Some(panel.current_path().clone());
        panel.watcher = Some(w);
    }
}