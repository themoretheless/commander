use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::OnceLock;
use std::time::SystemTime;
use serde::{Serialize, Deserialize};

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
            let mtime = std::time::UNIX_EPOCH
                + std::time::Duration::new(e.mtime_secs, e.mtime_nanos);
            (p, (mtime, e.size))
        })
        .collect()
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
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico"
            | "heic" | "heif" | "tiff" | "tif"
            | "dng" | "cr2" | "cr3" | "nef" | "arw" | "orf" | "raf" | "rw2" | "pef" | "srw"
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
        if self.is_dir {
            if let Some(&size) = dir_sizes.get(&self.path) {
                return format_size(size);
            }
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
        let Ok(meta) = fs::metadata(&entry.path) else { return None };
        if meta.len() > 1024 * 1024 {
            return None; // Too large
        }
        let Ok(content) = fs::read_to_string(&entry.path) else { return None };
        Some(PreviewContent::Text {
            path: entry.path.clone(),
            content,
        })
    }
}

/// Wake-up callback into the UI (e.g. a repaint request). Panels never
/// talk to the UI toolkit directly.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

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
        self.compute_dir_sizes();
        self.start_watcher();
    }

    /// Re-read the directory, preserving selection and cursor position
    /// by path (entries may have been added, removed or re-sorted).
    fn reload_entries(&mut self) {
        let cursor_path = if self.cursor > 0 {
            self.filtered_entries()
                .get(self.cursor - 1)
                .map(|e| e.path.clone())
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
            None => self.cursor = self.cursor.min(self.filtered_entries().len()),
        }
    }

    /// Check if fs watcher flagged a change; if so, refresh.
    /// Returns `true` when the directory was re-read.
    pub fn poll_fs_changes(&mut self) -> bool {
        let should = self
            .needs_refresh
            .swap(false, std::sync::atomic::Ordering::Relaxed);
        if should {
            self.reload_entries();
            self.compute_dir_sizes();
        }
        should
    }

    fn start_watcher(&mut self) {
        use notify::{Watcher, RecursiveMode, Event};

        // Skip if already watching this path
        if self.watched_path.as_ref() == Some(&self.current_path) {
            return;
        }

        // Drop old watcher
        self.watcher = None;
        self.watched_path = None;

        let flag = Arc::clone(&self.needs_refresh);
        let wake = self.notify.clone();

        let watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if res.is_ok() {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
                if let Some(wake) = &wake {
                    wake();
                }
            }
        });

        if let Ok(mut w) = watcher {
            let _ = w.watch(&self.current_path, RecursiveMode::NonRecursive);
            self.watched_path = Some(self.current_path.clone());
            self.watcher = Some(w);
        }
    }

    fn compute_dir_sizes(&self) {
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

        for entry in &self.entries {
            if !entry.is_dir {
                continue;
            }

            need_count.push(entry.path.clone());

            let dir_mtime = fs::metadata(&entry.path)
                .and_then(|m| m.modified())
                .ok();

            // Check global cache: if mtime matches, reuse cached size
            if let Some(mtime) = dir_mtime {
                if let Ok(cache) = dir_size_cache().lock() {
                    if let Some(&(cached_mtime, cached_size)) = cache.get(&entry.path) {
                        if cached_mtime == mtime {
                            if let Ok(mut sizes) = self.dir_sizes.lock() {
                                sizes.insert(entry.path.clone(), cached_size);
                            }
                            continue;
                        }
                    }
                }
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
                        .map(|(p, mt)| (p.clone(), *mt, crate::fs_util::dir_size_recursive(p)))
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
                // name_lower is precomputed at load: no per-comparison allocs.
                SortColumn::Name => a.name_lower.cmp(&b.name_lower),
                SortColumn::Size => a.size.cmp(&b.size),
                SortColumn::Modified => a.modified.cmp(&b.modified),
            };

            match order {
                SortOrder::Asc => cmp,
                SortOrder::Desc => cmp.reverse(),
            }
        });
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
        if let Some(parent) = self.current_path.parent().map(|p| p.to_path_buf()) {
            self.navigate_to(parent);
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

    pub fn filtered_entries(&self) -> Vec<&FileEntry> {
        if self.search_query.is_empty() {
            self.entries.iter().collect()
        } else {
            let q = self.search_query.to_lowercase();
            self.entries
                .iter()
                .filter(|e| e.name_lower.contains(&q))
                .collect()
        }
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
            let file_idx = self.cursor - 1;
            let filtered = self.filtered_entries();
            if let Some(entry) = filtered.get(file_idx) {
                vec![(*entry).clone()]
            } else {
                vec![]
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
        let computed = self.entries.iter().filter(|e| e.is_dir).filter_map(|e| sizes.get(&e.path)).count();
        if computed == 0 && dir_count > 0 {
            return None;
        }
        let file_total: u64 = self.entries.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
        let dir_total: u64 = self.entries.iter().filter(|e| e.is_dir).filter_map(|e| sizes.get(&e.path).copied()).sum();
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
            size_str: if is_dir { "…".to_string() } else { format_size(size) },
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
        let mut p = panel_with(vec![
            entry("a.txt", false, 1),
            entry("b.txt", false, 2),
        ]);
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
        let names: Vec<&str> = p.filtered_entries().iter().map(|e| e.name.as_str()).collect();
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
    fn breadcrumbs_start_at_root() {
        let p = PanelState::new(PathBuf::from("/tmp/foo"));
        let crumbs = p.breadcrumbs();
        assert_eq!(crumbs[0].0, "/");
        assert_eq!(crumbs.last().unwrap().0, "foo");
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

