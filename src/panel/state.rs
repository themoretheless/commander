//! Core state for a panel. Extracted for SRP: keeps the data model separate
//! from logic (which lives in submodules like nav, selection, etc.).
//! This makes PanelState easier to understand in small pieces.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::super::panel::{DirStatus, FacetSet, FileEntry, FilterCache, PreviewContent, SortColumn, SortOrder};

pub struct PanelState {
    // Core data: prefer accessors over direct mutation. pub(crate) so crate internals see, UI uses fns.
    pub(crate) current_path: PathBuf,
    pub(crate) entries: Vec<FileEntry>,
    /// Selected entries, keyed by path so selection survives
    /// filtering, sorting and directory refreshes.
    pub(crate) selected: std::collections::HashSet<PathBuf>,
    pub(crate) cursor: usize,
    pub(crate) scroll_to_cursor: bool,
    /// Why the current listing is empty/non-empty (for the empty-state UI).
    pub(crate) dir_status: DirStatus,
    /// Visible rows in the list viewport, set by the renderer each frame and
    /// read by PageUp/PageDown. Zero until the panel has been drawn once.
    pub(crate) page_rows: usize,
    pub(crate) preview: Option<PreviewContent>,
    /// Height of the bottom preview area when open (for resizable non-replacing preview per designer iter3).
    pub(crate) preview_height: Option<f32>,
    /// Show hex dump instead of text in preview (idea #98).
    pub(crate) preview_hex: bool,
    /// Cached bytes for current preview when in hex mode (avoid re-read every frame in render).
    pub(crate) preview_hex_bytes: Option<Vec<u8>>,
    pub(crate) history: Vec<PathBuf>,
    pub(crate) history_pos: usize,
    pub(crate) search_query: String,
    /// If true, treat search_query as regex (case-insensitive); else fuzzy subsequence.
    pub(crate) search_regex: bool,
    /// Active quick-filter facets, ANDed with the substring filter.
    pub(crate) facets: FacetSet,
    pub(crate) sort_col: SortColumn,
    pub(crate) sort_order: SortOrder,
    pub(crate) show_hidden: bool,
    /// Git status letter for each path in this listing ('M' modified, 'A' added, '?' untracked, etc). Refreshed on reload if .git present.
    /// Owned now, updates come via channel or direct (moving toward ownership + channels).
    pub(crate) git_status: HashMap<PathBuf, char>,
    pub(crate) last_git_refresh: Option<Instant>,
    pub dir_sizes: Arc<Mutex<HashMap<PathBuf, u64>>>,
    pub dir_counts: Arc<Mutex<HashMap<PathBuf, usize>>>,
    pub notify: Option<Arc<dyn Fn() + Send + Sync>>,
    pub(crate) drag_entries: Vec<PathBuf>,
    pub(crate) drop_target: Option<PathBuf>,
    pub(crate) needs_refresh: Arc<std::sync::atomic::AtomicBool>,
    /// Set by deep watcher events: directory sizes need recomputing,
    /// but the listing itself is unchanged.
    pub(crate) sizes_dirty: Arc<std::sync::atomic::AtomicBool>,
    pub(crate) last_sizes_recompute: Option<Instant>,
    /// Bumped whenever `entries` content or order changes.
    pub(crate) entries_gen: u64,
    pub(crate) filter_cache: std::cell::RefCell<FilterCache>,
    pub(crate) watcher: Option<notify::RecommendedWatcher>,
    pub(crate) watched_path: Option<PathBuf>,
    /// Sender for git updates to avoid shared Arc<Mutex>, use channel from App.
    pub(crate) git_tx: Option<tokio::sync::mpsc::UnboundedSender<(PathBuf, HashMap<PathBuf, char>)>>,
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
            dir_status: DirStatus::Empty,
            page_rows: 0,
            preview: None,
            preview_height: None,
            preview_hex: false,
            preview_hex_bytes: None,
            history: vec![path],
            history_pos: 0,
            search_query: String::new(),
            search_regex: false,
            facets: FacetSet::default(),
            sort_col: SortColumn::Name,
            sort_order: SortOrder::Asc,
            show_hidden: false,
            git_status: HashMap::new(),
            last_git_refresh: None,
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
            git_tx: None,
        }
    }

    // Accessors for better encapsulation (reduces direct pub field mutation from UI layers)
    pub fn cursor(&self) -> usize { self.cursor }
    pub fn set_cursor(&mut self, c: usize) { self.cursor = c; }
    pub fn scroll_to_cursor(&self) -> bool { self.scroll_to_cursor }
    pub fn set_scroll_to_cursor(&mut self, v: bool) { self.scroll_to_cursor = v; }

    // Convenience for common patterns (linked scroll, playback)
    pub fn set_cursor_and_scroll(&mut self, c: usize) {
        self.cursor = c;
        self.scroll_to_cursor = true;
    }

    pub fn search_query(&self) -> &str { &self.search_query }
    pub fn set_search(&mut self, q: String, regex: bool) {
        self.search_query = q;
        self.search_regex = regex;
    }

    pub fn preview(&self) -> Option<&PreviewContent> { self.preview.as_ref() }
    pub fn set_preview(&mut self, p: Option<PreviewContent>, h: Option<f32>) {
        let is_none = p.is_none();
        self.preview = p;
        self.preview_height = h;
        if is_none {
            self.preview_hex = false;
            self.preview_hex_bytes = None;
        }
    }

    pub fn preview_hex(&self) -> bool { self.preview_hex }
    pub fn set_preview_hex(&mut self, hex: bool) { self.preview_hex = hex; }

    // More accessors for encapsulation (reduce direct pub mutation from UI and cross modules)
    pub fn facets(&self) -> &FacetSet { &self.facets }
    pub fn facets_mut(&mut self) -> &mut FacetSet { &mut self.facets }
    pub fn set_facets(&mut self, f: FacetSet) { self.facets = f; }

    pub fn search_regex(&self) -> bool { self.search_regex }

    pub fn sort_col(&self) -> SortColumn { self.sort_col }
    pub fn set_sort_col(&mut self, c: SortColumn) { self.sort_col = c; }

    pub fn sort_order(&self) -> SortOrder { self.sort_order }
    pub fn set_sort_order(&mut self, o: SortOrder) { self.sort_order = o; }

    pub fn show_hidden(&self) -> bool { self.show_hidden }
    pub fn set_show_hidden(&mut self, h: bool) { self.show_hidden = h; }

    pub fn history(&self) -> &[PathBuf] { &self.history }
    pub fn history_pos(&self) -> usize { self.history_pos }
    pub fn set_history_pos(&mut self, p: usize) { self.history_pos = p; }
    pub fn truncate_history(&mut self, len: usize) { self.history.truncate(len); }
    pub fn push_history(&mut self, p: PathBuf) { self.history.push(p); }

    pub fn selected(&self) -> &std::collections::HashSet<PathBuf> { &self.selected }
    pub fn selected_mut(&mut self) -> &mut std::collections::HashSet<PathBuf> { &mut self.selected }

    pub fn entries(&self) -> &[FileEntry] { &self.entries }
    pub fn entries_mut(&mut self) -> &mut Vec<FileEntry> { &mut self.entries }
    pub fn set_entries(&mut self, e: Vec<FileEntry>) { self.entries = e; }

    pub fn page_rows(&self) -> usize { self.page_rows }
    pub fn set_page_rows(&mut self, r: usize) { self.page_rows = r; }

    pub fn drag_entries(&self) -> &[PathBuf] { &self.drag_entries }
    pub fn drag_entries_mut(&mut self) -> &mut Vec<PathBuf> { &mut self.drag_entries }

    pub fn drop_target(&self) -> Option<&PathBuf> { self.drop_target.as_ref() }
    pub fn set_drop_target(&mut self, t: Option<PathBuf>) { self.drop_target = t; }

    pub fn current_path(&self) -> &PathBuf { &self.current_path }
    pub fn set_current_path(&mut self, p: PathBuf) { self.current_path = p; }
    pub fn dir_status(&self) -> DirStatus { self.dir_status }
    pub fn set_dir_status(&mut self, d: DirStatus) { self.dir_status = d; }

    pub fn git_status(&self) -> &HashMap<PathBuf, char> { &self.git_status }
    pub fn set_git_status(&mut self, m: HashMap<PathBuf, char>) { self.git_status = m; }

    pub fn preview_content(&self) -> Option<&PreviewContent> { self.preview.as_ref() }
    pub fn set_preview_content(&mut self, p: Option<PreviewContent>, h: Option<f32>) {
        let clearing = p.is_none();
        self.preview = p;
        self.preview_height = h;
        if clearing {
            self.preview_hex = false;
            self.preview_hex_bytes = None;
        }
    }

    pub fn preview_hex_bytes(&self) -> Option<&Vec<u8>> { self.preview_hex_bytes.as_ref() }
    pub fn set_preview_hex_bytes(&mut self, b: Option<Vec<u8>>) { self.preview_hex_bytes = b; }

    pub fn set_preview_height(&mut self, h: Option<f32>) { self.preview_height = h; }

    // Note: other methods like set_notify, refresh, etc. are delegated
    // from the main panel.rs or submodules for DRY/SRP.
}