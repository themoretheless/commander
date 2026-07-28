use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Instant, SystemTime};

use super::{
    DirMeasurement, FileEntry, FolderOverview, Notify, ScanEpoch, VolumePathKey, WALK_COOLDOWN,
    WALK_EXPENSIVE, begin_panel_scan, capture_path_epoch, dir_size_recursive_until, flush_cache,
    lock_recover, publish_cached_size_if_current, publish_dir_measurements_if_current,
    publish_scan_values_if_current, scan_commit_boundary, scan_is_current, visible_window,
    walk_log,
};

const SIZES_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);

pub(super) struct SizeScanInput<'a> {
    pub(super) path: &'a Path,
    pub(super) entries: &'a [FileEntry],
    pub(super) filtered: Arc<[usize]>,
    pub(super) scroll_anchor: usize,
    pub(super) page_rows: usize,
    pub(super) notify: Option<Notify>,
}

/// Immutable, revisioned view of directory metrics. Building it clones the
/// maps once per publication; warm UI frames reuse the same `Arc`.
#[derive(Debug, Default)]
pub struct SizeSnapshot {
    revision: u64,
    sizes: HashMap<PathBuf, u64>,
    counts: HashMap<PathBuf, usize>,
}

impl SizeSnapshot {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn size_of(&self, path: &Path) -> Option<u64> {
        self.sizes.get(path).copied()
    }

    pub fn count_of(&self, path: &Path) -> Option<usize> {
        self.counts.get(path).copied()
    }

    pub fn display_size(&self, entry: &FileEntry) -> u64 {
        if entry.is_dir {
            self.size_of(&entry.path).unwrap_or(0)
        } else {
            entry.size
        }
    }
}

#[derive(Clone)]
struct OverviewCache {
    listing_revision: u64,
    size_revision: u64,
    value: FolderOverview,
}

#[derive(Clone)]
struct DisplayMaxCache {
    listing_revision: u64,
    size_revision: u64,
    filtered: Weak<[usize]>,
    value: u64,
}

/// Owns panel-local recursive-size state, cancellation, dirty debounce and
/// renderer snapshots. Filesystem workers only receive narrow publication
/// handles; callers never observe the mutable maps.
pub(super) struct SizeIndex {
    sizes: Arc<Mutex<HashMap<PathBuf, u64>>>,
    counts: Arc<Mutex<HashMap<PathBuf, usize>>>,
    scan_epoch: Arc<Mutex<Arc<ScanEpoch>>>,
    revision: Arc<AtomicU64>,
    snapshot_cache: Mutex<Option<Arc<SizeSnapshot>>>,
    overview_cache: Mutex<Option<OverviewCache>>,
    display_max_cache: Mutex<Option<DisplayMaxCache>>,
    binding: PathBuf,
    dirty: bool,
    last_recompute: Option<Instant>,
    #[cfg(test)]
    overview_computations: AtomicU64,
}

impl SizeIndex {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            sizes: Arc::new(Mutex::new(HashMap::new())),
            counts: Arc::new(Mutex::new(HashMap::new())),
            scan_epoch: Arc::new(Mutex::new(Arc::new(ScanEpoch::new()))),
            revision: Arc::new(AtomicU64::new(0)),
            snapshot_cache: Mutex::new(None),
            overview_cache: Mutex::new(None),
            display_max_cache: Mutex::new(None),
            binding: path,
            dirty: false,
            last_recompute: None,
            #[cfg(test)]
            overview_computations: AtomicU64::new(0),
        }
    }

    pub(super) fn bind(&mut self, path: &Path) {
        if self.binding == path {
            return;
        }
        let _boundary = lock_recover(scan_commit_boundary());
        {
            let next = Arc::new(ScanEpoch::new());
            let mut current = lock_recover(&self.scan_epoch);
            current.cancel();
            *current = next;
        }
        lock_recover(&self.sizes).clear();
        lock_recover(&self.counts).clear();
        self.binding = path.to_path_buf();
        self.dirty = false;
        self.last_recompute = None;
        self.revision.fetch_add(1, Ordering::Release);
    }

    pub(super) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub(super) fn refresh(&mut self, input: SizeScanInput<'_>, forced: bool) {
        self.bind(input.path);
        self.dirty = self.schedule(&input, forced);
        self.last_recompute = Some(Instant::now());
    }

    pub(super) fn poll(&mut self, input: SizeScanInput<'_>) {
        if !self.dirty {
            return;
        }
        let due = self
            .last_recompute
            .is_none_or(|last| last.elapsed() >= SIZES_DEBOUNCE);
        if due {
            self.last_recompute = Some(Instant::now());
            self.dirty = self.schedule(&input, false);
            crate::watcher_health::record_size_reconciliation();
        } else if let Some(wake) = input.notify {
            wake();
        }
    }

    pub(super) fn snapshot(&self) -> Arc<SizeSnapshot> {
        let revision = self.revision.load(Ordering::Acquire);
        if let Some(snapshot) = lock_recover(&self.snapshot_cache).as_ref()
            && snapshot.revision == revision
        {
            return Arc::clone(snapshot);
        }

        let _boundary = lock_recover(scan_commit_boundary());
        let revision = self.revision.load(Ordering::Acquire);
        if let Some(snapshot) = lock_recover(&self.snapshot_cache).as_ref()
            && snapshot.revision == revision
        {
            return Arc::clone(snapshot);
        }
        let snapshot = Arc::new(SizeSnapshot {
            revision,
            sizes: lock_recover(&self.sizes).clone(),
            counts: lock_recover(&self.counts).clone(),
        });
        *lock_recover(&self.snapshot_cache) = Some(Arc::clone(&snapshot));
        snapshot
    }

    pub(super) fn folder_overview(
        &self,
        listing_revision: u64,
        entries: &[FileEntry],
    ) -> FolderOverview {
        let snapshot = self.snapshot();
        if let Some(cache) = lock_recover(&self.overview_cache).as_ref()
            && cache.listing_revision == listing_revision
            && cache.size_revision == snapshot.revision()
        {
            return cache.value.clone();
        }

        #[cfg(test)]
        self.overview_computations.fetch_add(1, Ordering::Relaxed);

        let mut directory_count = 0usize;
        let mut computed_directories = 0usize;
        let mut total = 0u64;
        let mut largest: Option<(usize, u64)> = None;
        let mut oldest: Option<(usize, SystemTime)> = None;
        for (index, entry) in entries.iter().enumerate() {
            let size = if entry.is_dir {
                directory_count += 1;
                match snapshot.size_of(&entry.path) {
                    Some(size) => {
                        computed_directories += 1;
                        total = total.saturating_add(size);
                        size
                    }
                    None => 0,
                }
            } else {
                total = total.saturating_add(entry.size);
                entry.size
            };
            if largest.is_none_or(|(_, current)| size > current) {
                largest = Some((index, size));
            }
            if let Some(modified) = entry.modified
                && oldest.is_none_or(|(_, current)| modified < current)
            {
                oldest = Some((index, modified));
            }
        }
        let value = FolderOverview {
            total: if computed_directories == 0 && directory_count > 0 {
                None
            } else {
                Some(total)
            },
            largest: largest.map(|(index, size)| (entries[index].name.clone(), size)),
            oldest: oldest.map(|(index, modified)| (entries[index].name.clone(), modified)),
        };
        *lock_recover(&self.overview_cache) = Some(OverviewCache {
            listing_revision,
            size_revision: snapshot.revision(),
            value: value.clone(),
        });
        value
    }

    pub(super) fn max_display_size(
        &self,
        listing_revision: u64,
        entries: &[FileEntry],
        filtered: &Arc<[usize]>,
    ) -> u64 {
        let snapshot = self.snapshot();
        let filtered_identity = Arc::downgrade(filtered);
        if let Some(cache) = lock_recover(&self.display_max_cache).as_ref()
            && cache.listing_revision == listing_revision
            && cache.size_revision == snapshot.revision()
            && cache.filtered.strong_count() > 0
            && Weak::ptr_eq(&cache.filtered, &filtered_identity)
        {
            return cache.value;
        }

        let value = filtered
            .iter()
            .filter_map(|index| entries.get(*index))
            .map(|entry| snapshot.display_size(entry))
            .max()
            .unwrap_or(0);
        *lock_recover(&self.display_max_cache) = Some(DisplayMaxCache {
            listing_revision,
            size_revision: snapshot.revision(),
            filtered: filtered_identity,
            value,
        });
        value
    }

    fn schedule(&self, input: &SizeScanInput<'_>, forced: bool) -> bool {
        let retained_paths: HashSet<PathBuf> = input
            .entries
            .iter()
            .filter(|entry| entry.is_dir)
            .map(|entry| entry.path.clone())
            .collect();
        let scan_epoch = begin_panel_scan(
            &self.scan_epoch,
            &self.sizes,
            &self.counts,
            &retained_paths,
            &self.revision,
        );

        let mut need_count = Vec::new();
        let mut need_size = Vec::new();
        let mut retry = false;

        for entry in input.entries {
            if !entry.is_dir {
                continue;
            }
            need_count.push(entry.path.clone());

            let dir_mtime = entry.modified;
            let cache_key = VolumePathKey::observe(&entry.path);
            if let Some(mtime) = dir_mtime
                && publish_cached_size_if_current(
                    &self.scan_epoch,
                    &scan_epoch,
                    &self.sizes,
                    &entry.path,
                    &cache_key,
                    Some(mtime),
                    &self.revision,
                )
            {
                continue;
            }

            #[cfg(test)]
            let guards_enabled = std::env::var("COMMANDER_DISABLE_WALK_GUARDS").is_err();
            #[cfg(not(test))]
            let guards_enabled = true;

            let mut skip = false;
            if guards_enabled {
                let _boundary = lock_recover(scan_commit_boundary());
                if !scan_is_current(&self.scan_epoch, &scan_epoch) {
                    return retry;
                }
                if let Some(&(when, cost)) = lock_recover(walk_log()).get(&cache_key) {
                    if when.elapsed() < WALK_COOLDOWN {
                        skip = true;
                        retry = true;
                    } else if !forced && cost > WALK_EXPENSIVE {
                        skip = true;
                    }
                }
            }
            if skip {
                publish_cached_size_if_current(
                    &self.scan_epoch,
                    &scan_epoch,
                    &self.sizes,
                    &entry.path,
                    &cache_key,
                    None,
                    &self.revision,
                );
                continue;
            }
            need_size.push((entry.path.clone(), dir_mtime, cache_key));
        }

        let visible_paths: HashSet<PathBuf> =
            visible_window(input.filtered.len(), input.scroll_anchor, input.page_rows)
                .filter_map(|index| {
                    input
                        .filtered
                        .get(index)
                        .and_then(|entry| input.entries.get(*entry))
                })
                .filter(|entry| entry.is_dir)
                .map(|entry| entry.path.clone())
                .collect();
        let (visible_counts, background_counts): (Vec<_>, Vec<_>) = need_count
            .into_iter()
            .partition(|path| visible_paths.contains(path));
        let (visible_sizes, background_sizes): (Vec<_>, Vec<_>) = need_size
            .into_iter()
            .partition(|(path, _, _)| visible_paths.contains(path));
        let worker_cap = crate::volume_profile::profile(input.path)
            .capabilities
            .max_concurrency
            .clamp(1, 4);

        if !visible_counts.is_empty() || !background_counts.is_empty() {
            let counts = Arc::clone(&self.counts);
            let current = Arc::clone(&self.scan_epoch);
            let epoch = Arc::clone(&scan_epoch);
            let revision = Arc::clone(&self.revision);
            let wake = input.notify.clone();
            fs_pool(worker_cap).spawn_fifo(move || {
                use rayon::prelude::*;
                fn count(path: &PathBuf, epoch: &ScanEpoch) -> Option<(PathBuf, usize)> {
                    if epoch.is_cancelled() {
                        return None;
                    }
                    let entries = std::fs::read_dir(path).ok()?;
                    let mut count = 0;
                    for entry in entries {
                        if epoch.is_cancelled() {
                            return None;
                        }
                        count += usize::from(entry.is_ok());
                    }
                    (!epoch.is_cancelled()).then(|| (path.clone(), count))
                }

                let visible_results: Vec<_> = visible_counts
                    .iter()
                    .filter_map(|path| count(path, &epoch))
                    .collect();
                if publish_scan_values_if_current(
                    &current,
                    &epoch,
                    &counts,
                    visible_results,
                    &revision,
                ) && let Some(wake) = &wake
                {
                    wake();
                }
                if epoch.is_cancelled() {
                    return;
                }
                let background_results: Vec<_> = background_counts
                    .par_iter()
                    .filter_map(|path| count(path, &epoch))
                    .collect();
                if publish_scan_values_if_current(
                    &current,
                    &epoch,
                    &counts,
                    background_results,
                    &revision,
                ) && let Some(wake) = &wake
                {
                    wake();
                }
            });
        }

        if !visible_sizes.is_empty() || !background_sizes.is_empty() {
            let sizes = Arc::clone(&self.sizes);
            let current = Arc::clone(&self.scan_epoch);
            let epoch = Arc::clone(&scan_epoch);
            let revision = Arc::clone(&self.revision);
            let wake = input.notify.clone();
            fs_pool(worker_cap).spawn_fifo(move || {
                use rayon::prelude::*;
                fn measure(
                    (path, modified, cache_key): &(PathBuf, Option<SystemTime>, VolumePathKey),
                    scan_epoch: &Arc<ScanEpoch>,
                ) -> Option<DirMeasurement> {
                    if scan_epoch.is_cancelled() {
                        return None;
                    }
                    let path_epoch = capture_path_epoch(cache_key);
                    let started = Instant::now();
                    let size = dir_size_recursive_until(path, || {
                        scan_epoch.is_cancelled() || path_epoch.is_cancelled()
                    })?;
                    let completed_at = Instant::now();
                    Some(DirMeasurement {
                        path: path.clone(),
                        modified: *modified,
                        cache_key: cache_key.clone(),
                        path_epoch,
                        size,
                        completed_at,
                        elapsed: completed_at.duration_since(started),
                    })
                }

                let visible_results: Vec<_> = visible_sizes
                    .iter()
                    .filter_map(|item| measure(item, &epoch))
                    .collect();
                let visible_outcome = publish_dir_measurements_if_current(
                    &current,
                    &epoch,
                    &sizes,
                    &visible_results,
                    &revision,
                );
                if visible_outcome.cache_changed {
                    flush_cache();
                }
                if visible_outcome.published > 0
                    && let Some(wake) = &wake
                {
                    wake();
                }
                if epoch.is_cancelled() {
                    return;
                }

                let background_results: Vec<_> = background_sizes
                    .par_iter()
                    .filter_map(|item| measure(item, &epoch))
                    .collect();
                let background_outcome = publish_dir_measurements_if_current(
                    &current,
                    &epoch,
                    &sizes,
                    &background_results,
                    &revision,
                );
                if background_outcome.cache_changed {
                    flush_cache();
                }
                if background_outcome.published > 0
                    && let Some(wake) = &wake
                {
                    wake();
                }
            });
        }

        retry
    }

    #[cfg(test)]
    fn insert_for_test(&self, path: PathBuf, size: u64, count: usize) {
        let _boundary = lock_recover(scan_commit_boundary());
        lock_recover(&self.sizes).insert(path.clone(), size);
        lock_recover(&self.counts).insert(path, count);
        self.revision.fetch_add(1, Ordering::Release);
    }

    #[cfg(test)]
    fn overview_computations(&self) -> u64 {
        self.overview_computations.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(super) fn binding_for_test(&self) -> &Path {
        &self.binding
    }

    #[cfg(test)]
    pub(super) fn mark_dirty_immediately_for_test(&mut self) {
        self.dirty = true;
        self.last_recompute = None;
    }
}

fn build_fs_pool(workers: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(move |index| format!("fs-worker-{workers}-{index}"))
        .build()
        .expect("filesystem worker pool")
}

fn fs_pool(workers: usize) -> &'static rayon::ThreadPool {
    static SERIAL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    static BALANCED: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    static FAST: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    match workers {
        1 => SERIAL.get_or_init(|| build_fs_pool(1)),
        2 => BALANCED.get_or_init(|| build_fs_pool(2)),
        _ => FAST.get_or_init(|| build_fs_pool(4)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &Path, is_dir: bool, size: u64) -> FileEntry {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        FileEntry {
            name_lower: name.to_lowercase(),
            name,
            path: path.to_path_buf(),
            identity: crate::panel::ListingIdentity::Unavailable,
            is_dir,
            size,
            extension: String::new(),
            modified: Some(SystemTime::UNIX_EPOCH),
            modified_str: String::new(),
            size_str: String::new(),
        }
    }

    #[test]
    fn stale_scan_cannot_publish_after_navigation() {
        let mut index = SizeIndex::new(PathBuf::from("/old"));
        let retained = HashSet::from([PathBuf::from("/old/dir")]);
        let old_epoch = begin_panel_scan(
            &index.scan_epoch,
            &index.sizes,
            &index.counts,
            &retained,
            &index.revision,
        );

        index.bind(Path::new("/new"));
        assert!(!publish_scan_values_if_current(
            &index.scan_epoch,
            &old_epoch,
            &index.sizes,
            [(PathBuf::from("/old/dir"), 99)],
            &index.revision,
        ));
        assert_eq!(index.snapshot().size_of(Path::new("/old/dir")), None);
    }

    #[test]
    fn overview_cache_reuses_and_invalidates_by_both_revisions() {
        let index = SizeIndex::new(PathBuf::from("/root"));
        let directory = PathBuf::from("/root/dir");
        let entries = vec![entry(&directory, true, 0)];
        index.insert_for_test(directory.clone(), 42, 1);

        let first = index.folder_overview(1, &entries);
        let second = index.folder_overview(1, &entries);
        assert_eq!(first, second);
        assert_eq!(index.overview_computations(), 1);

        let _ = index.folder_overview(2, &entries);
        assert_eq!(index.overview_computations(), 2);
        index.insert_for_test(directory, 84, 1);
        let changed = index.folder_overview(2, &entries);
        assert_eq!(changed.total, Some(84));
        assert_eq!(index.overview_computations(), 3);
    }
}
