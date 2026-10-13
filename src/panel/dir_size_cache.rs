//! Process-global directory-size cache: the in-memory map keyed by volume
//! and path, its bounded on-disk snapshot, the walk cooldown log, and the
//! per-path scan epochs that keep a stale walk from publishing.
//!
//! `PanelState` reads and publishes through the `pub(super)` API below.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(super) struct VolumePathKey {
    pub(super) path: PathBuf,
    pub(super) volume_id: u64,
    pub(super) generation: u64,
}

impl VolumePathKey {
    pub(super) fn observe(path: &Path) -> Self {
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
pub(super) struct PersistedCacheEntry {
    pub(super) key: VolumePathKey,
    pub(super) mtime_secs: u64,
    pub(super) mtime_nanos: u32,
    pub(super) size: u64,
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct PersistedCache {
    pub(super) schema: u32,
    pub(super) entries: Vec<PersistedCacheEntry>,
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
pub(super) fn dir_size_cache() -> &'static Mutex<HashMap<VolumePathKey, (SystemTime, u64)>> {
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
    pub(super) candidates: BoundedUniqueMap<VolumePathKey, (SystemTime, u64)>,
}

impl BoundedCacheAccumulator {
    pub(super) fn new(limit: usize, retain: usize) -> Self {
        Self {
            candidates: BoundedUniqueMap::new(limit, retain),
        }
    }

    pub(super) fn push(&mut self, entry: PersistedCacheEntry) {
        let Some(mtime) = std::time::UNIX_EPOCH.checked_add(std::time::Duration::new(
            entry.mtime_secs,
            entry.mtime_nanos,
        )) else {
            return;
        };
        self.candidates
            .insert_by(entry.key, (mtime, entry.size), &mut compare_cache_entries);
    }

    pub(super) fn finish(self) -> HashMap<VolumePathKey, (SystemTime, u64)> {
        self.candidates.finish_by(&mut compare_cache_entries)
    }
}

pub(super) fn compare_cache_entries(
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
    pub(super) limit: usize,
    pub(super) retain: usize,
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

pub(super) fn load_cache_from_reader_with_bounds(
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
pub(super) fn walk_log()
-> &'static Mutex<HashMap<VolumePathKey, (std::time::Instant, std::time::Duration)>> {
    static LOG: OnceLock<Mutex<HashMap<VolumePathKey, (std::time::Instant, std::time::Duration)>>> =
        OnceLock::new();
    LOG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_flush_lock() -> &'static Mutex<()> {
    static FLUSH: Mutex<()> = Mutex::new(());
    &FLUSH
}

pub(super) const WALK_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10);
pub(super) const WALK_EXPENSIVE: std::time::Duration = std::time::Duration::from_secs(2);
const WALK_LOG_LIMIT: usize = 4_096;
const WALK_LOG_RETAIN: usize = 3_584;
pub(super) const DIR_SIZE_CACHE_LIMIT: usize = 10_000;
pub(super) const DIR_SIZE_CACHE_RETAIN: usize = 9_000;

fn compare_volume_path_keys(a: &VolumePathKey, b: &VolumePathKey) -> Ordering {
    a.path
        .cmp(&b.path)
        .then(a.volume_id.cmp(&b.volume_id))
        .then(a.generation.cmp(&b.generation))
}

/// These mutexes protect advisory caches and cancellable scan state. Recovering
/// their values after a panic is safe: every commit revalidates both epoch
/// identities, and a subsequent scan replaces panel-local snapshots.
pub(super) fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Serializes scan replacement, watcher invalidation and result publication.
/// The boundary closes the check-before-lock race across all participating
/// maps; filesystem traversal itself never holds it.
pub(super) fn scan_commit_boundary() -> &'static Mutex<()> {
    static BOUNDARY: Mutex<()> = Mutex::new(());
    &BOUNDARY
}

#[derive(Debug)]
pub(super) struct ScanEpoch {
    pub(super) cancelled: AtomicBool,
}

impl ScanEpoch {
    pub(super) fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
        }
    }

    pub(super) fn cancel(&self) {
        self.cancelled.store(true, AtomicOrdering::Release);
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.cancelled.load(AtomicOrdering::Acquire)
    }
}

/// Weak identities keep per-path invalidation state only while a walk is
/// active. Identity comparison avoids counter wraparound/ABA entirely.
fn active_path_epochs() -> &'static Mutex<HashMap<VolumePathKey, Weak<ScanEpoch>>> {
    static EPOCHS: OnceLock<Mutex<HashMap<VolumePathKey, Weak<ScanEpoch>>>> = OnceLock::new();
    EPOCHS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn capture_path_epoch(key: &VolumePathKey) -> Arc<ScanEpoch> {
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

pub(super) fn scan_is_current(current: &Mutex<Arc<ScanEpoch>>, candidate: &Arc<ScanEpoch>) -> bool {
    !candidate.is_cancelled() && Arc::ptr_eq(&lock_recover(current), candidate)
}

pub(super) fn begin_panel_scan(
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

pub(super) fn hash_map_capacity_upper_bound(limit: usize) -> usize {
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
    pub(super) entries: HashMap<K, V>,
    pub(super) limit: usize,
    pub(super) retain: usize,
    overflowed: bool,
}

impl<K, V> BoundedUniqueMap<K, V>
where
    K: Eq + Hash,
{
    pub(super) fn new(limit: usize, retain: usize) -> Self {
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

pub(super) fn extend_bounded_by<K, V>(
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
pub(super) fn prune_walk_log_to(
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

pub(super) fn prune_dir_size_cache_to(
    cache: &mut HashMap<VolumePathKey, (SystemTime, u64)>,
    limit: usize,
    retain: usize,
) {
    if cache.len() > limit {
        retain_top_map_by(cache, retain.min(limit), &mut compare_cache_entries);
    }
    shrink_bounded_map(cache, limit);
}

pub(super) fn publish_scan_values_if_current<V>(
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

pub(super) fn publish_cached_size_if_current(
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

pub(super) fn dir_size_recursive_until(
    path: &Path,
    is_cancelled: impl Fn() -> bool,
) -> Option<u64> {
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

pub(super) struct DirMeasurement {
    pub(super) path: PathBuf,
    pub(super) modified: Option<SystemTime>,
    pub(super) cache_key: VolumePathKey,
    pub(super) path_epoch: Arc<ScanEpoch>,
    pub(super) size: u64,
    pub(super) completed_at: std::time::Instant,
    pub(super) elapsed: std::time::Duration,
}

#[derive(Clone, Copy)]
pub(super) struct RankedDirSize {
    pub(super) modified: SystemTime,
    pub(super) completed_at: Option<std::time::Instant>,
    pub(super) size: u64,
}

pub(super) fn compare_ranked_dir_sizes(
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
pub(super) struct PublishOutcome {
    pub(super) published: usize,
    pub(super) cache_changed: bool,
}

pub(super) fn publish_dir_measurements_if_current(
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

#[cfg(test)]
pub(crate) fn reset_walk_log() {
    let _boundary = lock_recover(scan_commit_boundary());
    lock_recover(walk_log()).clear();
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
