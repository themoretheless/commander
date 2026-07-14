use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

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
    let dir = dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("commander");
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
    let Ok(data) = fs::read_to_string(&path) else {
        return HashMap::new();
    };
    let Ok(cache): Result<PersistedCache, _> = serde_json::from_str(&data) else {
        return HashMap::new();
    };
    if cache.schema != 1 {
        return HashMap::new();
    }
    cache
        .entries
        .into_iter()
        .map(|entry| {
            let mtime = std::time::UNIX_EPOCH
                + std::time::Duration::new(entry.mtime_secs, entry.mtime_nanos);
            (entry.key, (mtime, entry.size))
        })
        .collect()
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

const WALK_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10);
const WALK_EXPENSIVE: std::time::Duration = std::time::Duration::from_secs(2);
const DEFAULT_VISIBLE_ROWS: usize = 32;

fn visible_window(total: usize, anchor: usize, page_rows: usize) -> std::ops::Range<usize> {
    let rows = page_rows.max(DEFAULT_VISIBLE_ROWS).min(total);
    let start = anchor.min(total.saturating_sub(rows));
    let end = start.saturating_add(rows).min(total);
    start..end
}

#[cfg(test)]
pub(crate) fn reset_walk_log() {
    if let Ok(mut log) = walk_log().lock() {
        log.clear();
    }
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

/// Mark cached sizes stale for every directory that contains `path`.
/// A change at `path` (watcher event) makes all its ancestors' sizes stale,
/// even though their mtimes don't move (mtime only reflects direct children).
/// The size value is kept (still useful for display); the stored mtime is
/// reset to the epoch so the next mtime comparison can never match.
pub fn invalidate_size_cache(path: &Path) {
    if let Ok(mut cache) = dir_size_cache().lock() {
        for (key, entry) in cache.iter_mut() {
            if path.starts_with(&key.path) {
                entry.0 = std::time::UNIX_EPOCH;
            }
        }
    }
}

fn flag_watcher_gap(
    generation: &std::sync::atomic::AtomicU64,
    reload: &std::sync::atomic::AtomicBool,
) {
    generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    reload.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Save current cache to disk (best-effort, called from background threads).
/// Snapshot under the mutex, then serialize and write after releasing it.
/// Stale paths are harmless because reuse always verifies mtime; probing them
/// here could block every panel behind the mutex when a remote volume is down.
pub fn flush_cache() {
    let entries = {
        let Ok(cache) = dir_size_cache().lock() else {
            return;
        };
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
    let persisted = PersistedCache { schema: 1, entries };
    if let Ok(json) = serde_json::to_string(&persisted) {
        crate::fs_util::write_atomic(&cache_path(), &json);
    }
}

/// Why a directory listing is the way it is, so an empty list can be told
/// apart from an unreadable or vanished directory.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DirStatus {
    /// Read succeeded and there are entries.
    Listed,
    /// Read succeeded but the directory is empty.
    Empty,
    /// Permission denied.
    Denied,
    /// The directory no longer exists.
    Gone,
}

/// Classify a directory read for UI messaging. `is_empty` is whether the
/// listing came back with zero entries.
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

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub name_lower: String,
    pub path: PathBuf,
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

    pub fn icon(&self) -> &str {
        if self.is_dir {
            return "📁";
        }
        match self.extension.as_str() {
            "rs" => "🦀",
            "py" => "🐍",
            "js" | "ts" | "jsx" | "tsx" => "🟨",
            "html" | "css" | "scss" => "🌐",
            "json" | "toml" | "yaml" | "yml" | "xml" => "⚙️",
            "md" | "txt" | "rtf" | "doc" | "docx" => "📄",
            "pdf" => "📕",
            "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => "🖼️",
            "mp4" | "mov" | "avi" | "mkv" | "webm" => "🎬",
            "mp3" | "wav" | "flac" | "aac" | "ogg" => "🎵",
            "zip" | "tar" | "gz" | "7z" | "rar" | "bz2" | "xz" => "📦",
            "sh" | "bash" | "zsh" | "fish" => "💻",
            "exe" | "dmg" | "app" | "msi" => "⚡",
            "swift" => "🐦",
            "go" => "🐹",
            "java" | "kt" => "☕",
            "c" | "cpp" | "h" | "hpp" => "🔧",
            "lock" => "🔒",
            _ => "📄",
        }
    }

    pub fn size_display(&self) -> &str {
        &self.size_str
    }

    pub fn size_display_with_dir_size(&self, dir_sizes: &HashMap<PathBuf, u64>) -> String {
        if self.is_dir
            && let Some(&size) = dir_sizes.get(&self.path)
        {
            return format_size(size);
        }
        self.size_str.clone()
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

#[derive(Debug, Clone, PartialEq)]
pub enum PreviewContent {
    Image(PathBuf),
    Text { path: PathBuf, content: String },
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

/// Create preview content for a file entry (image marker or text body).
pub fn make_preview(entry: &FileEntry) -> Option<PreviewContent> {
    if entry.is_dir {
        return None;
    }
    let kind = if entry.is_image() {
        crate::ports::PreviewKind::Image
    } else {
        crate::ports::PreviewKind::Text
    };
    let capability = match kind {
        crate::ports::PreviewKind::Image => {
            crate::provider_runtime::ProviderCapability::PreviewImage
        }
        crate::ports::PreviewKind::Text => crate::provider_runtime::ProviderCapability::PreviewText,
    };
    let root = entry.path.parent().unwrap_or(Path::new("/"));
    if !crate::provider_runtime::activate_builtin(
        "native-preview",
        &crate::provider_runtime::ActivationRequest {
            capability,
            root,
            extension: (!entry.extension.is_empty()).then_some(entry.extension.as_str()),
            bytes: Some(entry.size),
        },
    ) {
        return None;
    }
    let request = crate::ports::PreviewRequest {
        path: &entry.path,
        kind,
        max_bytes: crate::ports::DEFAULT_TEXT_PREVIEW_BYTES,
    };
    match crate::ports::PreviewProvider::preview(&crate::ports::NativePreviewProvider, &request)
        .ok()?
    {
        crate::ports::PreviewArtifact::Image => Some(PreviewContent::Image(entry.path.clone())),
        crate::ports::PreviewArtifact::Text(content) => Some(PreviewContent::Text {
            path: entry.path.clone(),
            content,
        }),
    }
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
pub fn entry_display_size(entry: &FileEntry, dir_sizes: &HashMap<PathBuf, u64>) -> u64 {
    if entry.is_dir {
        dir_sizes.get(&entry.path).copied().unwrap_or(0)
    } else {
        entry.size
    }
}

/// Natural ("human") ordering: runs of digits compare by numeric value, so
/// "file2" sorts before "file10". Non-digit runs compare by char. Inputs are
/// expected pre-lowercased (we sort on `name_lower`).
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        let (ca, cb) = (a[i], b[j]);
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let si = i;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            // Compare by numeric value: drop leading zeros, then longer run
            // wins, then lexically; finally fewer leading zeros sorts first.
            let va = strip_leading_zeros(&a[si..i]);
            let vb = strip_leading_zeros(&b[sj..j]);
            let ord = va
                .len()
                .cmp(&vb.len())
                .then_with(|| va.iter().cmp(vb.iter()))
                .then_with(|| (i - si).cmp(&(j - sj)));
            if ord != Ordering::Equal {
                return ord;
            }
        } else {
            match ca.cmp(&cb) {
                Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                ord => return ord,
            }
        }
    }
    // One ran out: the shorter string sorts first.
    (a.len() - i).cmp(&(b.len() - j))
}

fn strip_leading_zeros(s: &[char]) -> &[char] {
    let mut k = 0;
    while k + 1 < s.len() && s[k] == '0' {
        k += 1;
    }
    &s[k..]
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

/// Cached filtered view: indices into `entries` matching `query` and facets.
/// Valid while `generation`, `query` and `facets` are unchanged.
struct FilterCache {
    generation: u64,
    query: String,
    facets: FacetSet,
    indices: Vec<usize>,
}

impl FilterCache {
    fn stale() -> Self {
        FilterCache {
            generation: u64::MAX, // sentinel: never computed
            query: String::new(),
            facets: FacetSet::default(),
            indices: Vec::new(),
        }
    }
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

/// A directory's sort/filter/hidden/density settings, remembered per path so
/// returning to a directory restores how it was last left. In-memory only:
/// scoped to the running session, not persisted across restarts (unlike the
/// current directory's own settings, which the session file already saves).
#[derive(Debug, Clone, PartialEq)]
pub struct ViewSettings {
    pub sort_col: SortColumn,
    pub sort_order: SortOrder,
    pub show_hidden: bool,
    pub folders_first: bool,
    pub natural_name_sort: bool,
    pub facets: FacetSet,
    pub density: crate::density::Density,
    pub cursor_path: Option<PathBuf>,
    pub scroll_anchor: usize,
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
    pub entries: Vec<FileEntry>,
    /// Selected entries, keyed by path so selection survives
    /// filtering, sorting and directory refreshes.
    pub selected: std::collections::HashSet<PathBuf>,
    /// Files flagged for later reference, independent of `selected`: not
    /// touched by select-all/invert/clear-selection, and available to the
    /// selection algebra the same way the stash is.
    pub marked: std::collections::HashSet<PathBuf>,
    pub cursor: usize,
    pub scroll_to_cursor: bool,
    /// First row visible in the virtualized list. Stored with the focused path
    /// so a directory can reopen in the same neighborhood.
    pub scroll_anchor: usize,
    /// Why the current listing is empty/non-empty (for the empty-state UI).
    pub dir_status: DirStatus,
    /// Visible rows in the list viewport, set by the renderer each frame and
    /// read by PageUp/PageDown. Zero until the panel has been drawn once.
    pub page_rows: usize,
    pub preview: Option<PreviewContent>,
    /// Per-pane directory history (back/forward); a vim-style jump trail that
    /// truncates its forward tail on a new navigation.
    pub history: crate::jumplist::JumpList,
    pub search_query: String,
    /// Active quick-filter facets, ANDed with the substring filter.
    pub facets: FacetSet,
    pub sort_col: SortColumn,
    pub sort_order: SortOrder,
    /// Pin folders to the top of the listing (classic dual-pane default).
    pub folders_first: bool,
    /// Natural numeric name ordering (`file2` < `file10`); off = plain A-Z.
    pub natural_name_sort: bool,
    pub show_hidden: bool,
    /// List density tier (row sizes). Restored from and saved to the
    /// session, and remembered per directory in `view_memory` like the
    /// other view fields above.
    pub density: crate::density::Density,
    /// Sort/filter/hidden/density settings remembered per visited
    /// directory (session-lifetime only), keyed by that directory's path.
    /// Applied by `navigate_to` when returning to a remembered directory.
    pub view_memory: HashMap<PathBuf, ViewSettings>,
    pub dir_sizes: Arc<Mutex<HashMap<PathBuf, u64>>>,
    pub dir_counts: Arc<Mutex<HashMap<PathBuf, usize>>>,
    notify: Option<Notify>,
    pub drag_entries: Vec<PathBuf>,
    pub drop_target: Option<PathBuf>,
    pub needs_refresh: Arc<std::sync::atomic::AtomicBool>,
    /// Incremented when the backend reports an overflow/rescan flag or an
    /// event-stream error. The next poll replaces the whole listing snapshot.
    watcher_rescan_generation: Arc<std::sync::atomic::AtomicU64>,
    applied_rescan_generation: u64,
    /// Set by deep watcher events: directory sizes need recomputing,
    /// but the listing itself is unchanged.
    sizes_dirty: Arc<std::sync::atomic::AtomicBool>,
    last_sizes_recompute: Option<std::time::Instant>,
    /// Bumped whenever `entries` content or order changes.
    entries_gen: u64,
    filter_cache: std::cell::RefCell<FilterCache>,
    watcher: Option<notify::RecommendedWatcher>,
    watched_path: Option<PathBuf>,
}

impl PanelState {
    /// Create a panel pointed at `path`. The directory is NOT read yet:
    /// call [`refresh`](Self::refresh) (the UI does this when wiring the
    /// notify callback on the first frame).
    pub fn new(path: PathBuf) -> Self {
        PanelState {
            current_path: path.clone(),
            entries: Vec::new(),
            selected: std::collections::HashSet::new(),
            marked: std::collections::HashSet::new(),
            cursor: 0,
            scroll_to_cursor: false,
            scroll_anchor: 0,
            dir_status: DirStatus::Empty,
            page_rows: 0,
            preview: None,
            history: {
                let mut h = crate::jumplist::JumpList::new();
                h.push(path);
                h
            },
            search_query: String::new(),
            facets: FacetSet::default(),
            sort_col: SortColumn::Name,
            sort_order: SortOrder::Asc,
            folders_first: true,
            natural_name_sort: true,
            show_hidden: false,
            density: crate::density::Density::default(),
            view_memory: HashMap::new(),
            dir_sizes: Arc::new(Mutex::new(HashMap::new())),
            dir_counts: Arc::new(Mutex::new(HashMap::new())),
            notify: None,
            drag_entries: Vec::new(),
            drop_target: None,
            needs_refresh: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            watcher_rescan_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            applied_rescan_generation: 0,
            sizes_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            last_sizes_recompute: None,
            entries_gen: 0,
            filter_cache: std::cell::RefCell::new(FilterCache::stale()),
            watcher: None,
            watched_path: None,
        }
    }

    /// Wire the UI wake-up callback and restart the watcher so its
    /// notifications reach the UI.
    pub fn set_notify(&mut self, notify: Notify) {
        self.notify = Some(notify);
        self.watcher = None;
        self.watched_path = None;
        self.start_watcher();
    }

    pub fn has_notify(&self) -> bool {
        self.notify.is_some()
    }

    pub fn refresh(&mut self) {
        self.reload_entries();
        if self.compute_dir_sizes(true) {
            self.sizes_dirty
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.last_sizes_recompute = Some(std::time::Instant::now());
        self.start_watcher();
    }

    /// Re-read the directory, preserving selection and cursor position
    /// by path (entries may have been added, removed or re-sorted).
    fn reload_entries(&mut self) {
        let cursor_path = if self.cursor > 0 {
            self.filtered_get(self.cursor - 1).map(|e| e.path.clone())
        } else {
            None
        };

        self.entries = Self::read_dir(&self.current_path, self.show_hidden);
        self.dir_status = classify_dir(&self.current_path, self.entries.is_empty());
        self.sort_entries();

        {
            let existing: std::collections::HashSet<&PathBuf> =
                self.entries.iter().map(|e| &e.path).collect();
            self.selected.retain(|p| existing.contains(p));
            self.marked.retain(|p| existing.contains(p));
        }

        let restored = cursor_path
            .and_then(|path| self.filtered_entries().iter().position(|e| e.path == path));
        match restored {
            Some(idx) => self.cursor = idx + 1,
            None => self.cursor = self.cursor.min(self.filtered_count()),
        }
    }

    /// Check if fs watcher flagged a change; if so, refresh.
    /// Returns `true` when the directory listing was re-read.
    /// Deep events (below the watched dir) only recompute directory
    /// sizes, debounced so event floods during transfers don't thrash.
    pub fn poll_fs_changes(&mut self) -> bool {
        const SIZES_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);

        let reload = self
            .needs_refresh
            .swap(false, std::sync::atomic::Ordering::Relaxed);
        if reload {
            let rescan_generation = self
                .watcher_rescan_generation
                .load(std::sync::atomic::Ordering::Relaxed);
            if rescan_generation != self.applied_rescan_generation {
                invalidate_size_cache(&self.current_path);
                self.applied_rescan_generation = rescan_generation;
            }
            self.reload_entries();
            let retry = self.compute_dir_sizes(true);
            self.sizes_dirty
                .store(retry, std::sync::atomic::Ordering::Relaxed);
            self.last_sizes_recompute = Some(std::time::Instant::now());
            return true;
        }

        if self.sizes_dirty.load(std::sync::atomic::Ordering::Relaxed) {
            let due = self
                .last_sizes_recompute
                .is_none_or(|t| t.elapsed() >= SIZES_DEBOUNCE);
            if due {
                self.last_sizes_recompute = Some(std::time::Instant::now());
                // Keep the dirty flag when some dir is still in its walk
                // cooldown: a later poll picks it up.
                let retry = self.compute_dir_sizes(false);
                self.sizes_dirty
                    .store(retry, std::sync::atomic::Ordering::Relaxed);
            } else if let Some(wake) = &self.notify {
                // Poll again on a later frame once the debounce expires.
                wake();
            }
        }
        false
    }

    fn start_watcher(&mut self) {
        use notify::{Event, RecursiveMode, Watcher};

        // Skip if already watching this path
        if self.watched_path.as_ref() == Some(&self.current_path) {
            return;
        }

        // Drop old watcher
        self.watcher = None;
        self.watched_path = None;

        let flag = Arc::clone(&self.needs_refresh);
        let sizes_flag = Arc::clone(&self.sizes_dirty);
        let rescan_generation = Arc::clone(&self.watcher_rescan_generation);
        let wake = self.notify.clone();
        let watched = self.current_path.clone();

        let watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                if event.need_rescan() {
                    invalidate_size_cache(&watched);
                    flag_watcher_gap(&rescan_generation, &flag);
                    if let Some(wake) = &wake {
                        wake();
                    }
                    return;
                }
                // A change anywhere under a cached directory makes its
                // size stale, even though its own mtime doesn't move.
                let mut direct = event.paths.is_empty();
                for p in &event.paths {
                    invalidate_size_cache(p);
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
            } else {
                invalidate_size_cache(&watched);
                flag_watcher_gap(&rescan_generation, &flag);
                if let Some(wake) = &wake {
                    wake();
                }
            }
        });

        if let Ok(mut w) = watcher {
            // Recursive: deep events never reload the listing, but they
            // must invalidate cached sizes (see the callback above).
            let _ = w.watch(&self.current_path, RecursiveMode::Recursive);
            self.watched_path = Some(self.current_path.clone());
            self.watcher = Some(w);
        }
    }

    /// Schedule background recomputation of subdirectory sizes and counts.
    /// `forced` marks user-driven refreshes: they may re-walk expensive
    /// dirs, background (watcher-noise) recomputes may not.
    /// Returns `true` when some dir was skipped because of the walk
    /// cooldown and the caller should retry later.
    fn compute_dir_sizes(&self, forced: bool) -> bool {
        // Clear panel-local sizes and counts for current directory listing
        if let Ok(mut sizes) = self.dir_sizes.lock() {
            sizes.clear();
        }
        if let Ok(mut counts) = self.dir_counts.lock() {
            counts.clear();
        }

        // Collect dirs that need background work
        let mut need_count: Vec<PathBuf> = Vec::new();
        let mut need_size: Vec<(PathBuf, Option<SystemTime>, VolumePathKey)> = Vec::new();
        let mut retry = false;

        for entry in &self.entries {
            if !entry.is_dir {
                continue;
            }

            need_count.push(entry.path.clone());

            let dir_mtime = fs::metadata(&entry.path).and_then(|m| m.modified()).ok();
            let cache_key = VolumePathKey::observe(&entry.path);

            // Check global cache: if mtime matches, reuse cached size
            if let Some(mtime) = dir_mtime
                && let Ok(cache) = dir_size_cache().lock()
                && let Some(&(cached_mtime, cached_size)) = cache.get(&cache_key)
                && cached_mtime == mtime
            {
                if let Ok(mut sizes) = self.dir_sizes.lock() {
                    sizes.insert(entry.path.clone(), cached_size);
                }
                continue;
            }

            // Walk-log guards (see walk_log docs).
            // In test builds the guards can be switched off via env to
            // benchmark the unguarded behaviour (profiling harness).
            #[cfg(test)]
            let guards_enabled = std::env::var("COMMANDER_DISABLE_WALK_GUARDS").is_err();
            #[cfg(not(test))]
            let guards_enabled = true;

            let mut skip = false;
            if guards_enabled
                && let Ok(log) = walk_log().lock()
                && let Some(&(when, cost)) = log.get(&cache_key)
            {
                if when.elapsed() < WALK_COOLDOWN {
                    skip = true;
                    retry = true;
                } else if !forced && cost > WALK_EXPENSIVE {
                    skip = true;
                }
            }
            if skip {
                // Keep showing the last known size instead of "…".
                if let Ok(cache) = dir_size_cache().lock()
                    && let Some(&(_, cached_size)) = cache.get(&cache_key)
                    && let Ok(mut sizes) = self.dir_sizes.lock()
                {
                    sizes.insert(entry.path.clone(), cached_size);
                }
                continue;
            }

            need_size.push((entry.path.clone(), dir_mtime, cache_key));
        }

        let filtered = self.filtered_indices();
        let visible_paths: std::collections::HashSet<PathBuf> =
            visible_window(filtered.len(), self.scroll_anchor, self.page_rows)
                .filter_map(|index| {
                    filtered
                        .get(index)
                        .and_then(|entry| self.entries.get(*entry))
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

        // Dedicated thread pool (max 10 threads) for filesystem work
        fn fs_pool() -> &'static rayon::ThreadPool {
            static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
            POOL.get_or_init(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(10)
                    .thread_name(|i| format!("fs-worker-{}", i))
                    .build()
                    .unwrap()
            })
        }

        // Subdir counts
        let counts = Arc::clone(&self.dir_counts);
        let wake1 = self.notify.clone();
        fs_pool().spawn_fifo(move || {
            use rayon::prelude::*;
            let count = |path: &PathBuf| {
                let count = fs::read_dir(path)
                    .map(|entries| entries.filter_map(Result::ok).count())
                    .unwrap_or(0);
                (path.clone(), count)
            };
            for path in &visible_counts {
                let (path, count) = count(path);
                if let Ok(mut map) = counts.lock() {
                    map.insert(path, count);
                }
                if let Some(wake) = &wake1 {
                    wake();
                }
            }
            let background_results: Vec<_> =
                fs_pool().install(|| background_counts.par_iter().map(count).collect());
            if let Ok(mut map) = counts.lock() {
                map.extend(background_results);
            }
            if let Some(wake) = &wake1 {
                wake();
            }
        });

        // Dir sizes
        if !visible_sizes.is_empty() || !background_sizes.is_empty() {
            let sizes = Arc::clone(&self.dir_sizes);
            let wake2 = self.notify.clone();
            fs_pool().spawn_fifo(move || {
                use rayon::prelude::*;
                fn measure(
                    (path, modified, cache_key): &(PathBuf, Option<SystemTime>, VolumePathKey),
                ) -> (PathBuf, Option<SystemTime>, VolumePathKey, u64) {
                    let started = std::time::Instant::now();
                    let size = crate::fs_util::dir_size_recursive(path);
                    if let Ok(mut log) = walk_log().lock() {
                        log.retain(|key, _| key.path != *path || key == cache_key);
                        log.insert(
                            cache_key.clone(),
                            (std::time::Instant::now(), started.elapsed()),
                        );
                    }
                    (path.clone(), *modified, cache_key.clone(), size)
                }

                let publish = |results: &[(PathBuf, Option<SystemTime>, VolumePathKey, u64)]| {
                    if let Ok(mut map) = sizes.lock() {
                        for (path, _, _, size) in results {
                            map.insert(path.clone(), *size);
                        }
                    }
                    if let Ok(mut cache) = dir_size_cache().lock() {
                        for (path, modified, key, size) in results {
                            if let Some(modified) = modified {
                                cache.retain(|old, _| old.path != *path || old == key);
                                cache.insert(key.clone(), (*modified, *size));
                            }
                        }
                    }
                };

                for item in &visible_sizes {
                    let result = measure(item);
                    publish(std::slice::from_ref(&result));
                    if let Some(wake) = &wake2 {
                        wake();
                    }
                }
                if !visible_sizes.is_empty() {
                    flush_cache();
                }
                let background_results: Vec<_> =
                    fs_pool().install(|| background_sizes.par_iter().map(measure).collect());
                if !background_results.is_empty() {
                    publish(&background_results);
                    flush_cache();
                }
                if let Some(wake) = &wake2 {
                    wake();
                }
            });
        }

        retry
    }

    fn read_dir(path: &Path, show_hidden: bool) -> Vec<FileEntry> {
        jwalk::WalkDir::new(path)
            .max_depth(1)
            .skip_hidden(!show_hidden)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.depth() == 1) // skip the root dir itself
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                FileEntry::from_meta(e.path(), &meta)
            })
            .collect()
    }

    pub fn sort_entries(&mut self) {
        let col = self.sort_col;
        let order = self.sort_order;
        let folders_first = self.folders_first;
        let natural = self.natural_name_sort;

        self.entries.sort_by(|a, b| {
            // Folders pinned to the top, unless that grouping is turned off.
            if folders_first {
                match (a.is_dir, b.is_dir) {
                    (true, false) => return Ordering::Less,
                    (false, true) => return Ordering::Greater,
                    _ => {}
                }
            }

            let cmp = match col {
                // Natural order over the precomputed lowercase name, so
                // "file2" sorts before "file10"; plain A-Z when disabled.
                SortColumn::Name if natural => natural_cmp(&a.name_lower, &b.name_lower),
                SortColumn::Name => a.name_lower.cmp(&b.name_lower),
                SortColumn::Size => a.size.cmp(&b.size),
                SortColumn::Modified => a.modified.cmp(&b.modified),
                // Group by extension, then by name within an extension.
                SortColumn::Extension => a
                    .extension
                    .cmp(&b.extension)
                    .then_with(|| natural_cmp(&a.name_lower, &b.name_lower)),
                // Group by coarse kind, then by name within a kind.
                SortColumn::Kind => crate::selection_summary::kind_of(a)
                    .cmp(&crate::selection_summary::kind_of(b))
                    .then_with(|| natural_cmp(&a.name_lower, &b.name_lower)),
            };

            match order {
                SortOrder::Asc => cmp,
                SortOrder::Desc => cmp.reverse(),
            }
        });
        // Content/order changed: filtered indices must be rebuilt.
        self.entries_gen = self.entries_gen.wrapping_add(1);
        self.ensure_cursor_valid();
    }

    /// Toggle pinning folders to the top, then re-sort in place.
    pub fn toggle_folders_first(&mut self) {
        self.folders_first = !self.folders_first;
        self.sort_entries();
    }

    /// Toggle natural vs plain A-Z name ordering, then re-sort in place.
    pub fn toggle_natural_sort(&mut self) {
        self.natural_name_sort = !self.natural_name_sort;
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
            sort_col: self.sort_col,
            sort_order: self.sort_order,
            show_hidden: self.show_hidden,
            folders_first: self.folders_first,
            natural_name_sort: self.natural_name_sort,
            facets: self.facets,
            density: self.density,
            cursor_path: self
                .cursor
                .checked_sub(1)
                .and_then(|index| self.filtered_get(index))
                .map(|entry| entry.path.clone()),
            scroll_anchor: self.scroll_anchor,
        }
    }

    /// Remember the current directory's view settings under its own path.
    fn stash_view_settings(&mut self) {
        let settings = self.snapshot_view_settings();
        self.view_memory.insert(self.current_path.clone(), settings);
    }

    /// Apply the current directory's remembered view settings, if any.
    /// Leaves everything unchanged (carrying over whatever was already
    /// active) when this directory has never been visited this session.
    fn restore_view_settings(&mut self) -> Option<(Option<PathBuf>, usize)> {
        let s = self.view_memory.get(&self.current_path)?.clone();
        self.sort_col = s.sort_col;
        self.sort_order = s.sort_order;
        self.show_hidden = s.show_hidden;
        self.folders_first = s.folders_first;
        self.natural_name_sort = s.natural_name_sort;
        self.facets = s.facets;
        self.density = s.density;
        Some((s.cursor_path, s.scroll_anchor))
    }

    fn load_remembered_path(&mut self, path: PathBuf) {
        self.current_path = path;
        self.search_query.clear();
        let remembered = self.restore_view_settings();
        if let Some((_, scroll_anchor)) = &remembered {
            self.scroll_anchor = *scroll_anchor;
        }
        self.refresh();
        if let Some((cursor_path, scroll_anchor)) = remembered {
            self.scroll_anchor = scroll_anchor.min(self.filtered_count().saturating_sub(1));
            self.cursor = cursor_path
                .and_then(|path| {
                    self.filtered_entries()
                        .iter()
                        .position(|entry| entry.path == path)
                        .map(|index| index + 1)
                })
                .unwrap_or_else(|| {
                    self.scroll_anchor
                        .saturating_add(1)
                        .min(self.filtered_count())
                });
            self.scroll_to_cursor = self.cursor > 0;
        } else {
            self.scroll_anchor = 0;
        }
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
            if let Some(name) = child
                && let Some(idx) = self.filtered_entries().iter().position(|e| e.name == name)
            {
                self.cursor = idx + 1;
                self.scroll_to_cursor = true;
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
        let pos = {
            let entries = self.filtered_entries();
            entries
                .iter()
                .position(|e| e.name_lower.starts_with(&q))
                .or_else(|| entries.iter().position(|e| e.name_lower.contains(&q)))
        };
        if let Some(idx) = pos {
            self.cursor = idx + 1;
            self.scroll_to_cursor = true;
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
            self.selected.insert(p);
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
            if self.selected.insert(p) {
                added += 1;
            }
        }
        added
    }

    /// Select every filtered file sharing the cursor file's extension. Does
    /// nothing if the cursor is on `..`, a folder, or an extension-less file.
    /// Returns how many entries were added.
    pub fn select_same_extension_as_cursor(&mut self) -> usize {
        let ext = match self
            .cursor
            .checked_sub(1)
            .and_then(|i| self.filtered_get(i))
        {
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
            self.selected.insert(p);
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
            self.selected.insert(p);
        }
        added
    }

    /// Flip the current sort order (ascending <-> descending) and re-sort.
    pub fn reverse_sort(&mut self) {
        self.sort_order = match self.sort_order {
            SortOrder::Asc => SortOrder::Desc,
            SortOrder::Desc => SortOrder::Asc,
        };
        self.sort_entries();
    }

    /// Entries to export/copy as a listing: the current selection if anything
    /// is selected, otherwise the whole filtered view (in display order).
    pub fn listing_entries(&self) -> Vec<&FileEntry> {
        let all = self.filtered_entries();
        if self.selected.is_empty() {
            all
        } else {
            all.into_iter()
                .filter(|e| self.selected.contains(&e.path))
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
                self.selected.insert(path);
                added += 1;
            } else {
                self.selected.remove(&path);
            }
        }
        added
    }

    /// How many filtered entries any term of `mask` matches (live preview,
    /// no mutation).
    pub fn mask_match_count(&self, mask: &str) -> usize {
        let terms = parse_mask(mask);
        if terms.is_empty() {
            return 0;
        }
        self.filtered_entries()
            .iter()
            .filter(|e| {
                terms
                    .iter()
                    .any(|(t, _)| term_matches(t, &e.name_lower, &e.extension))
            })
            .count()
    }

    /// Add the file under the cursor to the selection (range-select step).
    pub fn select_cursor(&mut self) {
        if self.cursor == 0 {
            return;
        }
        if let Some(path) = self.filtered_get(self.cursor - 1).map(|e| e.path.clone()) {
            self.selected.insert(path);
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.history.can_back()
    }

    pub fn can_go_forward(&self) -> bool {
        self.history.can_forward()
    }

    pub fn go_back(&mut self) {
        // `back` walks the existing trail without recording a new jump.
        self.stash_view_settings();
        if let Some(path) = self.history.back().map(|p| p.to_path_buf()) {
            record_visit(&path);
            self.load_remembered_path(path);
        }
    }

    pub fn go_forward(&mut self) {
        self.stash_view_settings();
        if let Some(path) = self.history.forward().map(|p| p.to_path_buf()) {
            record_visit(&path);
            self.load_remembered_path(path);
        }
    }

    /// Rebuild the cached filtered indices if entries or query changed.
    /// A warm cache costs two comparisons; the string matching over all
    /// entries runs only when something actually changed.
    fn ensure_filter_cache(&self) {
        let mut cache = self.filter_cache.borrow_mut();
        let query = self.search_query.trim();
        if cache.generation == self.entries_gen
            && cache.query == query
            && cache.facets == self.facets
        {
            return;
        }
        cache.generation = self.entries_gen;
        cache.query = query.to_string();
        cache.facets = self.facets;
        cache.indices.clear();

        // Fuzzy subsequence match (shared with the command palette), so "scn"
        // narrows to "scanner.rs". This is more permissive than a substring
        // filter; the sort order is left untouched (we narrow, never reorder).
        let facets = self.facets;
        let no_facets = facets.is_empty();
        let now = SystemTime::now();
        cache.indices.extend(
            self.entries
                .iter()
                .enumerate()
                .filter(|(_, e)| crate::fuzzy::is_match(query, &e.name))
                .filter(|(_, e)| no_facets || facet_matches(e, &facets, now))
                .map(|(i, _)| i),
        );
    }

    /// Number of entries matching the current filter (no allocation).
    pub fn filtered_count(&self) -> usize {
        self.ensure_filter_cache();
        self.filter_cache.borrow().indices.len()
    }

    /// Clamp the cursor after any filter, facet, or ordering change. Cursor 0
    /// is the synthetic parent row; real rows occupy 1..=filtered_count().
    pub fn ensure_cursor_valid(&mut self) {
        let clamped = self.cursor.min(self.filtered_count());
        if self.cursor != clamped {
            self.cursor = clamped;
            self.scroll_to_cursor = true;
        }
    }

    /// Clear both text and facet filters as one invariant-preserving action.
    pub fn clear_filters(&mut self) {
        self.search_query.clear();
        self.facets = FacetSet::default();
        self.ensure_cursor_valid();
    }

    /// The i-th entry of the filtered view (no allocation).
    pub fn filtered_get(&self, i: usize) -> Option<&FileEntry> {
        self.ensure_filter_cache();
        let idx = *self.filter_cache.borrow().indices.get(i)?;
        self.entries.get(idx)
    }

    /// Snapshot of the filtered view as indices into `entries`.
    /// Cheap (a `Vec<usize>` clone); used by the virtualized list renderer.
    pub fn filtered_indices(&self) -> Vec<usize> {
        self.ensure_filter_cache();
        self.filter_cache
            .borrow()
            .indices
            .iter()
            .copied()
            .filter(|&i| i < self.entries.len())
            .collect()
    }

    pub fn filtered_entries(&self) -> Vec<&FileEntry> {
        self.ensure_filter_cache();
        let cache = self.filter_cache.borrow();
        cache
            .indices
            .iter()
            .filter_map(|&i| self.entries.get(i))
            .collect()
    }

    pub fn toggle_select(&mut self, path: PathBuf) {
        if !self.selected.remove(&path) {
            self.selected.insert(path);
        }
    }

    /// Start a row drag. An unselected anchor always drags only itself; a
    /// selected anchor drags the visible selected set in listing order.
    pub fn begin_drag(&mut self, anchor: PathBuf) {
        self.drag_entries = if self.selected.contains(&anchor) {
            self.filtered_entries()
                .into_iter()
                .filter(|entry| self.selected.contains(&entry.path))
                .map(|entry| entry.path.clone())
                .collect()
        } else {
            vec![anchor]
        };
    }

    /// Flip `path`'s membership in the mark set. Unlike `toggle_select`,
    /// marks are never cleared by select-all/invert/clear-selection.
    pub fn toggle_mark(&mut self, path: PathBuf) {
        if !self.marked.remove(&path) {
            self.marked.insert(path);
        }
    }

    pub fn clear_marks(&mut self) {
        self.marked.clear();
    }

    pub fn select_all(&mut self) {
        let all: Vec<PathBuf> = self
            .filtered_entries()
            .iter()
            .map(|e| e.path.clone())
            .collect();
        let all_selected =
            self.selected.len() == all.len() && all.iter().all(|p| self.selected.contains(p));
        if all_selected {
            self.selected.clear();
        } else {
            self.selected = all.into_iter().collect();
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
            if !self.selected.remove(&p) {
                self.selected.insert(p);
            }
        }
    }

    /// Add `paths` to the current selection, keeping any existing picks.
    /// Used by relationship-based selectors (e.g. "select files also in the
    /// other panel") so selections compose instead of replacing each other.
    pub fn extend_selection(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.selected.extend(paths);
    }

    pub fn selected_entries(&self) -> Vec<FileEntry> {
        self.filtered_entries()
            .into_iter()
            .filter(|e| self.selected.contains(&e.path))
            .cloned()
            .collect()
    }

    pub fn selected_or_cursor(&self) -> Result<Vec<FileEntry>, StaleCursor> {
        if self.selected.is_empty() {
            // cursor 0 = ".." row, real files start at cursor 1
            if self.cursor == 0 {
                return Ok(vec![]);
            }
            match self.filtered_get(self.cursor - 1) {
                Some(entry) => Ok(vec![entry.clone()]),
                None => Err(StaleCursor {
                    cursor: self.cursor,
                    visible_entries: self.filtered_entries().len(),
                }),
            }
        } else {
            Ok(self.selected_entries())
        }
    }

    pub fn total_size_selected(&self) -> u64 {
        self.ensure_filter_cache();
        let sizes = self.dir_sizes.lock().ok();
        let cache = self.filter_cache.borrow();
        cache
            .indices
            .iter()
            .filter_map(|&i| self.entries.get(i))
            .filter(|e| self.selected.contains(&e.path))
            .map(|e| {
                if e.is_dir {
                    sizes
                        .as_ref()
                        .and_then(|s| s.get(&e.path).copied())
                        .unwrap_or(0)
                } else {
                    e.size
                }
            })
            .sum()
    }

    /// Folder aggregates for the status bar, computed in a single pass over the
    /// listing with one lock on the size map: total bytes, the largest entry,
    /// and the oldest. `total` is `None` until at least one subdirectory has
    /// been sized, so it never flashes a misleadingly-small figure mid-scan.
    /// Subdirectory sizes come from the background-computed map; an unsized
    /// subdir counts as 0. Largest/oldest keep the first-seen entry on ties.
    pub fn folder_overview(&self) -> FolderOverview {
        let sizes = self.dir_sizes.lock().ok();
        let mut dir_count = 0usize;
        let mut computed = 0usize;
        let mut total = 0u64;
        // Track winners by index and clone their names once at the end, so the
        // per-frame pass allocates at most twice (not once per new maximum).
        let mut largest: Option<(usize, u64)> = None;
        let mut oldest: Option<(usize, SystemTime)> = None;
        for (i, e) in self.entries.iter().enumerate() {
            let size = if e.is_dir {
                dir_count += 1;
                match sizes.as_ref().and_then(|s| s.get(&e.path).copied()) {
                    Some(s) => {
                        computed += 1;
                        total += s;
                        s
                    }
                    None => 0,
                }
            } else {
                total += e.size;
                e.size
            };
            if largest.is_none_or(|(_, sz)| size > sz) {
                largest = Some((i, size));
            }
            if let Some(m) = e.modified
                && oldest.is_none_or(|(_, om)| m < om)
            {
                oldest = Some((i, m));
            }
        }
        let total = if computed == 0 && dir_count > 0 {
            None
        } else {
            Some(total)
        };
        FolderOverview {
            total,
            largest: largest.map(|(i, sz)| (self.entries[i].name.clone(), sz)),
            oldest: oldest.map(|(i, m)| (self.entries[i].name.clone(), m)),
        }
    }

    /// Monotonic generation of this panel's entry list, bumped on every content
    /// or order change. Lets the app cache per-panel derived data (e.g. the
    /// cross-panel compare map) and rebuild only when the entries change.
    pub fn entries_gen(&self) -> u64 {
        self.entries_gen
    }

    pub fn set_sort(&mut self, col: SortColumn) {
        if self.sort_col == col {
            self.sort_order = match self.sort_order {
                SortOrder::Asc => SortOrder::Desc,
                SortOrder::Desc => SortOrder::Asc,
            };
        } else {
            self.sort_col = col;
            self.sort_order = SortOrder::Asc;
        }
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

    pub fn sort_indicator(&self, col: SortColumn) -> &str {
        if self.sort_col == col {
            match self.sort_order {
                SortOrder::Asc => " ▲",
                SortOrder::Desc => " ▼",
            }
        } else {
            ""
        }
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

    fn panel_with(entries: Vec<FileEntry>) -> PanelState {
        let mut p = PanelState::new(PathBuf::from("/test"));
        p.entries = entries;
        p
    }

    #[test]
    fn dragging_an_unselected_row_does_not_use_the_stale_selection() {
        let mut panel = panel_with(vec![
            entry("selected.txt", false, 1),
            entry("dragged.txt", false, 1),
        ]);
        panel.selected.insert(PathBuf::from("/test/selected.txt"));

        panel.begin_drag(PathBuf::from("/test/dragged.txt"));

        assert_eq!(panel.drag_entries, [PathBuf::from("/test/dragged.txt")]);
    }

    #[test]
    fn dragging_a_selected_row_uses_the_visible_selection() {
        let mut panel = panel_with(vec![entry("a.txt", false, 1), entry("b.txt", false, 1)]);
        panel
            .selected
            .extend([PathBuf::from("/test/a.txt"), PathBuf::from("/test/b.txt")]);

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
        let names: Vec<&str> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Apple", "zoo", "beta.txt", "zeta.txt"]);
    }

    #[test]
    fn sort_desc_reverses_within_groups() {
        let mut p = panel_with(vec![entry("a.txt", false, 1), entry("b.txt", false, 2)]);
        p.set_sort(SortColumn::Size); // asc
        p.set_sort(SortColumn::Size); // same column again -> desc
        let names: Vec<&str> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["b.txt", "a.txt"]);
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
        let names: Vec<&str> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Apple", "beta.txt", "zeta.txt", "zoo"]);
        // Back on: folders return to the top.
        p.toggle_folders_first();
        let names: Vec<&str> = p.entries.iter().map(|e| e.name.as_str()).collect();
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
        let names: Vec<&str> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["file2.txt", "file10.txt"]);
        // ASCII: "file10" sorts before "file2" lexicographically.
        p.toggle_natural_sort();
        let names: Vec<&str> = p.entries.iter().map(|e| e.name.as_str()).collect();
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
        for e in &mut p.entries {
            e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
        }
        p.set_sort(SortColumn::Extension);
        let names: Vec<&str> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["a.rs", "z.rs", "b.txt", "c.txt"]);
    }

    #[test]
    fn sort_by_kind_orders_image_before_doc_before_code() {
        let mut p = panel_with(vec![
            entry("main.rs", false, 1),
            entry("pic.jpg", false, 1),
            entry("doc.pdf", false, 1),
        ]);
        for e in &mut p.entries {
            e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
        }
        p.set_sort(SortColumn::Kind);
        let names: Vec<&str> = p.entries.iter().map(|e| e.name.as_str()).collect();
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
            .selected
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert!(names.contains(".DS_Store"));
        assert!(names.contains("Thumbs.db"));
        assert!(!names.contains("photo.jpg"));
    }

    fn selected_names(p: &PanelState) -> std::collections::HashSet<String> {
        p.selected
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
        for e in &mut p.entries {
            e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
        }
        p.cursor = 1; // first filtered entry: a.rs
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
        let names: Vec<&str> = p.entries.iter().map(|e| e.name.as_str()).collect();
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
        p.selected.insert(PathBuf::from("/test/b"));
        let names: Vec<String> = p.listing_entries().iter().map(|e| e.name.clone()).collect();
        assert_eq!(names, vec!["b"]);
    }

    #[test]
    fn filter_matches_case_insensitively() {
        let mut p = panel_with(vec![
            entry("Cargo.toml", false, 1),
            entry("main.rs", false, 1),
        ]);
        p.search_query = "CARGO".to_string();
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
        p.search_query = "   ".to_string();
        assert!(!filter_is_active(&p.search_query, &p.facets));
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
        p.search_query = "scn".to_string();
        let names: Vec<&str> = p
            .filtered_entries()
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert_eq!(names, vec!["scanner.rs"]);

        // Empty query shows everything.
        p.search_query.clear();
        assert_eq!(p.filtered_count(), 3);

        // A non-subsequence excludes every row.
        p.search_query = "zzz".to_string();
        assert_eq!(p.filtered_count(), 0);
    }

    #[test]
    fn toggle_select_adds_then_removes() {
        let mut p = panel_with(vec![entry("a", false, 1)]);
        let path = p.entries[0].path.clone();
        p.toggle_select(path.clone());
        assert!(p.selected.contains(&path));
        p.toggle_select(path.clone());
        assert!(!p.selected.contains(&path));
    }

    #[test]
    fn toggle_mark_adds_then_removes_independently_of_selection() {
        let mut p = panel_with(vec![entry("a", false, 1)]);
        let path = p.entries[0].path.clone();
        p.toggle_select(path.clone());
        p.toggle_mark(path.clone());
        assert!(p.marked.contains(&path));
        assert!(
            p.selected.contains(&path),
            "marking does not touch selection"
        );
        p.toggle_mark(path.clone());
        assert!(!p.marked.contains(&path));
        assert!(
            p.selected.contains(&path),
            "unmarking does not touch selection"
        );
    }

    #[test]
    fn clear_marks_empties_the_set_without_touching_selection() {
        let mut p = panel_with(vec![entry("a", false, 1)]);
        let path = p.entries[0].path.clone();
        p.toggle_select(path.clone());
        p.toggle_mark(path.clone());
        p.clear_marks();
        assert!(p.marked.is_empty());
        assert!(p.selected.contains(&path));
    }

    #[test]
    fn select_all_toggles_between_all_and_none() {
        let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
        p.select_all();
        assert_eq!(p.selected.len(), 2);
        p.select_all();
        assert!(p.selected.is_empty());
    }

    #[test]
    fn invert_selection_flips_only_filtered_rows() {
        let mut p = panel_with(vec![
            entry("alpha", false, 1),
            entry("album", false, 1),
            entry("zebra", false, 1),
        ]);
        let alpha = p.entries[0].path.clone();
        let album = p.entries[1].path.clone();
        let zebra = p.entries[2].path.clone();
        // Pre-select one visible (alpha) and one that the filter will hide (zebra).
        p.selected.insert(alpha.clone());
        p.selected.insert(zebra.clone());
        // Filter to the "al" rows; zebra is now hidden from the view.
        p.search_query = "al".to_string();
        p.invert_selection();
        assert!(!p.selected.contains(&alpha), "visible+selected -> cleared");
        assert!(
            p.selected.contains(&album),
            "visible+unselected -> selected"
        );
        assert!(
            p.selected.contains(&zebra),
            "filtered-out row keeps its state"
        );
    }

    #[test]
    fn selected_or_cursor_falls_back_to_cursor_row() {
        let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
        p.cursor = 0; // ".." row
        assert!(p.selected_or_cursor().unwrap().is_empty());
        p.cursor = 2; // second file
        let picked = p.selected_or_cursor().unwrap();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].name, "b");
    }

    #[test]
    fn sorting_reclamps_a_cursor_after_the_filter_changes() {
        let mut p = panel_with(vec![entry("alpha", false, 1), entry("beta", false, 1)]);
        p.cursor = 2;
        p.search_query = "alpha".to_string();

        p.sort_entries();

        assert_eq!(p.cursor, 1);
        assert_eq!(p.filtered_get(0).unwrap().name, "alpha");
    }

    #[test]
    fn stale_filter_indices_are_bounded_and_cursor_miss_is_explicit() {
        let mut p = panel_with(vec![entry("alpha", false, 1), entry("beta", false, 1)]);
        p.cursor = 2;
        assert_eq!(p.filtered_count(), 2); // warm the index cache
        p.entries.clear(); // simulate an invariant violation without a generation bump

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
        assert_eq!(p.entries.len(), 3);

        // Cursor on "b.txt" (row 2), select and mark "c.txt".
        p.cursor = 2;
        p.toggle_select(doomed.clone());
        p.toggle_mark(doomed.clone());

        // A new file shifts sort order; a selected/marked file disappears.
        tmp.file("0-first.txt", "0");
        std::fs::remove_file(&doomed).unwrap();
        p.refresh();

        let under_cursor = p.filtered_entries()[p.cursor - 1].path.clone();
        assert!(under_cursor.ends_with("b.txt"), "cursor follows the path");
        assert!(p.selected.is_empty(), "selection drops deleted paths");
        assert!(p.marked.is_empty(), "marks drop deleted paths");
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
        let under = p.filtered_get(p.cursor - 1).unwrap();
        assert_eq!(under.name, "mmm");
    }

    #[test]
    fn navigate_to_restores_the_view_remembered_for_that_directory() {
        let tmp = TempDir::new();
        let downloads = tmp.dir("downloads");
        let docs = tmp.dir("docs");

        let mut p = PanelState::new(downloads.clone());
        p.refresh();
        p.sort_col = SortColumn::Size;
        p.sort_order = SortOrder::Desc;
        p.show_hidden = true;
        p.density = crate::density::Density::Compact;

        // "docs" has never been visited: like before per-folder memory
        // existed, its view carries over from wherever we came from.
        p.navigate_to(docs.clone());
        assert_eq!(p.sort_col, SortColumn::Size);
        assert_eq!(p.sort_order, SortOrder::Desc);
        assert!(p.show_hidden);
        assert_eq!(p.density, crate::density::Density::Compact);

        // Now give "docs" its own, different view.
        p.sort_col = SortColumn::Extension;
        p.sort_order = SortOrder::Asc;
        p.show_hidden = false;
        p.density = crate::density::Density::Spacious;

        // Back to "downloads": its own remembered view returns, not "docs"'s.
        p.navigate_to(downloads.clone());
        assert_eq!(p.sort_col, SortColumn::Size);
        assert_eq!(p.sort_order, SortOrder::Desc);
        assert!(p.show_hidden);
        assert_eq!(p.density, crate::density::Density::Compact);

        // And "docs" kept its own distinct view too.
        p.navigate_to(docs);
        assert_eq!(p.sort_col, SortColumn::Extension);
        assert_eq!(p.sort_order, SortOrder::Asc);
        assert!(!p.show_hidden);
        assert_eq!(p.density, crate::density::Density::Spacious);
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
        p.cursor = p
            .filtered_entries()
            .iter()
            .position(|entry| entry.path == focused)
            .unwrap()
            + 1;
        p.scroll_anchor = 1;

        p.navigate_to(docs);
        p.navigate_to(downloads);

        assert_eq!(p.filtered_get(p.cursor - 1).unwrap().path, focused);
        assert_eq!(p.scroll_anchor, 1);
        assert!(p.scroll_to_cursor);
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
        p.cursor = 2;
        p.scroll_anchor = 1;
        p.navigate_to(docs);
        std::fs::remove_file(focused).unwrap();
        p.navigate_to(downloads);

        assert_eq!(p.cursor, 2.min(p.filtered_count()));
        assert!(p.cursor <= p.filtered_count());
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
        let names: Vec<&str> = p.entries.iter().map(|e| e.name.as_str()).collect();
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
        for e in &mut p.entries {
            e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
        }
        p.facets = FacetSet {
            kind: Some(KindFacet::Images),
            ..Default::default()
        };
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
        for e in &mut p.entries {
            e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
        }
        p.sort_entries();

        // Add all jpgs, then subtract anything containing "raw".
        // raw.jpg matches the add but the subtract wins, so it is not added.
        let added = p.select_by_mask("*.jpg, !*raw*");
        assert_eq!(added, 2);
        let names: Vec<String> = p
            .selected
            .iter()
            .filter_map(|pth| pth.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert!(names.contains(&"a.jpg".to_string()));
        assert!(names.contains(&"b.jpg".to_string()));
        assert!(!names.contains(&"raw.jpg".to_string()));
        assert!(!names.contains(&"c.png".to_string()));
        assert_eq!(p.selected.len(), 2);
    }

    #[test]
    fn bare_extension_term_matches_by_extension() {
        let mut p = panel_with(vec![
            entry("doc.pdf", false, 1),
            entry("note.txt", false, 1),
        ]);
        for e in &mut p.entries {
            e.extension = e.name.rsplit('.').next().unwrap().to_lowercase();
        }
        assert_eq!(p.mask_match_count("pdf"), 1);
        p.select_by_mask("pdf");
        assert_eq!(p.selected.len(), 1);
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
        assert_eq!(p.filtered_get(p.cursor - 1).unwrap().name, "banana.txt");

        // No prefix hit -> substring fallback finds "cherry-banana".
        assert!(p.type_ahead("cherry"));
        assert_eq!(
            p.filtered_get(p.cursor - 1).unwrap().name,
            "cherry-banana.txt"
        );

        assert!(!p.type_ahead("zzz"));
    }

    #[test]
    fn select_cursor_adds_current_row() {
        let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
        p.cursor = 2; // second file
        p.select_cursor();
        assert!(p.selected.contains(&p.entries[1].path));
        assert_eq!(p.selected.len(), 1);
    }

    #[test]
    fn filter_cache_tracks_query_and_entry_changes() {
        let mut p = panel_with(vec![entry("alpha", false, 1), entry("beta", false, 1)]);
        assert_eq!(p.filtered_count(), 2);

        // Query change invalidates the cache.
        p.search_query = "al".to_string();
        assert_eq!(p.filtered_count(), 1);
        assert_eq!(p.filtered_get(0).unwrap().name, "alpha");

        // Entry change (generation bump via sort) invalidates it too.
        p.entries.push(entry("alps", false, 1));
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
        p.search_query = "keep".to_string();
        let idx = p.filtered_indices();
        assert_eq!(idx.len(), 2);
        for i in idx {
            assert!(p.entries[i].name.contains("keep"));
        }
    }

    #[test]
    fn size_cache_invalidation_marks_ancestors_only() {
        let tmp = TempDir::new();
        let a = tmp.dir("a");
        let other = tmp.dir("other");
        let a_key = VolumePathKey::observe(&a);
        let other_key = VolumePathKey::observe(&other);
        let now = SystemTime::now();
        {
            let mut cache = dir_size_cache().lock().unwrap();
            cache.insert(a_key.clone(), (now, 100));
            cache.insert(other_key.clone(), (now, 5));
        }

        invalidate_size_cache(&a.join("b/c/file.txt"));

        let cache = dir_size_cache().lock().unwrap();
        let (a_mtime, a_size) = cache[&a_key];
        assert_eq!(a_mtime, std::time::UNIX_EPOCH, "ancestor mtime is reset");
        assert_eq!(a_size, 100, "stale size is kept for display");
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
    fn deep_change_recomputes_dir_size_without_reloading_listing() {
        fn wait_for_size(p: &PanelState, dir: &Path, expected: u64) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if p.dir_sizes.lock().unwrap().get(dir) == Some(&expected) {
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
        p.sizes_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
        p.last_sizes_recompute = None; // bypass the debounce in the test
        reset_walk_log(); // bypass the walk cooldown in the test

        let reloaded = p.poll_fs_changes();
        assert!(!reloaded, "sizes-only events must not reload the listing");
        wait_for_size(&p, &sub, 12);
    }

    #[test]
    fn watcher_gap_replaces_the_listing_and_applies_its_generation() {
        let tmp = TempDir::new();
        tmp.file("before.txt", "before");
        let mut panel = PanelState::new(tmp.path().to_path_buf());
        panel.refresh();
        assert!(panel.entries.iter().any(|entry| entry.name == "before.txt"));

        std::fs::remove_file(tmp.path().join("before.txt")).unwrap();
        tmp.file("after.txt", "after");
        flag_watcher_gap(&panel.watcher_rescan_generation, &panel.needs_refresh);

        assert!(panel.poll_fs_changes());
        assert!(panel.entries.iter().any(|entry| entry.name == "after.txt"));
        assert!(!panel.entries.iter().any(|entry| entry.name == "before.txt"));
        assert_eq!(
            panel.applied_rescan_generation,
            panel
                .watcher_rescan_generation
                .load(std::sync::atomic::Ordering::Relaxed)
        );
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
    fn make_preview_reads_text_and_skips_dirs() {
        let tmp = TempDir::new();
        let file = tmp.file("note.txt", "hello");
        let meta = std::fs::metadata(&file).unwrap();
        let fe = FileEntry::from_meta(file, &meta).unwrap();
        match make_preview(&fe) {
            Some(PreviewContent::Text { content, .. }) => assert_eq!(content, "hello"),
            other => panic!("expected text preview, got {:?}", other.is_some()),
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
