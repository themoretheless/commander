use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::Hash;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::SystemTime;

mod listing;
mod listing_job;
mod selection;
mod size_index;
mod sort;
mod view;
mod watcher;

use listing::ListingState;
use listing_job::{ListingJobController, PendingFocus};
use selection::{Focus, SelectionState};
pub use size_index::SizeSnapshot;
use size_index::{SizeIndex, SizeScanInput};
#[cfg(test)]
use sort::natural_cmp;
use sort::sort_entries;
pub use view::ViewConfig;
use view::{ViewSettings, ViewState};
use watcher::DirectoryWatcherState;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct VolumePathKey {
    path: PathBuf,
    volume_id: u64,
    generation: u64,
}

impl VolumePathKey {
    fn observe(path: &Path) -> Self {
        let profile = crate::volume_profile::profile(path);
        Self {
            path: path.to_path_buf(),
            volume_id: profile.volume_id,
            generation: profile.generation,
        }
    }
}

/// On-disk entry: mount identity plus mtime and measured size.
#[derive(Serialize, Deserialize)]
struct PersistedCacheEntry {
    key: VolumePathKey,
    mtime_secs: u64,
    mtime_nanos: u32,
    size: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct PersistedCache {
    schema: u32,
    entries: Vec<PersistedCacheEntry>,
}

fn cache_path() -> PathBuf {
    #[cfg(test)]
    let dir = std::env::temp_dir().join(format!("commander-test-cache-{}", std::process::id()));
    #[cfg(not(test))]
    let dir = crate::fs_util::storage_root_override().map_or_else(
        || {
            dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("commander")
        },
        |root| root.join("cache"),
    );
    let _ = fs::create_dir_all(&dir);
    dir.join("dir_sizes.json")
}

/// Global cache: volume generation + path -> (mtime, size).
/// Loaded from disk on first access, saved on every update.
fn dir_size_cache() -> &'static Mutex<HashMap<VolumePathKey, (SystemTime, u64)>> {
    static CACHE: OnceLock<Mutex<HashMap<VolumePathKey, (SystemTime, u64)>>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let map = load_cache_from_disk();
        Mutex::new(map)
    })
}

fn load_cache_from_disk() -> HashMap<VolumePathKey, (SystemTime, u64)> {
    let path = cache_path();
    let Ok(file) = fs::File::open(path) else {
        return HashMap::new();
    };
    load_cache_from_reader_with_bounds(
        std::io::BufReader::new(file),
        DIR_SIZE_CACHE_LIMIT,
        DIR_SIZE_CACHE_RETAIN,
    )
    .unwrap_or_default()
}

struct BoundedCacheAccumulator {
    candidates: BoundedUniqueMap<VolumePathKey, (SystemTime, u64)>,
}

impl BoundedCacheAccumulator {
    fn new(limit: usize, retain: usize) -> Self {
        Self {
            candidates: BoundedUniqueMap::new(limit, retain),
        }
    }

    fn push(&mut self, entry: PersistedCacheEntry) {
        let Some(mtime) = std::time::UNIX_EPOCH.checked_add(std::time::Duration::new(
            entry.mtime_secs,
            entry.mtime_nanos,
        )) else {
            return;
        };
        self.candidates
            .insert_by(entry.key, (mtime, entry.size), &mut compare_cache_entries);
    }

    fn finish(self) -> HashMap<VolumePathKey, (SystemTime, u64)> {
        self.candidates.finish_by(&mut compare_cache_entries)
    }
}

fn compare_cache_entries(
    key_a: &VolumePathKey,
    value_a: &(SystemTime, u64),
    key_b: &VolumePathKey,
    value_b: &(SystemTime, u64),
) -> Ordering {
    value_a
        .0
        .cmp(&value_b.0)
        .then_with(|| compare_volume_path_keys(key_a, key_b))
        .then(value_a.1.cmp(&value_b.1))
}

struct PersistedEntriesSeed<'a> {
    accumulator: &'a mut BoundedCacheAccumulator,
}

impl<'de> serde::de::DeserializeSeed<'de> for PersistedEntriesSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(PersistedEntriesVisitor {
            accumulator: self.accumulator,
        })
    }
}

struct PersistedEntriesVisitor<'a> {
    accumulator: &'a mut BoundedCacheAccumulator,
}

impl<'de> serde::de::Visitor<'de> for PersistedEntriesVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an array of persisted directory-size entries")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        while let Some(entry) = sequence.next_element::<PersistedCacheEntry>()? {
            self.accumulator.push(entry);
        }
        Ok(())
    }
}

struct PersistedCacheVisitor {
    limit: usize,
    retain: usize,
}

impl<'de> serde::de::Visitor<'de> for PersistedCacheVisitor {
    type Value = HashMap<VolumePathKey, (SystemTime, u64)>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a Commander directory-size cache object")
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut schema = None;
        let mut saw_entries = false;
        let mut accumulator = BoundedCacheAccumulator::new(self.limit, self.retain);

        while let Some(field) = object.next_key::<String>()? {
            match field.as_str() {
                "schema" => {
                    if schema.is_some() {
                        return Err(serde::de::Error::duplicate_field("schema"));
                    }
                    schema = Some(object.next_value::<u32>()?);
                }
                "entries" => {
                    if saw_entries {
                        return Err(serde::de::Error::duplicate_field("entries"));
                    }
                    saw_entries = true;
                    object.next_value_seed(PersistedEntriesSeed {
                        accumulator: &mut accumulator,
                    })?;
                }
                _ => {
                    object.next_value::<serde::de::IgnoredAny>()?;
                }
            }
        }

        Ok(if schema == Some(1) {
            accumulator.finish()
        } else {
            HashMap::new()
        })
    }
}

fn load_cache_from_reader_with_bounds(
    reader: impl Read,
    limit: usize,
    retain: usize,
) -> Result<HashMap<VolumePathKey, (SystemTime, u64)>, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let cache = serde::Deserializer::deserialize_map(
        &mut deserializer,
        PersistedCacheVisitor { limit, retain },
    )?;
    deserializer.end()?;
    Ok(cache)
}

/// When each directory was last size-walked and how long the walk took.
/// Shared across panels. Guards against the watcher-noise feedback loop:
/// system writes deep in huge dirs (e.g. ~/Library) keep invalidating
/// their cached size, and re-walking them on every event burns CPU/disk
/// forever. Recently-walked dirs wait out a cooldown; dirs whose walk is
/// expensive are only re-walked on an explicit refresh.
fn walk_log() -> &'static Mutex<HashMap<VolumePathKey, (std::time::Instant, std::time::Duration)>> {
    static LOG: OnceLock<Mutex<HashMap<VolumePathKey, (std::time::Instant, std::time::Duration)>>> =
        OnceLock::new();
    LOG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_flush_lock() -> &'static Mutex<()> {
    static FLUSH: Mutex<()> = Mutex::new(());
    &FLUSH
}

const WALK_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10);
const WALK_EXPENSIVE: std::time::Duration = std::time::Duration::from_secs(2);
const WALK_LOG_LIMIT: usize = 4_096;
const WALK_LOG_RETAIN: usize = 3_584;
const DIR_SIZE_CACHE_LIMIT: usize = 10_000;
const DIR_SIZE_CACHE_RETAIN: usize = 9_000;
const DEFAULT_VISIBLE_ROWS: usize = 32;
pub(crate) const WATCHER_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

fn compare_volume_path_keys(a: &VolumePathKey, b: &VolumePathKey) -> Ordering {
    a.path
        .cmp(&b.path)
        .then(a.volume_id.cmp(&b.volume_id))
        .then(a.generation.cmp(&b.generation))
}

/// These mutexes protect advisory caches and cancellable scan state. Recovering
/// their values after a panic is safe: every commit revalidates both epoch
/// identities, and a subsequent scan replaces panel-local snapshots.
fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Serializes scan replacement, watcher invalidation and result publication.
/// The boundary closes the check-before-lock race across all participating
/// maps; filesystem traversal itself never holds it.
fn scan_commit_boundary() -> &'static Mutex<()> {
    static BOUNDARY: Mutex<()> = Mutex::new(());
    &BOUNDARY
}

#[derive(Debug)]
struct ScanEpoch {
    cancelled: AtomicBool,
}

impl ScanEpoch {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, AtomicOrdering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(AtomicOrdering::Acquire)
    }
}

/// Weak identities keep per-path invalidation state only while a walk is
/// active. Identity comparison avoids counter wraparound/ABA entirely.
fn active_path_epochs() -> &'static Mutex<HashMap<VolumePathKey, Weak<ScanEpoch>>> {
    static EPOCHS: OnceLock<Mutex<HashMap<VolumePathKey, Weak<ScanEpoch>>>> = OnceLock::new();
    EPOCHS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn capture_path_epoch(key: &VolumePathKey) -> Arc<ScanEpoch> {
    let mut epochs = lock_recover(active_path_epochs());
    epochs.retain(|_, epoch| epoch.strong_count() > 0);
    if let Some(epoch) = epochs.get(key).and_then(Weak::upgrade)
        && !epoch.is_cancelled()
    {
        return epoch;
    }

    let epoch = Arc::new(ScanEpoch::new());
    epochs.insert(key.clone(), Arc::downgrade(&epoch));
    epoch
}

fn scan_is_current(current: &Mutex<Arc<ScanEpoch>>, candidate: &Arc<ScanEpoch>) -> bool {
    !candidate.is_cancelled() && Arc::ptr_eq(&lock_recover(current), candidate)
}

fn begin_panel_scan(
    current: &Mutex<Arc<ScanEpoch>>,
    sizes: &Mutex<HashMap<PathBuf, u64>>,
    counts: &Mutex<HashMap<PathBuf, usize>>,
    retained_paths: &HashSet<PathBuf>,
    revision: &AtomicU64,
) -> Arc<ScanEpoch> {
    let _boundary = lock_recover(scan_commit_boundary());
    let next = Arc::new(ScanEpoch::new());
    {
        let mut current = lock_recover(current);
        current.cancel();
        *current = Arc::clone(&next);
    }
    let mut sizes = lock_recover(sizes);
    let mut counts = lock_recover(counts);
    let old_sizes = sizes.len();
    let old_counts = counts.len();
    sizes.retain(|path, _| retained_paths.contains(path));
    counts.retain(|path, _| retained_paths.contains(path));
    if sizes.len() != old_sizes || counts.len() != old_counts {
        revision.fetch_add(1, AtomicOrdering::Release);
    }
    next
}

fn hash_map_capacity_upper_bound(limit: usize) -> usize {
    if limit == 0 {
        0
    } else {
        limit.saturating_mul(2).max(3)
    }
}

fn partial_retain_newest<T>(
    values: &mut Vec<T>,
    retain: usize,
    compare: &mut impl FnMut(&T, &T) -> Ordering,
) {
    if values.len() <= retain {
        return;
    }
    if retain == 0 {
        values.clear();
        return;
    }

    let discard = values.len() - retain;
    values.select_nth_unstable_by(discard, compare);
    values.drain(..discard);
}

fn shrink_bounded_map<K: Eq + Hash, V>(map: &mut HashMap<K, V>, limit: usize) {
    map.shrink_to(limit);
    debug_assert!(map.len() <= limit);
    debug_assert!(map.capacity() <= hash_map_capacity_upper_bound(limit));
}

fn insert_comparator_max_by<K, V>(
    map: &mut HashMap<K, V>,
    key: K,
    value: V,
    compare: &mut impl FnMut(&K, &V, &K, &V) -> Ordering,
) where
    K: Eq + Hash,
{
    let replace = map
        .get_key_value(&key)
        .is_none_or(|(current_key, current_value)| {
            compare(current_key, current_value, &key, &value) == Ordering::Less
        });
    if replace {
        map.insert(key, value);
    }
}

fn retain_top_map_by<K, V>(
    map: &mut HashMap<K, V>,
    retain: usize,
    compare: &mut impl FnMut(&K, &V, &K, &V) -> Ordering,
) where
    K: Eq + Hash,
{
    if map.len() <= retain {
        return;
    }

    let mut candidates: Vec<_> = map.drain().collect();
    partial_retain_newest(
        &mut candidates,
        retain,
        &mut |(key_a, value_a), (key_b, value_b)| compare(key_a, value_a, key_b, value_b),
    );
    map.extend(candidates);
}

/// Keeps one comparator-max value per key. Once unique cardinality crosses
/// `limit`, discarded values cannot reach the final top set because retained
/// values are only replaced by comparator-greater duplicates.
struct BoundedUniqueMap<K, V> {
    entries: HashMap<K, V>,
    limit: usize,
    retain: usize,
    overflowed: bool,
}

impl<K, V> BoundedUniqueMap<K, V>
where
    K: Eq + Hash,
{
    fn new(limit: usize, retain: usize) -> Self {
        Self::from_map(HashMap::with_capacity(limit), limit, retain)
    }

    fn from_map(entries: HashMap<K, V>, limit: usize, retain: usize) -> Self {
        Self {
            entries,
            limit,
            retain: retain.min(limit),
            overflowed: false,
        }
    }

    fn insert_by(
        &mut self,
        key: K,
        value: V,
        compare: &mut impl FnMut(&K, &V, &K, &V) -> Ordering,
    ) {
        insert_comparator_max_by(&mut self.entries, key, value, compare);
        if self.entries.len() > self.limit {
            self.overflowed = true;
            retain_top_map_by(&mut self.entries, self.retain, compare);
        }
    }

    fn finish_by(mut self, compare: &mut impl FnMut(&K, &V, &K, &V) -> Ordering) -> HashMap<K, V> {
        if self.entries.len() > self.limit {
            self.overflowed = true;
            retain_top_map_by(&mut self.entries, self.retain, compare);
        }
        if self.overflowed {
            retain_top_map_by(&mut self.entries, self.retain, compare);
        }
        shrink_bounded_map(&mut self.entries, self.limit);
        self.entries
    }
}

fn extend_bounded_by<K, V>(
    map: &mut HashMap<K, V>,
    incoming: impl IntoIterator<Item = (K, V)>,
    limit: usize,
    retain: usize,
    mut compare: impl FnMut(&K, &V, &K, &V) -> Ordering,
) where
    K: Eq + Hash,
{
    let mut bounded = BoundedUniqueMap::from_map(std::mem::take(map), limit, retain);
    for (key, value) in incoming {
        bounded.insert_by(key, value, &mut compare);
    }
    *map = bounded.finish_by(&mut compare);
}

#[cfg(test)]
fn prune_walk_log_to(
    log: &mut HashMap<VolumePathKey, (std::time::Instant, std::time::Duration)>,
    limit: usize,
    retain: usize,
) {
    if log.len() > limit {
        retain_top_map_by(
            log,
            retain.min(limit),
            &mut |key_a, (when_a, duration_a), key_b, (when_b, duration_b)| {
                when_a
                    .cmp(when_b)
                    .then_with(|| compare_volume_path_keys(key_a, key_b))
                    .then(duration_a.cmp(duration_b))
            },
        );
    }
    shrink_bounded_map(log, limit);
}

fn prune_dir_size_cache_to(
    cache: &mut HashMap<VolumePathKey, (SystemTime, u64)>,
    limit: usize,
    retain: usize,
) {
    if cache.len() > limit {
        retain_top_map_by(cache, retain.min(limit), &mut compare_cache_entries);
    }
    shrink_bounded_map(cache, limit);
}

fn publish_scan_values_if_current<V>(
    current: &Mutex<Arc<ScanEpoch>>,
    epoch: &Arc<ScanEpoch>,
    target: &Mutex<HashMap<PathBuf, V>>,
    values: impl IntoIterator<Item = (PathBuf, V)>,
    revision: &AtomicU64,
) -> bool {
    let values: Vec<_> = values.into_iter().collect();
    if values.is_empty() {
        return false;
    }

    let _boundary = lock_recover(scan_commit_boundary());
    if !scan_is_current(current, epoch) {
        return false;
    }
    lock_recover(target).extend(values);
    revision.fetch_add(1, AtomicOrdering::Release);
    true
}

fn publish_cached_size_if_current(
    current: &Mutex<Arc<ScanEpoch>>,
    epoch: &Arc<ScanEpoch>,
    sizes: &Mutex<HashMap<PathBuf, u64>>,
    path: &Path,
    key: &VolumePathKey,
    expected_mtime: Option<SystemTime>,
    revision: &AtomicU64,
) -> bool {
    let _boundary = lock_recover(scan_commit_boundary());
    if !scan_is_current(current, epoch) {
        return false;
    }
    let cache = lock_recover(dir_size_cache());
    let Some(&(cached_mtime, cached_size)) = cache.get(key) else {
        return false;
    };
    if expected_mtime.is_some_and(|expected| cached_mtime != expected) {
        return false;
    }
    lock_recover(sizes).insert(path.to_path_buf(), cached_size);
    revision.fetch_add(1, AtomicOrdering::Release);
    true
}

fn dir_size_recursive_until(path: &Path, is_cancelled: impl Fn() -> bool) -> Option<u64> {
    let mut pending = vec![path.to_path_buf()];
    let mut total = 0u64;

    while let Some(directory) = pending.pop() {
        if is_cancelled() {
            return None;
        }
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries {
            if is_cancelled() {
                return None;
            }
            let Ok(entry) = entry else {
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if let Ok(metadata) = fs::symlink_metadata(entry.path()) {
                total = total.saturating_add(metadata.len());
            }
        }
    }

    (!is_cancelled()).then_some(total)
}

struct DirMeasurement {
    path: PathBuf,
    modified: Option<SystemTime>,
    cache_key: VolumePathKey,
    path_epoch: Arc<ScanEpoch>,
    size: u64,
    completed_at: std::time::Instant,
    elapsed: std::time::Duration,
}

#[derive(Clone, Copy)]
struct RankedDirSize {
    modified: SystemTime,
    completed_at: Option<std::time::Instant>,
    size: u64,
}

fn compare_ranked_dir_sizes(
    key_a: &VolumePathKey,
    value_a: &RankedDirSize,
    key_b: &VolumePathKey,
    value_b: &RankedDirSize,
) -> Ordering {
    if key_a == key_b {
        return value_a
            .completed_at
            .cmp(&value_b.completed_at)
            .then(value_a.modified.cmp(&value_b.modified))
            .then(value_a.size.cmp(&value_b.size));
    }

    value_a
        .modified
        .cmp(&value_b.modified)
        .then(value_a.completed_at.cmp(&value_b.completed_at))
        .then_with(|| compare_volume_path_keys(key_a, key_b))
        .then(value_a.size.cmp(&value_b.size))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct PublishOutcome {
    published: usize,
    cache_changed: bool,
}

fn publish_dir_measurements_if_current(
    current: &Mutex<Arc<ScanEpoch>>,
    scan_epoch: &Arc<ScanEpoch>,
    sizes: &Mutex<HashMap<PathBuf, u64>>,
    results: &[DirMeasurement],
    revision: &AtomicU64,
) -> PublishOutcome {
    if results.is_empty() {
        return PublishOutcome::default();
    }

    let _boundary = lock_recover(scan_commit_boundary());
    if !scan_is_current(current, scan_epoch) {
        return PublishOutcome::default();
    }

    let epochs = lock_recover(active_path_epochs());
    let valid: Vec<_> = results
        .iter()
        .filter(|result| {
            !result.path_epoch.is_cancelled()
                && epochs
                    .get(&result.cache_key)
                    .and_then(Weak::upgrade)
                    .is_some_and(|current| Arc::ptr_eq(&current, &result.path_epoch))
        })
        .collect();
    drop(epochs);
    if valid.is_empty() {
        return PublishOutcome::default();
    }

    let paths: HashSet<&Path> = valid.iter().map(|result| result.path.as_path()).collect();
    let keys: HashSet<&VolumePathKey> = valid.iter().map(|result| &result.cache_key).collect();
    {
        let mut log = lock_recover(walk_log());
        log.retain(|old, _| !paths.contains(old.path.as_path()) || keys.contains(old));
        extend_bounded_by(
            &mut log,
            valid.iter().map(|result| {
                (
                    result.cache_key.clone(),
                    (result.completed_at, result.elapsed),
                )
            }),
            WALK_LOG_LIMIT,
            WALK_LOG_RETAIN,
            |key_a, (when_a, duration_a), key_b, (when_b, duration_b)| {
                when_a
                    .cmp(when_b)
                    .then_with(|| compare_volume_path_keys(key_a, key_b))
                    .then(duration_a.cmp(duration_b))
            },
        );
    }

    let cache_changed = valid.iter().any(|result| result.modified.is_some());
    if cache_changed {
        let cache_paths: HashSet<&Path> = valid
            .iter()
            .filter(|result| result.modified.is_some())
            .map(|result| result.path.as_path())
            .collect();
        let cache_keys: HashSet<&VolumePathKey> = valid
            .iter()
            .filter(|result| result.modified.is_some())
            .map(|result| &result.cache_key)
            .collect();
        let mut cache = lock_recover(dir_size_cache());
        let previous = std::mem::take(&mut *cache);
        let previous = previous.into_iter().filter_map(|(key, (modified, size))| {
            (!cache_paths.contains(key.path.as_path()) || cache_keys.contains(&key)).then_some((
                key,
                RankedDirSize {
                    modified,
                    completed_at: None,
                    size,
                },
            ))
        });
        let incoming = valid.iter().filter_map(|result| {
            result.modified.map(|modified| {
                (
                    result.cache_key.clone(),
                    RankedDirSize {
                        modified,
                        completed_at: Some(result.completed_at),
                        size: result.size,
                    },
                )
            })
        });
        let mut ranked = HashMap::new();
        extend_bounded_by(
            &mut ranked,
            previous.chain(incoming),
            DIR_SIZE_CACHE_LIMIT,
            DIR_SIZE_CACHE_RETAIN,
            compare_ranked_dir_sizes,
        );
        cache.extend(
            ranked
                .into_iter()
                .map(|(key, value)| (key, (value.modified, value.size))),
        );
        shrink_bounded_map(&mut cache, DIR_SIZE_CACHE_LIMIT);
    }

    lock_recover(sizes).extend(
        valid
            .iter()
            .map(|result| (result.path.clone(), result.size)),
    );
    revision.fetch_add(1, AtomicOrdering::Release);
    {
        let mut epochs = lock_recover(active_path_epochs());
        for result in &valid {
            let owned_only_by_result = Arc::strong_count(&result.path_epoch) == 1;
            let still_registered = epochs
                .get(&result.cache_key)
                .and_then(Weak::upgrade)
                .is_some_and(|current| Arc::ptr_eq(&current, &result.path_epoch));
            if owned_only_by_result && still_registered {
                epochs.remove(&result.cache_key);
            }
        }
    }
    PublishOutcome {
        published: valid.len(),
        cache_changed,
    }
}

fn visible_window(total: usize, anchor: usize, page_rows: usize) -> std::ops::Range<usize> {
    let rows = page_rows.max(DEFAULT_VISIBLE_ROWS).min(total);
    let start = anchor.min(total.saturating_sub(rows));
    let end = start.saturating_add(rows).min(total);
    start..end
}

#[cfg(test)]
pub(crate) fn reset_walk_log() {
    let _boundary = lock_recover(scan_commit_boundary());
    lock_recover(walk_log()).clear();
}

/// Session-wide most-recent-first list of visited directories (for the
/// Cmd+P quick switcher), distinct from each panel's linear history.
fn visited_log() -> &'static Mutex<Vec<PathBuf>> {
    static V: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(Vec::new()))
}

fn visit_stats() -> &'static Mutex<VisitStats> {
    static STATS: OnceLock<Mutex<VisitStats>> = OnceLock::new();
    STATS.get_or_init(|| Mutex::new(VisitStats::default()))
}

pub const VISITED_CAP: usize = 200;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisitUsage {
    pub count: u32,
    pub last: u64,
}

/// Persisted frequency and recency information for the `Cmd+P` destination
/// switcher. Paths remain in a separate ordered list so chronological mode is
/// exact and old session files can default this field independently.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisitStats {
    pub uses: HashMap<PathBuf, VisitUsage>,
    pub tick: u64,
}

impl VisitStats {
    pub fn record(&mut self, path: &Path) {
        self.tick = self.tick.saturating_add(1);
        let usage = self.uses.entry(path.to_path_buf()).or_default();
        usage.count = usage.count.saturating_add(1);
        usage.last = self.tick;
    }

    fn usage(&self, path: &Path) -> VisitUsage {
        self.uses.get(path).cloned().unwrap_or_default()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecentOrder {
    #[default]
    Frecency,
    Chronological,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentMatch {
    pub path: PathBuf,
    pub count: u32,
    pub last: u64,
    pub score: i64,
}

/// Push `path` to the front of `list`, de-duplicating and capping. Pure, so
/// the ordering logic is unit-testable without the global.
pub fn push_visit(list: &mut Vec<PathBuf>, path: &Path, cap: usize) {
    list.retain(|p| p != path);
    list.insert(0, path.to_path_buf());
    list.truncate(cap);
}

/// Record a visit to `path` in the global recent list.
pub fn record_visit(path: &Path) {
    if let Ok(mut v) = visited_log().lock() {
        push_visit(&mut v, path, VISITED_CAP);
    }
    if let Ok(mut stats) = visit_stats().lock() {
        stats.record(path);
    }
}

/// Snapshot of recently visited directories, most recent first.
pub fn visited_paths() -> Vec<PathBuf> {
    visited_log().lock().map(|v| v.clone()).unwrap_or_default()
}

pub fn visit_snapshot() -> (Vec<PathBuf>, VisitStats) {
    (
        visited_paths(),
        visit_stats().lock().map(|s| s.clone()).unwrap_or_default(),
    )
}

pub fn restore_visit_snapshot(paths: &[PathBuf], stats: &VisitStats) {
    if let Ok(mut log) = visited_log().lock() {
        *log = paths.iter().take(VISITED_CAP).cloned().collect();
    }
    if let Ok(mut current) = visit_stats().lock() {
        *current = stats.clone();
        current.uses.retain(|path, _| paths.contains(path));
    }
}

/// Rank recent destinations by a bounded frequency/recency score. The fuzzy
/// component only affects a non-empty query; chronological mode preserves the
/// exact most-recent-first ordering of `paths`.
pub fn rank_visited(
    paths: &[PathBuf],
    query: &str,
    order: RecentOrder,
    stats: &VisitStats,
) -> Vec<RecentMatch> {
    let query = query.trim();
    let mut matches: Vec<(usize, RecentMatch)> = paths
        .iter()
        .enumerate()
        .filter_map(|(index, path)| {
            let label = path.to_string_lossy();
            let fuzzy = if query.is_empty() {
                0
            } else {
                crate::fuzzy::score(query, &label)?.score as i64
            };
            let usage = stats.usage(path);
            let age = stats.tick.saturating_sub(usage.last).min(64) as i64;
            let recency = 64 - age;
            let frequency = i64::from(usage.count.min(32)) * 4;
            Some((
                index,
                RecentMatch {
                    path: path.clone(),
                    count: usage.count,
                    last: usage.last,
                    score: fuzzy + recency + frequency,
                },
            ))
        })
        .collect();

    if order == RecentOrder::Frecency {
        matches.sort_by(|(index_a, a), (index_b, b)| {
            b.score
                .cmp(&a.score)
                .then(b.last.cmp(&a.last))
                .then(index_a.cmp(index_b))
        });
    }
    matches.into_iter().map(|(_, item)| item).collect()
}

/// Invalidate cached sizes for every directory that contains `path`.
/// A change at `path` (watcher event) makes all its ancestors' sizes stale,
/// even though their mtimes don't move (mtime only reflects direct children).
/// Active per-path epochs are cancelled before cache entries are removed, so
/// an already-running walk cannot recreate data invalidated by this event.
pub fn invalidate_size_cache(path: &Path) {
    let _boundary = lock_recover(scan_commit_boundary());
    {
        let mut epochs = lock_recover(active_path_epochs());
        for (key, epoch) in epochs.iter() {
            if path.starts_with(&key.path)
                && let Some(epoch) = epoch.upgrade()
            {
                epoch.cancel();
            }
        }
        epochs.retain(|key, epoch| !path.starts_with(&key.path) && epoch.strong_count() > 0);
    }
    lock_recover(dir_size_cache()).retain(|key, _| !path.starts_with(&key.path));
    lock_recover(walk_log()).retain(|key, _| !path.starts_with(&key.path));
}

/// Save current cache to disk (best-effort, called from background threads).
/// Snapshot under the mutex, then serialize and write after releasing it.
/// Stale paths are harmless because reuse always verifies mtime; probing them
/// here could block every panel behind the mutex when a remote volume is down.
pub fn flush_cache() {
    // Serialize snapshot + commit so an older panel worker cannot publish
    // after a newer one. The persistence layer supplies a unique private temp
    // file and durable atomic replacement.
    let _flush = lock_recover(cache_flush_lock());
    let mut entries = {
        let _boundary = lock_recover(scan_commit_boundary());
        let mut cache = lock_recover(dir_size_cache());
        prune_dir_size_cache_to(&mut cache, DIR_SIZE_CACHE_LIMIT, DIR_SIZE_CACHE_RETAIN);
        cache
            .iter()
            .filter_map(|(p, (mtime, size))| {
                let dur = mtime.duration_since(std::time::UNIX_EPOCH).ok()?;
                Some(PersistedCacheEntry {
                    key: p.clone(),
                    mtime_secs: dur.as_secs(),
                    mtime_nanos: dur.subsec_nanos(),
                    size: *size,
                })
            })
            .collect::<Vec<_>>()
        // Lock dropped here: serialization and IO happen outside it.
    };
    entries.sort_by(|a, b| compare_volume_path_keys(&a.key, &b.key));
    let persisted = PersistedCache { schema: 1, entries };
    let _ = crate::persistence::save_json_atomic(&cache_path(), &persisted);
}

/// Why a directory listing is the way it is, so an empty list can be told
/// apart from an unreadable or vanished directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirStatus {
    /// Read succeeded and there are entries.
    Listed,
    /// Read succeeded but the directory is empty.
    Empty,
    /// Permission denied.
    Denied,
    /// The directory no longer exists.
    Gone,
    /// The directory opened, but at least one child could not be observed.
    /// The previous complete snapshot remains authoritative.
    Partial,
    /// An asynchronous listing for this binding is in flight; rows are empty.
    Loading,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum ViewApplyOutcome {
    Applied,
    ReadRejected(DirStatus),
}

pub(super) enum DirectoryRead {
    Complete(Vec<FileEntry>),
    Incomplete(DirStatus),
}

/// Classify a directory read for UI messaging. `is_empty` is whether the
/// listing came back with zero entries.
#[cfg(test)]
pub fn classify_dir(path: &Path, is_empty: bool) -> DirStatus {
    match fs::read_dir(path) {
        Ok(_) => {
            if is_empty {
                DirStatus::Empty
            } else {
                DirStatus::Listed
            }
        }
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => DirStatus::Gone,
            _ => DirStatus::Denied,
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListingIdentity {
    Captured(crate::path_identity::PathIdentity),
    CaptureFailed(crate::ports::NativeFailure),
    Unavailable,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub name_lower: String,
    pub path: PathBuf,
    /// Lexical filesystem binding captured with the listing the user sees.
    /// Operations must not re-observe it synchronously on the UI thread.
    pub identity: ListingIdentity,
    pub is_dir: bool,
    pub size: u64,
    pub extension: String,
    pub modified: Option<std::time::SystemTime>,
    /// Pre-formatted date string (rendered every frame, formatted once).
    pub modified_str: String,
    /// Pre-formatted size string for files ("…" for dirs).
    pub size_str: String,
}

impl FileEntry {
    /// Build an entry from a path and its (already fetched) metadata.
    pub fn from_meta(path: PathBuf, meta: &fs::Metadata) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().to_string();
        let is_dir = meta.is_dir();
        let size = if is_dir { 0 } else { meta.len() };
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let modified = meta.modified().ok();
        let modified_str = match modified {
            Some(time) => {
                let datetime: chrono::DateTime<chrono::Local> = time.into();
                datetime.format("%d %b %y  %H:%M").to_string()
            }
            None => "–".to_string(),
        };
        let size_str = if is_dir {
            "…".to_string()
        } else {
            format_size(size)
        };
        let name_lower = name.to_lowercase();
        Some(FileEntry {
            name,
            name_lower,
            identity: ListingIdentity::Captured(crate::path_identity::PathIdentity::from_metadata(
                path.clone(),
                meta,
            )),
            path,
            is_dir,
            size,
            extension,
            modified,
            modified_str,
            size_str,
        })
    }

    pub fn is_image(&self) -> bool {
        self.is_static_image() || self.is_video()
    }

    pub fn is_static_image(&self) -> bool {
        matches!(
            self.extension.as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "bmp"
                | "webp"
                | "svg"
                | "ico"
                | "heic"
                | "heif"
                | "tiff"
                | "tif"
                | "dng"
                | "cr2"
                | "cr3"
                | "nef"
                | "arw"
                | "orf"
                | "raf"
                | "rw2"
                | "pef"
                | "srw"
        )
    }

    pub fn is_video(&self) -> bool {
        matches!(
            self.extension.as_str(),
            "mp4" | "mov" | "avi" | "mkv" | "webm" | "m4v" | "wmv" | "flv"
        )
    }

    pub fn size_display(&self) -> &str {
        &self.size_str
    }

    pub fn modified_display(&self) -> &str {
        &self.modified_str
    }
}

pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewIdentity {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

impl PreviewIdentity {
    pub fn from_entry(entry: &FileEntry) -> Self {
        Self {
            path: entry.path.clone(),
            size: entry.size,
            modified: entry.modified,
        }
    }

    pub fn matches_entry(&self, entry: &FileEntry) -> bool {
        self.path == entry.path && self.size == entry.size && self.modified == entry.modified
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PreviewContent {
    Image(PathBuf),
    Pending(PreviewIdentity),
    Text {
        identity: PreviewIdentity,
        content: Arc<str>,
    },
    Info(InfoCard),
}

/// Precomputed metadata card for the Get-Info inspector (display-only).
#[derive(Debug, Clone, PartialEq)]
pub struct InfoCard {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub size: String,
    pub children: Option<usize>,
    pub modified: String,
    pub permissions: String,
}

/// Format the low 9 bits of a unix mode as "rwxr-xr-x".
pub fn format_mode(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let triplet = (mode >> shift) & 0o7;
        s.push(if triplet & 0o4 != 0 { 'r' } else { '-' });
        s.push(if triplet & 0o2 != 0 { 'w' } else { '-' });
        s.push(if triplet & 0o1 != 0 { 'x' } else { '-' });
    }
    s
}

/// Build a Get-Info card for `entry`. `dir_size`/`children` come from the
/// panel's already-computed maps (None while still measuring).
pub fn make_info(entry: &FileEntry, dir_size: Option<u64>, children: Option<usize>) -> InfoCard {
    let kind = if entry.is_dir {
        "Folder".to_string()
    } else if entry.extension.is_empty() {
        "Document".to_string()
    } else {
        format!("{} file", entry.extension.to_uppercase())
    };
    let size = if entry.is_dir {
        dir_size.map_or_else(|| "\u{2026}".to_string(), format_size)
    } else {
        format_size(entry.size)
    };
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(&entry.path)
            .map(|m| format_mode(m.permissions().mode()))
            .unwrap_or_else(|_| "---------".to_string())
    };
    InfoCard {
        name: entry.name.clone(),
        path: entry.path.display().to_string(),
        kind,
        size,
        children: if entry.is_dir { children } else { None },
        modified: entry.modified_str.clone(),
        permissions,
    }
}

/// Create a preview marker without reading file contents. Text is resolved by
/// the app's cancellable background preview pipeline.
pub fn make_preview(entry: &FileEntry) -> Option<PreviewContent> {
    if entry.is_dir {
        return None;
    }
    if !entry.is_image() {
        return Some(PreviewContent::Pending(PreviewIdentity::from_entry(entry)));
    }

    let root = entry.path.parent().unwrap_or(Path::new("/"));
    if !crate::provider_runtime::activate_builtin(
        "native-preview",
        &crate::provider_runtime::ActivationRequest {
            capability: crate::provider_runtime::ProviderCapability::PreviewImage,
            root,
            extension: (!entry.extension.is_empty()).then_some(entry.extension.as_str()),
            bytes: Some(entry.size),
        },
    ) {
        return None;
    }
    Some(PreviewContent::Image(entry.path.clone()))
}

/// Glob match supporting `*` (any run) and `?` (one char). Inputs are
/// expected lowercased; matching is greedy with backtracking.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Parse a select-by-mask line into `(term, is_subtract)` pairs. Terms are
/// comma-separated; a leading `!` or `-` marks subtraction. Terms are
/// lowercased for case-insensitive matching.
pub fn parse_mask(mask: &str) -> Vec<(String, bool)> {
    mask.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(
            |t| match t.strip_prefix('!').or_else(|| t.strip_prefix('-')) {
                Some(rest) => (rest.trim().to_lowercase(), true),
                None => (t.to_lowercase(), false),
            },
        )
        .collect()
}

/// Whether a single lowercased `term` matches an entry. A term with no
/// wildcard and no `.` matches by extension ("jpg" selects all *.jpg);
/// otherwise it is globbed against the full name.
fn term_matches(term: &str, name_lower: &str, ext: &str) -> bool {
    let wild = term.contains('*') || term.contains('?');
    if !wild && !term.contains('.') {
        return ext == term;
    }
    glob_match(term, name_lower)
}

/// Size used for the occupancy bar: a file's own size, or a directory's
/// resolved recursive size (0 while it is still being measured).
#[cfg(test)]
pub fn entry_display_size(entry: &FileEntry, dir_sizes: &HashMap<PathBuf, u64>) -> u64 {
    if entry.is_dir {
        dir_sizes.get(&entry.path).copied().unwrap_or(0)
    } else {
        entry.size
    }
}

/// Wake-up callback into the UI (e.g. a repaint request). Panels never
/// talk to the UI toolkit directly.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// A category facet for the quick-filter chips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KindFacet {
    Folders,
    Images,
    Docs,
    Archives,
    Code,
}

/// Active quick-filter facets, ANDed with the substring filter. Default is
/// "no facets" (everything passes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FacetSet {
    pub kind: Option<KindFacet>,
    /// Minimum file size in bytes (folders are not filtered by size).
    pub min_size: Option<u64>,
    /// Maximum age in days by mtime (entries with unknown mtime fail this).
    pub max_age_days: Option<u64>,
    /// Minimum age in days by mtime: the entry must be at least this old
    /// (entries with unknown mtime fail this). Drives the "Older" chip.
    pub min_age_days: Option<u64>,
}

impl FacetSet {
    pub fn is_empty(&self) -> bool {
        self.kind.is_none()
            && self.min_size.is_none()
            && self.max_age_days.is_none()
            && self.min_age_days.is_none()
    }

    pub fn active_count(&self) -> usize {
        usize::from(self.kind.is_some())
            + usize::from(self.min_size.is_some())
            + usize::from(self.max_age_days.is_some())
            + usize::from(self.min_age_days.is_some())
    }
}

pub fn filter_is_active(search_query: &str, facets: &FacetSet) -> bool {
    !search_query.trim().is_empty() || !facets.is_empty()
}

/// Whether `entry` passes all active facets, relative to `now`. Pure.
pub fn facet_matches(entry: &FileEntry, facets: &FacetSet, now: SystemTime) -> bool {
    if let Some(kind) = facets.kind {
        let ok = match kind {
            KindFacet::Folders => entry.is_dir,
            KindFacet::Images => entry.is_image(),
            KindFacet::Docs => matches!(
                entry.extension.as_str(),
                "pdf" | "txt" | "md" | "rtf" | "doc" | "docx" | "pages" | "odt" | "tex"
            ),
            KindFacet::Archives => matches!(
                entry.extension.as_str(),
                "zip" | "tar" | "gz" | "tgz" | "7z" | "rar" | "bz2" | "xz" | "zst"
            ),
            KindFacet::Code => matches!(
                entry.extension.as_str(),
                "rs" | "py"
                    | "js"
                    | "ts"
                    | "jsx"
                    | "tsx"
                    | "c"
                    | "cpp"
                    | "h"
                    | "hpp"
                    | "go"
                    | "java"
                    | "kt"
                    | "swift"
                    | "rb"
                    | "php"
                    | "sh"
                    | "toml"
                    | "json"
                    | "yaml"
                    | "yml"
            ),
        };
        if !ok {
            return false;
        }
    }
    if let Some(min) = facets.min_size {
        // Folders are not filtered by size (their byte size is 0 here).
        if !entry.is_dir && entry.size < min {
            return false;
        }
    }
    if let Some(days) = facets.max_age_days {
        let Some(modified) = entry.modified else {
            return false;
        };
        let cutoff = std::time::Duration::from_secs(days * 24 * 60 * 60);
        match now.duration_since(modified) {
            Ok(age) if age <= cutoff => {}
            Ok(_) => return false,
            // modified in the future: treat as fresh (passes).
            Err(_) => {}
        }
    }
    if let Some(days) = facets.min_age_days {
        let Some(modified) = entry.modified else {
            return false;
        };
        let cutoff = std::time::Duration::from_secs(days * 24 * 60 * 60);
        match now.duration_since(modified) {
            Ok(age) if age >= cutoff => {} // old enough: passes
            Ok(_) => return false,         // too fresh
            // modified in the future: definitely not old enough.
            Err(_) => return false,
        }
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SortColumn {
    Name,
    Size,
    Modified,
    Extension,
    Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SortOrder {
    Asc,
    Desc,
}

/// One-pass folder aggregates for the status bar: total bytes (`None` until at
/// least one subdirectory has been sized), the largest entry by size, and the
/// oldest entry by mtime.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderOverview {
    pub total: Option<u64>,
    pub largest: Option<(String, u64)>,
    pub oldest: Option<(String, SystemTime)>,
}

/// A non-parent cursor row no longer exists in the filtered view. This should
/// be prevented by [`PanelState::ensure_cursor_valid`], but remains explicit at
/// file-operation call sites so a future invariant regression cannot silently
/// turn a command into an empty selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleCursor {
    pub cursor: usize,
    pub visible_entries: usize,
}

pub struct PanelState {
    pub current_path: PathBuf,
    listing: ListingState,
    selection: SelectionState,
    pub preview: Option<PreviewContent>,
    /// Per-pane directory history (back/forward); a vim-style jump trail that
    /// truncates its forward tail on a new navigation.
    pub history: crate::jumplist::JumpList,
    view: ViewState,
    sizes: SizeIndex,
    watcher: DirectoryWatcherState,
    pub drag_entries: Vec<PathBuf>,
    pub drop_target: Option<PathBuf>,
    listing_job: ListingJobController,
    workload: Option<crate::workload::WorkloadHandle>,
}

impl PanelState {
    /// Create a panel pointed at `path`. The directory is NOT read yet:
    /// call [`refresh`](Self::refresh) (the UI does this when wiring the
    /// notify callback on the first frame).
    #[cfg(test)]
    pub fn new(path: PathBuf) -> Self {
        Self::new_with_view(path, ViewConfig::default())
    }

    /// Seed persisted view configuration before the first directory listing.
    /// This prevents sort/hidden indicators from temporarily disagreeing with
    /// rows loaded under default settings.
    pub(crate) fn new_with_view(path: PathBuf, config: ViewConfig) -> Self {
        PanelState {
            current_path: path.clone(),
            listing: ListingState::new(path.clone()),
            selection: SelectionState::new(path.clone()),
            preview: None,
            history: {
                let mut h = crate::jumplist::JumpList::new();
                h.push(path.clone());
                h
            },
            view: ViewState::with_config(config),
            sizes: SizeIndex::new(path.clone()),
            watcher: DirectoryWatcherState::default(),
            drag_entries: Vec::new(),
            drop_target: None,
            listing_job: ListingJobController::default(),
            workload: None,
        }
    }

    /// Attach the shared workload handle used for off-thread listings.
    pub fn set_workload(&mut self, workload: crate::workload::WorkloadHandle) {
        self.workload = Some(workload);
    }

    /// Wire the UI wake-up callback and restart the watcher so its
    /// notifications reach the UI.
    pub fn set_notify(&mut self, notify: Notify) {
        self.watcher.set_notify(notify);
        self.watcher.ensure_binding(&self.current_path);
    }

    pub fn has_notify(&self) -> bool {
        self.watcher.has_notify()
    }

    pub(crate) fn notify_callback(&self) -> Option<Notify> {
        self.watcher.notify()
    }

    pub fn entries(&self) -> &[FileEntry] {
        self.listing.entries()
    }

    #[cfg(test)]
    fn replace_entries_for_test(&mut self, entries: Vec<FileEntry>) {
        self.selection.publish_complete(&self.current_path);
        self.listing.replace_for_test(entries);
    }

    #[cfg(test)]
    fn mutate_entries_for_test(&mut self, mutate: impl FnOnce(&mut Vec<FileEntry>)) {
        self.listing.mutate_for_test(mutate);
    }

    #[cfg(test)]
    fn push_entry_for_test(&mut self, entry: FileEntry) {
        self.listing.push_for_test(entry);
    }

    #[cfg(test)]
    fn clear_entries_without_revision_for_test(&mut self) {
        self.listing
            .mutate_entries_without_revision_for_test(Vec::clear);
    }

    pub fn dir_status(&self) -> DirStatus {
        self.listing.status()
    }

    pub fn selected_paths(&self) -> &HashSet<PathBuf> {
        self.selection.selected()
    }

    pub fn marked_paths(&self) -> &HashSet<PathBuf> {
        self.selection.marked()
    }

    pub fn selection_is_empty(&self) -> bool {
        self.selection.selected().is_empty()
    }

    pub fn selected_count(&self) -> usize {
        self.selection.selected().len()
    }

    pub fn is_selected(&self, path: &Path) -> bool {
        self.selection.selected().contains(path)
    }

    pub fn is_marked(&self, path: &Path) -> bool {
        self.selection.marked().contains(path)
    }

    #[cfg(test)]
    pub(crate) fn select_path(&mut self, path: PathBuf) -> bool {
        self.selection.insert_selected(path)
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear_selected();
    }

    pub fn replace_selection(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.selection
            .replace_selected(paths.into_iter().collect::<HashSet<_>>());
    }

    #[cfg(test)]
    pub(crate) fn replace_marks_for_test(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.selection
            .replace_marked(paths.into_iter().collect::<HashSet<_>>());
    }

    pub fn cursor(&self) -> usize {
        self.selection.cursor()
    }

    pub fn set_cursor(&mut self, row: usize) {
        let row = row.min(self.filtered_count());
        let path = row
            .checked_sub(1)
            .and_then(|index| self.filtered_get(index))
            .map(|entry| entry.path.clone());
        self.selection.set_cursor(row, path);
    }

    pub fn cursor_entry(&self) -> Option<&FileEntry> {
        let Focus::Entry(expected) = self.selection.focus() else {
            return None;
        };
        self.cursor()
            .checked_sub(1)
            .and_then(|index| self.filtered_get(index))
            .filter(|entry| &entry.path == expected)
    }

    pub fn scroll_to_cursor(&self) -> bool {
        self.selection.scroll_to_cursor()
    }

    pub fn set_scroll_to_cursor(&mut self, value: bool) {
        self.selection.set_scroll_to_cursor(value);
    }

    pub fn scroll_anchor(&self) -> usize {
        self.selection.scroll_anchor()
    }

    pub fn set_scroll_anchor(&mut self, anchor: usize) {
        self.selection.set_scroll_anchor(anchor);
    }

    pub fn page_rows(&self) -> usize {
        self.selection.page_rows()
    }

    pub fn set_page_rows(&mut self, rows: usize) {
        self.selection.set_page_rows(rows);
    }

    pub fn view_config(&self) -> ViewConfig {
        self.view.config()
    }

    pub fn search_query(&self) -> &str {
        self.view.search_query()
    }

    pub fn set_search_query(&mut self, query: impl Into<String>) {
        let focus = self.focused_path();
        let old_cursor = self.cursor();
        self.view.set_search_query(query);
        self.restore_cursor_focus(focus, old_cursor);
    }

    pub fn facets(&self) -> FacetSet {
        self.view.facets()
    }

    pub fn set_facets(&mut self, facets: FacetSet) {
        let focus = self.focused_path();
        let old_cursor = self.cursor();
        *self.view.facets_mut() = facets;
        self.restore_cursor_focus(focus, old_cursor);
    }

    #[cfg(test)]
    pub(crate) fn sort_column(&self) -> SortColumn {
        self.view.sort_col()
    }

    #[cfg(test)]
    pub(crate) fn sort_order(&self) -> SortOrder {
        self.view.sort_order()
    }

    pub fn show_hidden(&self) -> bool {
        self.view.show_hidden()
    }

    /// Toggle hidden entries as one view/listing transition.
    ///
    /// A rejected read leaves both the prior configuration and its complete
    /// rows authoritative; callers can present the typed failure without a
    /// split-brain hidden indicator.
    pub fn toggle_hidden(&mut self) -> ViewApplyOutcome {
        let candidate = self
            .view
            .config()
            .with_show_hidden(!self.view.show_hidden());
        let path = self.current_path.clone();
        let ticket = self.watcher.snapshot_ticket_for(&path);
        let entries = match Self::read_dir(&path, candidate.show_hidden()) {
            DirectoryRead::Complete(entries) => entries,
            DirectoryRead::Incomplete(status) => {
                self.watcher.defer_snapshot(ticket.as_ref());
                return ViewApplyOutcome::ReadRejected(status);
            }
        };

        self.view.commit_config(candidate);
        self.publish_complete_listing(entries, candidate);
        self.sizes.bind(&path);
        self.watcher.ensure_binding(&path);
        let listing_binding = self.listing.binding().to_path_buf();
        self.watcher.acknowledge_snapshot(ticket, &listing_binding);
        self.refresh_sizes(true);
        ViewApplyOutcome::Applied
    }

    pub fn density(&self) -> crate::density::Density {
        self.view.density()
    }

    pub fn set_density(&mut self, density: crate::density::Density) {
        self.view.set_density(density);
    }

    pub fn watcher_active(&self) -> bool {
        self.watcher.is_active()
    }

    pub fn refresh(&mut self) {
        self.schedule_or_reload(PendingFocus::None);
    }

    fn can_async_list(&self) -> bool {
        self.workload.is_some() && self.watcher.notify().is_some()
    }

    fn schedule_or_reload(&mut self, focus: PendingFocus) {
        if self.can_async_list() {
            self.schedule_listing(focus);
        } else {
            self.refresh_sync(focus);
        }
    }

    fn refresh_sync(&mut self, focus: PendingFocus) {
        let path = self.current_path.clone();
        self.sizes.bind(&path);
        // Subscribe before taking the snapshot. A callback racing with the
        // read stays queued with this exact binding and forces a later pass.
        self.watcher.ensure_binding(&path);
        let ticket = self.watcher.snapshot_ticket();
        if self.reload_entries() {
            let listing_binding = self.listing.binding().to_path_buf();
            self.watcher.acknowledge_snapshot(ticket, &listing_binding);
            self.refresh_sizes(true);
            self.apply_pending_focus(focus);
        } else {
            self.watcher.defer_snapshot(ticket.as_ref());
        }
    }

    fn schedule_listing(&mut self, focus: PendingFocus) {
        let path = self.current_path.clone();
        let show_hidden = self.view.show_hidden();
        let binding_changed = self.listing.binding() != path;
        self.sizes.bind(&path);
        self.watcher.ensure_binding(&path);
        let ticket = self.watcher.snapshot_ticket();
        if binding_changed {
            self.selection.bind(&path);
            self.drag_entries.clear();
            self.drop_target = None;
            self.listing.begin_loading(path.clone());
        }
        self.listing_job.request(path, show_hidden, ticket, focus);
        // Opportunistically admit/poll within this call so fast local disks
        // still settle before the next frame when the worker is free.
        let _ = self.poll_listing_results();
    }

    /// Drive in-flight listings and publish generation-checked results.
    /// Returns `true` when a listing was applied.
    pub fn poll_listing(&mut self) -> bool {
        self.poll_listing_results()
    }

    fn poll_listing_results(&mut self) -> bool {
        let Some(workload) = self.workload.clone() else {
            return false;
        };
        let Some(notify) = self.watcher.notify() else {
            return false;
        };
        self.listing_job
            .drive(&workload, notify, PanelState::read_dir);
        let Some(ready) = self.listing_job.take_ready() else {
            return false;
        };
        if ready.binding.path != self.current_path
            || ready.binding.show_hidden != self.view.show_hidden()
            || self
                .listing_job
                .desired_binding()
                .is_some_and(|desired| desired.generation > ready.binding.generation)
        {
            // Stale relative to panel intent; keep waiting for the current job.
            return false;
        }
        let applied = self.apply_directory_read(ready.read);
        if applied {
            let listing_binding = self.listing.binding().to_path_buf();
            self.watcher
                .acknowledge_snapshot(ready.ticket, &listing_binding);
            self.refresh_sizes(true);
            self.apply_pending_focus(ready.focus);
        } else if let Some(ticket) = ready.ticket.as_ref() {
            self.watcher.defer_snapshot(Some(ticket));
            let _ = ready.focus;
        } else {
            let _ = ready.focus;
        }
        applied
    }

    fn apply_pending_focus(&mut self, focus: PendingFocus) {
        match focus {
            PendingFocus::None => {}
            PendingFocus::Remembered {
                cursor_path,
                scroll_anchor,
            } => {
                self.set_scroll_anchor(scroll_anchor.min(self.filtered_count().saturating_sub(1)));
                let cursor = cursor_path
                    .and_then(|path| self.filtered_position(|entry| entry.path == path))
                    .map(|index| index + 1)
                    .unwrap_or_else(|| {
                        self.scroll_anchor()
                            .saturating_add(1)
                            .min(self.filtered_count())
                    });
                self.set_cursor(cursor);
                self.set_scroll_to_cursor(self.cursor() > 0);
            }
            PendingFocus::NamedChild(name) => {
                if let Some(idx) = self.filtered_position(|entry| entry.name == name) {
                    self.set_cursor(idx + 1);
                    self.set_scroll_to_cursor(true);
                }
            }
        }
    }

    /// Re-read the directory, preserving selection and cursor position
    /// by path (entries may have been added, removed or re-sorted).
    fn reload_entries(&mut self) -> bool {
        let read = Self::read_dir(&self.current_path, self.view.show_hidden());
        self.apply_directory_read(read)
    }

    fn apply_directory_read(&mut self, read: DirectoryRead) -> bool {
        let binding = self.current_path.clone();
        let binding_changed = self.listing.binding() != binding;
        match read {
            DirectoryRead::Complete(entries) => {
                self.publish_complete_listing(entries, self.view.config());
                true
            }
            DirectoryRead::Incomplete(status) => {
                if binding_changed {
                    self.selection.bind(&binding);
                    self.sizes.bind(&binding);
                    self.drag_entries.clear();
                    self.drop_target = None;
                }
                let changed = self.listing.mark_incomplete(binding, status);
                debug_assert_eq!(changed, binding_changed);
                false
            }
        }
    }

    fn publish_complete_listing(&mut self, mut entries: Vec<FileEntry>, config: ViewConfig) {
        let binding = self.current_path.clone();
        let binding_changed = self.listing.binding() != binding;
        let cursor_path = (!binding_changed).then(|| self.focused_path()).flatten();
        let old_cursor = if binding_changed { 0 } else { self.cursor() };

        if binding_changed {
            self.selection.bind(&binding);
            self.sizes.bind(&binding);
            self.drag_entries.clear();
            self.drop_target = None;
        }

        let status = if entries.is_empty() {
            DirStatus::Empty
        } else {
            DirStatus::Listed
        };
        sort_entries(&mut entries, config);
        self.selection.publish_complete(&binding);
        self.listing.replace(binding, entries, status);

        self.selection
            .retain_present(self.listing.entries().iter().map(|entry| &entry.path));

        if !binding_changed {
            self.restore_cursor_focus(cursor_path, old_cursor);
        }
    }

    /// Check if fs watcher flagged a change; if so, refresh.
    /// Returns `true` when the directory listing was re-read.
    /// Deep events (below the watched dir) only recompute directory
    /// sizes, debounced so event floods during transfers don't thrash.
    pub fn poll_fs_changes(&mut self) -> bool {
        let path = self.current_path.clone();
        let outcome = self.watcher.poll(&path);
        if outcome.sizes_dirty {
            self.sizes.mark_dirty();
        }
        if let Some(ticket) = outcome.ticket {
            if self.can_async_list() {
                let show_hidden = self.view.show_hidden();
                self.listing_job
                    .request(path, show_hidden, Some(ticket), PendingFocus::None);
                let applied = self.poll_listing_results();
                if applied {
                    crate::watcher_health::record_listing_reconciliation(outcome.recovered_gap);
                }
                return applied;
            }
            if self.reload_entries() {
                let listing_binding = self.listing.binding().to_path_buf();
                self.watcher
                    .acknowledge_snapshot(Some(ticket), &listing_binding);
                self.refresh_sizes(true);
                crate::watcher_health::record_listing_reconciliation(outcome.recovered_gap);
                return true;
            }
            self.watcher.defer_snapshot(Some(&ticket));
        }
        self.poll_sizes();
        false
    }

    /// Schedule background recomputation of subdirectory sizes and counts.
    /// `forced` marks user-driven refreshes: they may re-walk expensive
    /// dirs, background (watcher-noise) recomputes may not.
    /// Returns `true` when some dir was skipped because of the walk
    /// cooldown and the caller should retry later.
    fn refresh_sizes(&mut self, forced: bool) {
        let filtered = self.filtered_indices();
        let path = self.current_path.clone();
        let scroll_anchor = self.scroll_anchor();
        let page_rows = self.page_rows();
        let notify = self.watcher.notify();
        self.sizes.refresh(
            SizeScanInput {
                path: &path,
                entries: self.listing.entries(),
                filtered,
                scroll_anchor,
                page_rows,
                notify,
            },
            forced,
        );
    }

    fn poll_sizes(&mut self) {
        let filtered = self.filtered_indices();
        let path = self.current_path.clone();
        let input = SizeScanInput {
            path: &path,
            entries: self.listing.entries(),
            filtered,
            scroll_anchor: self.scroll_anchor(),
            page_rows: self.page_rows(),
            notify: self.watcher.notify(),
        };
        self.sizes.poll(input);
    }
    pub(super) fn read_dir(path: &Path, show_hidden: bool) -> DirectoryRead {
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) => {
                let status = if error.kind() == std::io::ErrorKind::NotFound {
                    DirStatus::Gone
                } else {
                    DirStatus::Denied
                };
                return DirectoryRead::Incomplete(status);
            }
        };

        let mut result = Vec::new();
        let mut complete = true;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            let path = entry.path();
            if !show_hidden
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            let lexical = entry
                .metadata()
                .map_err(|error| crate::ports::NativeFailure::from_io(&error));
            let followed = file_type
                .is_symlink()
                .then(|| fs::metadata(&path).ok())
                .flatten();
            let display_metadata = followed.as_ref().or_else(|| lexical.as_ref().ok());
            let Some(display_metadata) = display_metadata else {
                complete = false;
                continue;
            };
            match FileEntry::from_meta(path.clone(), display_metadata) {
                Some(mut file_entry) => {
                    file_entry.identity = match lexical {
                        Ok(metadata) => ListingIdentity::Captured(
                            crate::path_identity::PathIdentity::from_metadata(path, &metadata),
                        ),
                        Err(failure) => ListingIdentity::CaptureFailed(failure),
                    };
                    result.push(file_entry);
                }
                None => complete = false,
            }
        }
        if complete {
            DirectoryRead::Complete(result)
        } else {
            DirectoryRead::Incomplete(DirStatus::Partial)
        }
    }

    fn sort_entries(&mut self) {
        let focus = self.focused_path();
        let old_cursor = self.cursor();
        let config = self.view.config();
        self.listing.resort(|entries| sort_entries(entries, config));
        self.restore_cursor_focus(focus, old_cursor);
    }

    /// Toggle pinning folders to the top, then re-sort in place.
    pub fn toggle_folders_first(&mut self) {
        self.view.toggle_folders_first();
        self.sort_entries();
    }

    /// Toggle natural vs plain A-Z name ordering, then re-sort in place.
    pub fn toggle_natural_sort(&mut self) {
        self.view.toggle_natural_sort();
        self.sort_entries();
    }

    pub fn navigate_to(&mut self, path: PathBuf) {
        // Remember the outgoing directory's view before leaving it, then
        // restore the incoming one's if we've seen it before this session.
        self.stash_view_settings();
        // The jump trail truncates any forward tail and collapses a repeat of
        // the current directory, so every navigation entry point records here.
        self.history.push(path.clone());
        record_visit(&path);
        self.load_remembered_path(path);
    }

    fn snapshot_view_settings(&self) -> ViewSettings {
        ViewSettings {
            config: self.view.config(),
            search_query: self.view.search_query().to_string(),
            facets: self.view.facets(),
            cursor_path: self.focused_path(),
            scroll_anchor: self.scroll_anchor(),
        }
    }

    /// Remember the current directory's view settings under its own path.
    fn stash_view_settings(&mut self) {
        let settings = self.snapshot_view_settings();
        self.view.remember(&self.current_path, settings);
    }

    /// Apply the current directory's remembered view settings, if any.
    /// Leaves everything unchanged (carrying over whatever was already
    /// active) when this directory has never been visited this session.
    fn restore_view_settings(&mut self) -> Option<(Option<PathBuf>, usize)> {
        let s = self.view.restore(&self.current_path)?;
        Some((s.cursor_path, s.scroll_anchor))
    }

    fn load_remembered_path(&mut self, path: PathBuf) {
        self.current_path = path;
        let remembered = self.restore_view_settings();
        if remembered.is_none() {
            self.view.clear_filters();
        }
        let focus = match remembered {
            Some((cursor_path, scroll_anchor)) => {
                self.set_scroll_anchor(scroll_anchor);
                PendingFocus::Remembered {
                    cursor_path,
                    scroll_anchor,
                }
            }
            None => {
                self.set_scroll_anchor(0);
                PendingFocus::None
            }
        };
        self.schedule_or_reload(focus);
    }

    pub fn go_up(&mut self) {
        // Remember the directory we are leaving so the cursor can land on it
        // in the parent (classic dual-pane behaviour).
        let child = self
            .current_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string());
        if let Some(parent) = self.current_path.parent().map(|p| p.to_path_buf()) {
            self.navigate_to(parent);
            if let Some(name) = child {
                // Prefer the child-name landing over any remembered parent focus.
                if self.can_async_list() && self.listing_job.is_awaiting() {
                    self.listing_job.set_focus(PendingFocus::NamedChild(name));
                } else if let Some(idx) = self.filtered_position(|e| e.name == name) {
                    self.set_cursor(idx + 1);
                    self.set_scroll_to_cursor(true);
                }
            }
        }
    }

    /// Move the cursor to the first filtered entry whose name matches `buffer`
    /// (prefix first, then substring), both lowercased. Returns whether a
    /// match was found. Drives type-to-jump navigation.
    pub fn type_ahead(&mut self, buffer: &str) -> bool {
        if buffer.is_empty() {
            return false;
        }
        let q = buffer.to_lowercase();
        let pos = self
            .filtered_position(|e| e.name_lower.starts_with(&q))
            .or_else(|| self.filtered_position(|e| e.name_lower.contains(&q)));
        if let Some(idx) = pos {
            self.set_cursor(idx + 1);
            self.set_scroll_to_cursor(true);
            true
        } else {
            false
        }
    }

    /// Add the well-known clutter files in the filtered view to the selection
    /// (`.DS_Store`, `.localized`, `Thumbs.db`, `desktop.ini`, the custom-icon
    /// `Icon\r`). Folders are never matched. Returns how many were added.
    pub fn select_junk(&mut self) -> usize {
        const JUNK_NAMES: &[&str] = &[
            ".DS_Store",
            ".localized",
            "Thumbs.db",
            "desktop.ini",
            "Icon\r",
        ];
        let paths: Vec<PathBuf> = self
            .filtered_entries()
            .iter()
            .filter(|e| !e.is_dir && JUNK_NAMES.contains(&e.name.as_str()))
            .map(|e| e.path.clone())
            .collect();
        let added = paths.len();
        for p in paths {
            self.selection.insert_selected(p);
        }
        added
    }

    /// Select the `n` largest files in the filtered view (folders excluded).
    /// Returns how many entries were newly added to the selection.
    pub fn select_largest(&mut self, n: usize) -> usize {
        let mut sized: Vec<(PathBuf, u64)> = self
            .filtered_entries()
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| (e.path.clone(), e.size))
            .collect();
        sized.sort_by_key(|e| std::cmp::Reverse(e.1)); // largest first
        let mut added = 0;
        for (p, _) in sized.into_iter().take(n) {
            if self.selection.insert_selected(p) {
                added += 1;
            }
        }
        added
    }

    /// Select every filtered file sharing the cursor file's extension. Does
    /// nothing if the cursor is on `..`, a folder, or an extension-less file.
    /// Returns how many entries were added.
    pub fn select_same_extension_as_cursor(&mut self) -> usize {
        let ext = match self.cursor_entry() {
            Some(e) if !e.is_dir && !e.extension.is_empty() => e.extension.clone(),
            _ => return 0,
        };
        let paths: Vec<PathBuf> = self
            .filtered_entries()
            .iter()
            .filter(|e| !e.is_dir && e.extension == ext)
            .map(|e| e.path.clone())
            .collect();
        let added = paths.len();
        for p in paths {
            self.selection.insert_selected(p);
        }
        added
    }

    /// Select the zero-byte files in the filtered view (folders excluded).
    /// Returns how many entries were added.
    pub fn select_empty_files(&mut self) -> usize {
        let paths: Vec<PathBuf> = self
            .filtered_entries()
            .iter()
            .filter(|e| !e.is_dir && e.size == 0)
            .map(|e| e.path.clone())
            .collect();
        let added = paths.len();
        for p in paths {
            self.selection.insert_selected(p);
        }
        added
    }

    /// Flip the current sort order (ascending <-> descending) and re-sort.
    pub fn reverse_sort(&mut self) {
        self.view.reverse_sort();
        self.sort_entries();
    }

    /// Entries to export/copy as a listing: the current selection if anything
    /// is selected, otherwise the whole filtered view (in display order).
    pub fn listing_entries(&self) -> Vec<&FileEntry> {
        let all = self.filtered_entries();
        if self.selection.selected().is_empty() {
            all
        } else {
            all.into_iter()
                .filter(|e| self.selection.selected().contains(&e.path))
                .collect()
        }
    }

    /// Apply a select-by-mask line to the selection over the filtered view:
    /// add terms select matching entries, `!`/`-` terms deselect them
    /// (subtraction wins per entry). Returns how many entries were added.
    pub fn select_by_mask(&mut self, mask: &str) -> usize {
        let terms = parse_mask(mask);
        if terms.is_empty() {
            return 0;
        }
        let mut decisions: Vec<(PathBuf, bool)> = Vec::new();
        for e in self.filtered_entries() {
            let add = terms
                .iter()
                .any(|(t, sub)| !sub && term_matches(t, &e.name_lower, &e.extension));
            let rem = terms
                .iter()
                .any(|(t, sub)| *sub && term_matches(t, &e.name_lower, &e.extension));
            if rem {
                decisions.push((e.path.clone(), false));
            } else if add {
                decisions.push((e.path.clone(), true));
            }
        }
        let mut added = 0;
        for (path, is_add) in decisions {
            if is_add {
                self.selection.insert_selected(path);
                added += 1;
            } else {
                self.selection.remove_selected(&path);
            }
        }
        added
    }

    /// How many filtered entries would change selection state if `mask` were
    /// applied. A subtraction-only mask therefore reports zero until it has a
    /// selected match, which keeps the preview aligned with the actual action.
    pub fn mask_match_count(&self, mask: &str) -> usize {
        let terms = parse_mask(mask);
        if terms.is_empty() {
            return 0;
        }
        let mut changes = 0;
        self.visit_filtered(|_, entry| {
            let add = terms.iter().any(|(term, subtract)| {
                !subtract && term_matches(term, &entry.name_lower, &entry.extension)
            });
            let remove = terms.iter().any(|(term, subtract)| {
                *subtract && term_matches(term, &entry.name_lower, &entry.extension)
            });
            let selected = self.selection.selected().contains(&entry.path);
            let next = if remove {
                false
            } else if add {
                true
            } else {
                selected
            };
            changes += usize::from(next != selected);
            true
        });
        changes
    }

    /// Add the file under the cursor to the selection (range-select step).
    pub fn select_cursor(&mut self) {
        if let Some(path) = self.cursor_entry().map(|entry| entry.path.clone()) {
            self.selection.insert_selected(path);
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.history.can_back()
    }

    pub fn can_go_forward(&self) -> bool {
        self.history.can_forward()
    }

    pub fn go_back(&mut self) {
        // Walk the existing trail without recording a new jump. Directories
        // can disappear after being visited, so prune dead entries on sight.
        self.stash_view_settings();
        if let Some(path) = self
            .history
            .back_pruning(Path::is_dir)
            .map(|p| p.to_path_buf())
        {
            record_visit(&path);
            self.load_remembered_path(path);
        }
    }

    pub fn go_forward(&mut self) {
        self.stash_view_settings();
        if let Some(path) = self
            .history
            .forward_pruning(Path::is_dir)
            .map(|p| p.to_path_buf())
        {
            record_visit(&path);
            self.load_remembered_path(path);
        }
    }

    fn filtered_snapshot(&self) -> Arc<[usize]> {
        self.listing
            .filtered_snapshot(self.view.search_query(), self.view.facets())
    }

    fn focused_path(&self) -> Option<PathBuf> {
        self.selection.focused_path().map(Path::to_path_buf)
    }

    fn restore_cursor_focus(&mut self, focused: Option<PathBuf>, old_cursor: usize) {
        if old_cursor == 0 {
            self.set_cursor(0);
            return;
        }
        let cursor = focused
            .and_then(|path| self.filtered_position(|entry| entry.path == path))
            .map(|index| index + 1)
            .unwrap_or_else(|| old_cursor.min(self.filtered_count()));
        self.set_cursor(cursor);
        self.set_scroll_to_cursor(true);
    }

    /// Number of entries matching the current filter (no allocation).
    pub fn filtered_count(&self) -> usize {
        self.filtered_snapshot().len()
    }

    /// Clamp the cursor after any filter, facet, or ordering change. Cursor 0
    /// is the synthetic parent row; real rows occupy 1..=filtered_count().
    pub fn ensure_cursor_valid(&mut self) {
        let clamped = self.cursor().min(self.filtered_count());
        if self.cursor() != clamped {
            self.set_cursor(clamped);
            self.set_scroll_to_cursor(true);
        }
    }

    /// Clear both text and facet filters as one invariant-preserving action.
    pub fn clear_filters(&mut self) {
        let focus = self.focused_path();
        let old_cursor = self.cursor();
        self.view.clear_filters();
        self.restore_cursor_focus(focus, old_cursor);
    }

    /// The i-th entry of the filtered view (no allocation).
    pub fn filtered_get(&self, i: usize) -> Option<&FileEntry> {
        let snapshot = self.filtered_snapshot();
        let idx = *snapshot.get(i)?;
        self.listing.entries().get(idx)
    }

    /// Generation-bound shared snapshot of indices into [`Self::entries`].
    /// Cloning this is O(1), so a warm renderer frame does not allocate O(N).
    pub fn filtered_indices(&self) -> Arc<[usize]> {
        self.filtered_snapshot()
    }

    pub fn filtered_entries(&self) -> Vec<&FileEntry> {
        let snapshot = self.filtered_snapshot();
        snapshot
            .iter()
            .filter_map(|&i| self.listing.entries().get(i))
            .collect()
    }

    fn filtered_position(&self, mut predicate: impl FnMut(&FileEntry) -> bool) -> Option<usize> {
        let snapshot = self.filtered_snapshot();
        snapshot
            .iter()
            .filter_map(|&index| self.listing.entries().get(index))
            .position(&mut predicate)
    }

    fn filtered_available_count(&self) -> usize {
        self.filtered_snapshot()
            .iter()
            .filter(|&&index| self.listing.entries().get(index).is_some())
            .count()
    }

    /// Visit the filtered view without allocating a temporary `Vec`. Returning
    /// `false` stops the walk, which keeps action-bar capability checks cheap
    /// when the first actionable selection is near the front of the listing.
    pub(crate) fn visit_filtered(&self, mut visitor: impl FnMut(usize, &FileEntry) -> bool) {
        let snapshot = self.filtered_snapshot();
        for &index in snapshot.iter() {
            if let Some(entry) = self.listing.entries().get(index)
                && !visitor(index, entry)
            {
                break;
            }
        }
    }

    pub fn toggle_select(&mut self, path: PathBuf) {
        self.selection.toggle_selected(path);
    }

    /// Start a row drag. An unselected anchor always drags only itself; a
    /// selected anchor drags the visible selected set in listing order.
    pub fn begin_drag(&mut self, anchor: PathBuf) {
        self.drag_entries = if self.selection.selected().contains(&anchor) {
            self.filtered_entries()
                .into_iter()
                .filter(|entry| self.selection.selected().contains(&entry.path))
                .map(|entry| entry.path.clone())
                .collect()
        } else {
            vec![anchor]
        };
    }

    /// Flip `path`'s membership in the mark set. Unlike `toggle_select`,
    /// marks are never cleared by select-all/invert/clear-selection.
    pub fn toggle_mark(&mut self, path: PathBuf) {
        self.selection.toggle_marked(path);
    }

    pub fn clear_marks(&mut self) {
        self.selection.clear_marked();
    }

    pub fn select_all(&mut self) {
        let visible: Vec<PathBuf> = self
            .filtered_entries()
            .iter()
            .map(|e| e.path.clone())
            .collect();
        let all_selected = visible
            .iter()
            .all(|path| self.selection.selected().contains(path));
        if all_selected {
            for path in visible {
                self.selection.remove_selected(&path);
            }
        } else {
            self.selection.extend_selected(visible);
        }
    }

    /// Flip selection membership across the filtered view: selected entries
    /// become unselected and vice versa. Entries hidden by the current filter
    /// keep their state, so an invert respects what the user can actually see.
    pub fn invert_selection(&mut self) {
        let paths: Vec<PathBuf> = self
            .filtered_entries()
            .iter()
            .map(|e| e.path.clone())
            .collect();
        for p in paths {
            self.selection.toggle_selected(p);
        }
    }

    /// Add `paths` to the current selection, keeping any existing picks.
    /// Used by relationship-based selectors (e.g. "select files also in the
    /// other panel") so selections compose instead of replacing each other.
    pub fn extend_selection(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.selection.extend_selected(paths);
    }

    pub fn selected_entries(&self) -> Vec<FileEntry> {
        self.filtered_entries()
            .into_iter()
            .filter(|e| self.selection.selected().contains(&e.path))
            .cloned()
            .collect()
    }

    pub fn selected_or_cursor(&self) -> Result<Vec<FileEntry>, StaleCursor> {
        if self.selection.selected().is_empty() {
            match self.selection.focus() {
                Focus::Parent => Ok(vec![]),
                Focus::Entry(_) => match self.cursor_entry() {
                    Some(entry) => Ok(vec![entry.clone()]),
                    None => Err(StaleCursor {
                        cursor: self.cursor(),
                        visible_entries: self.filtered_available_count(),
                    }),
                },
            }
        } else {
            Ok(self.selected_entries())
        }
    }

    pub fn visible_selected_count(&self) -> usize {
        self.filtered_snapshot()
            .iter()
            .filter_map(|&index| self.listing.entries().get(index))
            .filter(|entry| self.selection.selected().contains(&entry.path))
            .count()
    }

    pub fn visible_selected_size(&self) -> u64 {
        let sizes = self.sizes.snapshot();
        self.filtered_snapshot()
            .iter()
            .filter_map(|&i| self.listing.entries().get(i))
            .filter(|e| self.selection.selected().contains(&e.path))
            .map(|entry| sizes.display_size(entry))
            .sum()
    }

    pub fn folder_overview(&self) -> FolderOverview {
        self.sizes
            .folder_overview(self.entries_gen(), self.listing.entries())
    }

    pub fn size_snapshot(&self) -> Arc<SizeSnapshot> {
        self.sizes.snapshot()
    }

    pub fn max_display_size(&self, filtered: &Arc<[usize]>) -> u64 {
        self.sizes
            .max_display_size(self.entries_gen(), self.listing.entries(), filtered)
    }

    /// Monotonic generation of this panel's entry list, bumped on every content
    /// or order change. Lets the app cache per-panel derived data (e.g. the
    /// cross-panel compare map) and rebuild only when the entries change.
    pub fn entries_gen(&self) -> u64 {
        self.listing.revision().value()
    }

    pub fn set_sort(&mut self, col: SortColumn) {
        self.view.toggle_sort(col);
        self.sort_entries();
    }

    /// List subdirectories of `path` (for tree view).
    pub fn subdirs(path: &Path, show_hidden: bool) -> Vec<PathBuf> {
        let Ok(rd) = fs::read_dir(path) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .filter(|p| {
                show_hidden
                    || !p
                        .file_name()
                        .map(|n| n.to_string_lossy().starts_with('.'))
                        .unwrap_or(false)
            })
            .collect();
        dirs.sort_by(|a, b| {
            a.file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .cmp(&b.file_name().map(|n| n.to_string_lossy().to_lowercase()))
        });
        dirs
    }

    pub fn sort_order_for(&self, col: SortColumn) -> Option<SortOrder> {
        (self.view.sort_col() == col).then(|| self.view.sort_order())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    /// In-memory entry for pure sort/filter/selection tests.
    fn entry(name: &str, is_dir: bool, size: u64) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            name_lower: name.to_lowercase(),
            path: PathBuf::from(format!("/test/{name}")),
            identity: ListingIdentity::Unavailable,
            is_dir,
            size,
            extension: String::new(),
            modified: None,
            modified_str: "–".to_string(),
            size_str: if is_dir {
                "…".to_string()
            } else {
                format_size(size)
            },
        }
    }

    #[cfg(unix)]
    #[test]
    fn listing_keeps_symlinks_lexically_bound_without_losing_directory_ux() {
        let temp = TempDir::new();
        temp.dir("target");
        std::os::unix::fs::symlink("target", temp.path().join("folder-link")).unwrap();
        std::os::unix::fs::symlink("missing", temp.path().join("broken-link")).unwrap();

        let DirectoryRead::Complete(entries) = PanelState::read_dir(temp.path(), true) else {
            panic!("symlink listing should be complete");
        };
        let folder = entries
            .iter()
            .find(|entry| entry.name == "folder-link")
            .expect("directory symlink");
        assert!(folder.is_dir, "working directory links remain navigable");
        assert!(matches!(
            &folder.identity,
            ListingIdentity::Captured(identity)
                if identity.kind == Some(crate::path_identity::PathKind::Symlink)
        ));

        let broken = entries
            .iter()
            .find(|entry| entry.name == "broken-link")
            .expect("broken symlink");
        assert!(!broken.is_dir);
        assert!(matches!(
            &broken.identity,
            ListingIdentity::Captured(identity)
                if identity.kind == Some(crate::path_identity::PathKind::Symlink)
        ));
    }

    fn panel_with(entries: Vec<FileEntry>) -> PanelState {
        let mut p = PanelState::new(PathBuf::from("/test"));
        p.replace_entries_for_test(entries);
        p
    }

    #[test]
    fn dragging_an_unselected_row_does_not_use_the_stale_selection() {
        let mut panel = panel_with(vec![
            entry("selected.txt", false, 1),
            entry("dragged.txt", false, 1),
        ]);
        panel.select_path(PathBuf::from("/test/selected.txt"));

        panel.begin_drag(PathBuf::from("/test/dragged.txt"));

        assert_eq!(panel.drag_entries, [PathBuf::from("/test/dragged.txt")]);
    }

    #[test]
    fn dragging_a_selected_row_uses_the_visible_selection() {
        let mut panel = panel_with(vec![entry("a.txt", false, 1), entry("b.txt", false, 1)]);
        panel.extend_selection([PathBuf::from("/test/a.txt"), PathBuf::from("/test/b.txt")]);

        panel.begin_drag(PathBuf::from("/test/b.txt"));

        assert_eq!(
            panel.drag_entries,
            [PathBuf::from("/test/a.txt"), PathBuf::from("/test/b.txt")]
        );
    }

    #[test]
    fn format_size_units() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.00 GB");
    }

    #[test]
    fn sort_puts_dirs_first_and_is_case_insensitive() {
        let mut p = panel_with(vec![
            entry("zeta.txt", false, 1),
            entry("Apple", true, 0),
            entry("beta.txt", false, 1),
            entry("zoo", true, 0),
        ]);
        p.sort_entries();
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Apple", "zoo", "beta.txt", "zeta.txt"]);
    }

    #[test]
    fn sort_desc_reverses_within_groups() {
        let mut p = panel_with(vec![entry("a.txt", false, 1), entry("b.txt", false, 2)]);
        p.set_sort(SortColumn::Size); // asc
        p.set_sort(SortColumn::Size); // same column again -> desc
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["b.txt", "a.txt"]);
    }

    #[test]
    fn descending_sort_keeps_folders_first_and_reverses_within_each_group() {
        let mut p = panel_with(vec![
            entry("small-dir", true, 1),
            entry("large-file", false, 20),
            entry("large-dir", true, 10),
            entry("small-file", false, 2),
        ]);

        p.set_sort(SortColumn::Size);
        p.set_sort(SortColumn::Size);

        let names: Vec<&str> = p
            .entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["large-dir", "small-dir", "large-file", "small-file"]
        );
    }

    #[test]
    fn folders_first_off_sorts_dirs_inline() {
        let mut p = panel_with(vec![
            entry("zeta.txt", false, 1),
            entry("Apple", true, 0),
            entry("beta.txt", false, 1),
            entry("zoo", true, 0),
        ]);
        // Off: folders are no longer pinned, names sort as one stream.
        p.toggle_folders_first();
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Apple", "beta.txt", "zeta.txt", "zoo"]);
        // Back on: folders return to the top.
        p.toggle_folders_first();
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Apple", "zoo", "beta.txt", "zeta.txt"]);
    }

    #[test]
    fn natural_sort_toggle_switches_to_ascii_order() {
        let mut p = panel_with(vec![
            entry("file10.txt", false, 1),
            entry("file2.txt", false, 1),
        ]);
        // Natural (default): file2 before file10.
        p.sort_entries();
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["file2.txt", "file10.txt"]);
        // ASCII: "file10" sorts before "file2" lexicographically.
        p.toggle_natural_sort();
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["file10.txt", "file2.txt"]);
    }

    #[test]
    fn sort_by_extension_groups_by_ext_then_name() {
        let mut p = panel_with(vec![
            entry("b.txt", false, 1),
            entry("a.rs", false, 1),
            entry("c.txt", false, 1),
            entry("z.rs", false, 1),
        ]);
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        p.set_sort(SortColumn::Extension);
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["a.rs", "z.rs", "b.txt", "c.txt"]);
    }

    #[test]
    fn sort_by_kind_orders_image_before_doc_before_code() {
        let mut p = panel_with(vec![
            entry("main.rs", false, 1),
            entry("pic.jpg", false, 1),
            entry("doc.pdf", false, 1),
        ]);
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        p.set_sort(SortColumn::Kind);
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        // Declaration order of Kind: Image, Document, Code.
        assert_eq!(names, vec!["pic.jpg", "doc.pdf", "main.rs"]);
    }

    #[test]
    fn select_junk_picks_known_clutter_only() {
        let mut p = panel_with(vec![
            entry(".DS_Store", false, 6),
            entry("photo.jpg", false, 100),
            entry("Thumbs.db", false, 10),
            entry("notes.txt", false, 20),
        ]);
        let n = p.select_junk();
        assert_eq!(n, 2);
        let names: std::collections::HashSet<String> = p
            .selected_paths()
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert!(names.contains(".DS_Store"));
        assert!(names.contains("Thumbs.db"));
        assert!(!names.contains("photo.jpg"));
    }

    fn selected_names(p: &PanelState) -> std::collections::HashSet<String> {
        p.selected_paths()
            .iter()
            .filter_map(|x| x.file_name().map(|s| s.to_string_lossy().to_string()))
            .collect()
    }

    #[test]
    fn select_largest_picks_top_n_by_size() {
        let mut p = panel_with(vec![
            entry("a", false, 10),
            entry("b", false, 50),
            entry("c", false, 30),
            entry("dir", true, 0),
        ]);
        assert_eq!(p.select_largest(2), 2);
        let names = selected_names(&p);
        assert!(names.contains("b")); // 50
        assert!(names.contains("c")); // 30
        assert!(!names.contains("a"));
        assert!(!names.contains("dir"));
    }

    #[test]
    fn select_like_cursor_matches_extension() {
        let mut p = panel_with(vec![
            entry("a.rs", false, 1),
            entry("b.txt", false, 1),
            entry("c.rs", false, 1),
        ]);
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        p.set_cursor(1); // first filtered entry: a.rs
        assert_eq!(p.select_same_extension_as_cursor(), 2);
        let names = selected_names(&p);
        assert!(names.contains("a.rs"));
        assert!(names.contains("c.rs"));
        assert!(!names.contains("b.txt"));
    }

    #[test]
    fn select_empty_files_picks_zero_byte_only() {
        let mut p = panel_with(vec![
            entry("empty", false, 0),
            entry("full", false, 100),
            entry("dir", true, 0),
        ]);
        assert_eq!(p.select_empty_files(), 1);
        let names = selected_names(&p);
        assert!(names.contains("empty"));
        assert!(!names.contains("full"));
        assert!(!names.contains("dir")); // a folder is never "empty file"
    }

    #[test]
    fn reverse_sort_flips_order() {
        let mut p = panel_with(vec![entry("a.txt", false, 1), entry("b.txt", false, 2)]);
        p.sort_entries(); // Name asc: a, b
        p.reverse_sort(); // -> desc: b, a
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["b.txt", "a.txt"]);
    }

    #[test]
    fn listing_entries_uses_selection_when_present() {
        let mut p = panel_with(vec![
            entry("a", false, 1),
            entry("b", false, 2),
            entry("c", false, 3),
        ]);
        // Nothing selected: the whole filtered view.
        assert_eq!(p.listing_entries().len(), 3);
        // With a selection: only the selected rows, in display order.
        p.select_path(PathBuf::from("/test/b"));
        let names: Vec<String> = p.listing_entries().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["b"]);
    }

    #[test]
    fn filter_matches_case_insensitively() {
        let mut p = panel_with(vec![
            entry("Cargo.toml", false, 1),
            entry("main.rs", false, 1),
        ]);
        p.set_search_query("CARGO");
        let names: Vec<&str> = p
            .filtered_entries()
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["Cargo.toml"]);
    }

    #[test]
    fn whitespace_only_filter_is_inactive_and_matches_everything() {
        let mut p = panel_with(vec![
            entry("Cargo.toml", false, 1),
            entry("main.rs", false, 1),
        ]);
        p.set_search_query("   ");
        assert!(!filter_is_active(p.search_query(), &p.facets()));
        assert_eq!(p.filtered_count(), 2);
    }

    #[test]
    fn filter_is_a_fuzzy_subsequence() {
        let mut p = panel_with(vec![
            entry("scanner.rs", false, 1),
            entry("main.rs", false, 1),
            entry("notes.txt", false, 1),
        ]);
        // "scn" is a subsequence of scanner.rs only (substring would miss it).
        p.set_search_query("scn");
        let names: Vec<&str> = p
            .filtered_entries()
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["scanner.rs"]);

        // Empty query shows everything.
        p.set_search_query("");
        assert_eq!(p.filtered_count(), 3);

        // A non-subsequence excludes every row.
        p.set_search_query("zzz");
        assert_eq!(p.filtered_count(), 0);
    }

    #[test]
    fn toggle_select_adds_then_removes() {
        let mut p = panel_with(vec![entry("a", false, 1)]);
        let path = p.entries()[0].path.clone();
        p.toggle_select(path.clone());
        assert!(p.is_selected(&path));
        p.toggle_select(path.clone());
        assert!(!p.is_selected(&path));
    }

    #[test]
    fn toggle_mark_adds_then_removes_independently_of_selection() {
        let mut p = panel_with(vec![entry("a", false, 1)]);
        let path = p.entries()[0].path.clone();
        p.toggle_select(path.clone());
        p.toggle_mark(path.clone());
        assert!(p.is_marked(&path));
        assert!(p.is_selected(&path), "marking does not touch selection");
        p.toggle_mark(path.clone());
        assert!(!p.is_marked(&path));
        assert!(p.is_selected(&path), "unmarking does not touch selection");
    }

    #[test]
    fn clear_marks_empties_the_set_without_touching_selection() {
        let mut p = panel_with(vec![entry("a", false, 1)]);
        let path = p.entries()[0].path.clone();
        p.toggle_select(path.clone());
        p.toggle_mark(path.clone());
        p.clear_marks();
        assert!(p.marked_paths().is_empty());
        assert!(p.is_selected(&path));
    }

    #[test]
    fn select_all_toggles_between_all_and_none() {
        let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
        p.select_all();
        assert_eq!(p.selected_count(), 2);
        p.select_all();
        assert!(p.selection_is_empty());
    }

    #[test]
    fn select_all_preserves_selection_hidden_by_filter() {
        let mut p = panel_with(vec![
            entry("alpha", false, 1),
            entry("album", false, 1),
            entry("zebra", false, 1),
        ]);
        let alpha = p.entries()[0].path.clone();
        let album = p.entries()[1].path.clone();
        let zebra = p.entries()[2].path.clone();
        p.select_path(zebra.clone());
        p.set_search_query("al");

        p.select_all();
        assert!(p.is_selected(&alpha));
        assert!(p.is_selected(&album));
        assert!(p.is_selected(&zebra));

        p.select_all();
        assert!(!p.is_selected(&alpha));
        assert!(!p.is_selected(&album));
        assert!(p.is_selected(&zebra));
    }

    #[test]
    fn visible_selection_metrics_do_not_count_filtered_out_items() {
        let mut panel = panel_with(vec![entry("alpha", false, 10), entry("zebra", false, 90)]);
        panel.extend_selection([PathBuf::from("/test/alpha"), PathBuf::from("/test/zebra")]);
        panel.set_search_query("alpha");

        assert_eq!(panel.selected_count(), 2);
        assert_eq!(panel.visible_selected_count(), 1);
        assert_eq!(panel.visible_selected_size(), 10);
        assert_eq!(
            panel.selected_or_cursor().unwrap()[0].path,
            PathBuf::from("/test/alpha")
        );
    }

    #[test]
    fn incomplete_snapshot_preserves_selection_marks_and_reconciliation() {
        let mut panel = panel_with(vec![entry("keep.txt", false, 10)]);
        let path = PathBuf::from("/test/keep.txt");
        panel.select_path(path.clone());
        panel.toggle_mark(path.clone());
        panel.watcher.activate_test_binding(Path::new("/test"));
        let ticket = panel.watcher.snapshot_ticket();

        for status in [DirStatus::Partial, DirStatus::Denied, DirStatus::Gone] {
            assert!(!panel.apply_directory_read(DirectoryRead::Incomplete(status)));
            assert_eq!(panel.entries().len(), 1);
            assert!(panel.is_selected(&path));
            assert!(panel.is_marked(&path));
            assert_eq!(panel.dir_status(), status);
        }
        panel.watcher.defer_snapshot(ticket.as_ref());
        assert!(panel.watcher.has_pending_reconciliation_for_test());
    }

    fn assert_incomplete_navigation_hides_old_binding(status: DirStatus) {
        let old_entry = entry("keep.txt", false, 10);
        let old_path = old_entry.path.clone();
        let mut panel = panel_with(vec![old_entry.clone()]);
        panel.select_path(old_path.clone());
        panel.toggle_mark(old_path.clone());
        panel.set_cursor(1);
        panel.begin_drag(old_path.clone());
        panel.drop_target = Some(PathBuf::from("/test"));

        let old_listing_revision = panel.entries_gen();
        let old_size_revision = panel.size_snapshot().revision();
        let old_filter = panel.filtered_indices();
        assert_eq!(panel.folder_overview().total, Some(10));

        panel.current_path = PathBuf::from("/unavailable");
        assert!(!panel.apply_directory_read(DirectoryRead::Incomplete(status)));

        assert_eq!(panel.listing.binding(), Path::new("/unavailable"));
        assert!(panel.entries().is_empty());
        assert!(panel.filtered_indices().is_empty());
        assert!(!Arc::ptr_eq(&old_filter, &panel.filtered_indices()));
        assert!(panel.entries_gen() > old_listing_revision);
        assert!(panel.size_snapshot().revision() > old_size_revision);
        assert_eq!(
            panel.folder_overview(),
            FolderOverview {
                total: Some(0),
                largest: None,
                oldest: None,
            }
        );
        assert!(panel.selected_paths().is_empty());
        assert!(panel.marked_paths().is_empty());
        assert!(panel.selected_or_cursor().unwrap().is_empty());
        assert_eq!(panel.cursor(), 0);
        assert!(panel.cursor_entry().is_none());
        assert!(panel.drag_entries.is_empty());
        assert!(panel.drop_target.is_none());

        panel.current_path = PathBuf::from("/test");
        assert!(panel.apply_directory_read(DirectoryRead::Complete(vec![old_entry])));
        assert!(panel.is_selected(&old_path));
        assert!(panel.is_marked(&old_path));
        let actionable = panel.selected_or_cursor().unwrap();
        assert_eq!(actionable.len(), 1);
        assert_eq!(actionable[0].path, old_path);
    }

    #[test]
    fn gone_after_binding_switch_exposes_no_old_actions_and_restores_on_return() {
        assert_incomplete_navigation_hides_old_binding(DirStatus::Gone);
    }

    #[test]
    fn denied_after_binding_switch_exposes_no_old_actions_and_restores_on_return() {
        assert_incomplete_navigation_hides_old_binding(DirStatus::Denied);
    }

    #[test]
    fn partial_after_binding_switch_exposes_no_old_actions_and_restores_on_return() {
        assert_incomplete_navigation_hides_old_binding(DirStatus::Partial);
    }

    #[test]
    fn invert_selection_flips_only_filtered_rows() {
        let mut p = panel_with(vec![
            entry("alpha", false, 1),
            entry("album", false, 1),
            entry("zebra", false, 1),
        ]);
        let alpha = p.entries()[0].path.clone();
        let album = p.entries()[1].path.clone();
        let zebra = p.entries()[2].path.clone();
        // Pre-select one visible (alpha) and one that the filter will hide (zebra).
        p.extend_selection([alpha.clone(), zebra.clone()]);
        // Filter to the "al" rows; zebra is now hidden from the view.
        p.set_search_query("al");
        p.invert_selection();
        assert!(!p.is_selected(&alpha), "visible+selected -> cleared");
        assert!(p.is_selected(&album), "visible+unselected -> selected");
        assert!(p.is_selected(&zebra), "filtered-out row keeps its state");
    }

    #[test]
    fn selected_or_cursor_falls_back_to_cursor_row() {
        let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
        p.set_cursor(0); // ".." row
        assert!(p.selected_or_cursor().unwrap().is_empty());
        p.set_cursor(2); // second file
        let picked = p.selected_or_cursor().unwrap();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].name, "b");
    }

    #[test]
    fn sorting_reclamps_a_cursor_after_the_filter_changes() {
        let mut p = panel_with(vec![entry("alpha", false, 1), entry("beta", false, 1)]);
        p.set_cursor(2);
        p.set_search_query("alpha");

        p.sort_entries();

        assert_eq!(p.cursor(), 1);
        assert_eq!(p.filtered_get(0).unwrap().name, "alpha");
    }

    #[test]
    fn sort_and_filter_keep_focus_on_the_same_path() {
        let mut p = panel_with(vec![
            entry("beta.txt", false, 20),
            entry("alpha.txt", false, 10),
            entry("notes.md", false, 30),
        ]);
        p.set_cursor(1);
        let focused = p.focused_path().unwrap();

        p.set_search_query("t");
        assert_eq!(p.focused_path(), Some(focused.clone()));
        p.set_sort(SortColumn::Size);
        assert_eq!(p.focused_path(), Some(focused));
    }

    #[test]
    fn equal_sort_keys_use_path_as_a_stable_tie_breaker() {
        let mut z = entry("same.txt", false, 10);
        z.path = PathBuf::from("/z/same.txt");
        let mut a = entry("same.txt", false, 10);
        a.path = PathBuf::from("/a/same.txt");
        let mut p = panel_with(vec![z, a]);

        p.set_sort(SortColumn::Size);

        assert_eq!(p.entries()[0].path, PathBuf::from("/a/same.txt"));
        assert_eq!(p.entries()[1].path, PathBuf::from("/z/same.txt"));

        p.set_sort(SortColumn::Size);
        assert_eq!(p.sort_order(), SortOrder::Desc);
        assert_eq!(p.entries()[0].path, PathBuf::from("/a/same.txt"));
        assert_eq!(p.entries()[1].path, PathBuf::from("/z/same.txt"));
    }

    #[test]
    fn stale_filter_indices_are_bounded_and_cursor_miss_is_explicit() {
        let mut p = panel_with(vec![entry("alpha", false, 1), entry("beta", false, 1)]);
        p.set_cursor(2);
        assert_eq!(p.filtered_count(), 2); // warm the index cache
        p.clear_entries_without_revision_for_test();

        assert!(p.filtered_entries().is_empty());
        assert!(p.filtered_indices().is_empty());
        assert_eq!(
            p.selected_or_cursor().unwrap_err(),
            StaleCursor {
                cursor: 2,
                visible_entries: 0,
            }
        );
    }

    #[test]
    fn reload_preserves_cursor_by_path_and_prunes_selection() {
        let tmp = TempDir::new();
        tmp.file("a.txt", "1");
        tmp.file("b.txt", "2");
        let doomed = tmp.file("c.txt", "3");

        let mut p = PanelState::new(tmp.path().to_path_buf());
        p.refresh();
        assert_eq!(p.entries().len(), 3);

        // Cursor on "b.txt" (row 2), select and mark "c.txt".
        p.set_cursor(2);
        p.toggle_select(doomed.clone());
        p.toggle_mark(doomed.clone());

        // A new file shifts sort order; a selected/marked file disappears.
        tmp.file("0-first.txt", "0");
        std::fs::remove_file(&doomed).unwrap();
        p.refresh();

        let under_cursor = p.filtered_entries()[p.cursor() - 1].path.clone();
        assert!(under_cursor.ends_with("b.txt"), "cursor follows the path");
        assert!(p.selection_is_empty(), "selection drops deleted paths");
        assert!(p.marked_paths().is_empty(), "marks drop deleted paths");
    }

    #[test]
    fn seeded_view_config_drives_the_first_complete_listing() {
        let root = TempDir::new();
        root.file("small.txt", "1");
        root.file("large.txt", "12345");
        root.file(".hidden.txt", "123");
        let config = ViewConfig::default()
            .with_sort(SortColumn::Size, SortOrder::Desc)
            .with_show_hidden(true)
            .with_density(crate::density::Density::Compact);
        let mut panel = PanelState::new_with_view(root.path().to_path_buf(), config);

        panel.refresh();

        assert_eq!(panel.view_config(), config);
        assert_eq!(
            panel
                .entries()
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["large.txt", ".hidden.txt", "small.txt"]
        );
    }

    #[test]
    fn hidden_toggle_commits_config_and_listing_together() {
        let root = TempDir::new();
        root.file("visible.txt", "v");
        root.file(".hidden.txt", "h");
        let mut panel = PanelState::new(root.path().to_path_buf());
        panel.refresh();
        let before_revision = panel.entries_gen();
        assert!(!panel.show_hidden());
        assert_eq!(panel.entries().len(), 1);
        panel.set_cursor(1);
        let focused = panel.cursor_entry().unwrap().path.clone();

        assert_eq!(panel.toggle_hidden(), ViewApplyOutcome::Applied);

        assert!(panel.show_hidden());
        assert_eq!(panel.entries().len(), 2);
        assert_eq!(panel.entries_gen(), before_revision + 1);
        assert_eq!(panel.cursor_entry().unwrap().path, focused);
    }

    #[test]
    fn rejected_hidden_toggle_preserves_complete_view_and_rows() {
        let root = TempDir::new();
        let folder = root.dir("folder");
        root.file("folder/visible.txt", "v");
        root.file("folder/second.txt", "s");
        let mut panel = PanelState::new(folder.clone());
        panel.refresh();
        let selected = panel.entries()[0].path.clone();
        panel.select_path(selected.clone());
        panel.toggle_mark(selected);
        panel.set_cursor(1);
        panel.set_scroll_anchor(1);
        panel.set_search_query("txt");
        panel.watcher.activate_test_binding(&folder);

        let config = panel.view_config();
        let paths = panel
            .entries()
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let revision = panel.entries_gen();
        let status = panel.dir_status();
        let cursor = panel.cursor();
        let scroll_anchor = panel.scroll_anchor();
        let selected = panel.selected_paths().clone();
        let marked = panel.marked_paths().clone();
        let filter = panel.filtered_indices();
        let size_revision = panel.size_snapshot().revision();
        let size_binding = panel.sizes.binding_for_test().to_path_buf();
        let watcher_state = panel.watcher.reconciliation_state_for_test();
        std::fs::remove_dir_all(&folder).unwrap();

        assert_eq!(
            panel.toggle_hidden(),
            ViewApplyOutcome::ReadRejected(DirStatus::Gone)
        );

        assert_eq!(panel.view_config(), config);
        assert_eq!(panel.entries_gen(), revision);
        assert_eq!(panel.dir_status(), status);
        assert_eq!(panel.cursor(), cursor);
        assert_eq!(panel.scroll_anchor(), scroll_anchor);
        assert_eq!(panel.selected_paths(), &selected);
        assert_eq!(panel.marked_paths(), &marked);
        assert!(Arc::ptr_eq(&filter, &panel.filtered_indices()));
        assert_eq!(panel.size_snapshot().revision(), size_revision);
        assert_eq!(panel.sizes.binding_for_test(), size_binding);
        assert_eq!(panel.watcher.reconciliation_state_for_test(), watcher_state);
        assert_eq!(
            panel
                .entries()
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            paths
        );
    }

    #[test]
    fn watcher_event_racing_hidden_snapshot_remains_pending_for_reconciliation() {
        let root = TempDir::new();
        root.file("before.txt", "before");
        let mut panel = PanelState::new(root.path().to_path_buf());
        panel.refresh();
        panel.watcher.activate_test_binding(root.path());

        let raced = root.file("raced.txt", "raced");
        panel.watcher.inject_current_change_for_test(raced.clone());

        assert_eq!(panel.toggle_hidden(), ViewApplyOutcome::Applied);
        assert!(panel.poll_fs_changes());
        assert!(panel.entries().iter().any(|entry| entry.path == raced));
        assert!(!panel.watcher.has_pending_reconciliation_for_test());
    }

    #[test]
    fn history_navigation_walks_back_and_forward() {
        let tmp = TempDir::new();
        let sub = tmp.dir("sub");

        let mut p = PanelState::new(tmp.path().to_path_buf());
        p.refresh();
        p.navigate_to(sub.clone());
        assert_eq!(p.current_path, sub);
        assert!(p.can_go_back());

        p.go_back();
        assert_eq!(p.current_path, tmp.path());
        assert!(p.can_go_forward());

        p.go_forward();
        assert_eq!(p.current_path, sub);
    }

    #[test]
    fn history_navigation_prunes_directories_that_disappeared() {
        let temp = TempDir::new();
        let first = temp.dir("first");
        let removed = temp.dir("removed");
        let last = temp.dir("last");

        let mut panel = PanelState::new(temp.path().to_path_buf());
        panel.refresh();
        panel.navigate_to(first.clone());
        panel.navigate_to(removed.clone());
        panel.navigate_to(last.clone());
        std::fs::remove_dir(&removed).unwrap();

        panel.go_back();
        assert_eq!(panel.current_path, first);
        panel.go_forward();
        assert_eq!(panel.current_path, last);
    }

    #[test]
    fn history_jump_back_twice_then_forward_and_truncate() {
        let tmp = TempDir::new();
        let a = tmp.dir("a");
        let b = tmp.dir("b");
        let c = tmp.dir("c");
        let d = tmp.dir("d");

        let mut p = PanelState::new(tmp.path().to_path_buf());
        p.refresh();
        p.navigate_to(a.clone()); // trail: [root, a]
        p.navigate_to(b.clone()); // [root, a, b]
        p.navigate_to(c.clone()); // [root, a, b, c]

        // Back twice lands on `a`; forward returns to `b`.
        p.go_back();
        p.go_back();
        assert_eq!(p.current_path, a);
        p.go_forward();
        assert_eq!(p.current_path, b);

        // A fresh navigation from `b` truncates the forward tail (drops c).
        p.navigate_to(d.clone());
        assert_eq!(p.current_path, d);
        assert!(!p.can_go_forward(), "forward tail truncated by a new jump");
        // Back now steps to `b`, not the discarded `c`.
        p.go_back();
        assert_eq!(p.current_path, b);
    }

    #[test]
    fn go_up_lands_cursor_on_the_left_directory() {
        let tmp = TempDir::new();
        tmp.dir("aaa");
        let mid = tmp.dir("mmm");
        tmp.dir("zzz");

        let mut p = PanelState::new(mid.clone());
        p.refresh();
        p.go_up();

        assert_eq!(p.current_path, tmp.path());
        // Cursor should sit on "mmm" (the dir we came from), not row 0.
        let under = p.filtered_get(p.cursor() - 1).unwrap();
        assert_eq!(under.name, "mmm");
    }

    #[test]
    fn navigate_to_restores_the_view_remembered_for_that_directory() {
        let tmp = TempDir::new();
        let downloads = tmp.dir("downloads");
        let docs = tmp.dir("docs");

        let mut p = PanelState::new(downloads.clone());
        p.refresh();
        p.set_sort(SortColumn::Size);
        p.reverse_sort();
        assert_eq!(p.toggle_hidden(), ViewApplyOutcome::Applied);
        p.set_density(crate::density::Density::Compact);
        p.set_search_query("download-filter");

        // "docs" has never been visited: like before per-folder memory
        // existed, its view carries over from wherever we came from.
        p.navigate_to(docs.clone());
        assert_eq!(p.sort_column(), SortColumn::Size);
        assert_eq!(p.sort_order(), SortOrder::Desc);
        assert!(p.show_hidden());
        assert_eq!(p.density(), crate::density::Density::Compact);
        assert_eq!(p.search_query(), "");

        // Now give "docs" its own, different view.
        p.set_sort(SortColumn::Extension);
        assert_eq!(p.toggle_hidden(), ViewApplyOutcome::Applied);
        p.set_density(crate::density::Density::Spacious);
        p.set_search_query("docs-filter");

        // Back to "downloads": its own remembered view returns, not "docs"'s.
        p.navigate_to(downloads.clone());
        assert_eq!(p.sort_column(), SortColumn::Size);
        assert_eq!(p.sort_order(), SortOrder::Desc);
        assert!(p.show_hidden());
        assert_eq!(p.density(), crate::density::Density::Compact);
        assert_eq!(p.search_query(), "download-filter");

        // And "docs" kept its own distinct view too.
        p.navigate_to(docs);
        assert_eq!(p.sort_column(), SortColumn::Extension);
        assert_eq!(p.sort_order(), SortOrder::Asc);
        assert!(!p.show_hidden());
        assert_eq!(p.density(), crate::density::Density::Spacious);
        assert_eq!(p.search_query(), "docs-filter");
    }

    #[test]
    fn navigate_to_restores_cursor_path_and_scroll_anchor() {
        let tmp = TempDir::new();
        let downloads = tmp.dir("downloads");
        let docs = tmp.dir("docs");
        tmp.file("downloads/a.txt", "a");
        let focused = tmp.file("downloads/b.txt", "b");
        tmp.file("downloads/c.txt", "c");

        let mut p = PanelState::new(downloads.clone());
        p.refresh();
        let cursor = p
            .filtered_entries()
            .iter()
            .position(|entry| entry.path == focused)
            .unwrap()
            + 1;
        p.set_cursor(cursor);
        p.set_scroll_anchor(1);

        p.navigate_to(docs);
        p.navigate_to(downloads);

        assert_eq!(p.filtered_get(p.cursor() - 1).unwrap().path, focused);
        assert_eq!(p.scroll_anchor(), 1);
        assert!(p.scroll_to_cursor());
    }

    #[test]
    fn missing_remembered_cursor_falls_back_to_scroll_anchor() {
        let tmp = TempDir::new();
        let downloads = tmp.dir("downloads");
        let docs = tmp.dir("docs");
        tmp.file("downloads/a.txt", "a");
        let focused = tmp.file("downloads/b.txt", "b");
        tmp.file("downloads/c.txt", "c");

        let mut p = PanelState::new(downloads.clone());
        p.refresh();
        p.set_cursor(2);
        p.set_scroll_anchor(1);
        p.navigate_to(docs);
        std::fs::remove_file(focused).unwrap();
        p.navigate_to(downloads);

        assert_eq!(p.cursor(), 2.min(p.filtered_count()));
        assert!(p.cursor() <= p.filtered_count());
    }

    #[test]
    fn natural_cmp_orders_numbers_by_value() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("file10", "file2"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        // Equal numeric value: the shorter run (fewer leading zeros) sorts first.
        assert_eq!(natural_cmp("img9", "img09"), Ordering::Less);
        assert_eq!(natural_cmp("v1.2", "v1.10"), Ordering::Less);
    }

    #[test]
    fn natural_sort_applies_to_listing() {
        let mut p = panel_with(vec![
            entry("file10.txt", false, 1),
            entry("file2.txt", false, 1),
            entry("file1.txt", false, 1),
        ]);
        p.sort_entries();
        let names: Vec<&str> = p.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["file1.txt", "file2.txt", "file10.txt"]);
    }

    #[test]
    fn classify_dir_distinguishes_empty_denied_gone() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new();

        let empty = tmp.dir("empty");
        assert_eq!(classify_dir(&empty, true), DirStatus::Empty);

        let full = tmp.dir("full");
        tmp.file("full/x.txt", "x");
        assert_eq!(classify_dir(&full, false), DirStatus::Listed);

        assert_eq!(
            classify_dir(&tmp.path().join("nope"), true),
            DirStatus::Gone
        );

        let denied = tmp.dir("denied");
        std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o000)).unwrap();
        let status = classify_dir(&denied, true);
        let _ = std::fs::set_permissions(&denied, std::fs::Permissions::from_mode(0o755));
        assert_eq!(status, DirStatus::Denied);
    }

    #[test]
    fn push_visit_dedupes_caps_and_orders_recent_first() {
        let mut v: Vec<PathBuf> = Vec::new();
        push_visit(&mut v, Path::new("/a"), 3);
        push_visit(&mut v, Path::new("/b"), 3);
        push_visit(&mut v, Path::new("/a"), 3); // revisit /a -> front, no dup
        assert_eq!(v, vec![PathBuf::from("/a"), PathBuf::from("/b")]);

        push_visit(&mut v, Path::new("/c"), 3);
        push_visit(&mut v, Path::new("/d"), 3); // cap 3 drops oldest
        assert_eq!(
            v,
            vec![
                PathBuf::from("/d"),
                PathBuf::from("/c"),
                PathBuf::from("/a"),
            ]
        );
    }

    #[test]
    fn recent_visit_policy_keeps_two_hundred_paths() {
        let mut v: Vec<PathBuf> = Vec::new();
        for i in 0..205 {
            push_visit(&mut v, Path::new(&format!("/recent/{i}")), VISITED_CAP);
        }

        assert_eq!(v.len(), 200);
        assert_eq!(v.first(), Some(&PathBuf::from("/recent/204")));
        assert_eq!(v.last(), Some(&PathBuf::from("/recent/5")));
    }

    #[test]
    fn recent_destinations_can_switch_between_frecency_and_chronology() {
        let a = PathBuf::from("/work/frequent");
        let b = PathBuf::from("/work/middle");
        let c = PathBuf::from("/work/latest");
        let paths = vec![c.clone(), b.clone(), a.clone()];
        let mut stats = VisitStats::default();
        stats.record(&a);
        stats.record(&a);
        stats.record(&a);
        stats.record(&b);
        stats.record(&c);

        let chronological = rank_visited(&paths, "", RecentOrder::Chronological, &stats);
        assert_eq!(chronological[0].path, c);

        let frecency = rank_visited(&paths, "", RecentOrder::Frecency, &stats);
        assert_eq!(frecency[0].path, a);
        assert_eq!(frecency[0].count, 3);
    }

    #[test]
    fn recent_destination_query_uses_fuzzy_path_matching() {
        let paths = vec![
            PathBuf::from("/Users/me/Documents"),
            PathBuf::from("/Users/me/Downloads"),
        ];
        let matches = rank_visited(
            &paths,
            "dwn",
            RecentOrder::Chronological,
            &VisitStats::default(),
        );
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, PathBuf::from("/Users/me/Downloads"));
    }

    #[test]
    fn format_mode_renders_rwx() {
        assert_eq!(format_mode(0o755), "rwxr-xr-x");
        assert_eq!(format_mode(0o644), "rw-r--r--");
        assert_eq!(format_mode(0o600), "rw-------");
        assert_eq!(format_mode(0o000), "---------");
        assert_eq!(format_mode(0o777), "rwxrwxrwx");
    }

    #[test]
    fn make_info_describes_file_and_folder() {
        let tmp = TempDir::new();
        let f = tmp.file("notes.txt", "hello");
        let meta = std::fs::metadata(&f).unwrap();
        let fe = FileEntry::from_meta(f, &meta).unwrap();
        let card = make_info(&fe, None, None);
        assert_eq!(card.kind, "TXT file");
        assert_eq!(card.size, "5 B");
        assert!(card.children.is_none());
        assert_eq!(card.permissions.len(), 9);

        let d = tmp.dir("box");
        let meta = std::fs::metadata(&d).unwrap();
        let de = FileEntry::from_meta(d, &meta).unwrap();
        let card = make_info(&de, Some(4096), Some(3));
        assert_eq!(card.kind, "Folder");
        assert_eq!(card.size, "4.0 KB");
        assert_eq!(card.children, Some(3));
    }

    #[test]
    fn facet_matches_kind_size_age() {
        use std::time::{Duration, UNIX_EPOCH};
        let now = UNIX_EPOCH + Duration::from_secs(100 * 24 * 60 * 60); // day 100
        let mut img = entry("photo.jpg", false, 5_000_000);
        img.extension = "jpg".into();
        img.modified = Some(UNIX_EPOCH + Duration::from_secs(99 * 24 * 60 * 60)); // 1 day old
        let mut code = entry("main.rs", false, 100);
        code.extension = "rs".into();
        code.modified = Some(UNIX_EPOCH + Duration::from_secs(50 * 24 * 60 * 60)); // 50 days old
        let dir = entry("box", true, 0);

        let images = FacetSet {
            kind: Some(KindFacet::Images),
            ..Default::default()
        };
        assert!(facet_matches(&img, &images, now));
        assert!(!facet_matches(&code, &images, now));

        let folders = FacetSet {
            kind: Some(KindFacet::Folders),
            ..Default::default()
        };
        assert!(facet_matches(&dir, &folders, now));
        assert!(!facet_matches(&img, &folders, now));

        let big = FacetSet {
            min_size: Some(1_000_000),
            ..Default::default()
        };
        assert!(facet_matches(&img, &big, now)); // 5MB
        assert!(!facet_matches(&code, &big, now)); // 100B
        assert!(facet_matches(&dir, &big, now)); // folders ignore size

        let recent = FacetSet {
            max_age_days: Some(7),
            ..Default::default()
        };
        assert!(facet_matches(&img, &recent, now)); // 1 day
        assert!(!facet_matches(&code, &recent, now)); // 50 days

        let stale = FacetSet {
            min_age_days: Some(30),
            ..Default::default()
        };
        assert!(!facet_matches(&img, &stale, now)); // 1 day: too fresh
        assert!(facet_matches(&code, &stale, now)); // 50 days: old enough
        assert!(!facet_matches(&dir, &stale, now)); // unknown mtime: fails

        // Empty facets pass everything.
        assert!(facet_matches(&code, &FacetSet::default(), now));
    }

    #[test]
    fn folder_overview_reports_largest_and_oldest() {
        use std::time::{Duration, UNIX_EPOCH};
        let mut a = entry("a.txt", false, 100);
        a.modified = Some(UNIX_EPOCH + Duration::from_secs(300));
        let mut big = entry("big.bin", false, 900);
        big.modified = Some(UNIX_EPOCH + Duration::from_secs(200));
        let mut old = entry("old.log", false, 50);
        old.modified = Some(UNIX_EPOCH + Duration::from_secs(100));
        let p = panel_with(vec![a, big, old]);
        let o = p.folder_overview();
        assert_eq!(o.total, Some(1050));
        assert_eq!(o.largest, Some(("big.bin".to_string(), 900)));
        assert_eq!(
            o.oldest,
            Some(("old.log".to_string(), UNIX_EPOCH + Duration::from_secs(100)))
        );
    }

    #[test]
    fn facets_filter_the_listing() {
        let mut p = panel_with(vec![
            entry("a.jpg", false, 1),
            entry("b.rs", false, 1),
            entry("c.jpg", false, 1),
        ]);
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        p.set_facets(FacetSet {
            kind: Some(KindFacet::Images),
            ..Default::default()
        });
        let names: Vec<&str> = p
            .filtered_entries()
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["a.jpg", "c.jpg"]);
    }

    #[test]
    fn glob_match_handles_star_and_question() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("img_*.jpg", "img_2024.jpg"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(!glob_match("*.rs", "main.txt"));
        assert!(glob_match("*", "anything"));
    }

    #[test]
    fn parse_mask_splits_and_marks_subtraction() {
        let terms = parse_mask("*.JPG, !*raw*, -tmp");
        assert_eq!(
            terms,
            vec![
                ("*.jpg".to_string(), false),
                ("*raw*".to_string(), true),
                ("tmp".to_string(), true),
            ]
        );
    }

    #[test]
    fn select_by_mask_adds_then_subtracts() {
        let mut p = panel_with(vec![
            entry("a.jpg", false, 1),
            entry("b.jpg", false, 1),
            entry("c.png", false, 1),
            entry("raw.jpg", false, 1),
        ]);
        // bare ext for files needs the extension field populated:
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        p.sort_entries();

        // Add all jpgs, then subtract anything containing "raw".
        // raw.jpg matches the add but the subtract wins, so it is not added.
        let added = p.select_by_mask("*.jpg, !*raw*");
        assert_eq!(added, 2);
        let names: Vec<String> = p
            .selected_paths()
            .iter()
            .filter_map(|pth| pth.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert!(names.contains(&"a.jpg".to_string()));
        assert!(names.contains(&"b.jpg".to_string()));
        assert!(!names.contains(&"raw.jpg".to_string()));
        assert!(!names.contains(&"c.png".to_string()));
        assert_eq!(p.selected_count(), 2);
    }

    #[test]
    fn mask_match_count_reports_selection_changes() {
        let mut p = panel_with(vec![
            entry("a.jpg", false, 1),
            entry("raw.jpg", false, 1),
            entry("note.txt", false, 1),
        ]);
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        let raw = p
            .entries()
            .iter()
            .find(|entry| entry.name == "raw.jpg")
            .unwrap()
            .path
            .clone();

        assert_eq!(p.mask_match_count("!*raw*"), 0);
        p.select_path(raw);
        assert_eq!(p.mask_match_count("!*raw*"), 1);
        assert_eq!(p.mask_match_count("*.jpg, !*raw*"), 2);
    }

    #[test]
    fn bare_extension_term_matches_by_extension() {
        let mut p = panel_with(vec![
            entry("doc.pdf", false, 1),
            entry("note.txt", false, 1),
        ]);
        p.mutate_entries_for_test(|entries| {
            for e in entries {
                e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
            }
        });
        assert_eq!(p.mask_match_count("pdf"), 1);
        p.select_by_mask("pdf");
        assert_eq!(p.selected_count(), 1);
    }

    #[test]
    fn entry_display_size_uses_dir_sizes_for_folders() {
        let file = entry("a.txt", false, 42);
        let dir = entry("sub", true, 0);
        let mut sizes = HashMap::new();
        sizes.insert(dir.path.clone(), 9000u64);

        assert_eq!(entry_display_size(&file, &sizes), 42);
        assert_eq!(entry_display_size(&dir, &sizes), 9000);
        // Unmeasured dir reads as 0 (draws no bar).
        let other = entry("pending", true, 0);
        assert_eq!(entry_display_size(&other, &sizes), 0);
    }

    #[test]
    fn type_ahead_jumps_by_prefix_then_substring() {
        let mut p = panel_with(vec![
            entry("apple.txt", false, 1),
            entry("banana.txt", false, 1),
            entry("cherry-banana.txt", false, 1),
        ]);
        p.sort_entries();

        assert!(p.type_ahead("ban"));
        assert_eq!(p.filtered_get(p.cursor() - 1).unwrap().name, "banana.txt");

        // No prefix hit -> substring fallback finds "cherry-banana".
        assert!(p.type_ahead("cherry"));
        assert_eq!(
            p.filtered_get(p.cursor() - 1).unwrap().name,
            "cherry-banana.txt"
        );

        assert!(!p.type_ahead("zzz"));
    }

    #[test]
    fn select_cursor_adds_current_row() {
        let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
        p.set_cursor(2); // second file
        p.select_cursor();
        assert!(p.is_selected(&p.entries()[1].path));
        assert_eq!(p.selected_count(), 1);
    }

    #[test]
    fn filter_cache_tracks_query_and_entry_changes() {
        let mut p = panel_with(vec![entry("alpha", false, 1), entry("beta", false, 1)]);
        assert_eq!(p.filtered_count(), 2);

        // Query change invalidates the cache.
        p.set_search_query("al");
        assert_eq!(p.filtered_count(), 1);
        assert_eq!(p.filtered_get(0).unwrap().name, "alpha");

        // Entry change (generation bump via sort) invalidates it too.
        p.push_entry_for_test(entry("alps", false, 1));
        p.sort_entries();
        assert_eq!(p.filtered_count(), 2);
        let names: Vec<&str> = p
            .filtered_entries()
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["alpha", "alps"]);
    }

    #[test]
    fn filtered_indices_point_into_entries() {
        let mut p = panel_with(vec![
            entry("keep.txt", false, 1),
            entry("skip.rs", false, 1),
            entry("keeper.txt", false, 1),
        ]);
        p.set_search_query("keep");
        let idx = p.filtered_indices();
        assert_eq!(idx.len(), 2);
        for &i in idx.iter() {
            assert!(p.entries()[i].name.contains("keep"));
        }
    }

    #[test]
    fn size_cache_invalidation_cancels_epoch_and_removes_ancestors_only() {
        let tmp = TempDir::new();
        let a = tmp.dir("a");
        let other = tmp.dir("other");
        let a_key = VolumePathKey::observe(&a);
        let other_key = VolumePathKey::observe(&other);
        let active_epoch = capture_path_epoch(&a_key);
        let now = SystemTime::now();
        {
            let _boundary = lock_recover(scan_commit_boundary());
            let mut cache = lock_recover(dir_size_cache());
            cache.insert(a_key.clone(), (now, 100));
            cache.insert(other_key.clone(), (now, 5));
        }

        invalidate_size_cache(&a.join("b/c/file.txt"));

        assert!(active_epoch.is_cancelled());
        let cache = lock_recover(dir_size_cache());
        assert!(!cache.contains_key(&a_key));
        assert_eq!(cache[&other_key].0, now, "unrelated dirs stay valid");
    }

    #[test]
    fn directory_cache_key_rejects_an_old_mount_generation() {
        let path = PathBuf::from("/fixture/folder");
        let first = VolumePathKey {
            path: path.clone(),
            volume_id: 7,
            generation: 1,
        };
        let remounted = VolumePathKey {
            path,
            volume_id: 7,
            generation: 2,
        };
        assert_ne!(first, remounted);
    }

    #[test]
    fn watcher_invalidation_wins_against_worker_paused_before_commit() {
        let tmp = TempDir::new();
        let directory = tmp.dir("subject");
        let changed = directory.join("nested/file.bin");
        let cache_key = VolumePathKey::observe(&directory);
        let path_epoch = capture_path_epoch(&cache_key);
        let current = Arc::new(Mutex::new(Arc::new(ScanEpoch::new())));
        let sizes = Arc::new(Mutex::new(HashMap::new()));
        let counts = Arc::new(Mutex::new(HashMap::new()));
        let revision = Arc::new(AtomicU64::new(0));
        let scan_epoch = begin_panel_scan(
            &current,
            &sizes,
            &counts,
            &HashSet::from([directory.clone()]),
            &revision,
        );

        {
            let _boundary = lock_recover(scan_commit_boundary());
            lock_recover(dir_size_cache()).remove(&cache_key);
            lock_recover(walk_log()).remove(&cache_key);
        }

        let barrier = Arc::new(std::sync::Barrier::new(2));
        let worker_barrier = Arc::clone(&barrier);
        let worker_current = Arc::clone(&current);
        let worker_sizes = Arc::clone(&sizes);
        let worker_revision = Arc::clone(&revision);
        let worker_scan_epoch = Arc::clone(&scan_epoch);
        let worker_path_epoch = Arc::clone(&path_epoch);
        let worker_key = cache_key.clone();
        let worker_path = directory.clone();
        let worker = std::thread::spawn(move || {
            let measurement = DirMeasurement {
                path: worker_path,
                modified: Some(SystemTime::now()),
                cache_key: worker_key,
                path_epoch: worker_path_epoch,
                size: 42,
                completed_at: std::time::Instant::now(),
                elapsed: std::time::Duration::from_millis(1),
            };
            worker_barrier.wait();
            worker_barrier.wait();
            publish_dir_measurements_if_current(
                &worker_current,
                &worker_scan_epoch,
                &worker_sizes,
                &[measurement],
                &worker_revision,
            )
        });

        barrier.wait();
        invalidate_size_cache(&changed);
        assert!(path_epoch.is_cancelled());
        assert!(scan_is_current(&current, &scan_epoch));
        barrier.wait();

        assert_eq!(worker.join().unwrap(), PublishOutcome::default());
        assert!(!lock_recover(&sizes).contains_key(&directory));
        let _boundary = lock_recover(scan_commit_boundary());
        assert!(!lock_recover(dir_size_cache()).contains_key(&cache_key));
        assert!(!lock_recover(walk_log()).contains_key(&cache_key));
    }

    #[test]
    fn recursive_size_walk_stops_between_entries_when_cancelled() {
        let tmp = TempDir::new();
        tmp.file("a.bin", "123");
        tmp.file("b.bin", "456");
        tmp.file("c.bin", "789");
        let checks = std::sync::atomic::AtomicUsize::new(0);

        let result = dir_size_recursive_until(tmp.path(), || {
            checks.fetch_add(1, AtomicOrdering::SeqCst) >= 2
        });

        assert_eq!(result, None);
        assert_eq!(checks.load(AtomicOrdering::SeqCst), 3);
    }

    #[test]
    fn shuffled_duplicate_batch_deduplicates_before_top_k() {
        let key = |name: &str| VolumePathKey {
            path: PathBuf::from(format!("/batch/{name}")),
            volume_id: 1,
            generation: 1,
        };
        let a = key("a");
        let b = key("b");
        let started = std::time::Instant::now();
        let completed = |seconds| started + std::time::Duration::from_secs(seconds);
        let modified = |seconds| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
        let entries = [
            (
                a.clone(),
                RankedDirSize {
                    modified: modified(1),
                    completed_at: Some(completed(100)),
                    size: 100,
                },
            ),
            (
                a.clone(),
                RankedDirSize {
                    modified: modified(200),
                    completed_at: Some(completed(99)),
                    size: 99,
                },
            ),
            (
                b.clone(),
                RankedDirSize {
                    modified: modified(98),
                    completed_at: Some(completed(98)),
                    size: 98,
                },
            ),
        ];
        let permutations = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];

        for order in permutations {
            let mut ordered = order.into_iter().map(|index| entries[index].clone());
            let mut cache = HashMap::new();
            let (first_key, first_value) = ordered.next().unwrap();
            cache.insert(first_key, first_value);

            extend_bounded_by(&mut cache, ordered, 2, 2, compare_ranked_dir_sizes);

            assert_eq!(cache.len(), 2, "order {order:?}");
            assert!(cache.capacity() <= hash_map_capacity_upper_bound(2));
            assert_eq!(
                cache[&a].completed_at,
                Some(completed(100)),
                "order {order:?}"
            );
            assert_eq!(cache[&a].size, 100, "order {order:?}");
            assert_eq!(
                cache[&b].completed_at,
                Some(completed(98)),
                "order {order:?}"
            );
        }
    }

    #[test]
    fn advisory_lock_state_is_recovered_after_poisoning() {
        let state = Mutex::new(1usize);
        let panic = std::panic::catch_unwind(|| {
            let mut state = state.lock().unwrap();
            *state = 2;
            panic!("poison advisory state");
        });

        assert!(panic.is_err());
        assert!(state.is_poisoned());
        assert_eq!(*lock_recover(&state), 2);
    }

    #[test]
    fn dir_size_cache_pruning_is_bounded_and_deterministic() {
        let key = |name: &str| VolumePathKey {
            path: PathBuf::from(format!("/cache/{name}")),
            volume_id: 1,
            generation: 1,
        };
        let oldest = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let mut cache = HashMap::from([
            (key("a"), (oldest, 1)),
            (key("b"), (oldest, 2)),
            (key("c"), (oldest + std::time::Duration::from_secs(1), 3)),
            (key("d"), (oldest + std::time::Duration::from_secs(2), 4)),
            (key("e"), (oldest + std::time::Duration::from_secs(3), 5)),
        ]);

        prune_dir_size_cache_to(&mut cache, 4, 3);

        assert_eq!(cache.len(), 3);
        assert!(cache.capacity() <= hash_map_capacity_upper_bound(4));
        assert_eq!(
            cache
                .keys()
                .map(|key| key.path.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from([key("c").path, key("d").path, key("e").path])
        );
    }

    #[test]
    fn walk_log_pruning_is_bounded_and_deterministic() {
        let key = |name: &str| VolumePathKey {
            path: PathBuf::from(format!("/walk/{name}")),
            volume_id: 1,
            generation: 1,
        };
        let oldest = std::time::Instant::now();
        let cost = std::time::Duration::from_millis(1);
        let mut log = HashMap::from([
            (key("a"), (oldest, cost)),
            (key("b"), (oldest, cost)),
            (key("c"), (oldest + std::time::Duration::from_secs(1), cost)),
            (key("d"), (oldest + std::time::Duration::from_secs(2), cost)),
            (key("e"), (oldest + std::time::Duration::from_secs(3), cost)),
        ]);

        prune_walk_log_to(&mut log, 4, 3);

        assert_eq!(log.len(), 3);
        assert!(log.capacity() <= hash_map_capacity_upper_bound(4));
        assert_eq!(
            log.keys()
                .map(|key| key.path.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from([key("c").path, key("d").path, key("e").path])
        );
    }

    #[test]
    fn oversized_batch_is_selected_before_extend_and_shrinks_capacity() {
        let key = |prefix: &str, index: usize| VolumePathKey {
            path: PathBuf::from(format!("/{prefix}/{index:03}")),
            volume_id: 1,
            generation: 1,
        };
        let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let mut cache = HashMap::with_capacity(1_024);
        cache.extend((0..4).map(|index| (key("old", index), (base, index as u64))));
        let incoming: Vec<_> = (0..100)
            .map(|index| {
                (
                    key("incoming", index),
                    (
                        base + std::time::Duration::from_secs(index as u64 + 1),
                        index as u64,
                    ),
                )
            })
            .collect();

        extend_bounded_by(&mut cache, incoming, 8, 6, compare_cache_entries);

        assert_eq!(cache.len(), 6);
        assert!(cache.capacity() <= hash_map_capacity_upper_bound(8));
        assert_eq!(
            cache
                .keys()
                .map(|key| key.path.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            (94..100).map(|index| key("incoming", index).path).collect()
        );
    }

    #[test]
    fn oversized_persisted_load_is_streamed_and_capacity_bounded() {
        let persisted = PersistedCache {
            schema: 1,
            entries: (0..50)
                .map(|index| PersistedCacheEntry {
                    key: VolumePathKey {
                        path: PathBuf::from(format!("/load/{index:03}")),
                        volume_id: 1,
                        generation: 1,
                    },
                    mtime_secs: index as u64 + 1,
                    mtime_nanos: 0,
                    size: index as u64,
                })
                .collect(),
        };
        let json = serde_json::to_vec(&persisted).unwrap();

        let cache = load_cache_from_reader_with_bounds(json.as_slice(), 8, 6).unwrap();

        assert_eq!(cache.len(), 6);
        assert!(cache.capacity() <= hash_map_capacity_upper_bound(8));
        assert_eq!(
            cache
                .keys()
                .map(|key| key.path.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            (44..50)
                .map(|index| PathBuf::from(format!("/load/{index:03}")))
                .collect()
        );
    }

    #[test]
    fn shuffled_persisted_duplicates_are_deduplicated_before_top_k() {
        let key = |name: &str| VolumePathKey {
            path: PathBuf::from(format!("/load/{name}")),
            volume_id: 1,
            generation: 1,
        };
        let a = key("a");
        let b = key("b");
        let entries = [
            PersistedCacheEntry {
                key: a.clone(),
                mtime_secs: 100,
                mtime_nanos: 0,
                size: 100,
            },
            PersistedCacheEntry {
                key: a.clone(),
                mtime_secs: 99,
                mtime_nanos: 0,
                size: 99,
            },
            PersistedCacheEntry {
                key: b.clone(),
                mtime_secs: 98,
                mtime_nanos: 0,
                size: 98,
            },
        ];
        let permutations = [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ];

        for order in permutations {
            let persisted = PersistedCache {
                schema: 1,
                entries: order
                    .into_iter()
                    .map(|index| PersistedCacheEntry {
                        key: entries[index].key.clone(),
                        mtime_secs: entries[index].mtime_secs,
                        mtime_nanos: entries[index].mtime_nanos,
                        size: entries[index].size,
                    })
                    .collect(),
            };
            let json = serde_json::to_vec(&persisted).unwrap();

            let cache = load_cache_from_reader_with_bounds(json.as_slice(), 2, 2).unwrap();

            assert_eq!(cache.len(), 2, "order {order:?}");
            assert!(cache.capacity() <= hash_map_capacity_upper_bound(2));
            assert_eq!(
                cache[&a],
                (
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(100),
                    100,
                ),
                "order {order:?}"
            );
            assert_eq!(
                cache[&b],
                (
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(98),
                    98,
                ),
                "order {order:?}"
            );
        }
    }

    #[test]
    fn batch_between_retain_and_limit_is_not_pruned() {
        let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let incoming = (0..9_500).map(|index| {
            (
                VolumePathKey {
                    path: PathBuf::from(format!("/batch-threshold/{index:05}")),
                    volume_id: 1,
                    generation: 1,
                },
                (
                    base + std::time::Duration::from_secs(index as u64),
                    index as u64,
                ),
            )
        });
        let mut cache = HashMap::new();

        extend_bounded_by(
            &mut cache,
            incoming,
            DIR_SIZE_CACHE_LIMIT,
            DIR_SIZE_CACHE_RETAIN,
            compare_cache_entries,
        );

        assert_eq!(cache.len(), 9_500);
        assert!(cache.capacity() <= hash_map_capacity_upper_bound(DIR_SIZE_CACHE_LIMIT));
    }

    #[test]
    fn persisted_load_between_retain_and_limit_is_not_pruned() {
        let persisted = PersistedCache {
            schema: 1,
            entries: (0..9_500)
                .map(|index| PersistedCacheEntry {
                    key: VolumePathKey {
                        path: PathBuf::from(format!("/load-threshold/{index:05}")),
                        volume_id: 1,
                        generation: 1,
                    },
                    mtime_secs: index as u64 + 1,
                    mtime_nanos: 0,
                    size: index as u64,
                })
                .collect(),
        };
        let json = serde_json::to_vec(&persisted).unwrap();

        let cache = load_cache_from_reader_with_bounds(
            json.as_slice(),
            DIR_SIZE_CACHE_LIMIT,
            DIR_SIZE_CACHE_RETAIN,
        )
        .unwrap();

        assert_eq!(cache.len(), 9_500);
        assert!(cache.capacity() <= hash_map_capacity_upper_bound(DIR_SIZE_CACHE_LIMIT));
    }

    #[test]
    fn deep_change_recomputes_dir_size_without_reloading_listing() {
        fn wait_for_size(p: &PanelState, dir: &Path, expected: u64) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if p.size_snapshot().size_of(dir) == Some(expected) {
                    return;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "size {expected} for {dir:?} not observed"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        let tmp = TempDir::new();
        tmp.dir("sub/deep");
        tmp.file("sub/deep/a.bin", "12345"); // 5 bytes
        let sub = tmp.path().join("sub");

        let mut p = PanelState::new(tmp.path().to_path_buf());
        p.refresh();
        wait_for_size(&p, &sub, 5);

        // A deep change: sub's own mtime does not move, so the mtime
        // cache alone would keep serving the stale 5 bytes.
        let new_file = tmp.file("sub/deep/b.bin", "1234567"); // +7 bytes

        // What the recursive watcher does on such an event:
        invalidate_size_cache(&new_file);
        p.sizes.mark_dirty_immediately_for_test();
        reset_walk_log(); // bypass the walk cooldown in the test

        let reloaded = p.poll_fs_changes();
        assert!(!reloaded, "sizes-only events must not reload the listing");
        wait_for_size(&p, &sub, 12);
    }

    #[test]
    fn mutation_during_subscription_snapshot_handoff_forces_reconciliation() {
        let tmp = TempDir::new();
        tmp.file("before.txt", "before");
        let mut panel = PanelState::new(tmp.path().to_path_buf());
        panel.refresh();

        panel.watcher.activate_test_binding(tmp.path());
        let subscription_ticket = panel.watcher.snapshot_ticket();

        // Deterministic handoff barrier: the subscription is active, but its
        // first complete listing has not yet been acknowledged.
        let created = tmp.file("during.txt", "during");
        panel
            .watcher
            .inject_current_change_for_test(created.clone());
        assert!(panel.reload_entries());
        let listing_binding = panel.listing.binding().to_path_buf();
        panel
            .watcher
            .acknowledge_snapshot(subscription_ticket.clone(), &listing_binding);

        assert!(panel.poll_fs_changes());
        assert!(panel.entries().iter().any(|entry| entry.path == created));
        assert!(!panel.watcher.has_pending_reconciliation_for_test());
    }

    #[test]
    fn watcher_gap_replaces_the_listing_and_applies_its_generation() {
        let health_before = crate::watcher_health::snapshot();
        let tmp = TempDir::new();
        tmp.file("before.txt", "before");
        let mut panel = PanelState::new(tmp.path().to_path_buf());
        panel.refresh();
        assert!(
            panel
                .entries()
                .iter()
                .any(|entry| entry.name == "before.txt")
        );

        std::fs::remove_file(tmp.path().join("before.txt")).unwrap();
        tmp.file("after.txt", "after");
        panel.watcher.activate_test_binding(tmp.path());
        panel.watcher.inject_gap_for_test();

        assert!(panel.poll_fs_changes());
        assert!(
            panel
                .entries()
                .iter()
                .any(|entry| entry.name == "after.txt")
        );
        assert!(
            !panel
                .entries()
                .iter()
                .any(|entry| entry.name == "before.txt")
        );
        assert!(!panel.watcher.has_pending_reconciliation_for_test());
        let health_after = crate::watcher_health::snapshot();
        assert!(health_after.listing_reconciliations > health_before.listing_reconciliations);
        assert!(health_after.gap_reconciliations > health_before.gap_reconciliations);
    }

    /// Profiling harness, not a test: drives a real watcher + poll loop
    /// at ~60 "fps" against COMMANDER_PROFILE_ROOT for
    /// COMMANDER_PROFILE_SECS seconds while an external driver generates
    /// fs activity and samples this process's CPU.
    /// Run: cargo test --release watcher_profile_harness -- --ignored --nocapture
    #[test]
    #[ignore = "profiling harness, run manually"]
    fn watcher_profile_harness() {
        let root = std::env::var("COMMANDER_PROFILE_ROOT").expect("set COMMANDER_PROFILE_ROOT");
        let secs: u64 = std::env::var("COMMANDER_PROFILE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);

        let mut p = PanelState::new(PathBuf::from(root));
        p.refresh();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        let mut reloads = 0u32;
        while std::time::Instant::now() < deadline {
            if p.poll_fs_changes() {
                reloads += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
        println!("harness done: {} listing reloads", reloads);
    }

    #[test]
    fn make_preview_marks_text_pending_without_reading_and_skips_dirs() {
        let tmp = TempDir::new();
        let file = tmp.file("note.txt", "hello");
        let meta = std::fs::metadata(&file).unwrap();
        let fe = FileEntry::from_meta(file.clone(), &meta).unwrap();
        std::fs::remove_file(file).unwrap();
        match make_preview(&fe) {
            Some(PreviewContent::Pending(identity)) => assert_eq!(identity.path, fe.path),
            other => panic!("expected pending preview, got {other:?}"),
        }

        let dir = tmp.dir("d");
        let meta = std::fs::metadata(&dir).unwrap();
        let de = FileEntry::from_meta(dir, &meta).unwrap();
        assert!(make_preview(&de).is_none());
    }

    #[test]
    fn visible_window_clamps_anchor_and_has_a_cold_start_default() {
        assert_eq!(visible_window(100, 40, 10), 40..72);
        assert_eq!(visible_window(8, 99, 20), 0..8);
        assert_eq!(visible_window(20, 0, 0), 0..20);
    }
}
