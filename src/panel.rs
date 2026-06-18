use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

mod entry;
mod filter;
mod sizes;
mod nav;
mod fs;
mod watcher;
mod columns;
mod sort;
mod recent;
mod selection;
mod pane;
mod state;

pub use entry::{FileEntry, format_size};
pub use filter::{glob_match, parse_mask, KindFacet, FacetSet, facet_matches};
pub use sizes::entry_display_size;
pub use nav::{go_back, go_forward, go_up, navigate_to, type_ahead};
pub use fs::{classify_dir, DirStatus};
pub use columns::{active_columns, column_width, ColumnConfig, FileColumn, GitColumn, NameColumn};
pub use sort::{set_sort, sort_entries, sort_indicator, SortColumn, SortOrder};
pub use recent::{filter_visited, push_visit, record_visit, visited_paths};
pub use selection::{select_all, select_by_mask, selected_entries, selected_or_cursor, toggle_select, total_size_selected};
pub use pane::Pane;
pub(crate) use filter::FilterCache;
pub(crate) use filter::term_matches;
pub(crate) use fs::read_dir;
pub(crate) use sizes::{dir_size_cache, walk_log, WALK_COOLDOWN, WALK_EXPENSIVE, fs_pool, invalidate_size_cache, flush_cache};
pub use watcher::poll_fs_changes;
pub(crate) use watcher::start_watcher;

// Recent/visited logic moved to src/panel/recent.rs (SRP for Cmd+P history, separate from panel nav).

// DirStatus, classify_dir now come from pub use fs::... (extracted to panel/fs.rs for SRP)

// Structure note (SOLID/DRY split for understandability in small pieces):
// panel/ now has focused modules:
// - entry.rs (row model + icons/sizes)
// - filter.rs (facets, glob, mask matching, cache)
// - sizes.rs (dir size caches, walk guards, fs_pool, flush/invalidate)
// - nav.rs (history, navigate, go up/back/forward, type-ahead)
// - fs.rs (read_dir, classify, subdirs)
// - watcher.rs (notify watcher + poll for reload/sizes)
// - columns.rs (FileColumn trait + Git/Name impls for future customization)
// - sort.rs
// - recent.rs
// - selection.rs
// Main panel.rs is thinner coordinator + PanelState.
// Compare to other editors: this mirrors how mc/FAR/TC split their "panel model" concerns.
// Reexports keep everything working.

// === TOP 100 IDEAS, SUGGESTIONS AND PROBLEM SOLUTIONS ===
// Analysis of current (post splits):
// PERF: sync shell git (debounced, now no double compute), dir_sizes Arc<Mutex> locks on render paths (sizes async via rayon pool), filtered views rebuild, full virtual list not used in main render (still ScrollArea+rows), ctx clones and per-frame polls.
// UIUX: tab bar text toggles (L C G etc) + * active marker hacky, many show_* dialogs every frame, grip affordance good (dots) but drag feedback thin, preview dismiss limited, column headers partial, no strong empty states or loading for sizes/git.
// ARCH: Workspace still god-like (pub left/right/requests/shelf/undo + direct tabs access); update.rs orchestrates 20+ dialogs; PanelState mixed (state + caches + notify + atomics); render takes many args.
// SEP/COUPLING: even with accessors, some direct pub(crate) field use remains (esp in handlers, nav, render_tab_bar, workspace logic); raw indexing in TabSide + duplicate close logic; no events/effects, imperative mutation from everywhere; App knows deep ws details.
// DESIGN: flags for show_*, mixed ownership (Arc<Mutex> lingering for sizes/transfer/progress, mpsc only for git).
// Recent fixes (this влей): pub fields -> pub(crate), cursor/path/git/preview accessors+setters migrated in nav/handlers/file_list/update/workspace/preview, TabSide helpers (set/close/duplicate/len) to cut indexing, removed double git work + simple channel send, preview no cross borrow.
// If from scratch: private fields only + query/mut fns, event bus or mpsc effects for all side (git/sizes/toast/refresh), TabManager struct (no left/right dupe+raw idx), thin App (pure wiring + egui), pure components not giant update painters, full async runtime (tokio) not rayon+shell, channels everywhere over shared mut, dedicated layout state separate from domain.

pub use crate::preview::{InfoCard, PreviewContent, make_info, make_preview};

// glob_match moved to src/panel/filter.rs (see reexport at top of this file)

// parse_mask moved to src/panel/filter.rs (reexported at top)

// term_matches moved to src/panel/filter.rs (internal to filter, called via reexported logic)

// entry_display_size moved to sizes.rs (public reexport at top of panel)

// natural_cmp, strip_leading_zeros, Sort* moved to src/panel/sort.rs (reexported)

/// Wake-up callback into the UI (e.g. a repaint request). Panels never
/// talk to the UI toolkit directly.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

// KindFacet, FacetSet, facet_matches moved to src/panel/filter.rs (reexported).
// The filter chips in render and mask/select logic continue to work via the reexports.

// FilterCache definition moved to src/panel/filter.rs (pub(crate) reexport at top keeps the field type and stale() call working in PanelState).

// SortColumn, SortOrder reexported from sort.rs

// PanelState extracted to state.rs for SRP (data model in small piece).
pub use state::PanelState;

impl PanelState {



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

        let ents = Self::read_dir(&self.current_path, self.show_hidden);
        self.set_entries(ents);
        self.set_dir_status(classify_dir(&self.current_path(), self.entries().is_empty()));
        self.sort_entries();

        {
            let existing: std::collections::HashSet<&PathBuf> =
                self.entries.iter().map(|e| &e.path).collect();
            self.selected.retain(|p| existing.contains(p));
        }

        let restored = cursor_path
            .and_then(|path| self.filtered_entries().iter().position(|e| e.path == path));
        match restored {
            Some(idx) => self.set_cursor_and_scroll(idx + 1),
            None => self.set_cursor(self.cursor().min(self.filtered_count())),
        }

        // Schedule git status in background only. Apply happens via channel message in App.
        self.refresh_git_status();
    }

    fn refresh_git_status(&mut self) {
        // Always bg now. No direct mutation here.
        crate::git::refresh_git_status(&self.current_path, &mut self.last_git_refresh, self.notify.clone(), self.git_tx.clone());
    }

    /// Check if fs watcher flagged a change; if so, refresh.
    /// Returns `true` when the directory listing was re-read.
    /// Deep events (below the watched dir) only recompute directory
    /// sizes, debounced so event floods during transfers don't thrash.
    pub fn poll_fs_changes(&mut self) -> bool {
        watcher::poll_fs_changes(self)
    }

    fn start_watcher(&mut self) {
        watcher::start_watcher(self);
    }

    /// Schedule background recomputation of subdirectory sizes and counts.
    /// `forced` marks user-driven refreshes: they may re-walk expensive
    /// dirs, background (watcher-noise) recomputes may not.
    /// Returns `true` when some dir was skipped because of the walk
    /// cooldown and the caller should retry later.
    fn compute_dir_sizes(&self, forced: bool) -> bool {
        sizes::compute_dir_sizes(self, forced)
    }

    fn read_dir(path: &Path, show_hidden: bool) -> Vec<FileEntry> {
        fs::read_dir(path, show_hidden)
    }

    pub fn sort_entries(&mut self) {
        sort::sort_entries(self);
    }

    pub fn navigate_to(&mut self, path: PathBuf) {
        nav::navigate_to(self, path);
    }

    pub fn go_up(&mut self) {
        nav::go_up(self);
    }

    pub fn type_ahead(&mut self, buffer: &str) -> bool {
        nav::type_ahead(self, buffer)
    }

    /// Apply a select-by-mask line to the selection over the filtered view:
    /// add terms select matching entries, `!`/`-` terms deselect them
    /// (subtraction wins per entry). Returns how many entries were added.
    pub fn select_by_mask(&mut self, mask: &str) -> usize {
        selection::select_by_mask(self, mask)
    }

    /// How many filtered entries any term of `mask` matches (live preview,
    /// no mutation).
    pub fn mask_match_count(&self, mask: &str) -> usize {
        selection::mask_match_count(self, mask)
    }

    /// Add the file under the cursor to the selection (range-select step).
    pub fn select_cursor(&mut self) {
        selection::select_cursor(self);
    }

    pub fn can_go_back(&self) -> bool {
        nav::can_go_back(self)
    }

    pub fn can_go_forward(&self) -> bool {
        nav::can_go_forward(self)
    }

    pub fn go_back(&mut self) {
        nav::go_back(self);
    }

    pub fn go_forward(&mut self) {
        nav::go_forward(self);
    }

    fn ensure_filter_cache(&self) {
        filter::ensure_filter_cache(self);
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
        selection::toggle_select(self, path);
    }

    pub fn select_all(&mut self) {
        selection::select_all(self);
    }

    pub fn invert_selection(&mut self) {
        selection::invert_selection(self);
    }

    pub fn extend_selection(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        selection::extend_selection(self, paths);
    }

    pub fn selected_entries(&self) -> Vec<FileEntry> {
        selection::selected_entries(self)
    }

    pub fn selected_or_cursor(&self) -> Vec<FileEntry> {
        selection::selected_or_cursor(self)
    }

    pub fn total_size_selected(&self) -> u64 {
        selection::total_size_selected(self)
    }

    pub fn total_dir_size(&self) -> Option<u64> {
        selection::total_dir_size(self)
    }

    pub fn set_sort(&mut self, col: SortColumn) {
        sort::set_sort(self, col);
    }

    /// List subdirectories of `path` (for tree view).
    pub fn subdirs(path: &Path, show_hidden: bool) -> Vec<PathBuf> {
        fs::subdirs(path, show_hidden)
    }

    pub fn sort_indicator(&self, col: SortColumn) -> &str {
        sort::sort_indicator(self, col)
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

    // natural_cmp test moved to src/panel/sort.rs


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
    fn filter_visited_substring_case_insensitive() {
        let paths = vec![
            PathBuf::from("/Users/me/Documents"),
            PathBuf::from("/Users/me/Downloads"),
            PathBuf::from("/tmp/work"),
        ];
        assert_eq!(filter_visited(&paths, "").len(), 3);
        let dn = filter_visited(&paths, "down");
        assert_eq!(dn, vec![PathBuf::from("/Users/me/Downloads")]);
        assert_eq!(filter_visited(&paths, "USERS").len(), 2);
    }

    #[test]
    fn format_mode_renders_rwx() {
        // format_mode lives in preview (moved for DRY)
        assert_eq!(crate::preview::format_mode(0o755), "rwxr-xr-x");
        assert_eq!(crate::preview::format_mode(0o644), "rw-r--r--");
        assert_eq!(crate::preview::format_mode(0o600), "rw-------");
        assert_eq!(crate::preview::format_mode(0o000), "---------");
        assert_eq!(crate::preview::format_mode(0o777), "rwxrwxrwx");
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

        // Empty facets pass everything.
        assert!(facet_matches(&code, &FacetSet::default(), now));
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
        if let Ok(mut log) = sizes::walk_log().lock() { log.clear(); } // bypass the walk cooldown in the test (reset_walk_log moved)

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

    #[test]
    fn bench_refresh_and_git_status() {
        // Simple before/after style benchmark for hot path (reload + git).
        // Run with `cargo test bench_refresh_and_git_status -- --nocapture`
        // Creates a temp dir + fake .git, populates files, times refreshes.
        use std::time::Instant;
        let tmp = TempDir::new();
        // Simulate git repo
        let _ = std::fs::create_dir_all(tmp.path().join(".git"));
        for i in 0..200 {
            tmp.file(&format!("f{:04}.txt", i), "x");
        }
        let mut p = PanelState::new(tmp.path().to_path_buf());
        let n = 5;
        let mut total = std::time::Duration::ZERO;
        for _ in 0..n {
            let t0 = Instant::now();
            p.refresh();
            total += t0.elapsed();
        }
        println!("BENCH before: {} refreshes avg {:?}", n, total / n as u32);

        // Time just git status (the expensive new part)
        let t0 = Instant::now();
        for _ in 0..10 {
            p.refresh_git_status();
        }
        let git_time = t0.elapsed() / 10;
        println!("BENCH git_status alone (10 calls): avg {:?}", git_time);
    }
}
