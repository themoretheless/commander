//! Size caching, walk guards and background directory size computation.
//! SRP small piece: all the "how do we know the recursive size of folders without
//! blocking the UI or thrashing on watcher events" logic.
//! Extracted from panel.rs so the global caches, cooldowns and rayon pool are
//! in one understandable file. PanelState keeps a thin method that uses this.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// On-disk entry: mtime as seconds+nanos since UNIX epoch, and size.
#[derive(Serialize, Deserialize)]
pub(crate) struct CacheEntry {
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
    pub size: u64,
}

fn cache_path() -> PathBuf {
    let dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("commander");
    let _ = fs::create_dir_all(&dir);
    dir.join("dir_sizes.json")
}

/// Global cache: path → (mtime, size).
/// Loaded from disk on first access, saved on every update.
pub(crate) fn dir_size_cache() -> &'static Mutex<HashMap<PathBuf, (SystemTime, u64)>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (SystemTime, u64)>>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let map = load_cache_from_disk();
        Mutex::new(map)
    })
}

fn load_cache_from_disk() -> HashMap<PathBuf, (SystemTime, u64)> {
    let path = cache_path();
    let Ok(data) = fs::read_to_string(&path) else {
        return HashMap::new();
    };
    let Ok(entries): Result<HashMap<PathBuf, CacheEntry>, _> = serde_json::from_str(&data) else {
        return HashMap::new();
    };
    entries
        .into_iter()
        .map(|(p, e)| {
            let mtime =
                std::time::UNIX_EPOCH + Duration::new(e.mtime_secs, e.mtime_nanos);
            (p, (mtime, e.size))
        })
        .collect()
}

/// When each directory was last size-walked and how long the walk took.
/// Shared across panels. Guards against the watcher-noise feedback loop:
/// system writes deep in huge dirs (e.g. ~/Library) keep invalidating
/// their cached size, and re-walking them on every event burns CPU/disk
/// forever. Recently-walked dirs wait out a cooldown; dirs whose walk is
/// expensive are only re-walked on an explicit refresh.
pub(crate) fn walk_log() -> &'static Mutex<HashMap<PathBuf, (Instant, Duration)>> {
    static LOG: OnceLock<Mutex<HashMap<PathBuf, (Instant, Duration)>>> =
        OnceLock::new();
    LOG.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) const WALK_COOLDOWN: Duration = Duration::from_secs(10);
pub(crate) const WALK_EXPENSIVE: Duration = Duration::from_secs(2);

#[cfg(test)]
pub(crate) fn reset_walk_log() {
    if let Ok(mut log) = walk_log().lock() {
        log.clear();
    }
}

/// Mark cached sizes stale for every directory that contains `path`.
/// A change at `path` (watcher event) makes all its ancestors' sizes stale,
/// even though their mtimes don't move (mtime only reflects direct children).
/// The size value is kept (still useful for display); the stored mtime is
/// reset to the epoch so the next mtime comparison can never match.
pub fn invalidate_size_cache(path: &Path) {
    if let Ok(mut cache) = dir_size_cache().lock() {
        for (dir, entry) in cache.iter_mut() {
            if path.starts_with(dir) {
                entry.0 = std::time::UNIX_EPOCH;
            }
        }
    }
}

/// Save current cache to disk (best-effort, called from background threads).
/// Prunes entries for paths that no longer exist and writes atomically
/// (temp file + rename) so a crash can't corrupt the cache.
pub fn flush_cache() {
    let entries: HashMap<PathBuf, CacheEntry> = {
        let Ok(mut cache) = dir_size_cache().lock() else {
            return;
        };
        cache.retain(|p, _| p.exists());
        cache
            .iter()
            .filter_map(|(p, (mtime, size))| {
                let dur = mtime.duration_since(std::time::UNIX_EPOCH).ok()?;
                Some((
                    p.clone(),
                    CacheEntry {
                        mtime_secs: dur.as_secs(),
                        mtime_nanos: dur.subsec_nanos(),
                        size: *size,
                    },
                ))
            })
            .collect()
        // Lock dropped here: serialization and IO happen outside it.
    };
    if let Ok(json) = serde_json::to_string(&entries) {
        let path = cache_path();
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &path);
        }
    }
}

/// Size used for the occupancy bar: a file's own size, or a directory's
/// resolved recursive size (0 while it is still being measured).
pub fn entry_display_size(entry: &crate::panel::FileEntry, dir_sizes: &HashMap<PathBuf, u64>) -> u64 {
    if entry.is_dir {
        dir_sizes.get(&entry.path).copied().unwrap_or(0)
    } else {
        entry.size
    }
}

/// Dedicated thread pool (max 10 threads) for filesystem work.
/// Moved here so the pool definition lives with the size-walking concern.
pub(crate) fn fs_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(10)
            .thread_name(|i| format!("fs-worker-{}", i))
            .build()
            .unwrap()
    })
}

/// Compute dir sizes - moved here for SRP (sizes concern).
/// Takes &PanelState to access private fields from sub module.
pub(crate) fn compute_dir_sizes(panel: &super::PanelState, forced: bool) -> bool {
    // Clear panel-local sizes and counts for current directory listing
    if let Ok(mut sizes) = panel.dir_sizes.lock() {
        sizes.clear();
    }
    if let Ok(mut counts) = panel.dir_counts.lock() {
        counts.clear();
    }

    // Collect dirs - move all mtime/cache/walk decision to bg to avoid blocking UI thread (perf fix)
    let mut need_count: Vec<PathBuf> = Vec::new();
    let mut need_size: Vec<PathBuf> = Vec::new();
    let mut retry = false;

    for entry in panel.entries() {
        if !entry.is_dir {
            continue;
        }

        need_count.push(entry.path.clone());
        need_size.push(entry.path.clone());
    }

    // (walk log guards and cache decision now inside bg tasks to keep UI responsive)

    // Subdir counts
    let counts = Arc::clone(&panel.dir_counts);
    let wake1 = panel.notify.clone();
    fs_pool().spawn(move || {
        use rayon::prelude::*;
        let results: Vec<_> = fs_pool().install(|| {
            need_count
                .par_iter()
                .map(|p| {
                    let count = std::fs::read_dir(p)
                        .map(|rd| rd.filter_map(|e| e.ok()).count())
                        .unwrap_or(0);
                    (p.clone(), count)
                })
                .collect()
        });
        if let Ok(mut map) = counts.lock() {
            for (p, c) in results {
                map.insert(p, c);
            }
        }
        if let Some(wake) = wake1 {
            wake();
        }
    });

    // Dir sizes - all mtime, guard, cache, walk now inside bg (moved sync fs off UI thread for perf)
    if !need_size.is_empty() {
        let sizes = Arc::clone(&panel.dir_sizes);
        let wake2 = panel.notify.clone();
        fs_pool().spawn(move || {
            use rayon::prelude::*;
            let results: Vec<_> = fs_pool().install(|| {
                need_size
                    .par_iter()
                    .map(|p| {
                        let dir_mtime = std::fs::metadata(p).and_then(|m| m.modified()).ok();
                        // Check cache
                        if let Some(mtime) = dir_mtime
                            && let Ok(cache) = dir_size_cache().lock()
                            && let Some(&(cached_mtime, cached_size)) = cache.get(p)
                            && cached_mtime == mtime
                        {
                            return (p.clone(), mtime, cached_size);
                        }
                        // guards
                        #[cfg(test)]
                        let guards_enabled = std::env::var("COMMANDER_DISABLE_WALK_GUARDS").is_err();
                        #[cfg(not(test))]
                        let guards_enabled = true;
                        let mut skip = false;
                        let mut cost = std::time::Duration::ZERO;
                        if guards_enabled
                            && let Ok(log) = walk_log().lock()
                            && let Some(&(when, c)) = log.get(p)
                        {
                            cost = c;
                            if when.elapsed() < WALK_COOLDOWN {
                                skip = true;
                            }
                        }
                        let started = std::time::Instant::now();
                        let size = if skip {
                            if let Ok(cache) = dir_size_cache().lock()
                                && let Some(&(_, s)) = cache.get(p)
                            {
                                s
                            } else {
                                0
                            }
                        } else {
                            let s = crate::fs_util::dir_size_recursive(p);
                            if let Ok(mut log) = walk_log().lock() {
                                log.insert(p.clone(), (std::time::Instant::now(), started.elapsed()));
                            }
                            s
                        };
                        (p.clone(), dir_mtime.unwrap_or(std::time::UNIX_EPOCH), size)
                    })
                    .collect()
            });
            if let Ok(mut map) = sizes.lock() {
                for (p, _, size) in &results {
                    map.insert(p.clone(), *size);
                }
            }
            if let Ok(mut cache) = dir_size_cache().lock() {
                for (p, mt, size) in &results {
                    cache.insert(p.clone(), (*mt, *size));
                }
            }
            flush_cache();
            if let Some(wake) = wake2 {
                wake();
            }
        });
    }

    retry
}
