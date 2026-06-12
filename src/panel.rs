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

/// Save current cache to disk (best-effort, non-blocking).
pub fn flush_cache() {
    let Ok(cache) = dir_size_cache().lock() else {
        return;
    };
    let entries: HashMap<&PathBuf, CacheEntry> = cache
        .iter()
        .filter_map(|(p, (mtime, size))| {
            let dur = mtime.duration_since(std::time::UNIX_EPOCH).ok()?;
            Some((
                p,
                CacheEntry {
                    mtime_secs: dur.as_secs(),
                    mtime_nanos: dur.subsec_nanos(),
                    size: *size,
                },
            ))
        })
        .collect();
    if let Ok(json) = serde_json::to_string(&entries) {
        let _ = fs::write(cache_path(), json);
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
}

impl FileEntry {
    pub fn from_path(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        let name = path.file_name()?.to_string_lossy().to_string();
        let is_dir = meta.is_dir();
        let size = if is_dir { 0 } else { meta.len() };
        let extension = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let modified = meta.modified().ok();

        let name_lower = name.to_lowercase();
        Some(FileEntry {
            name,
            name_lower,
            path: path.to_path_buf(),
            is_dir,
            size,
            extension,
            modified,
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

    pub fn is_text(&self) -> bool {
        matches!(
            self.extension.as_str(),
            "txt" | "md" | "rs" | "py" | "js" | "ts" | "jsx" | "tsx"
            | "html" | "css" | "scss" | "json" | "toml" | "yaml" | "yml" | "xml"
            | "sh" | "bash" | "zsh" | "fish" | "swift" | "go" | "java" | "kt"
            | "c" | "cpp" | "h" | "hpp" | "cs" | "rb" | "php" | "sql"
            | "log" | "csv" | "ini" | "cfg" | "conf" | "env"
            | "lock" | "gitignore" | "dockerfile" | "makefile"
        ) || self.name.starts_with('.')
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

    pub fn size_display(&self) -> String {
        if self.is_dir {
            return "…".to_string();
        }
        format_size(self.size)
    }

    pub fn size_display_with_dir_size(&self, dir_sizes: &HashMap<PathBuf, u64>) -> String {
        if self.is_dir {
            if let Some(&size) = dir_sizes.get(&self.path) {
                return format_size(size);
            }
            return "…".to_string();
        }
        format_size(self.size)
    }

    pub fn modified_display(&self) -> String {
        let Some(time) = self.modified else {
            return "—".to_string();
        };
        let datetime: chrono::DateTime<chrono::Local> = time.into();
        datetime.format("%d %b %y  %H:%M").to_string()
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

#[derive(Debug, Clone)]
pub enum ContextAction {
    Open(PathBuf),
    RevealInFinder(PathBuf),
    CopySelected,
    MoveSelected,
    DeleteSelected,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PreviewContent {
    Image(PathBuf),
    Text { path: PathBuf, content: String },
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
    pub selected: std::collections::HashSet<usize>,
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
    pub ctx: Option<egui::Context>,
    pub pending_action: Option<ContextAction>,
    pub drag_entries: Vec<PathBuf>,
    pub drop_target: Option<PathBuf>,
    pub show_tree: bool,
    pub tree_expanded: std::collections::HashSet<PathBuf>,
    pub tree_width: f32,
    pub tree_children_cache: HashMap<PathBuf, Vec<PathBuf>>,
    pub needs_refresh: Arc<Mutex<bool>>,
    watcher: Option<notify::RecommendedWatcher>,
    watched_path: Option<PathBuf>,
}

impl PanelState {
    pub fn new(path: PathBuf) -> Self {
        let mut panel = PanelState {
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
            ctx: None,
            pending_action: None,
            drag_entries: Vec::new(),
            drop_target: None,
            show_tree: false,
            tree_expanded: std::collections::HashSet::new(),
            tree_width: 180.0,
            tree_children_cache: HashMap::new(),
            needs_refresh: Arc::new(Mutex::new(false)),
            watcher: None,
            watched_path: None,
        };
        panel.refresh();
        panel
    }

    pub fn set_ctx(&mut self, ctx: egui::Context) {
        self.ctx = Some(ctx);
    }

    pub fn refresh(&mut self) {
        self.entries = Self::read_dir(&self.current_path, self.show_hidden);
        self.sort_entries();
        self.selected.clear();
        if self.cursor >= self.entries.len() {
            self.cursor = self.entries.len().saturating_sub(1);
        }
        self.invalidate_tree_cache();
        self.compute_dir_sizes();
        self.start_watcher();
    }

    /// Check if fs watcher flagged a change; if so, refresh.
    pub fn poll_fs_changes(&mut self) {
        let should = {
            let mut flag = self.needs_refresh.lock().unwrap();
            if *flag {
                *flag = false;
                true
            } else {
                false
            }
        };
        if should {
            self.entries = Self::read_dir(&self.current_path, self.show_hidden);
            self.sort_entries();
            if self.cursor >= self.entries.len() {
                self.cursor = self.entries.len().saturating_sub(1);
            }
            self.compute_dir_sizes();
        }
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
        let ctx = self.ctx.clone();

        let watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(_event) = res {
                if let Ok(mut f) = flag.lock() {
                    *f = true;
                }
                if let Some(ctx) = &ctx {
                    ctx.request_repaint();
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
        let ctx1 = self.ctx.clone();
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
            if let Some(ctx) = ctx1 {
                ctx.request_repaint();
            }
        });

        // Dir sizes
        if !need_size.is_empty() {
            let sizes = Arc::clone(&self.dir_sizes);
            let ctx2 = self.ctx.clone();
            fs_pool().spawn(move || {
                use rayon::prelude::*;
                let results: Vec<_> = fs_pool().install(|| {
                    need_size
                        .par_iter()
                        .map(|(p, mt)| (p.clone(), *mt, dir_size_recursive(p)))
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
                if let Some(ctx) = ctx2 {
                    ctx.request_repaint();
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
                let path = e.path();
                let meta = e.metadata().ok()?;
                let name = path.file_name()?.to_string_lossy().to_string();
                let is_dir = meta.is_dir();
                let size = if is_dir { 0 } else { meta.len() };
                let extension = path
                    .extension()
                    .map(|ext| ext.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                let modified = meta.modified().ok();
                let name_lower = name.to_lowercase();
                Some(FileEntry {
                    name,
                    name_lower,
                    path: path.to_path_buf(),
                    is_dir,
                    size,
                    extension,
                    modified,
                })
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
                SortColumn::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
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

    pub fn enter_selected(&mut self) {
        if let Some(entry) = self.filtered_entries().get(self.cursor).cloned() {
            if entry.is_dir {
                self.navigate_to(entry.path.clone());
            } else {
                let _ = open::that(&entry.path);
            }
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

    pub fn toggle_select(&mut self, idx: usize) {
        if self.selected.contains(&idx) {
            self.selected.remove(&idx);
        } else {
            self.selected.insert(idx);
        }
    }

    pub fn select_all(&mut self) {
        let count = self.filtered_entries().len();
        if self.selected.len() == count {
            self.selected.clear();
        } else {
            self.selected = (0..count).collect();
        }
    }

    pub fn selected_entries(&self) -> Vec<FileEntry> {
        let filtered = self.filtered_entries();
        self.selected
            .iter()
            .filter_map(|&i| filtered.get(i).cloned().cloned())
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

    /// Get cached subdirs for a path. Loads from disk on first access.
    pub fn tree_subdirs_cached(&mut self, path: &Path) -> Vec<PathBuf> {
        if let Some(cached) = self.tree_children_cache.get(path) {
            return cached.clone();
        }
        let dirs = Self::subdirs(path, self.show_hidden);
        self.tree_children_cache.insert(path.to_path_buf(), dirs.clone());
        dirs
    }

    /// Clear tree cache (on refresh / fs change).
    pub fn invalidate_tree_cache(&mut self) {
        self.tree_children_cache.clear();
    }

    /// Auto-expand tree nodes along the path to current_path.
    pub fn tree_expand_to_current(&mut self) {
        let mut p = self.current_path.clone();
        loop {
            self.tree_expanded.insert(p.clone());
            match p.parent() {
                Some(parent) if parent != p => p = parent.to_path_buf(),
                _ => break,
            }
        }
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

fn dir_size_recursive(path: &Path) -> u64 {
    jwalk::WalkDir::new(path)
        .skip_hidden(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| !m.is_dir())
        .map(|m| m.len())
        .sum()
}
