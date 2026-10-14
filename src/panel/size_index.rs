use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Instant, SystemTime};

use super::{
    DirMeasurement, FileEntry, FolderOverview, Notify, ScanEpoch, VolumePathKey, WALK_COOLDOWN,
    WALK_EXPENSIVE, begin_panel_scan, capture_path_epoch, dir_size_walk_until, flush_cache,
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
    pub const fn revision(&self) -> u64 {
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
    /// Whether something since the last scan may have changed child counts
    /// (a watcher event or refresh). A dirty flag kept only to retry walks
    /// held back by the cooldown does not re-read every subfolder's count.
    recount: bool,
    last_recompute: Option<Instant>,
    /// A delayed wake for the pending debounce is already scheduled.
    debounce_wake: Arc<AtomicBool>,
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
            recount: false,
            last_recompute: None,
            debounce_wake: Arc::new(AtomicBool::new(false)),
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
        self.recount = false;
        self.last_recompute = None;
        self.revision.fetch_add(1, Ordering::Release);
    }

    pub(super) const fn mark_dirty(&mut self) {
        self.dirty = true;
        self.recount = true;
    }

    pub(super) fn refresh(&mut self, input: SizeScanInput<'_>, forced: bool) {
        self.bind(input.path);
        self.dirty = self.schedule(&input, forced, true);
        self.recount = false;
        self.last_recompute = Some(Instant::now());
    }

    pub(super) fn poll(&mut self, input: SizeScanInput<'_>) {
        if !self.dirty {
            return;
        }
        let remaining = self
            .last_recompute
            .and_then(|last| SIZES_DEBOUNCE.checked_sub(last.elapsed()))
            .filter(|remaining| !remaining.is_zero());
        if let Some(remaining) = remaining {
            // Not due yet: wake once when it is, rather than repainting
            // every frame until then.
            if let Some(wake) = input.notify {
                self.schedule_debounce_wake(remaining, wake);
            }
            return;
        }
        self.last_recompute = Some(Instant::now());
        let recount = std::mem::take(&mut self.recount);
        self.dirty = self.schedule(&input, false, recount);
        crate::watcher_health::record_size_reconciliation();
    }

    fn schedule_debounce_wake(&self, delay: std::time::Duration, wake: Notify) {
        if self.debounce_wake.swap(true, Ordering::AcqRel) {
            return;
        }
        let pending = Arc::clone(&self.debounce_wake);
        let fallback = Arc::clone(&wake);
        let spawned = std::thread::Builder::new()
            .name("commander-size-debounce".to_string())
            .spawn(move || {
                std::thread::sleep(delay);
                pending.store(false, Ordering::Release);
                wake();
            });
        if let Err(error) = spawned {
            self.debounce_wake.store(false, Ordering::Release);
            log::warn!(
                target: "commander::sizes",
                "failed to spawn size debounce wake thread: {error}"
            );
            fallback();
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
            let size = if entry.is_dir && entry.is_symlink() {
                // Never measured (see `schedule`) and not part of the total.
                0
            } else if entry.is_dir {
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

    /// Subfolders whose child count should be (re)read. Without `recount`
    /// (a retry of walks held back by the cooldown) counts already known
    /// stay as they are.
    fn dirs_needing_count(&self, entries: &[FileEntry], recount: bool) -> Vec<PathBuf> {
        let dirs = entries.iter().filter(|entry| entry.is_dir);
        if recount {
            return dirs.map(|entry| entry.path.clone()).collect();
        }
        let counts = lock_recover(&self.counts);
        dirs.filter(|entry| !counts.contains_key(&entry.path))
            .map(|entry| entry.path.clone())
            .collect()
    }

    fn schedule(&self, input: &SizeScanInput<'_>, forced: bool, recount: bool) -> bool {
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

        let need_count = self.dirs_needing_count(input.entries, recount);
        let mut need_size = Vec::new();
        let mut retry = false;
        let profile = crate::volume_profile::profile(input.path);

        for entry in input.entries {
            if !entry.is_dir {
                continue;
            }
            // A folder symlink shows its target's type, but its target is
            // not part of this folder: walking it (a link to `/` or a big
            // tree) would inflate sizes and totals, so it gets no size.
            if entry.is_symlink() {
                continue;
            }

            let dir_mtime = entry.modified;
            let cache_key = VolumePathKey::for_listed_child(&entry.path, &entry.identity, &profile);
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
        let has_counts = !visible_counts.is_empty() || !background_counts.is_empty();
        let has_sizes = !visible_sizes.is_empty() || !background_sizes.is_empty();
        if !has_counts && !has_sizes {
            return retry;
        }
        let Some(pool) = fs_pool(&profile) else {
            // No worker threads: sizes and counts stay unknown this time.
            return retry;
        };

        if has_counts {
            let counts = Arc::clone(&self.counts);
            let current = Arc::clone(&self.scan_epoch);
            let epoch = Arc::clone(&scan_epoch);
            let revision = Arc::clone(&self.revision);
            let wake = input.notify.clone();
            pool.spawn_fifo(move || {
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

        if has_sizes {
            let sizes = Arc::clone(&self.sizes);
            let current = Arc::clone(&self.scan_epoch);
            let epoch = Arc::clone(&scan_epoch);
            let revision = Arc::clone(&self.revision);
            let wake = input.notify.clone();
            pool.spawn_fifo(move || {
                use rayon::prelude::*;
                fn measure(
                    (path, modified, cache_key): &(PathBuf, Option<SystemTime>, VolumePathKey),
                    scan_epoch: &Arc<ScanEpoch>,
                    background: bool,
                ) -> Option<DirMeasurement> {
                    if scan_epoch.is_cancelled() {
                        return None;
                    }
                    let path_epoch = capture_path_epoch(cache_key);
                    let started = Instant::now();
                    let walk = dir_size_walk_until(path, background, || {
                        scan_epoch.is_cancelled() || path_epoch.is_cancelled()
                    })?;
                    let completed_at = Instant::now();
                    Some(DirMeasurement {
                        path: path.clone(),
                        // A walk that skipped unreadable parts undercounts:
                        // show it, but do not cache it as the exact size.
                        modified: *modified,
                        complete: walk.complete,
                        cache_key: cache_key.clone(),
                        path_epoch,
                        size: walk.size,
                        completed_at,
                        elapsed: completed_at.duration_since(started),
                    })
                }

                // Visible folders are measured in parallel and each shows
                // as soon as its walk finishes, so one huge folder does not
                // hold back the sizes of the others on screen.
                let cache_changed = AtomicBool::new(false);
                visible_sizes.par_iter().for_each(|item| {
                    let Some(result) = measure(item, &epoch, false) else {
                        return;
                    };
                    let outcome = publish_dir_measurements_if_current(
                        &current,
                        &epoch,
                        &sizes,
                        std::slice::from_ref(&result),
                        &revision,
                    );
                    if outcome.cache_changed {
                        cache_changed.store(true, Ordering::Relaxed);
                    }
                    if outcome.published > 0
                        && let Some(wake) = &wake
                    {
                        wake();
                    }
                });

                // Persist the visible sizes before the background walks,
                // which can take long (or hang on a slow subtree) and would
                // otherwise hold them back until they return, or lose them
                // if the app exits first. Nothing changed means no rewrite.
                if !background_sizes.is_empty() && cache_changed.swap(false, Ordering::Relaxed) {
                    flush_cache();
                }

                if !epoch.is_cancelled() {
                    let background_results: Vec<_> = background_sizes
                        .par_iter()
                        .filter_map(|item| measure(item, &epoch, true))
                        .collect();
                    let background_outcome = publish_dir_measurements_if_current(
                        &current,
                        &epoch,
                        &sizes,
                        &background_results,
                        &revision,
                    );
                    if background_outcome.cache_changed {
                        cache_changed.store(true, Ordering::Relaxed);
                    }
                    if background_outcome.published > 0
                        && let Some(wake) = &wake
                    {
                        wake();
                    }
                }
                // At most one more rewrite, for what the background added.
                if cache_changed.into_inner() {
                    flush_cache();
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
    pub(super) const fn mark_dirty_immediately_for_test(&mut self) {
        self.dirty = true;
        self.last_recompute = None;
    }
}

/// A worker pool, or `None` when the OS refuses to start its threads (sizes
/// and counts then stay unknown rather than panicking the UI thread). A job
/// that panics is logged and ends there instead of aborting the process.
fn build_fs_pool(workers: usize, label: &'static str) -> Option<rayon::ThreadPool> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(move |index| format!("fs-{label}-{index}"))
        .panic_handler(move |_| {
            log::error!(target: "commander::sizes", "a {label} size/count job panicked");
        })
        .build()
        .map_err(|error| {
            log::warn!(
                target: "commander::sizes",
                "cannot start the {label} filesystem worker pool: {error}"
            );
        })
        .ok()
}

/// Size/count pools by volume class. Remote volumes get their own pool, so a
/// walk hung on a dead share never pins the workers that local slow and
/// removable disks rely on.
fn fs_pool(profile: &crate::volume_profile::VolumeProfile) -> Option<&'static rayon::ThreadPool> {
    static SERIAL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
    static BALANCED: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
    static REMOTE: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
    static FAST: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
    let workers = profile.capabilities.max_concurrency.clamp(1, 4);
    let pool = if profile.backend == crate::volume_profile::BackendKind::Remote {
        REMOTE.get_or_init(|| build_fs_pool(workers.min(2), "remote"))
    } else {
        match workers {
            1 => SERIAL.get_or_init(|| build_fs_pool(1, "serial")),
            2 => BALANCED.get_or_init(|| build_fs_pool(2, "balanced")),
            _ => FAST.get_or_init(|| build_fs_pool(4, "fast")),
        }
    };
    pool.as_ref()
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

    #[cfg(unix)]
    #[test]
    fn folder_symlinks_are_not_size_walked_or_totalled() {
        let tmp = crate::testutil::TempDir::new();
        let real = tmp.dir("real");
        tmp.file("real/a.bin", "12345");
        std::os::unix::fs::symlink("real", tmp.path().join("link")).unwrap();
        let crate::panel::DirectoryRead::Complete(mut entries) =
            crate::panel::PanelState::read_dir(tmp.path(), true)
        else {
            panic!("listing should be complete");
        };
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let link = entries.iter().find(|e| e.name == "link").unwrap();
        assert!(link.is_dir && link.is_symlink());

        let index = SizeIndex::new(tmp.path().to_path_buf());
        let filtered: Arc<[usize]> = (0..entries.len()).collect::<Vec<_>>().into();
        index.schedule(
            &SizeScanInput {
                path: tmp.path(),
                entries: &entries,
                filtered,
                scroll_anchor: 0,
                page_rows: 10,
                notify: None,
            },
            true,
            true,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while index.snapshot().size_of(&real).is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "real folder never sized"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Both were scheduled together; the link was never queued for a walk.
        assert_eq!(index.snapshot().size_of(&link.path), None);
        assert_eq!(index.folder_overview(1, &entries).total, Some(5));
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

    fn profile_for(
        backend: crate::volume_profile::BackendKind,
        workers: usize,
    ) -> crate::volume_profile::VolumeProfile {
        crate::volume_profile::VolumeProfile {
            volume_id: 1,
            generation: 1,
            backend,
            filesystem: String::new(),
            mount_point: PathBuf::from("/"),
            read_only: false,
            case_sensitive: None,
            capabilities: crate::volume_profile::VolumeCapabilities {
                atomic_rename: true,
                clone: false,
                sparse: false,
                resumable: true,
                delta: false,
                max_concurrency: workers,
            },
            reason: String::new(),
        }
    }

    #[test]
    fn remote_volumes_get_their_own_size_pool() {
        use crate::volume_profile::BackendKind;
        let remote = fs_pool(&profile_for(BackendKind::Remote, 2)).unwrap();
        let removable = fs_pool(&profile_for(BackendKind::Removable, 2)).unwrap();
        let slow = fs_pool(&profile_for(BackendKind::LocalSlow, 2)).unwrap();
        assert!(!std::ptr::eq(remote, removable));
        assert!(std::ptr::eq(removable, slow));
        assert_eq!(remote.current_num_threads(), 2);
    }

    #[test]
    fn a_panicking_size_job_does_not_take_down_the_pool() {
        let pool = build_fs_pool(1, "panic-test").unwrap();
        pool.spawn_fifo(|| panic!("size job failure"));
        let (sender, receiver) = std::sync::mpsc::channel();
        pool.spawn_fifo(move || {
            let _ = sender.send(());
        });
        receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the pool keeps running jobs after a panic");
    }

    #[cfg(unix)]
    #[test]
    fn listed_child_keys_match_observed_keys() {
        let tmp = crate::testutil::TempDir::new();
        tmp.dir("sub");
        let crate::panel::DirectoryRead::Complete(entries) =
            crate::panel::PanelState::read_dir(tmp.path(), true)
        else {
            panic!("listing should be complete");
        };
        let sub = entries.iter().find(|entry| entry.name == "sub").unwrap();
        let parent = crate::volume_profile::profile(tmp.path());
        assert_eq!(
            VolumePathKey::for_listed_child(&sub.path, &sub.identity, &parent),
            VolumePathKey::observe(&sub.path)
        );

        // A child on another device (a mount point) is observed, not derived.
        let mut elsewhere = parent.clone();
        elsewhere.volume_id = parent.volume_id.wrapping_add(1);
        elsewhere.generation = u64::MAX;
        let key = VolumePathKey::for_listed_child(&sub.path, &sub.identity, &elsewhere);
        assert_eq!(key, VolumePathKey::observe(&sub.path));
        // Without a captured identity the folder is observed too.
        let key = VolumePathKey::for_listed_child(
            &sub.path,
            &crate::panel::ListingIdentity::Unavailable,
            &elsewhere,
        );
        assert_ne!(key.generation, u64::MAX);
    }

    #[test]
    fn cooldown_retries_do_not_recount_known_folders() {
        let index = SizeIndex::new(PathBuf::from("/root"));
        let known = PathBuf::from("/root/known");
        let fresh = PathBuf::from("/root/fresh");
        let entries = vec![
            entry(&known, true, 0),
            entry(&fresh, true, 0),
            entry(Path::new("/root/file"), false, 3),
        ];
        index.insert_for_test(known.clone(), 1, 4);

        assert_eq!(
            index.dirs_needing_count(&entries, false),
            vec![fresh.clone()]
        );
        assert_eq!(index.dirs_needing_count(&entries, true), vec![known, fresh]);
    }

    #[test]
    fn debounced_sizes_wake_once_instead_of_every_frame() {
        let mut index = SizeIndex::new(PathBuf::from("/root"));
        index.mark_dirty();
        index.last_recompute = Some(Instant::now());
        let wakes = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&wakes);
        let notify: Notify = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let filtered: Arc<[usize]> = Arc::from(Vec::new());
        for _ in 0..20 {
            index.poll(SizeScanInput {
                path: Path::new("/root"),
                entries: &[],
                filtered: Arc::clone(&filtered),
                scroll_anchor: 0,
                page_rows: 10,
                notify: Some(Arc::clone(&notify)),
            });
        }
        assert_eq!(wakes.load(Ordering::SeqCst), 0, "no wake per frame");
        assert!(index.dirty, "still waiting for the debounce");

        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while wakes.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "the delayed wake never came");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }
}
