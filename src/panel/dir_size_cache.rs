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

    /// Key for a folder listed directly in a panel folder whose volume is
    /// `parent`, without re-observing it. `observe` costs two stats per call,
    /// once per subfolder on the UI thread. A child on the parent's volume
    /// shares its volume id and generation; one that may be a mount point
    /// (another device, or no captured identity) is observed as before.
    pub(super) fn for_listed_child(
        path: &Path,
        identity: &ListingIdentity,
        parent: &crate::volume_profile::VolumeProfile,
    ) -> Self {
        #[cfg(unix)]
        let same_volume = matches!(
            identity,
            ListingIdentity::Captured(identity) if identity.volume == Some(parent.volume_id)
        );
        // Off Unix the volume id is derived from the path's prefix, which a
        // child always shares with its parent.
        #[cfg(not(unix))]
        let same_volume = {
            let _ = identity;
            true
        };
        if same_volume {
            Self {
                path: path.to_path_buf(),
                volume_id: parent.volume_id,
                generation: parent.generation,
            }
        } else {
            Self::observe(path)
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

/// Where the size cache persists, or `None` to keep it in memory only. With
/// no per-user cache dir (HOME/XDG unset) there is no private place for it: a
/// shared `/tmp/commander` could be pre-created by another user and feed this
/// one wrong sizes.
fn cache_path() -> Option<PathBuf> {
    #[cfg(test)]
    let dir =
        Some(std::env::temp_dir().join(format!("commander-test-cache-{}", std::process::id())));
    #[cfg(not(test))]
    let dir = crate::fs_util::storage_root_override().map_or_else(
        || dirs::cache_dir().map(|dir| dir.join("commander")),
        |root| Some(root.join("cache")),
    );
    let dir = dir?;
    if let Err(error) = fs::create_dir_all(&dir)
        && first_cache_warning(&CACHE_DIR_WARNED)
    {
        log::warn!(
            target: "commander::sizes",
            "cannot create the folder-size cache directory {}: {error}; sizes are re-measured after restart",
            dir.display()
        );
    }
    Some(dir.join("dir_sizes.json"))
}

static CACHE_DIR_WARNED: AtomicBool = AtomicBool::new(false);
static CACHE_SAVE_WARNED: AtomicBool = AtomicBool::new(false);

/// True the first time a cache-persistence failure of one kind is seen, so
/// a full disk logs once instead of on every size refresh.
pub(super) fn first_cache_warning(warned: &AtomicBool) -> bool {
    !warned.swap(true, AtomicOrdering::Relaxed)
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

/// Lower-bound sizes of folders whose last walk skipped unreadable parts:
/// volume generation + path -> (mtime, size). Kept in memory only (never
/// persisted as an exact size), so an unrelated recompute reuses the last
/// partial measurement instead of re-walking the folder; watcher events in
/// the folder drop it like the exact cache.
pub(super) fn partial_size_cache() -> &'static Mutex<HashMap<VolumePathKey, (SystemTime, u64)>> {
    static PARTIAL: OnceLock<Mutex<HashMap<VolumePathKey, (SystemTime, u64)>>> = OnceLock::new();
    PARTIAL.get_or_init(|| Mutex::new(HashMap::new()))
}

const PARTIAL_SIZE_LIMIT: usize = 4_096;

fn load_cache_from_disk() -> HashMap<VolumePathKey, (SystemTime, u64)> {
    let Some(path) = cache_path() else {
        return HashMap::new();
    };
    let Ok(file) = fs::File::open(&path) else {
        return HashMap::new();
    };
    load_cache_from_reader_with_bounds(
        std::io::BufReader::new(file),
        DIR_SIZE_CACHE_LIMIT,
        DIR_SIZE_CACHE_RETAIN,
    )
    .unwrap_or_else(|error| {
        // Self-healing: the next flush rewrites the file from scratch.
        log::warn!(
            target: "commander::sizes",
            "ignoring unreadable folder-size cache {}: {error}",
            path.display()
        );
        HashMap::new()
    })
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
        // The writer never stores whole seconds in the nanos field, and
        // `Duration::new` panics when carrying them overflows the seconds:
        // a corrupt entry is dropped instead.
        if entry.mtime_nanos >= 1_000_000_000 {
            return;
        }
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

/// Walk-log order: oldest walk first, ties broken by key then cost.
fn compare_walk_log_entries(
    key_a: &VolumePathKey,
    (when_a, duration_a): &(std::time::Instant, std::time::Duration),
    key_b: &VolumePathKey,
    (when_b, duration_b): &(std::time::Instant, std::time::Duration),
) -> Ordering {
    when_a
        .cmp(when_b)
        .then_with(|| compare_volume_path_keys(key_a, key_b))
        .then(duration_a.cmp(duration_b))
}

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
    pub(super) const fn new() -> Self {
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
        retain_top_map_by(log, retain.min(limit), &mut compare_walk_log_entries);
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
    let cached = lock_recover(dir_size_cache()).get(key).copied();
    let cached = cached.or_else(|| lock_recover(partial_size_cache()).get(key).copied());
    let Some((cached_mtime, cached_size)) = cached else {
        return false;
    };
    if expected_mtime.is_some_and(|expected| cached_mtime != expected) {
        return false;
    }
    lock_recover(sizes).insert(path.to_path_buf(), cached_size);
    revision.fetch_add(1, AtomicOrdering::Release);
    true
}

/// The device a directory lives on, for staying on one filesystem.
#[cfg(unix)]
fn device_of(metadata: &fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.dev())
}

#[cfg(not(unix))]
const fn device_of(_metadata: &fs::Metadata) -> Option<u64> {
    None
}

/// Identity of a file with more than one hard link, so the walk counts its
/// bytes once however many links point at it.
#[cfg(unix)]
fn shared_inode(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    (metadata.nlink() > 1).then(|| (metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
const fn shared_inode(_metadata: &fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// Whether a folder that is itself a mount point holds real files worth
/// walking. Pseudo filesystems (`/proc`, `/sys`, `/dev` on macOS) report
/// absurd sizes, and network mounts are slow to walk, so those count 0;
/// local disks, partitions and subvolumes (a separate `/home`, an APFS
/// Data-volume firmlink, a drive under `/media`, including FUSE-backed ones
/// such as ntfs-3g) are measured.
#[cfg(target_os = "linux")]
fn mount_is_measurable(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY: `c_path` is NUL-terminated and `stats` is a writable statfs.
    if unsafe { libc::statfs(c_path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return true;
    }
    // SAFETY: statfs returned 0, so the struct is initialized.
    let magic = unsafe { stats.assume_init() }.f_type as u64 as u32;
    !matches!(
        magic,
        // Pseudo filesystems (`linux/magic.h`).
        0x9FA0 // proc
            | 0x6265_6572 // sysfs
            | 0x1CD1 // devpts
            | 0x0027_E0EB // cgroup
            | 0x6367_7270 // cgroup2
            | 0x6462_6720 // debugfs
            | 0x7472_6163 // tracefs
            | 0x7363_6673 // securityfs
            | 0xCAFE_4A11 // bpf
            | 0x6165_676C // pstore
            | 0x6265_6570 // configfs
            | 0xDE5E_81E4 // efivarfs
            | 0x6573_5543 // fusectl
            | 0x4249_4E4D // binfmt_misc
            | 0x0187 // autofs
            | 0x1980_0202 // mqueue
            | 0x9584_58F6 // hugetlbfs
            | 0xF97C_FF8C // selinuxfs
            | 0x6E73_6673 // nsfs
            // Network filesystems. FUSE is measured: it backs local disks
            // as often as remote ones (ntfs-3g, exfat-fuse on external drives).
            | 0x6969 // nfs
            | 0x517B // smb
            | 0xFF53_4D42 // cifs
            | 0xFE53_4D42 // smb2
            | 0x0102_1997 // 9p
            | 0x00C3_6400 // ceph
    )
}

#[cfg(target_os = "macos")]
fn mount_is_measurable(path: &Path) -> bool {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;

    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return true;
    };
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY: `c_path` is NUL-terminated and `stats` is a writable statfs.
    if unsafe { libc::statfs(c_path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return true;
    }
    // SAFETY: statfs returned 0, so the struct is initialized.
    let stats = unsafe { stats.assume_init() };
    if stats.f_flags & libc::MNT_LOCAL as u32 == 0 {
        return false;
    }
    // SAFETY: f_fstypename is a NUL-terminated C string filled by statfs.
    let name = unsafe { CStr::from_ptr(stats.f_fstypename.as_ptr()) }.to_string_lossy();
    !matches!(name.as_ref(), "devfs" | "autofs" | "fdesc" | "nullfs")
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
const fn mount_is_measurable(_path: &Path) -> bool {
    true
}

/// Recursive byte total of `path`, like `du -x`: the walk stays on the
/// folder's own filesystem and does not descend into other mounts below it,
/// and a hard-linked file counts once. A folder that is itself a mount point
/// is measured in full when it is a local disk; a pseudo or network mount
/// (e.g. `/proc` listed in `/`) is not walked and counts 0.
///
/// A `background` walk (sizes the user is not looking at) yields briefly
/// before each folder while foreground listing/preview I/O is active.
#[cfg(test)]
pub(super) fn dir_size_recursive_until(
    path: &Path,
    background: bool,
    is_cancelled: impl Fn() -> bool,
) -> Option<u64> {
    dir_size_walk_until(path, background, is_cancelled).map(|walk| walk.size)
}

/// Result of [`dir_size_walk_until`]: the bytes counted, and whether every
/// folder and entry below `path` could actually be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DirWalk {
    pub(super) size: u64,
    /// False when an unreadable subfolder or entry was skipped, so `size` is
    /// only a lower bound and must not be cached as the folder's size.
    pub(super) complete: bool,
}

/// A walk error that leaves a gap in the total. Something that vanished
/// mid-walk is gone, not uncounted.
pub(super) fn walk_error_leaves_gap(error: &std::io::Error) -> bool {
    error.kind() != std::io::ErrorKind::NotFound
}

/// [`dir_size_recursive_until`] that also reports skipped, unreadable parts.
pub(super) fn dir_size_walk_until(
    path: &Path,
    background: bool,
    is_cancelled: impl Fn() -> bool,
) -> Option<DirWalk> {
    let mut pending = vec![path.to_path_buf()];
    let mut total = 0u64;
    let mut complete = true;
    let device = fs::metadata(path).ok().as_ref().and_then(device_of);
    let parent_device = path
        .parent()
        .and_then(|parent| fs::metadata(parent).ok())
        .as_ref()
        .and_then(device_of);
    if device.is_some()
        && parent_device.is_some()
        && device != parent_device
        && !mount_is_measurable(path)
    {
        pending.clear();
    }
    let mut seen_inodes = HashSet::new();

    while let Some(directory) = pending.pop() {
        if is_cancelled() {
            return None;
        }
        if background && !crate::io_budget::background_checkpoint(&is_cancelled) {
            return None;
        }
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) => {
                complete &= !walk_error_leaves_gap(&error);
                continue;
            }
        };
        for entry in entries {
            if is_cancelled() {
                return None;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    complete &= !walk_error_leaves_gap(&error);
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    complete &= !walk_error_leaves_gap(&error);
                    continue;
                }
            };
            if file_type.is_dir() {
                let other_device = device.is_some()
                    && entry.metadata().ok().as_ref().and_then(device_of) != device;
                if !other_device {
                    pending.push(entry.path());
                }
                continue;
            }
            // `DirEntry::metadata` does not follow symlinks (like
            // `symlink_metadata`) and stats relative to the open directory
            // instead of building and re-resolving the full path per file.
            match entry.metadata() {
                Ok(metadata) => {
                    if shared_inode(&metadata).is_none_or(|inode| seen_inodes.insert(inode)) {
                        total = total.saturating_add(metadata.len());
                    }
                }
                Err(error) => complete &= !walk_error_leaves_gap(&error),
            }
        }
    }

    (!is_cancelled()).then_some(DirWalk {
        size: total,
        complete,
    })
}

pub(super) struct DirMeasurement {
    pub(super) path: PathBuf,
    pub(super) modified: Option<SystemTime>,
    /// Whether the walk read every subfolder; a partial size is only a
    /// lower bound and is never stored in the persistent cache.
    pub(super) complete: bool,
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
            compare_walk_log_entries,
        );
    }

    {
        let mut partial = lock_recover(partial_size_cache());
        for result in &valid {
            match result.modified {
                Some(modified) if !result.complete => {
                    if partial.len() >= PARTIAL_SIZE_LIMIT {
                        partial.clear();
                    }
                    partial.insert(result.cache_key.clone(), (modified, result.size));
                }
                _ => {
                    partial.remove(&result.cache_key);
                }
            }
        }
    }

    let cacheable: Vec<&DirMeasurement> = valid
        .iter()
        .copied()
        .filter(|result| result.complete && result.modified.is_some())
        .collect();
    let cache_changed = !cacheable.is_empty();
    if cache_changed {
        let mut cache = lock_recover(dir_size_cache());
        *cache = merge_measurements_into_cache(
            std::mem::take(&mut *cache),
            &cacheable,
            DIR_SIZE_CACHE_LIMIT,
            DIR_SIZE_CACHE_RETAIN,
        );
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

/// The exact-size cache after storing `measured`: a measured folder's
/// entries under any other key (an old mount generation) are dropped, then
/// the newest `retain` entries are kept once the cache passes `limit`.
/// Measurements without a known mtime are skipped; they cannot be checked
/// for staleness on reuse.
pub(super) fn merge_measurements_into_cache(
    previous: HashMap<VolumePathKey, (SystemTime, u64)>,
    measured: &[&DirMeasurement],
    limit: usize,
    retain: usize,
) -> HashMap<VolumePathKey, (SystemTime, u64)> {
    let measured_paths: HashSet<&Path> = measured
        .iter()
        .map(|result| result.path.as_path())
        .collect();
    let measured_keys: HashSet<&VolumePathKey> =
        measured.iter().map(|result| &result.cache_key).collect();
    let previous = previous.into_iter().filter_map(|(key, (modified, size))| {
        (!measured_paths.contains(key.path.as_path()) || measured_keys.contains(&key)).then_some((
            key,
            RankedDirSize {
                modified,
                completed_at: None,
                size,
            },
        ))
    });
    let incoming = measured.iter().filter_map(|result| {
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
        limit,
        retain,
        compare_ranked_dir_sizes,
    );
    let mut cache: HashMap<_, _> = ranked
        .into_iter()
        .map(|(key, value)| (key, (value.modified, value.size)))
        .collect();
    shrink_bounded_map(&mut cache, limit);
    cache
}

#[cfg(test)]
pub fn reset_walk_log() {
    let _boundary = lock_recover(scan_commit_boundary());
    lock_recover(walk_log()).clear();
}

/// Invalidate cached sizes for every directory that contains `path`.
/// A change at `path` (watcher event) makes all its ancestors' sizes stale,
/// even though their mtimes don't move (mtime only reflects direct children).
/// Active per-path epochs are cancelled before cache entries are removed, so
/// an already-running walk cannot recreate data invalidated by this event.
pub fn invalidate_size_cache(path: &Path) {
    invalidate_size_cache_many([path]);
}

/// [`invalidate_size_cache`] for a batch of changed paths (one watcher
/// poll): the caches are scanned once under one boundary acquisition, not
/// once per path, so an event flood costs O(cache + paths × depth).
pub(super) fn invalidate_size_cache_many<'a>(paths: impl IntoIterator<Item = &'a Path>) {
    // Every directory that contains (or is) a changed path. Ancestors are
    // added leaf-first, so meeting one already present means the rest of
    // the chain is too.
    let mut stale: HashSet<&Path> = HashSet::new();
    for path in paths {
        for ancestor in path.ancestors() {
            if !stale.insert(ancestor) {
                break;
            }
        }
    }
    if stale.is_empty() {
        return;
    }
    let is_stale = |key: &VolumePathKey| stale.contains(key.path.as_path());
    let _boundary = lock_recover(scan_commit_boundary());
    {
        let mut epochs = lock_recover(active_path_epochs());
        for (key, epoch) in epochs.iter() {
            if is_stale(key)
                && let Some(epoch) = epoch.upgrade()
            {
                epoch.cancel();
            }
        }
        epochs.retain(|key, epoch| !is_stale(key) && epoch.strong_count() > 0);
    }
    lock_recover(dir_size_cache()).retain(|key, _| !is_stale(key));
    lock_recover(partial_size_cache()).retain(|key, _| !is_stale(key));
    lock_recover(walk_log()).retain(|key, _| !is_stale(key));
}

/// Save current cache to disk (best-effort, called from background threads).
/// Snapshot under the mutex, then serialize and write after releasing it.
/// Stale paths are harmless because reuse always verifies mtime; probing them
/// here could block every panel behind the mutex when a remote volume is down.
pub fn flush_cache() {
    let Some(path) = cache_path() else {
        return;
    };
    // Serialize snapshot + commit so an older panel worker cannot publish
    // after a newer one. The persistence layer supplies a unique private temp
    // file and durable atomic replacement.
    let _flush = lock_recover(cache_flush_lock());
    let persisted = {
        let _boundary = lock_recover(scan_commit_boundary());
        let mut cache = lock_recover(dir_size_cache());
        prune_dir_size_cache_to(&mut cache, DIR_SIZE_CACHE_LIMIT, DIR_SIZE_CACHE_RETAIN);
        persisted_snapshot(&cache)
        // Lock dropped here: serialization and IO happen outside it.
    };
    if let Err(error) = crate::persistence::save_json_atomic(&path, &persisted)
        && first_cache_warning(&CACHE_SAVE_WARNED)
    {
        log::warn!(
            target: "commander::sizes",
            "cannot save the folder-size cache to {}: {error:?}; sizes are re-measured after restart",
            path.display()
        );
    }
}

/// The on-disk form of `cache`, entries in key order. An mtime before the
/// Unix epoch cannot be stored and its entry is left out.
pub(super) fn persisted_snapshot(
    cache: &HashMap<VolumePathKey, (SystemTime, u64)>,
) -> PersistedCache {
    let mut entries: Vec<PersistedCacheEntry> = cache
        .iter()
        .filter_map(|(key, (mtime, size))| {
            let since_epoch = mtime.duration_since(std::time::UNIX_EPOCH).ok()?;
            Some(PersistedCacheEntry {
                key: key.clone(),
                mtime_secs: since_epoch.as_secs(),
                mtime_nanos: since_epoch.subsec_nanos(),
                size: *size,
            })
        })
        .collect();
    entries.sort_by(|a, b| compare_volume_path_keys(&a.key, &b.key));
    PersistedCache { schema: 1, entries }
}
