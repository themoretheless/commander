//! UI layer: renders the [`Workspace`] and forwards input to it.
//! All file-manager behaviour lives in `crate::workspace`; this module
//! owns only presentation state (theme, zoom, image cache, tree widget).

mod archive_dialog;
mod batch_rename_dialog;
mod collections_dialog;
mod confirm_dialog;
mod developer_panel;
mod diff_dialog;
mod duplicates_dialog;
mod file_list;
mod find_dialog;
mod history_dialog;
mod keys;
mod mask_dialog;
mod palette_dialog;
mod path_dialog;
mod preload;
mod queue_dialog;
mod recent_dialog;
mod recovery_dialog;
mod rename_dialog;
mod render;
mod run_command_dialog;
mod safe_state_dialog;
mod saved_search_dialog;
mod sync_dialog;
mod toolbar;
mod transfer_dialog;
mod tree;
mod treemap_dialog;
mod ui_state;
mod update;

use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, Sense, Stroke, Vec2};
use std::path::PathBuf;
use std::rc::Rc;

use crate::panel::{PanelState, SortColumn, format_size};
use crate::theme::{ThemeColors, ThemeMode, apply_theme};
pub(crate) use crate::transfer::{CopyMethod, TransferKind};
pub(crate) use crate::ui_request::{UiModal, UiRequest};
pub(crate) use crate::workspace::{ActivePanel, PendingOp, Workspace};

pub struct App {
    /// UI-independent application core (panels, ops, transfers).
    pub ws: Workspace,
    /// Transient interaction state and the complete set of app-owned modals.
    pub(crate) ui: ui_state::UiState,
    /// Main-thread-owned desktop integration injected by the composition root.
    pub(crate) context_menu: Rc<dyn crate::ports::ContextMenuPort>,
    pub(crate) clipboard: Rc<dyn crate::ports::ClipboardPort>,
    pub(crate) opener: Rc<dyn crate::ports::OpenerPort>,
    pub ui_scale: f32,
    pub theme_mode: ThemeMode,
    pub colors: ThemeColors,
    pub(crate) accessibility_preferences: crate::accessibility::Preferences,
    pub(crate) prev_window_width: f32,
    pub(crate) image_cache: crate::image_cache::ImageCache,
    pub(crate) show_tree: bool,
    pub(crate) tree_expanded: std::collections::HashSet<PathBuf>,
    pub(crate) tree_children_cache: std::collections::HashMap<PathBuf, Vec<PathBuf>>,
    pub(crate) tree_width: f32,
    /// Paint relative size occupancy bars behind file rows.
    pub(crate) show_size_bars: bool,
    /// Compare mode: tint each row by how it differs from the other panel.
    pub(crate) show_compare: bool,
    /// Whether the unified queue/history/errors/recovery surface is visible.
    pub(crate) show_operations_center: bool,
    pub(crate) operations_tab: OperationsTab,
    pub(crate) operations_search: String,
    pub(crate) operation_failures: crate::operation_view::FailureInbox,
    pub(crate) failure_notice_seen: std::collections::HashSet<crate::operation::TransferAttemptId>,
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
    /// Startup-scanned durable recovery and orphan-staging model.
    pub(crate) recovery: RecoveryState,
    /// Command-palette usage history (recency/frequency ranking).
    pub(crate) palette_usage: crate::command::UsageStats,
    /// Monotonic counter stamped onto each palette command run.
    pub(crate) palette_tick: u64,
    /// Saved searches, loaded lazily on first use.
    pub(crate) smart_folders: Option<crate::smart_folder::SmartFolders>,
    /// Persisted multi-root projects and active virtual-view UI state.
    pub(crate) project_collections: crate::collections::ProjectCollections,
    /// Saved command templates, loaded lazily on first use.
    pub(crate) command_templates: Option<crate::cmdtemplate::Templates>,
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
    /// Startup instrumentation remains live through the first directory read.
    pub(crate) startup_trace: Option<crate::measurement::StartupTrace>,
    pub(crate) show_developer_panel: bool,
    pub(crate) developer_notice: Option<DeveloperNotice>,
    pub(crate) persistence_issue_seen: u64,
}

#[cfg(feature = "visual-qa")]
pub(crate) struct VisualQaSeed {
    pub left: PathBuf,
    pub right: PathBuf,
    pub ui_scale: f32,
    pub theme_mode: ThemeMode,
    pub accessibility_preferences: crate::accessibility::Preferences,
    pub show_tree: bool,
}

pub(crate) struct DeveloperNotice {
    pub message: String,
    pub path: Option<PathBuf>,
    pub error: bool,
}

impl DeveloperNotice {
    fn success(message: impl Into<String>, path: PathBuf) -> Self {
        Self {
            message: message.into(),
            path: Some(path),
            error: false,
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            path: None,
            error: true,
        }
    }
}

/// UI state for the run-command / open-with bar.
pub(crate) struct RunCommandState {
    /// The editable command line (placeholders expand against the selection).
    pub line: String,
    /// Separates egui scroll memory from earlier openings of this dialog.
    pub scroll_nonce: u64,
    /// Immutable panel/selection context captured when the dialog opens.
    pub opening: RunCommandOpeningContext,
}

#[derive(Clone)]
pub(crate) struct RunCommandOpeningContext {
    pub active_panel: ActivePanel,
    pub selection: Vec<crate::panel::FileEntry>,
    pub left_dir: PathBuf,
    pub right_dir: PathBuf,
}

impl RunCommandOpeningContext {
    fn capture(workspace: &Workspace) -> Self {
        let active_panel = workspace.active;
        let active = workspace.active_panel_ref();
        Self {
            active_panel,
            selection: active.selected_or_cursor().unwrap_or_default(),
            left_dir: workspace.left.current_path.clone(),
            right_dir: workspace.right.current_path.clone(),
        }
    }

    fn dir(&self) -> &std::path::Path {
        match self.active_panel {
            ActivePanel::Left => &self.left_dir,
            ActivePanel::Right => &self.right_dir,
        }
    }

    fn dir_other(&self) -> &std::path::Path {
        match self.active_panel {
            ActivePanel::Left => &self.right_dir,
            ActivePanel::Right => &self.left_dir,
        }
    }

    fn selection_context(&self) -> crate::cmdtemplate::SelectionCtx {
        crate::cmdtemplate::SelectionCtx {
            paths: self
                .selection
                .iter()
                .map(|entry| entry.path.clone())
                .collect(),
            dir: self.dir().to_path_buf(),
            dir_other: self.dir_other().to_path_buf(),
        }
    }
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
    pub version_retention: crate::operation::VersionRetentionPolicy,
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
    /// Separates egui scroll memory from earlier openings of this dialog.
    pub scroll_nonce: u64,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HistoryReplayMode {
    Undo,
    Redo,
}

pub(crate) struct HistoryPreviewState {
    pub mode: HistoryReplayMode,
    pub preview: crate::undo::ReplayPreview,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RecoverySection {
    #[default]
    Operations,
    Staging,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RecoveryDetail {
    #[default]
    Inspect,
    Repair,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum OperationsTab {
    #[default]
    Queue,
    History,
    Errors,
    Recovery,
}

impl OperationsTab {
    pub(crate) const ALL: [Self; 4] = [Self::Queue, Self::History, Self::Errors, Self::Recovery];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Queue => "Queue",
            Self::History => "History",
            Self::Errors => "Errors",
            Self::Recovery => "Recovery",
        }
    }
}

#[derive(Default)]
pub(crate) struct RecoveryState {
    pub section: RecoverySection,
    pub detail: RecoveryDetail,
    pub operations: Vec<crate::operation_journal::OperationRecord>,
    pub orphans: Vec<crate::operation_journal::OrphanStaging>,
    pub selected: Option<crate::operation::OperationId>,
    pub repair_plan: Option<crate::operation_journal::RepairPlan>,
    pub versions: Vec<crate::version_store::VersionRecord>,
    pub error: Option<String>,
    pub outcome: Option<String>,
    pub scanning: bool,
    pub repair_loaded: bool,
    pub scan_rx: Option<std::sync::mpsc::Receiver<RecoveryScanResult>>,
}

pub(crate) struct RecoveryScanResult {
    pub inventory: Result<crate::operation_journal::RecoveryInventory, String>,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        context_menu: Rc<dyn crate::ports::ContextMenuPort>,
        clipboard: Rc<dyn crate::ports::ClipboardPort>,
        opener: Rc<dyn crate::ports::OpenerPort>,
        trash: std::sync::Arc<dyn crate::ports::TrashPort>,
        free_space: std::sync::Arc<dyn crate::ports::FreeSpacePort>,
    ) -> Self {
        let mut startup = crate::measurement::StartupTrace::start();
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let session = crate::session::load();
        if let Some(saved) = &session {
            crate::panel::restore_visit_snapshot(&saved.recent_paths, &saved.recent_stats);
        }
        startup.checkpoint(crate::measurement::StartupPhase::SessionRestore);

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
        let accessibility_preferences = crate::accessibility::Preferences::system();
        debug_assert!(
            crate::accessibility::control_audit_failures(&crate::accessibility::CONTROL_CATALOG)
                .is_empty()
        );
        debug_assert_eq!(
            crate::accessibility::visual_channel_snapshot()
                .lines()
                .count(),
            crate::accessibility::VisualChannel::ALL.len()
        );
        apply_theme(&cc.egui_ctx, mode, accessibility_preferences);
        startup.checkpoint(crate::measurement::StartupPhase::Appearance);

        let (left, right) = session
            .as_ref()
            .map(|s| s.sanitized_paths(&home))
            .unwrap_or_else(|| (home.clone(), home.clone()));
        let mut ws = Workspace::with_ports(left, right, trash, free_space);

        let ui_scale =
            crate::accessibility::sanitize_text_scale(session.as_ref().map_or(1.0, |s| s.ui_scale));
        cc.egui_ctx.set_zoom_factor(ui_scale);

        if let Some(s) = &session {
            ws.active = if s.active_left {
                ActivePanel::Left
            } else {
                ActivePanel::Right
            };
            ws.left.restore_view_config(crate::panel::ViewConfig {
                sort_col: s.left_sort_col,
                sort_order: s.left_sort_order,
                show_hidden: s.left_hidden,
                folders_first: s.left_folders_first,
                natural_name_sort: s.left_natural_sort,
                density: s.left_density,
            });
            ws.right.restore_view_config(crate::panel::ViewConfig {
                sort_col: s.right_sort_col,
                sort_order: s.right_sort_order,
                show_hidden: s.right_hidden,
                folders_first: s.right_folders_first,
                natural_name_sort: s.right_natural_sort,
                density: s.right_density,
            });
            ws.durability_profile = s.durability_profile;
            ws.version_retention = s.version_retention;
            ws.sync_guard_policy = s.sync_guard_policy.clone();
            ws.name_policy = s.name_policy;
            ws.symlink_policy = s.symlink_policy;
        }
        startup.checkpoint(crate::measurement::StartupPhase::WorkspaceRestore);

        let recovery = RecoveryState::scan(&ws);
        startup.checkpoint(crate::measurement::StartupPhase::RecoveryScan);
        let image_cache = crate::image_cache::ImageCache::new();
        let content_index =
            crate::content_index::ContentIndex::load(crate::workload::global_handle());
        let project_collections = crate::collections::load();
        startup.checkpoint(crate::measurement::StartupPhase::StoreLoad);
        let mut app = App {
            ws,
            ui: ui_state::UiState::default(),
            context_menu,
            clipboard,
            opener,
            ui_scale,
            theme_mode: mode,
            colors: ThemeColors::for_preferences(mode, accessibility_preferences),
            accessibility_preferences,
            prev_window_width: 0.0,
            image_cache,
            show_tree: session.as_ref().is_some_and(|s| s.show_tree),
            tree_expanded: std::collections::HashSet::new(),
            tree_children_cache: std::collections::HashMap::new(),
            tree_width: session.as_ref().map_or(200.0, |s| s.tree_width),
            show_size_bars: session.as_ref().is_some_and(|s| s.show_size_bars),
            show_compare: session.as_ref().is_some_and(|s| s.show_compare),
            show_operations_center: false,
            operations_tab: OperationsTab::default(),
            operations_search: String::new(),
            operation_failures: crate::operation_view::FailureInbox::default(),
            failure_notice_seen: std::collections::HashSet::new(),
            recent_order: session
                .as_ref()
                .map_or(crate::panel::RecentOrder::Frecency, |s| s.recent_order),
            search_engine: crate::search::SearchEngine::default(),
            search_history: session
                .as_ref()
                .map(|s| s.search_history.clone())
                .unwrap_or_default(),
            content_index,
            toasts: crate::toasts::ToastQueue::default(),
            receipts: crate::receipts::ReceiptLog::default(),
            recovery,
            palette_usage: session
                .as_ref()
                .map(|s| s.palette_usage.clone())
                .unwrap_or_default(),
            palette_tick: session.as_ref().map_or(0, |s| s.palette_tick),
            smart_folders: None,
            project_collections,
            command_templates: None,
            compare_cache: None,
            startup_trace: Some(startup),
            show_developer_panel: false,
            developer_notice: None,
            persistence_issue_seen: 0,
        };
        if let Some(trace) = &mut app.startup_trace {
            trace.checkpoint(crate::measurement::StartupPhase::AppAssembly);
        }
        app
    }

    #[cfg(feature = "visual-qa")]
    pub(crate) fn new_visual_qa(
        cc: &eframe::CreationContext<'_>,
        seed: VisualQaSeed,
        context_menu: Rc<dyn crate::ports::ContextMenuPort>,
        clipboard: Rc<dyn crate::ports::ClipboardPort>,
        opener: Rc<dyn crate::ports::OpenerPort>,
        trash: std::sync::Arc<dyn crate::ports::TrashPort>,
        free_space: std::sync::Arc<dyn crate::ports::FreeSpacePort>,
    ) -> Self {
        apply_theme(
            &cc.egui_ctx,
            seed.theme_mode,
            seed.accessibility_preferences,
        );
        cc.egui_ctx.set_zoom_factor(seed.ui_scale);
        let ws = Workspace::with_ports_and_bookmarks(
            seed.left,
            seed.right,
            trash,
            free_space,
            crate::bookmarks::Bookmarks::default(),
        );
        Self {
            ws,
            ui: ui_state::UiState::default(),
            context_menu,
            clipboard,
            opener,
            ui_scale: seed.ui_scale,
            theme_mode: seed.theme_mode,
            colors: ThemeColors::for_preferences(seed.theme_mode, seed.accessibility_preferences),
            accessibility_preferences: seed.accessibility_preferences,
            prev_window_width: 0.0,
            image_cache: crate::image_cache::ImageCache::new(),
            show_tree: seed.show_tree,
            tree_expanded: std::collections::HashSet::new(),
            tree_children_cache: std::collections::HashMap::new(),
            tree_width: 200.0,
            show_size_bars: true,
            show_compare: false,
            show_operations_center: false,
            operations_tab: OperationsTab::default(),
            operations_search: String::new(),
            operation_failures: crate::operation_view::FailureInbox::default(),
            failure_notice_seen: std::collections::HashSet::new(),
            recent_order: crate::panel::RecentOrder::Frecency,
            search_engine: crate::search::SearchEngine::default(),
            search_history: crate::search::QueryHistory::default(),
            content_index: crate::content_index::ContentIndex::empty(
                crate::workload::global_handle(),
            ),
            toasts: crate::toasts::ToastQueue::default(),
            receipts: crate::receipts::ReceiptLog::default(),
            recovery: RecoveryState::default(),
            palette_usage: crate::command::UsageStats::default(),
            palette_tick: 0,
            smart_folders: None,
            project_collections: crate::collections::ProjectCollections::default(),
            command_templates: None,
            compare_cache: None,
            startup_trace: None,
            show_developer_panel: false,
            developer_notice: None,
            persistence_issue_seen: crate::persistence::issue_generation(),
        }
    }

    pub(crate) fn issue_transient_nonce(&mut self) -> u64 {
        self.ui.transient_nonce = self
            .ui
            .transient_nonce
            .checked_add(1)
            .expect("transient UI nonce space exhausted");
        self.ui.transient_nonce
    }

    pub(crate) fn mark_modal_opened(ctx: &egui::Context, modal: UiModal) {
        ctx.data_mut(|data| {
            data.insert_temp(egui::Id::new(("ui_request_opened", modal)), true);
        });
    }

    pub(crate) fn take_modal_opened(ctx: &egui::Context, modal: UiModal) -> bool {
        ctx.data_mut(|data| {
            let id = egui::Id::new(("ui_request_opened", modal));
            let opened = data.get_temp::<bool>(id).unwrap_or(false);
            data.remove::<bool>(id);
            opened
        })
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
        let left_view = self.ws.left.view_config();
        let right_view = self.ws.right.view_config();
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
            left_sort_col: left_view.sort_col,
            left_sort_order: left_view.sort_order,
            left_hidden: left_view.show_hidden,
            right_sort_col: right_view.sort_col,
            right_sort_order: right_view.sort_order,
            right_hidden: right_view.show_hidden,
            left_folders_first: left_view.folders_first,
            left_natural_sort: left_view.natural_name_sort,
            right_folders_first: right_view.folders_first,
            right_natural_sort: right_view.natural_name_sort,
            left_density: left_view.density,
            right_density: right_view.density,
            palette_usage: self.palette_usage.clone(),
            palette_tick: self.palette_tick,
            recent_paths,
            recent_stats,
            recent_order: self.recent_order,
            search_history: self.search_history.clone(),
            durability_profile: self.ws.durability_profile,
            version_retention: self.ws.version_retention,
            sync_guard_policy: self.ws.sync_guard_policy.clone(),
            name_policy: self.ws.name_policy,
            symlink_policy: self.ws.symlink_policy,
        }
    }

    /// Confirm the pending operation; all filesystem work completes through
    /// background controller polling.
    pub(crate) fn confirm_pending_op(&mut self, ctx: &egui::Context) {
        let ctx2 = ctx.clone();
        self.ws.confirm_pending_op(move || ctx2.request_repaint());
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
