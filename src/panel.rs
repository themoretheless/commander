use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// On-disk entry: mtime as seconds+nanos since UNIX epoch, and size.
#[derive(Serialize, Deserialize)]
struct CacheEntry {
    mtime_secs: u64,
    mtime_nanos: u32,
    size: u64,
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
fn dir_size_cache() -> &'static Mutex<HashMap<PathBuf, (SystemTime, u64)>> {
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
                std::time::UNIX_EPOCH + std::time::Duration::new(e.mtime_secs, e.mtime_nanos);
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
fn walk_log() -> &'static Mutex<HashMap<PathBuf, (std::time::Instant, std::time::Duration)>> {
    static LOG: OnceLock<Mutex<HashMap<PathBuf, (std::time::Instant, std::time::Duration)>>> =
        OnceLock::new();
    LOG.get_or_init(|| Mutex::new(HashMap::new()))
}

const WALK_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10);
const WALK_EXPENSIVE: std::time::Duration = std::time::Duration::from_secs(2);

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
}

/// Create preview content for a file entry (image marker or text body).
pub fn make_preview(entry: &FileEntry) -> Option<PreviewContent> {
    if entry.is_dir {
        return None;
    }
    if entry.is_image() {
        Some(PreviewContent::Image(entry.path.clone()))
    } else {
        // Try to read as text (limit to 1MB)
        let Ok(meta) = fs::metadata(&entry.path) else {
            return None;
        };
        if meta.len() > 1024 * 1024 {
            return None; // Too large
        }
        let Ok(content) = fs::read_to_string(&entry.path) else {
            return None;
        };
        Some(PreviewContent::Text {
            path: entry.path.clone(),
            content,
        })
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

/// Cached filtered view: indices into `entries` matching `query`.
/// Valid while `generation` matches the panel's `entries_gen` and the
/// query is unchanged; recomputed lazily otherwise.
struct FilterCache {
    generation: u64,
    query: String,
    indices: Vec<usize>,
}

impl FilterCache {
    fn stale() -> Self {
        FilterCache {
            generation: u64::MAX, // sentinel: never computed
            query: String::new(),
            indices: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortColumn {
    Name,
    Size,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortOrder {
    Asc,
    Desc,
}

pub struct PanelState {
    pub current_path: PathBuf,
    pub entries: Vec<FileEntry>,
    /// Selected entries, keyed by path so selection survives
    /// filtering, sorting and directory refreshes.
    pub selected: std::collections::HashSet<PathBuf>,
    pub cursor: usize,
    pub scroll_to_cursor: bool,
    /// Visible rows in the list viewport, set by the renderer each frame and
    /// read by PageUp/PageDown. Zero until the panel has been drawn once.
    pub page_rows: usize,
    pub preview: Option<PreviewContent>,
    pub history: Vec<PathBuf>,
    pub history_pos: usize,
    pub search_query: String,
    pub sort_col: SortColumn,
    pub sort_order: SortOrder,
    pub show_hidden: bool,
    pub dir_sizes: Arc<Mutex<HashMap<PathBuf, u64>>>,
    pub dir_counts: Arc<Mutex<HashMap<PathBuf, usize>>>,
    notify: Option<Notify>,
    pub drag_entries: Vec<PathBuf>,
    pub drop_target: Option<PathBuf>,
    pub needs_refresh: Arc<std::sync::atomic::AtomicBool>,
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
            cursor: 0,
            scroll_to_cursor: false,
            page_rows: 0,
            preview: None,
            history: vec![path],
            history_pos: 0,
            search_query: String::new(),
            sort_col: SortColumn::Name,
            sort_order: SortOrder::Asc,
            show_hidden: false,
            dir_sizes: Arc::new(Mutex::new(HashMap::new())),
            dir_counts: Arc::new(Mutex::new(HashMap::new())),
            notify: None,
            drag_entries: Vec::new(),
            drop_target: None,
            needs_refresh: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        self.sort_entries();

        {
            let existing: std::collections::HashSet<&PathBuf> =
                self.entries.iter().map(|e| &e.path).collect();
            self.selected.retain(|p| existing.contains(p));
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
        let wake = self.notify.clone();
        let watched = self.current_path.clone();

        let watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
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
        let mut need_size: Vec<(PathBuf, Option<SystemTime>)> = Vec::new();
        let mut retry = false;

        for entry in &self.entries {
            if !entry.is_dir {
                continue;
            }

            need_count.push(entry.path.clone());

            let dir_mtime = fs::metadata(&entry.path).and_then(|m| m.modified()).ok();

            // Check global cache: if mtime matches, reuse cached size
            if let Some(mtime) = dir_mtime
                && let Ok(cache) = dir_size_cache().lock()
                && let Some(&(cached_mtime, cached_size)) = cache.get(&entry.path)
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
                && let Some(&(when, cost)) = log.get(&entry.path)
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
                    && let Some(&(_, cached_size)) = cache.get(&entry.path)
                    && let Ok(mut sizes) = self.dir_sizes.lock()
                {
                    sizes.insert(entry.path.clone(), cached_size);
                }
                continue;
            }

            need_size.push((entry.path.clone(), dir_mtime));
        }

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
        fs_pool().spawn(move || {
            use rayon::prelude::*;
            let results: Vec<_> = fs_pool().install(|| {
                need_count
                    .par_iter()
                    .map(|p| {
                        let count = fs::read_dir(p)
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

        // Dir sizes
        if !need_size.is_empty() {
            let sizes = Arc::clone(&self.dir_sizes);
            let wake2 = self.notify.clone();
            fs_pool().spawn(move || {
                use rayon::prelude::*;
                let results: Vec<_> = fs_pool().install(|| {
                    need_size
                        .par_iter()
                        .map(|(p, mt)| {
                            let started = std::time::Instant::now();
                            let size = crate::fs_util::dir_size_recursive(p);
                            if let Ok(mut log) = walk_log().lock() {
                                log.insert(
                                    p.clone(),
                                    (std::time::Instant::now(), started.elapsed()),
                                );
                            }
                            (p.clone(), *mt, size)
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
                        if let Some(mt) = mt {
                            cache.insert(p.clone(), (*mt, *size));
                        }
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

        self.entries.sort_by(|a, b| {
            // Dirs always first
            match (a.is_dir, b.is_dir) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => {}
            }

            let cmp = match col {
                // Natural order over the precomputed lowercase name, so
                // "file2" sorts before "file10".
                SortColumn::Name => natural_cmp(&a.name_lower, &b.name_lower),
                SortColumn::Size => a.size.cmp(&b.size),
                SortColumn::Modified => a.modified.cmp(&b.modified),
            };

            match order {
                SortOrder::Asc => cmp,
                SortOrder::Desc => cmp.reverse(),
            }
        });
        // Content/order changed: filtered indices must be rebuilt.
        self.entries_gen = self.entries_gen.wrapping_add(1);
    }

    pub fn navigate_to(&mut self, path: PathBuf) {
        // Trim forward history when navigating to a new path
        if self.history_pos + 1 < self.history.len() {
            self.history.truncate(self.history_pos + 1);
        }
        self.history.push(path.clone());
        self.history_pos = self.history.len() - 1;
        self.current_path = path;
        self.search_query.clear();
        self.refresh();
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
        self.history_pos > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_pos + 1 < self.history.len()
    }

    pub fn go_back(&mut self) {
        if self.can_go_back() {
            self.history_pos -= 1;
            self.current_path = self.history[self.history_pos].clone();
            self.search_query.clear();
            self.refresh();
        }
    }

    pub fn go_forward(&mut self) {
        if self.can_go_forward() {
            self.history_pos += 1;
            self.current_path = self.history[self.history_pos].clone();
            self.search_query.clear();
            self.refresh();
        }
    }

    /// Rebuild the cached filtered indices if entries or query changed.
    /// A warm cache costs two comparisons; the string matching over all
    /// entries runs only when something actually changed.
    fn ensure_filter_cache(&self) {
        let mut cache = self.filter_cache.borrow_mut();
        if cache.generation == self.entries_gen && cache.query == self.search_query {
            return;
        }
        cache.generation = self.entries_gen;
        cache.query = self.search_query.clone();
        cache.indices.clear();
        if self.search_query.is_empty() {
            cache.indices.extend(0..self.entries.len());
        } else {
            let q = self.search_query.to_lowercase();
            cache.indices.extend(
                self.entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.name_lower.contains(&q))
                    .map(|(i, _)| i),
            );
        }
    }

    /// Number of entries matching the current filter (no allocation).
    pub fn filtered_count(&self) -> usize {
        self.ensure_filter_cache();
        self.filter_cache.borrow().indices.len()
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
        self.filter_cache.borrow().indices.clone()
    }

    pub fn filtered_entries(&self) -> Vec<&FileEntry> {
        self.ensure_filter_cache();
        let cache = self.filter_cache.borrow();
        cache.indices.iter().map(|&i| &self.entries[i]).collect()
    }

    pub fn toggle_select(&mut self, path: PathBuf) {
        if !self.selected.remove(&path) {
            self.selected.insert(path);
        }
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

    pub fn selected_entries(&self) -> Vec<FileEntry> {
        self.filtered_entries()
            .into_iter()
            .filter(|e| self.selected.contains(&e.path))
            .cloned()
            .collect()
    }

    pub fn selected_or_cursor(&self) -> Vec<FileEntry> {
        if self.selected.is_empty() {
            // cursor 0 = ".." row, real files start at cursor 1
            if self.cursor == 0 {
                return vec![];
            }
            match self.filtered_get(self.cursor - 1) {
                Some(entry) => vec![entry.clone()],
                None => vec![],
            }
        } else {
            self.selected_entries()
        }
    }

    pub fn breadcrumbs(&self) -> Vec<(String, PathBuf)> {
        let mut segments = Vec::new();
        let mut current = self.current_path.clone();
        loop {
            let name = current
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());
            segments.push((name, current.clone()));
            match current.parent() {
                Some(parent) if parent != current => current = parent.to_path_buf(),
                _ => break,
            }
        }
        segments.reverse();
        segments
    }

    pub fn total_size_selected(&self) -> u64 {
        let sizes = self.dir_sizes.lock().ok();
        self.selected_entries()
            .iter()
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

    pub fn total_dir_size(&self) -> Option<u64> {
        let sizes = self.dir_sizes.lock().ok()?;
        let dir_count = self.entries.iter().filter(|e| e.is_dir).count();
        let computed = self
            .entries
            .iter()
            .filter(|e| e.is_dir)
            .filter_map(|e| sizes.get(&e.path))
            .count();
        if computed == 0 && dir_count > 0 {
            return None;
        }
        let file_total: u64 = self
            .entries
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.size)
            .sum();
        let dir_total: u64 = self
            .entries
            .iter()
            .filter(|e| e.is_dir)
            .filter_map(|e| sizes.get(&e.path).copied())
            .sum();
        Some(file_total + dir_total)
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
    fn toggle_select_adds_then_removes() {
        let mut p = panel_with(vec![entry("a", false, 1)]);
        let path = p.entries[0].path.clone();
        p.toggle_select(path.clone());
        assert!(p.selected.contains(&path));
        p.toggle_select(path.clone());
        assert!(!p.selected.contains(&path));
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
    fn selected_or_cursor_falls_back_to_cursor_row() {
        let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
        p.cursor = 0; // ".." row
        assert!(p.selected_or_cursor().is_empty());
        p.cursor = 2; // second file
        let picked = p.selected_or_cursor();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].name, "b");
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

        // Cursor on "b.txt" (row 2), select "c.txt".
        p.cursor = 2;
        p.toggle_select(doomed.clone());

        // A new file shifts sort order; a selected file disappears.
        tmp.file("0-first.txt", "0");
        std::fs::remove_file(&doomed).unwrap();
        p.refresh();

        let under_cursor = p.filtered_entries()[p.cursor - 1].path.clone();
        assert!(under_cursor.ends_with("b.txt"), "cursor follows the path");
        assert!(p.selected.is_empty(), "selection drops deleted paths");
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
    fn select_cursor_adds_current_row() {
        let mut p = panel_with(vec![entry("a", false, 1), entry("b", false, 1)]);
        p.cursor = 2; // second file
        p.select_cursor();
        assert!(p.selected.contains(&p.entries[1].path));
        assert_eq!(p.selected.len(), 1);
    }

    #[test]
    fn breadcrumbs_start_at_root() {
        let p = PanelState::new(PathBuf::from("/tmp/foo"));
        let crumbs = p.breadcrumbs();
        assert_eq!(crumbs[0].0, "/");
        assert_eq!(crumbs.last().unwrap().0, "foo");
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
        let now = SystemTime::now();
        {
            let mut cache = dir_size_cache().lock().unwrap();
            cache.insert(a.clone(), (now, 100));
            cache.insert(other.clone(), (now, 5));
        }

        invalidate_size_cache(&a.join("b/c/file.txt"));

        let cache = dir_size_cache().lock().unwrap();
        let (a_mtime, a_size) = cache[&a];
        assert_eq!(a_mtime, std::time::UNIX_EPOCH, "ancestor mtime is reset");
        assert_eq!(a_size, 100, "stale size is kept for display");
        assert_eq!(cache[&other].0, now, "unrelated dirs stay valid");
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
}
