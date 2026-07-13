//! UI layer: renders the [`Workspace`] and forwards input to it.
//! All file-manager behaviour lives in `crate::workspace`; this module
//! owns only presentation state (theme, zoom, image cache, tree widget).

mod archive_dialog;
mod batch_rename_dialog;
mod collections_dialog;
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
mod queue_dialog;
mod receipts_dialog;
mod recent_dialog;
mod rename_dialog;
mod render;
mod run_command_dialog;
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
    /// Pending vim-style chord leader (`'g'` or `'s'`) and when it was
    /// pressed (seconds, from egui). Expires after a short idle.
    pub(crate) chord: Option<(char, f64)>,
    /// Paint relative size occupancy bars behind file rows.
    pub(crate) show_size_bars: bool,
    /// Compare mode: tint each row by how it differs from the other panel.
    pub(crate) show_compare: bool,
    /// Whether the transfer-queue panel is visible.
    pub(crate) show_queue_panel: bool,
    /// One-shot dense work mode: chrome is hidden until pointer movement/Esc.
    pub(crate) focus_mode: bool,
    pub(crate) focus_started_at: f64,
    /// Active select-by-mask input buffer.
    pub(crate) mask_input: Option<String>,
    /// Active go-to-path input buffer.
    pub(crate) path_input: Option<String>,
    /// Active recent-directories quick-switcher filter buffer.
    pub(crate) recent_input: Option<String>,
    /// Ranking mode for recent destinations: habitual (frecency) or strictly
    /// chronological. Persisted with the session.
    pub(crate) recent_order: crate::panel::RecentOrder,
    /// Generation-based background search engine and replayable query history.
    pub(crate) search_engine: crate::search::SearchEngine,
    pub(crate) search_history: crate::search::QueryHistory,
    pub(crate) content_index: crate::content_index::ContentIndex,
    /// Transient operation toasts (move / rename confirmations with Undo).
    pub(crate) toasts: crate::toasts::ToastQueue,
    /// Searchable history of completed moves/deletes/batch-renames.
    pub(crate) receipts: crate::receipts::ReceiptLog,
    /// Active receipts-search buffer; `Some` while the dialog is open.
    pub(crate) receipts_input: Option<String>,
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
    /// Active disk-usage map and cancellable compressed-tree scan.
    pub(crate) treemap: Option<DiskUsageState>,
    /// Active recursive-find sheet state.
    pub(crate) find: Option<FindState>,
    /// Active read-only archive inspector.
    pub(crate) archive: Option<ArchiveState>,
    /// Saved searches, loaded lazily on first use.
    pub(crate) smart_folders: Option<crate::smart_folder::SmartFolders>,
    /// Whether the saved-search picker is open.
    pub(crate) saved_search_open: bool,
    /// Persisted multi-root projects and active virtual-view UI state.
    pub(crate) project_collections: crate::collections::ProjectCollections,
    pub(crate) collections_dialog: Option<CollectionsDialogState>,
    /// Saved command templates, loaded lazily on first use.
    pub(crate) command_templates: Option<crate::cmdtemplate::Templates>,
    /// Active run-command bar state (the editable command line).
    pub(crate) run_command: Option<RunCommandState>,
    /// Cached cross-panel compare maps and the panel generations they were built
    /// from, so compare mode does not rebuild two HashMaps (cloning every
    /// `name_lower`) on every painted frame. `(right_gen, left_gen, left_map,
    /// right_map)`: `left_map` indexes the right panel and vice versa.
    pub(crate) compare_cache: Option<(
        u64,
        u64,
        crate::compare::CompareMap,
        crate::compare::CompareMap,
    )>,
}

/// UI state for the run-command / open-with bar.
pub(crate) struct RunCommandState {
    /// The editable command line (placeholders expand against the selection).
    pub line: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DiskUsageMode {
    #[default]
    Map,
    Tree,
}

pub(crate) struct DiskUsageState {
    pub initial: crate::workspace::TreemapSnapshot,
    pub mode: DiskUsageMode,
    pub run: Option<crate::tree_overview::OverviewRun>,
    pub progress: crate::tree_overview::ScanProgress,
    pub overview: Option<crate::tree_overview::OverviewSnapshot>,
    pub stopping: bool,
    pub error: Option<String>,
}

pub(crate) struct ArchiveState {
    pub path: PathBuf,
    pub filter: String,
    pub run: Option<crate::archive::ListingRun>,
    pub listing: Option<crate::archive::ArchiveListing>,
    pub selected: Option<usize>,
    pub error: Option<String>,
    pub focused: bool,
}

/// UI state for the recursive-find sheet. The matching lives in `crate::query`;
/// this holds the editable fields, the search root, and the results.
pub(crate) struct FindState {
    pub expression: String,
    pub mode: crate::query::MatchMode,
    pub root: PathBuf,
    pub results: Vec<crate::search::SearchHit>,
    pub run: Option<crate::search::SearchRun>,
    pub generation: u64,
    pub ran: bool,
    pub searching: bool,
    pub focused: bool,
    pub scanned: usize,
    pub matched: usize,
    pub elapsed: std::time::Duration,
    pub truncated: bool,
    pub content_skipped: usize,
    pub error: Option<String>,
    pub stable_order: Vec<crate::search::FileIdentity>,
    pub last_query: Option<(String, crate::query::MatchMode, PathBuf)>,
    pub time_pivot: crate::search::TimePivot,
    pub history_open: bool,
    pub explanation_open: Option<crate::search::FileIdentity>,
    pub pending_rerun: bool,
    pub last_edit_at: f64,
    pub search_source: String,
    pub index_details_open: bool,
    pub index_exclusions: String,
    pub index_rerun_after_build: bool,
    /// Name to save this query under (smart folder).
    pub save_name: String,
}

pub(crate) struct CollectionsDialogState {
    pub name: String,
    pub include_left: bool,
    pub include_right: bool,
    pub selected: Option<String>,
    pub rows: Vec<crate::collections::VirtualEntry>,
    pub run: Option<crate::collections::ViewRun>,
    pub scanned: usize,
    pub unavailable_roots: Vec<PathBuf>,
    pub truncated: bool,
    pub error: Option<String>,
}

impl Default for CollectionsDialogState {
    fn default() -> Self {
        Self {
            name: String::new(),
            include_left: true,
            include_right: true,
            selected: None,
            rows: Vec::new(),
            run: None,
            scanned: 0,
            unavailable_roots: Vec::new(),
            truncated: false,
            error: None,
        }
    }
}

impl Default for FindState {
    fn default() -> Self {
        Self {
            expression: String::new(),
            mode: crate::query::MatchMode::Exact,
            root: PathBuf::new(),
            results: Vec::new(),
            run: None,
            generation: 0,
            ran: false,
            searching: false,
            focused: false,
            scanned: 0,
            matched: 0,
            elapsed: std::time::Duration::ZERO,
            truncated: false,
            content_skipped: 0,
            error: None,
            stable_order: Vec::new(),
            last_query: None,
            time_pivot: crate::search::TimePivot::None,
            history_open: false,
            explanation_open: None,
            pending_rerun: false,
            last_edit_at: 0.0,
            search_source: "Live".to_string(),
            index_details_open: false,
            index_exclusions: String::new(),
            index_rerun_after_build: false,
            save_name: String::new(),
        }
    }
}

impl FindState {
    /// Build the query the current fields describe.
    pub(crate) fn build_query(&self) -> Result<crate::query::Query, crate::query::QueryError> {
        crate::query::Query::parse(&self.expression, self.mode)
    }

    /// Reconstruct the editable fields from a saved smart-folder definition.
    pub(crate) fn from_definition(def: &crate::smart_folder::Definition) -> Self {
        FindState {
            expression: def.query.to_expression(),
            mode: def.query.mode,
            root: def.root.clone(),
            pending_rerun: true,
            last_edit_at: -1.0,
            ..Default::default()
        }
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
    pub durability: crate::operation::DurabilityProfile,
    pub actions: Vec<crate::sync::SyncAction>,
    pub left_dir: PathBuf,
    pub right_dir: PathBuf,
    pub left_show_hidden: bool,
    pub right_show_hidden: bool,
    pub guard: crate::sync_guard::GuardPolicy,
    pub stamp: Option<crate::sync_guard::PlanStamp>,
    pub settings_fingerprint: u64,
    pub allow_large_plan: bool,
    pub marker_enabled: bool,
    pub marker_input: String,
    pub error: Option<String>,
}

/// UI state for the batch-rename studio. The transform itself lives in
/// `crate::rename`; this only holds the editable rule fields.
pub(crate) struct BatchRenameState {
    pub context: crate::workspace::BatchRenameContext,
    pub find: String,
    pub replace: String,
    /// Treat `find` as a regular expression (`$1`-style groups in `replace`)
    /// instead of a literal substring.
    pub regex_mode: bool,
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
            regex: self.regex_mode,
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
    pub siblings: Vec<String>,
    pub buffer: String,
    pub error: Option<String>,
    /// Set once so the text field grabs focus on the first frame.
    pub focused: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let session = crate::session::load();
        if let Some(saved) = &session {
            crate::panel::restore_visit_snapshot(&saved.recent_paths, &saved.recent_stats);
        }

        // Theme: a saved session wins, otherwise follow the system appearance.
        let mode = match &session {
            Some(s) if s.theme_dark => ThemeMode::Dark,
            Some(_) => ThemeMode::Light,
            None if cc.egui_ctx.theme() == egui::Theme::Dark => ThemeMode::Dark,
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
            ws.left.folders_first = s.left_folders_first;
            ws.left.natural_name_sort = s.left_natural_sort;
            ws.left.density = s.left_density;
            ws.right.sort_col = s.right_sort_col;
            ws.right.sort_order = s.right_sort_order;
            ws.right.show_hidden = s.right_hidden;
            ws.right.folders_first = s.right_folders_first;
            ws.right.natural_name_sort = s.right_natural_sort;
            ws.right.density = s.right_density;
            ws.durability_profile = s.durability_profile;
            ws.sync_guard_policy = s.sync_guard_policy.clone();
        }

        App {
            ws,
            ui_scale,
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
            chord: None,
            show_size_bars: session.as_ref().is_some_and(|s| s.show_size_bars),
            show_compare: session.as_ref().is_some_and(|s| s.show_compare),
            show_queue_panel: false,
            focus_mode: false,
            focus_started_at: 0.0,
            mask_input: None,
            path_input: None,
            recent_input: None,
            recent_order: session
                .as_ref()
                .map_or(crate::panel::RecentOrder::Frecency, |s| s.recent_order),
            search_engine: crate::search::SearchEngine::default(),
            search_history: session
                .as_ref()
                .map(|s| s.search_history.clone())
                .unwrap_or_default(),
            content_index: crate::content_index::ContentIndex::load(),
            toasts: crate::toasts::ToastQueue::default(),
            receipts: crate::receipts::ReceiptLog::default(),
            receipts_input: None,
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
            archive: None,
            smart_folders: None,
            saved_search_open: false,
            project_collections: crate::collections::load(),
            collections_dialog: None,
            command_templates: None,
            run_command: None,
            compare_cache: None,
        }
    }

    /// The command-template store, loaded from disk on first access.
    pub(crate) fn command_templates_mut(&mut self) -> &mut crate::cmdtemplate::Templates {
        self.command_templates
            .get_or_insert_with(crate::cmdtemplate::load)
    }

    /// The saved-search store, loaded from disk on first access.
    pub(crate) fn smart_folders_mut(&mut self) -> &mut crate::smart_folder::SmartFolders {
        self.smart_folders
            .get_or_insert_with(crate::smart_folder::load)
    }

    /// Snapshot the current state into a persistable [`Session`].
    fn to_session(&self) -> crate::session::Session {
        let (recent_paths, recent_stats) = crate::panel::visit_snapshot();
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
            left_folders_first: self.ws.left.folders_first,
            left_natural_sort: self.ws.left.natural_name_sort,
            right_folders_first: self.ws.right.folders_first,
            right_natural_sort: self.ws.right.natural_name_sort,
            left_density: self.ws.left.density,
            right_density: self.ws.right.density,
            palette_usage: self.palette_usage.clone(),
            palette_tick: self.palette_tick,
            recent_paths,
            recent_stats,
            recent_order: self.recent_order,
            search_history: self.search_history.clone(),
            durability_profile: self.ws.durability_profile,
            sync_guard_policy: self.ws.sync_guard_policy.clone(),
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
            if outcome.trashed > 0 {
                self.receipts.push(crate::receipts::Receipt {
                    verb: "Deleted",
                    item_count: outcome.trashed,
                    timestamp: now,
                    jump_to: self.ws.active_panel_ref().current_path.clone(),
                    // No undo path for a delete in this app today; jump-back
                    // still gets you to where it happened.
                    undo_action: None,
                });
            }
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
