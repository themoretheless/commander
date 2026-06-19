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
pub(crate) use crate::transfer::{CopyMethod, TransferKind};
pub(crate) use crate::workspace::{ActivePanel, PendingOp, Workspace};

pub struct App {
    /// UI-independent application core (panels, ops, transfers).
    pub ws: Workspace,
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
    /// One-shot dense work mode: chrome is hidden until pointer movement/Esc.
    pub(crate) focus_mode: bool,
    pub(crate) focus_started_at: f64,
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
            ws.left.sort_col = s.left_sort_col;
            ws.left.sort_order = s.left_sort_order;
            ws.left.show_hidden = s.left_hidden;
            ws.right.sort_col = s.right_sort_col;
            ws.right.sort_order = s.right_sort_order;
            ws.right.show_hidden = s.right_hidden;
        }

        App {
            ws,
            ui_scale,
            density: session.as_ref().map(|s| s.density).unwrap_or_default(),
            theme_mode: mode,
            colors: match mode {
                ThemeMode::Light => ThemeColors::light(),
                ThemeMode::Dark => ThemeColors::dark(),
            },
            prev_window_width: 0.0,
            image_cache: crate::image_cache::ImageCache::new(),
            show_tree: session.as_ref().is_some_and(|s| s.show_tree),
            tree_expanded: std::collections::HashSet::new(),
            tree_children_cache: std::collections::HashMap::new(),
            tree_width: session.as_ref().map_or(200.0, |s| s.tree_width),
            renaming: None,
            type_ahead: None,
            show_size_bars: session.as_ref().is_some_and(|s| s.show_size_bars),
            show_compare: session.as_ref().is_some_and(|s| s.show_compare),
            focus_mode: false,
            focus_started_at: 0.0,
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
        }
    }

    /// The saved-search store, loaded from disk on first access.
    pub(crate) fn smart_folders_mut(&mut self) -> &mut crate::smart_folder::SmartFolders {
        self.smart_folders
            .get_or_insert_with(crate::smart_folder::load)
    }

    /// Snapshot the current state into a persistable [`Session`].
    fn to_session(&self) -> crate::session::Session {
        crate::session::Session {
            left_path: self.ws.left.current_path.clone(),
            right_path: self.ws.right.current_path.clone(),
            active_left: self.ws.active == ActivePanel::Left,
            theme_dark: self.theme_mode == ThemeMode::Dark,
            ui_scale: self.ui_scale,
            show_tree: self.show_tree,
            tree_width: self.tree_width,
            show_size_bars: self.show_size_bars,
            show_compare: self.show_compare,
            left_sort_col: self.ws.left.sort_col,
            left_sort_order: self.ws.left.sort_order,
            left_hidden: self.ws.left.show_hidden,
            right_sort_col: self.ws.right.sort_col,
            right_sort_order: self.ws.right.sort_order,
            right_hidden: self.ws.right.show_hidden,
            density: self.density,
            palette_usage: self.palette_usage.clone(),
            palette_tick: self.palette_tick,
        }
    }

    /// Confirm the pending operation; progress wakes the UI via repaint.
    /// A Delete reports its outcome synchronously, so confirm it with a toast
    /// (and flag anything the Trash refused) rather than letting it vanish
    /// without acknowledgement.
    pub(crate) fn confirm_pending_op(&mut self, ctx: &egui::Context) {
        let ctx2 = ctx.clone();
        if let Some(outcome) = self.ws.confirm_pending_op(move || ctx2.request_repaint()) {
            let now = ctx.input(|i| i.time);
            let item = |n: usize| if n == 1 { "item" } else { "items" };
            let (message, kind) = if outcome.failed == 0 {
                (
                    format!(
                        "Moved {} {} to Trash",
                        outcome.trashed,
                        item(outcome.trashed)
                    ),
                    crate::toasts::ToastKind::Success,
                )
            } else if outcome.trashed == 0 {
                (
                    format!(
                        "Could not delete {} {}",
                        outcome.failed,
                        item(outcome.failed)
                    ),
                    crate::toasts::ToastKind::Error,
                )
            } else {
                (
                    format!(
                        "Moved {} to Trash, {} failed",
                        outcome.trashed, outcome.failed
                    ),
                    crate::toasts::ToastKind::Error,
                )
            };
            self.toasts
                .push(crate::toasts::Toast::new(message, kind, false, now));
        }
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
