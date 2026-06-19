//! UI layer: renders the [`Workspace`] and forwards input to it.
//! All file-manager behaviour lives in `crate::workspace`; this module
//! owns only presentation state (theme, zoom, image cache, tree widget).

mod batch_rename_dialog;
mod confirm_dialog;
mod diff_dialog;
mod duplicates_dialog;
mod file_list;
mod find_dialog;
mod keys;
mod mask_dialog;
mod palette_dialog;
mod path_dialog;
mod preload;
mod recent_dialog;
mod rename_dialog;
mod render;
mod preview_pane;
mod layout;
mod status_bar;
mod virtual_list;
mod input;
mod facet;
mod ui_common;
mod bookmarks_ui;
mod virtual_tree;
mod saved_search_dialog;
mod sync_dialog;
mod toolbar;
mod transfer_dialog;
mod tree;
mod treemap_dialog;
mod update;

use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, Sense, Stroke, Vec2};
use std::path::PathBuf;

use crate::panel::{PanelState, SortColumn, format_size};
use crate::theme::{ThemeColors, ThemeMode, apply_theme};
use crate::config::AppConfig;
pub(crate) use crate::transfer::{CopyMethod, TransferKind};
pub(crate) use crate::workspace::{ActivePanel, PendingOp, Workspace};

pub struct App {
    /// UI-independent application core (panels, ops, transfers).
    pub ws: Workspace,
    pub config: AppConfig,
    pub ui_scale: f32,
    /// List density tier (row sizes), restored from and saved to the session.
    pub(crate) density: crate::density::Density,
    pub theme_mode: ThemeMode,
    pub colors: ThemeColors,
    pub(crate) prev_window_width: f32,
    pub(crate) image_cache: crate::image_cache::ImageCache,
    pub(crate) show_tree: bool,
    pub(crate) tree_expanded: std::collections::HashSet<PathBuf>,
    pub(crate) tree_children_cache: std::collections::HashMap<PathBuf, Vec<PathBuf>>,
    pub(crate) tree_width: f32,
    /// Active inline rename: the entry being renamed and the edit buffer.
    pub(crate) renaming: Option<RenameState>,
    /// Type-ahead buffer and the input time of its last keystroke (seconds,
    /// from egui). Expires after a short idle.
    pub(crate) type_ahead: Option<(String, f64)>,
    /// Paint relative size occupancy bars behind file rows.
    pub(crate) show_size_bars: bool,
    /// Compare mode: tint each row by how it differs from the other panel.
    pub(crate) show_compare: bool,
    /// Active select-by-mask input buffer.
    pub(crate) mask_input: Option<String>,
    /// Active go-to-path input buffer.
    pub(crate) path_input: Option<String>,
    /// Active recent-directories quick-switcher filter buffer.
    pub(crate) recent_input: Option<String>,
    /// Transient operation toasts (move / rename confirmations with Undo).
    pub(crate) toasts: crate::toasts::ToastQueue,
    /// Active command-palette filter buffer.
    pub(crate) palette_input: Option<String>,
    /// Command-palette usage history (recency/frequency ranking).
    pub(crate) palette_usage: crate::command::UsageStats,
    /// Monotonic counter stamped onto each palette command run.
    pub(crate) palette_tick: u64,
    /// Active batch-rename studio state.
    pub(crate) batch_rename: Option<BatchRenameState>,
    /// Active synchronise-sheet state.
    pub(crate) sync: Option<SyncState>,
    /// Active duplicate-finder sheet state.
    pub(crate) duplicates: Option<DupState>,
    /// Active read-only diff sheet state.
    pub(crate) diff: Option<DiffState>,
    /// Active disk-usage treemap state (entries with their bytes, sorted).
    pub(crate) treemap: Option<Vec<(crate::panel::FileEntry, u64)>>,
    /// Active recursive-find sheet state.
    pub(crate) find: Option<FindState>,
    /// Saved searches, loaded lazily on first use.
    pub(crate) smart_folders: Option<crate::smart_folder::SmartFolders>,
    /// Whether the saved-search picker is open.
    pub(crate) saved_search_open: bool,
    /// For starting drag reorder on tabs (left or right).
    pub(crate) dragged_tab: Option<(bool, usize)>, // (is_left, index)
    /// Bookmarks dialog open + filter buffer (full UI from stub).
    pub(crate) bookmarks_open: Option<String>,
    /// Column config dialog open (for widths, toggles like TC).
    pub(crate) column_config_open: bool,
    /// Mini terminal bottom pane open (idea #11).
    pub(crate) terminal_open: bool,
    pub(crate) terminal_history: Vec<String>,
    /// Macro recorder active (idea #37/46).
    pub(crate) macro_recording: bool,
    pub(crate) macro_steps: Vec<String>,
    /// Grid view toggle (idea #65/72).
    pub(crate) grid_view: bool,
    /// Saved named macros (idea #82). key=name, value=steps.
    pub(crate) saved_macros: std::collections::HashMap<String, Vec<String>>,
    /// Tag editor open (idea #73).
    pub(crate) user_tag_editor_open: bool,
    /// Permissions dialog open (idea #83).
    pub(crate) permissions_open: bool,
    /// Archive browser open stub (idea #84).
    pub(crate) archive_open: bool,
    /// Notes editor open (idea #92).
    pub(crate) notes_open: bool,
}

/// UI state for the recursive-find sheet. The matching lives in `crate::query`;
/// this holds the editable fields, the search root, and the results.
#[derive(Default)]
pub(crate) struct FindState {
    pub name: String,
    pub min_mb: String,
    pub max_age_days: String,
    pub kind: Option<crate::selection_summary::Kind>,
    pub root: PathBuf,
    pub results: Vec<crate::panel::FileEntry>,
    pub ran: bool,
    pub focused: bool,
    /// Name to save this query under (smart folder).
    pub save_name: String,
}

impl FindState {
    /// Build the query the current fields describe.
    pub(crate) fn build_query(&self) -> crate::query::Query {
        use crate::query::Predicate;
        let mut preds = Vec::new();
        if !self.name.trim().is_empty() {
            preds.push(Predicate::NameContains(self.name.trim().to_string()));
        }
        if let Some(k) = self.kind {
            preds.push(Predicate::Kind(k));
        }
        if let Ok(mb) = self.min_mb.trim().parse::<u64>()
            && mb > 0
        {
            preds.push(Predicate::MinSize(mb * 1024 * 1024));
        }
        if let Ok(d) = self.max_age_days.trim().parse::<u64>()
            && d > 0
        {
            preds.push(Predicate::MaxAgeDays(d));
        }
        crate::query::Query { predicates: preds }
    }

    /// Reconstruct the editable fields from a saved smart-folder definition.
    pub(crate) fn from_definition(def: &crate::smart_folder::Definition) -> Self {
        use crate::query::Predicate;
        let mut s = FindState {
            root: def.root.clone(),
            ..Default::default()
        };
        for p in &def.query.predicates {
            match p {
                Predicate::NameContains(n) => s.name = n.clone(),
                Predicate::Kind(k) => s.kind = Some(*k),
                Predicate::MinSize(b) => s.min_mb = (b / (1024 * 1024)).to_string(),
                Predicate::MaxAgeDays(d) => s.max_age_days = d.to_string(),
            }
        }
        s
    }
}

/// UI state for the read-only diff sheet. The diff itself lives in
/// `crate::textdiff`; this holds the two names and the computed lines (or a
/// message when the pair cannot be diffed as text).
pub(crate) struct DiffState {
    pub name_a: String,
    pub name_b: String,
    pub lines: Vec<crate::textdiff::DiffLine>,
    pub message: Option<String>,
}

/// UI state for the duplicate-finder sheet. Grouping lives in `crate::dedup`;
/// this holds the groups, the per-group keep choice, and the policy.
pub(crate) struct DupState {
    pub groups: Vec<crate::dedup::DupGroup>,
    /// Index (into each group's `files`) of the file to keep.
    pub keep: Vec<usize>,
    pub policy: crate::dedup::KeepPolicy,
}

/// UI state for the directory-synchronise sheet. The diff lives in
/// `crate::sync`; this holds the chosen policy and the editable action rows.
pub(crate) struct SyncState {
    pub policy: crate::sync::SyncPolicy,
    pub actions: Vec<crate::sync::SyncAction>,
}

/// UI state for the batch-rename studio. The transform itself lives in
/// `crate::rename`; this only holds the editable rule fields.
pub(crate) struct BatchRenameState {
    pub find: String,
    pub replace: String,
    pub prefix: String,
    pub suffix: String,
    pub case: crate::rename::CaseMode,
    pub numbering_on: bool,
    pub num_start: u32,
    pub num_step: u32,
    pub num_pad: u32,
    /// Set once so the first text field grabs focus on the opening frame.
    pub focused: bool,
    pub error: Option<String>,
}

impl BatchRenameState {
    /// Build the pure rename rule from the current field values.
    pub(crate) fn rule(&self) -> crate::rename::RenameRule {
        crate::rename::RenameRule {
            find: self.find.clone(),
            replace: self.replace.clone(),
            prefix: self.prefix.clone(),
            suffix: self.suffix.clone(),
            case: self.case,
            numbering: self.numbering_on.then_some(crate::rename::Numbering {
                start: self.num_start,
                step: self.num_step.max(1),
                pad: self.num_pad as usize,
            }),
        }
    }
}

/// UI state for the rename editor.
pub(crate) struct RenameState {
    pub path: PathBuf,
    pub buffer: String,
    pub error: Option<String>,
    /// Set once so the text field grabs focus on the first frame.
    pub focused: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let session = crate::session::load();

        // Theme: a saved session wins, otherwise follow the system appearance.
        let mode = match &session {
            Some(s) if s.theme_dark => ThemeMode::Dark,
            Some(_) => ThemeMode::Light,
            None if cc.egui_ctx.style().visuals.dark_mode => ThemeMode::Dark,
            None => {
                let is_dark = std::process::Command::new("defaults")
                    .args(["read", "-g", "AppleInterfaceStyle"])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).contains("Dark"))
                    .unwrap_or(false);
                if is_dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                }
            }
        };
        apply_theme(&cc.egui_ctx, mode);

        let (left, right) = session
            .as_ref()
            .map(|s| s.sanitized_paths(&home))
            .unwrap_or_else(|| (home.clone(), home.clone()));
        let mut ws = Workspace::new(left, right);

        let ui_scale = session.as_ref().map_or(1.0, |s| s.ui_scale);
        cc.egui_ctx.set_zoom_factor(ui_scale);

        if let Some(s) = &session {
            ws.active = if s.active_left {
                ActivePanel::Left
            } else {
                ActivePanel::Right
            };
            // PR2 tabs: if saved tabs present, populate from them (with active indices); else legacy single -> tabs[0]
            if !s.left_tabs.is_empty() {
                ws.left.tabs.clear();
                for t in &s.left_tabs {
                    let mut st = PanelState::new(t.path.clone());
                    st.sort_col = t.sort_col;
                    st.sort_order = t.sort_order;
                    st.show_hidden = t.hidden;
                    ws.left.tabs.push(crate::workspace::PanelTab { state: st });
                }
                ws.left.active = s.left_active.min(ws.left.tabs.len().saturating_sub(1));
            } else {
                // legacy (use current active or 0)
                let li = ws.left.active.min(ws.left.tabs.len().saturating_sub(1));
                ws.left.tabs[li].state.sort_col = s.left_sort_col;
                ws.left.tabs[li].state.sort_order = s.left_sort_order;
                ws.left.tabs[li].state.show_hidden = s.left_hidden;
            }
            // bookmarks from session (or empty)
            ws.bookmarks = s.bookmarks.clone();
            if !s.right_tabs.is_empty() {
                ws.right.tabs.clear();
                for t in &s.right_tabs {
                    let mut st = PanelState::new(t.path.clone());
                    st.sort_col = t.sort_col;
                    st.sort_order = t.sort_order;
                    st.show_hidden = t.hidden;
                    ws.right.tabs.push(crate::workspace::PanelTab { state: st });
                }
                ws.right.active = s.right_active.min(ws.right.tabs.len().saturating_sub(1));
            } else {
                let ri = ws.right.active.min(ws.right.tabs.len().saturating_sub(1));
                ws.right.tabs[ri].state.sort_col = s.right_sort_col;
                ws.right.tabs[ri].state.sort_order = s.right_sort_order;
                ws.right.tabs[ri].state.show_hidden = s.right_hidden;
            }
        }

        let config = crate::config::AppConfig::default();
        ws.show_git_status = session.as_ref().map_or(config.show_git_status, |s| s.show_git_status);
        ws.linked_scroll = session.as_ref().map_or(false, |s| s.linked_scroll);

        let show_tree_default = config.show_tree;
        let density_default = config.density;

        // Channel owned by ws (thin App, tokio unbounded). Create here, wire to tabs, store on ws.
        let (git_tx, git_rx) = tokio::sync::mpsc::unbounded_channel();
        ws.git_tx = Some(git_tx.clone());
        ws.git_rx = Some(git_rx);

        // Wire to all current tabs (multi-tab support).
        for tab in &mut ws.left.tabs {
            tab.state.git_tx = Some(git_tx.clone());
        }
        for tab in &mut ws.right.tabs {
            tab.state.git_tx = Some(git_tx.clone());
        }

        let mut app = App {
            ws,
            config,
            ui_scale,
            density: session.as_ref().map(|s| s.density).unwrap_or(density_default),
            theme_mode: mode,
            colors: match mode {
                ThemeMode::Light => ThemeColors::light(),
                ThemeMode::Dark => ThemeColors::dark(),
            },
            prev_window_width: 0.0,
            image_cache: crate::image_cache::ImageCache::new(),
            show_tree: session.as_ref().map(|s| s.show_tree).unwrap_or(show_tree_default),
            tree_expanded: std::collections::HashSet::new(),
            tree_children_cache: std::collections::HashMap::new(),
            tree_width: session.as_ref().map_or(200.0, |s| s.tree_width),
            renaming: None,
            type_ahead: None,
            show_size_bars: session.as_ref().is_some_and(|s| s.show_size_bars),
            show_compare: session.as_ref().is_some_and(|s| s.show_compare),
            mask_input: None,
            path_input: None,
            recent_input: None,
            toasts: crate::toasts::ToastQueue::default(),
            palette_input: None,
            palette_usage: session
                .as_ref()
                .map(|s| s.palette_usage.clone())
                .unwrap_or_default(),
            palette_tick: session.as_ref().map_or(0, |s| s.palette_tick),
            batch_rename: None,
            sync: None,
            duplicates: None,
            diff: None,
            treemap: None,
            find: None,
            smart_folders: None,
            saved_search_open: false,
            terminal_open: false,
            terminal_history: vec![],
            macro_recording: false,
            macro_steps: vec![],
            grid_view: false,
            dragged_tab: None,
            saved_macros: std::collections::HashMap::new(),
            user_tag_editor_open: false,
            permissions_open: false,
            archive_open: false,
            notes_open: false,
            bookmarks_open: None,
            column_config_open: false,
        };
        app
    }

    /// The saved-search store, loaded from disk on first access.
    pub(crate) fn smart_folders_mut(&mut self) -> &mut crate::smart_folder::SmartFolders {
        self.smart_folders
            .get_or_insert_with(crate::smart_folder::load)
    }

    /// Snapshot the current state into a persistable [`Session`].
    fn to_session(&self) -> crate::session::Session {
        // PR2: full tabs snapshot + active indices. Keep legacy paths/sorts for old sessions compat.
        let left_snap: Vec<crate::session::TabSnapshot> = self.ws.left.tabs.iter().map(|t| crate::session::TabSnapshot {
            path: t.state.current_path().clone(),
            sort_col: t.state.sort_col,
            sort_order: t.state.sort_order,
            hidden: t.state.show_hidden,
        }).collect();
        let right_snap: Vec<crate::session::TabSnapshot> = self.ws.right.tabs.iter().map(|t| crate::session::TabSnapshot {
            path: t.state.current_path().clone(),
            sort_col: t.state.sort_col,
            sort_order: t.state.sort_order,
            hidden: t.state.show_hidden,
        }).collect();
        crate::session::Session {
            left_path: self.ws.left.tabs.get(0).map(|t| t.state.current_path().clone()).unwrap_or_default(),
            right_path: self.ws.right.tabs.get(0).map(|t| t.state.current_path().clone()).unwrap_or_default(),
            active_left: self.ws.active == ActivePanel::Left,
            theme_dark: self.theme_mode == ThemeMode::Dark,
            ui_scale: self.ui_scale,
            show_tree: self.show_tree,
            tree_width: self.tree_width,
            show_size_bars: self.show_size_bars,
            show_compare: self.show_compare,
            left_sort_col: self.ws.left.tabs.get(0).map(|t| t.state.sort_col).unwrap_or(crate::panel::SortColumn::Name),
            left_sort_order: self.ws.left.tabs.get(0).map(|t| t.state.sort_order).unwrap_or(crate::panel::SortOrder::Asc),
            left_hidden: self.ws.left.tabs.get(0).map(|t| t.state.show_hidden).unwrap_or(false),
            right_sort_col: self.ws.right.tabs.get(0).map(|t| t.state.sort_col).unwrap_or(crate::panel::SortColumn::Name),
            right_sort_order: self.ws.right.tabs.get(0).map(|t| t.state.sort_order).unwrap_or(crate::panel::SortOrder::Asc),
            right_hidden: self.ws.right.tabs.get(0).map(|t| t.state.show_hidden).unwrap_or(false),
            density: self.density,
            palette_usage: self.palette_usage.clone(),
            palette_tick: self.palette_tick,
            left_tabs: left_snap,
            right_tabs: right_snap,
            left_active: self.ws.left.active,
            right_active: self.ws.right.active,
            bookmarks: self.ws.bookmarks.clone(),
            show_git_status: self.ws.show_git_status,
            linked_scroll: self.ws.linked_scroll,
        }
    }

    /// Confirm the pending operation; progress wakes the UI via repaint.
    pub(crate) fn confirm_pending_op(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        self.ws.confirm_pending_op(move || ctx.request_repaint());
    }

    pub(crate) fn tree_expand_to_path(&mut self, path: &std::path::Path) {
        let mut p = path.to_path_buf();
        loop {
            self.tree_expanded.insert(p.clone());
            match p.parent() {
                Some(parent) if parent != p => p = parent.to_path_buf(),
                _ => break,
            }
        }
    }
}
