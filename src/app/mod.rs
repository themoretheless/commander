//! UI layer: renders the [`Workspace`] and forwards input to it.
//! All file-manager behaviour lives in `crate::workspace`; this module
//! owns only presentation state (theme, zoom, image cache, tree widget).

mod activity;
mod archive_dialog;
mod batch_rename_dialog;
mod collections_dialog;
mod confirm_dialog;
mod developer_panel;
mod diff_dialog;
mod duplicates_dialog;
mod file_list;
mod find_dialog;
pub mod glyphs;
mod history_dialog;
mod keyboard_help;
mod keys;
mod mask_dialog;
mod omnibar_dialog;
mod permissions_dialog;
mod preload;
mod queue_dialog;
mod recovery_dialog;
mod rename_dialog;
mod render;
mod run_command_dialog;
mod safe_state_dialog;
mod saved_search_dialog;
#[cfg(test)]
mod smoke;
mod sync_dialog;
mod tabs;
mod toolbar;
mod tree;
mod treemap_dialog;
mod ui_state;
mod update;

use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, Sense, Stroke, Vec2};
use std::path::PathBuf;
use std::rc::Rc;

use crate::panel::{PanelState, SortColumn, format_size};
use crate::theme::{ThemeColors, ThemeMode, apply_theme};
pub use crate::transfer::{CopyMethod, TransferKind};
pub use crate::ui_request::{UiModal, UiRequest};
pub use crate::workspace::{ActivePanel, PendingOp, Workspace};

pub struct AppServices {
    pub(crate) context_menu: Rc<dyn crate::ports::ContextMenuPort>,
    pub(crate) clipboard: Rc<dyn crate::ports::ClipboardPort>,
    pub(crate) opener: Rc<dyn crate::ports::OpenerPort>,
    pub(crate) trash: std::sync::Arc<dyn crate::ports::TrashPort>,
    pub(crate) free_space: std::sync::Arc<dyn crate::ports::FreeSpacePort>,
    pub(crate) persistence: std::sync::Arc<dyn crate::persistence::Persist>,
    pub(crate) workload: crate::workload::WorkloadHandle,
    pub(crate) directory_probe: std::sync::Arc<dyn crate::pathname::DirectoryProbePort>,
}

pub struct App {
    /// UI-independent application core (panels, ops, transfers).
    pub ws: Workspace,
    /// Transient interaction state and the complete set of app-owned modals.
    pub(crate) ui: ui_state::UiState,
    /// Main-thread-owned desktop integration injected by the composition root.
    pub(crate) context_menu: Rc<dyn crate::ports::ContextMenuPort>,
    pub(crate) clipboard: Rc<dyn crate::ports::ClipboardPort>,
    pub(crate) opener: Rc<dyn crate::ports::OpenerPort>,
    pub(crate) workload: crate::workload::WorkloadHandle,
    pub(crate) directory_probe: std::sync::Arc<dyn crate::pathname::DirectoryProbePort>,
    pub(crate) persistence: std::sync::Arc<dyn crate::persistence::Persist>,
    pub(crate) session_gate: crate::persistence::StoreGate,
    pub ui_scale: f32,
    pub theme_mode: ThemeMode,
    /// Track the OS appearance each frame instead of a fixed `theme_mode`.
    pub(crate) follow_system_theme: bool,
    pub(crate) theme_palette: Option<String>,
    pub colors: ThemeColors,
    pub(crate) accessibility_preferences: crate::accessibility::Preferences,
    /// Re-reads [`Self::accessibility_preferences`] on window focus gain.
    pub(crate) accessibility_watch: crate::accessibility::PreferenceWatch,
    pub(crate) prev_window_width: f32,
    pub(crate) image_cache: crate::image_cache::ImageCache,
    pub(crate) pdf_cache: crate::pdf_preview::Cache,
    pub(crate) show_tree: bool,
    pub(crate) tree_expanded: std::collections::HashSet<PathBuf>,
    tree_children_cache: tree::ChildCache,
    tree_child_loader: tree::ChildLoader,
    /// The selection HUD's text for the selection it last summarized.
    pub(crate) selection_hud_cache: Option<update::SelectionHudCache>,
    pub(crate) drag_announcement: update::DragAnnouncementMemo,
    pub(crate) tree_width: f32,
    /// The window's inner size in points as last seen while not maximized
    /// or fullscreen, saved with the session.
    pub(crate) window_size: Option<[f32; 2]>,
    /// Paint relative size occupancy bars behind file rows.
    pub(crate) show_size_bars: bool,
    /// Compare mode: tint each row by how it differs from the other panel.
    pub(crate) show_compare: bool,
    /// Keep the key bar visible; otherwise it appears while a modifier is held.
    pub(crate) show_key_bar: bool,
    /// Open egui context menu (panel, target, anchor) on platforms without
    /// a native one.
    pub(crate) fallback_menu: Option<(ActivePanel, PathBuf, egui::Pos2)>,
    /// Whether the unified queue/history/errors/recovery surface is visible.
    pub(crate) show_operations_center: bool,
    pub(crate) operations_tab: OperationsTab,
    pub(crate) operations_search: String,
    pub(crate) operation_failures: crate::operation_view::FailureInbox,
    pub(crate) assistive_timeline: crate::assistive_timeline::AssistiveTimeline,
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
    pub(crate) rename_presets: crate::rename_presets::Presets,
    /// Cached cross-panel compare maps and the panel generations they were built
    /// from, so compare mode does not rebuild two `HashMaps` (cloning every
    /// `name_lower`) on every painted frame. `(right_gen, left_gen, left_map,
    /// right_map)`: `left_map` indexes the right panel and vice versa.
    pub(crate) compare_cache: Option<(
        u64,
        u64,
        crate::compare::CompareMap,
        crate::compare::CompareMap,
    )>,
    /// Data the confirm dialog derives from the pending operation once
    /// instead of on every repaint of its flow animation.
    pub(crate) confirm_cache: confirm_dialog::ConfirmCache,
    /// Startup instrumentation remains live through the first directory read.
    pub(crate) startup_trace: Option<crate::measurement::StartupTrace>,
    /// Entries the panels focus once their first listing is scheduled (a
    /// file launch argument), taken on the first frame.
    pub(crate) launch_focus: (Option<String>, Option<String>),
    pub(crate) show_developer_panel: bool,
    pub(crate) developer_notice: Option<DeveloperNotice>,
    pub(crate) developer_store_usage: developer_panel::StoreUsageCache,
    pub(crate) persistence_issue_seen: u64,
    /// Last background-helper failure generation already shown as a toast.
    pub(crate) helper_failure_seen: u64,
    /// Close the native window once all user-cancelled mutations retire.
    pub(crate) quit_when_operations_settle: bool,
    pub(crate) quit_confirmation: bool,
    pub(crate) hex_viewer: Option<crate::hex_preview::HexViewer>,
    checksum_task: Option<ChecksumTask>,
    checksum_report: Option<crate::checksum::ChecksumReport>,
    checksum_generation: u64,
    checksum_expected: String,
    checksum_algorithm: crate::checksum::Algorithm,
}

struct ChecksumTask {
    receiver: std::sync::mpsc::Receiver<crate::checksum::ChecksumReport>,
    task: crate::workload::TaskHandle,
    completed: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    total: usize,
    cancel_requested: bool,
}

#[cfg(feature = "visual-qa")]
pub struct VisualQaSeed {
    pub left: PathBuf,
    pub right: PathBuf,
    pub ui_scale: f32,
    pub theme_mode: ThemeMode,
    pub accessibility_preferences: crate::accessibility::Preferences,
    pub show_tree: bool,
}

/// Frame time in whole milliseconds, the clock receipts, timeline entries and
/// failure notices are stamped with.
pub fn frame_millis(ctx: &egui::Context) -> u64 {
    secs_to_millis(ctx.input(|input| input.time))
}

/// Seconds of egui time as whole milliseconds. The cast saturates, so a
/// negative or NaN time reads as 0 rather than wrapping.
pub fn secs_to_millis(secs: f64) -> u64 {
    (secs * 1_000.0) as u64
}

/// Everything a worker channel holds right now, and whether its sender is
/// gone. Events sent before the disconnect are still returned.
pub fn drain_events<T>(
    mut try_recv: impl FnMut() -> Result<T, std::sync::mpsc::TryRecvError>,
) -> (Vec<T>, bool) {
    let mut events = Vec::new();
    loop {
        match try_recv() {
            Ok(event) => events.push(event),
            Err(std::sync::mpsc::TryRecvError::Empty) => return (events, false),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return (events, true),
        }
    }
}

/// The frame every dialog and sheet draws: panel fill, `margin` points of
/// padding and a one-point border.
pub fn dialog_frame(t: ThemeColors, margin: i8) -> Frame {
    Frame::NONE
        .fill(t.bg_panel)
        .inner_margin(Margin::same(margin))
        .stroke(Stroke::new(1.0_f32, t.border))
}

/// A fixed-size, untitled sheet centered on the window. `id` names the
/// window for egui and assistive technology; it is not drawn.
pub fn sheet_window(
    id: impl Into<egui::WidgetText>,
    t: ThemeColors,
    margin: i8,
) -> egui::Window<'static> {
    egui::Window::new(id)
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(dialog_frame(t, margin))
}

/// Fixed fill/text pairings for dialog buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonRole {
    /// The dialog's main action (accent fill, white text).
    Primary,
    /// Cancel and neutral actions.
    Secondary,
    /// Destructive actions.
    Danger,
    /// Stop/interrupt actions.
    Warning,
}

impl ButtonRole {
    const fn colors(self, t: ThemeColors) -> (Color32, Color32) {
        match self {
            Self::Primary => (t.accent, Color32::WHITE),
            Self::Secondary => (t.bg_card, t.text_primary),
            Self::Danger => (t.accent_red, Color32::WHITE),
            Self::Warning => (t.accent_warning, Color32::WHITE),
        }
    }
}

/// A dialog button styled for `role`; callers keep sizing their own text.
pub fn dialog_button<'a>(
    t: ThemeColors,
    role: ButtonRole,
    text: egui::RichText,
) -> egui::Button<'a> {
    let (fill, foreground) = role.colors(t);
    egui::Button::new(text.color(foreground))
        .fill(fill)
        .corner_radius(CornerRadius::ZERO)
}

pub struct DeveloperNotice {
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
pub struct RunCommandState {
    pub template_name: String,
    pub template_extensions: String,
    /// The editable command line (placeholders expand against the selection).
    pub line: String,
    /// Separates egui scroll memory from earlier openings of this dialog.
    pub scroll_nonce: u64,
    /// Immutable panel/selection context captured when the dialog opens.
    pub opening: RunCommandOpeningContext,
    pub run: Option<CommandOutputRun>,
    pub output: Option<CommandOutput>,
}

pub struct CommandOutputRun {
    pub receiver: std::sync::mpsc::Receiver<CommandOutput>,
    /// The running process, shared with its worker so Stop (or closing the
    /// bar, via `Drop`) can kill it instead of leaving it running unseen.
    pub child: CommandChildSlot,
}

pub type CommandChildSlot = std::sync::Arc<std::sync::Mutex<Option<std::process::Child>>>;

impl CommandOutputRun {
    pub fn stop(&self) {
        if let Some(child) = crate::lock_util::recover(&self.child).as_mut() {
            // The shell leads its own process group (see `run_shell`); kill
            // the whole group so forked pipeline stages and `a; b` steps stop
            // too and release the output pipes. The slot is cleared once the
            // pipes are drained, so the group id is still ours here.
            #[cfg(unix)]
            if let Ok(pgid) = libc::pid_t::try_from(child.id()) {
                // SAFETY: plain syscall; a negative pid targets the group.
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            }
            // std never signals a child it has already reaped, so this is
            // safe after normal completion too.
            let _ = child.kill();
        }
    }
}

impl Drop for CommandOutputRun {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone, Debug)]
pub struct CommandOutput {
    pub cmdline: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct RunCommandOpeningContext {
    pub active_panel: ActivePanel,
    pub selection: Vec<crate::panel::FileEntry>,
    pub left_dir: PathBuf,
    pub right_dir: PathBuf,
    /// The cursor pointed at a row that no longer exists when the dialog
    /// opened, so `selection` is empty rather than the item the user meant.
    pub stale_cursor: bool,
}

impl RunCommandOpeningContext {
    fn capture(workspace: &Workspace) -> Self {
        let active_panel = workspace.active;
        let active = workspace.active_panel_ref();
        let picked = active.selected_or_cursor();
        Self {
            active_panel,
            stale_cursor: picked.is_err(),
            selection: picked.unwrap_or_default(),
            left_dir: workspace.left.current_path.clone(),
            right_dir: workspace.right.current_path.clone(),
        }
    }

    /// Warning shown in the dialog when the selection was lost.
    fn stale_cursor_warning(&self) -> Option<&'static str> {
        self.stale_cursor.then_some(
            "The cursor item is no longer in the list, so no item is selected. \
             Close this dialog, pick the item again and reopen it.",
        )
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
pub enum DiskUsageMode {
    #[default]
    Map,
    Tree,
}

pub struct DiskUsageState {
    pub initial: crate::workspace::TreemapSnapshot,
    pub mode: DiskUsageMode,
    pub run: Option<crate::tree_overview::OverviewRun>,
    pub progress: crate::tree_overview::ScanProgress,
    pub overview: Option<crate::tree_overview::OverviewSnapshot>,
    pub stopping: bool,
    pub error: Option<String>,
}

pub struct ArchiveState {
    pub path: PathBuf,
    pub filter: String,
    pub run: Option<crate::archive::ListingRun>,
    pub extract: Option<crate::archive::ExtractRun>,
    pub listing: Option<crate::archive::ArchiveListing>,
    /// Filter matches for `listing`; reset whenever the listing is replaced.
    pub member_filter: archive_dialog::MemberFilter,
    pub selected: std::collections::BTreeSet<usize>,
    pub status: Option<String>,
    pub error: Option<String>,
}

/// UI state for the recursive-find sheet. The matching lives in `crate::query`;
/// this holds the editable fields, the search root, and the results.
pub struct FindState {
    pub expression: String,
    pub mode: crate::query::MatchMode,
    pub root: PathBuf,
    pub results: Vec<crate::search::SearchHit>,
    pub run: Option<crate::search::SearchRun>,
    pub generation: u64,
    pub ran: bool,
    pub scanned: usize,
    pub matched: usize,
    pub elapsed: std::time::Duration,
    pub truncated: bool,
    pub content_skipped: usize,
    /// The first archive open error of the last search, shown with the
    /// skipped count.
    pub archive_error: Option<String>,
    pub error: Option<String>,
    pub stable_order: Vec<crate::search::FileIdentity>,
    pub last_query: Option<(String, crate::query::MatchMode, PathBuf)>,
    pub time_pivot: crate::search::TimePivot,
    pub history_open: bool,
    /// The hit whose "why matched" line is open. Keyed by identity and
    /// location: hard links share an identity, so identity alone would open
    /// the first link's explanation for every link.
    pub explanation_open: Option<find_dialog::ExplainKey>,
    /// Display-order row selected with Up/Down while the query field owns focus.
    pub highlighted_result: Option<usize>,
    pub highlight_scroll_pending: bool,
    pub pending_rerun: bool,
    pub last_edit_at: f64,
    pub search_source: String,
    pub index_details_open: bool,
    pub index_exclusions: String,
    pub index_rerun_after_build: bool,
    /// Name to save this query under (smart folder).
    pub save_name: String,
}

pub struct CollectionsDialogState {
    pub name: String,
    pub include_left: bool,
    pub include_right: bool,
    pub selected: Option<String>,
    pub rows: Vec<crate::collections::VirtualEntry>,
    pub run: Option<crate::collections::ViewRun>,
    pub scanned: usize,
    pub unavailable_roots: Vec<crate::collections::UnavailableRoot>,
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
            scanned: 0,
            matched: 0,
            elapsed: std::time::Duration::ZERO,
            truncated: false,
            content_skipped: 0,
            archive_error: None,
            error: None,
            stable_order: Vec::new(),
            last_query: None,
            time_pivot: crate::search::TimePivot::None,
            history_open: false,
            explanation_open: None,
            highlighted_result: None,
            highlight_scroll_pending: false,
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
    /// A search is in flight exactly while its run is held; completion,
    /// cancellation and a dead worker all drop it.
    pub(crate) const fn searching(&self) -> bool {
        self.run.is_some()
    }

    /// Build the query the current fields describe.
    pub(crate) fn build_query(&self) -> Result<crate::query::Query, crate::query::QueryError> {
        crate::query::Query::parse(&self.expression, self.mode)
    }

    /// Reconstruct the editable fields from a saved smart-folder definition.
    pub(crate) fn from_definition(def: &crate::smart_folder::Definition) -> Self {
        Self {
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
pub struct DiffState {
    pub name_a: String,
    pub name_b: String,
    pub lines: Vec<crate::textdiff::DiffLine>,
    pub message: Option<String>,
    pub keyboard_focused: bool,
    pub scroll_offset: egui::Vec2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ScrollCommand {
    #[default]
    LineUp,
    LineDown,
    LineLeft,
    LineRight,
    PageUp,
    PageDown,
    Home,
    End,
}

fn apply_scroll_command(
    offset: egui::Vec2,
    command: ScrollCommand,
    page_height: f32,
) -> egui::Vec2 {
    const LINE: f32 = 24.0;
    match command {
        ScrollCommand::LineUp => egui::vec2(offset.x, (offset.y - LINE).max(0.0)),
        ScrollCommand::LineDown => egui::vec2(offset.x, offset.y + LINE),
        ScrollCommand::LineLeft => egui::vec2((offset.x - LINE).max(0.0), offset.y),
        ScrollCommand::LineRight => egui::vec2(offset.x + LINE, offset.y),
        ScrollCommand::PageUp => egui::vec2(offset.x, (offset.y - page_height).max(0.0)),
        ScrollCommand::PageDown => egui::vec2(offset.x, offset.y + page_height),
        ScrollCommand::Home => egui::vec2(offset.x, 0.0),
        ScrollCommand::End => egui::vec2(offset.x, f32::MAX),
    }
}

/// UI state for the duplicate-finder sheet. Grouping lives in `crate::dedup`;
/// this holds the groups, the per-group keep choice, and the policy.
pub struct DupState {
    pub groups: Vec<crate::dedup::DupGroup>,
    /// Index (into each group's `files`) of the file to keep; `None` keeps
    /// every copy of that group ("Keep all").
    pub keep: Vec<Option<usize>>,
    pub policy: crate::dedup::KeepPolicy,
    /// The background hash scan while it runs; `groups` fill in when done.
    pub scan: Option<crate::dedup_scan::DupScan>,
}

/// UI state for the directory-synchronise sheet. The diff lives in
/// `crate::sync`; this holds the chosen policy and the editable action rows.
pub struct SyncState {
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
    /// When true, Apply only reports the planned actions without transferring.
    pub dry_run: bool,
    /// Plan being built off the UI thread; Apply waits for it.
    pub scan: Option<sync_dialog::PlanScan>,
    /// Rows whose earlier edits overrode the new policy on the last policy
    /// switch; shown as a note so the switch does not look ignored.
    pub kept_edits: usize,
}

/// UI state for the batch-rename studio. The transform itself lives in
/// `crate::rename`; this only holds the editable rule fields.
pub struct BatchRenameState {
    pub context: crate::workspace::BatchRenameContext,
    /// Panel listing revision used to build `context.existing`. The selected
    /// targets stay fixed, but sibling collisions follow watcher refreshes.
    pub listing_generation: u64,
    pub remove_prefix: String,
    pub remove_suffix: String,
    pub find: String,
    pub replace: String,
    /// Treat `find` as a regular expression (`$1`-style groups in `replace`)
    /// instead of a literal substring.
    pub regex_mode: bool,
    pub ignore_case: bool,
    pub first_only: bool,
    pub trim_spaces: bool,
    pub collapse_repeats: bool,
    pub normalization: crate::rename::NameNormalization,
    pub stem_limit_on: bool,
    pub stem_limit: u32,
    pub replace_target: crate::rename::ReplaceTarget,
    pub prefix: String,
    pub suffix: String,
    pub case: crate::rename::CaseMode,
    pub extension_case: crate::rename::ExtensionCase,
    pub numbering_on: bool,
    pub number_position: crate::rename::NumberPosition,
    pub num_start: i64,
    pub num_step: i64,
    pub num_pad: u32,
    pub preset_name: String,
    pub preset_selected: Option<String>,
    pub preset_status: Option<Result<String, String>>,
    pub error: Option<String>,
    /// Separates egui scroll memory from earlier openings of this dialog.
    pub scroll_nonce: u64,
    /// Plan for the last rule shown; rebuilt only when the rule changes.
    pub validation: Option<batch_rename_dialog::ValidatedBatchRename>,
}

impl BatchRenameState {
    fn reset_rule(&mut self) {
        self.load_rule(&crate::rename::RenameRule::default());
        self.num_start = 1;
        self.num_step = 1;
        self.num_pad = 2;
        self.stem_limit = 32;
        self.preset_name.clear();
        self.preset_selected = None;
        self.preset_status = Some(Ok("Rule reset".into()));
        self.error = None;
    }

    fn load_rule(&mut self, rule: &crate::rename::RenameRule) {
        self.remove_prefix = rule.remove_prefix.clone();
        self.remove_suffix = rule.remove_suffix.clone();
        self.find = rule.find.clone();
        self.replace = rule.replace.clone();
        self.regex_mode = rule.regex;
        self.ignore_case = rule.ignore_case;
        self.first_only = rule.first_only;
        self.trim_spaces = rule.trim_spaces;
        self.collapse_repeats = rule.collapse_repeats;
        self.normalization = rule.normalization;
        self.stem_limit_on = rule.stem_limit.is_some();
        if let Some(limit) = rule.stem_limit {
            self.stem_limit = limit as u32;
        }
        self.replace_target = rule.replace_target;
        self.prefix = rule.prefix.clone();
        self.suffix = rule.suffix.clone();
        self.case = rule.case;
        self.extension_case = rule.extension_case;
        self.numbering_on = rule.numbering.is_some();
        self.number_position = rule.number_position;
        if let Some(numbering) = rule.numbering {
            self.num_start = numbering.start;
            self.num_step = numbering.step;
            self.num_pad = numbering.pad as u32;
        }
        self.validation = None;
    }
    /// Build the pure rename rule from the current field values.
    pub(crate) fn rule(&self) -> crate::rename::RenameRule {
        crate::rename::RenameRule {
            remove_prefix: self.remove_prefix.clone(),
            remove_suffix: self.remove_suffix.clone(),
            find: self.find.clone(),
            replace: self.replace.clone(),
            regex: self.regex_mode,
            ignore_case: self.ignore_case,
            first_only: self.first_only,
            trim_spaces: self.trim_spaces,
            collapse_repeats: self.collapse_repeats,
            normalization: self.normalization,
            stem_limit: self.stem_limit_on.then_some(self.stem_limit as usize),
            replace_target: self.replace_target,
            prefix: self.prefix.clone(),
            suffix: self.suffix.clone(),
            case: self.case,
            extension_case: self.extension_case,
            number_position: self.number_position,
            numbering: self.numbering_on.then_some(crate::rename::Numbering {
                start: self.num_start,
                step: self.num_step,
                pad: self.num_pad as usize,
            }),
        }
    }
}

/// UI state for the rename editor.
pub struct RenameState {
    pub path: PathBuf,
    pub siblings: Vec<String>,
    pub buffer: String,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryReplayMode {
    Undo,
    Redo,
}

pub struct HistoryPreviewState {
    pub mode: HistoryReplayMode,
    pub preview: crate::undo::ReplayPreview,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RecoverySection {
    #[default]
    Operations,
    Staging,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RecoveryDetail {
    #[default]
    Inspect,
    Repair,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OperationsTab {
    #[default]
    Queue,
    History,
    Errors,
    Recovery,
}

impl OperationsTab {
    pub(crate) const ALL: [Self; 4] = [Self::Queue, Self::History, Self::Errors, Self::Recovery];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Queue => "Queue",
            Self::History => "History",
            Self::Errors => "Errors",
            Self::Recovery => "Recovery",
        }
    }
}

#[derive(Default)]
pub struct RecoveryState {
    pub section: RecoverySection,
    pub detail: RecoveryDetail,
    pub operations: Vec<crate::operation_journal::OperationRecord>,
    pub orphans: Vec<crate::operation_journal::OrphanStaging>,
    pub retained_staging: Vec<crate::operation_journal::RetainedStaging>,
    pub selected: Option<crate::operation::OperationId>,
    /// An operation explicitly opened (from a toast or the queue) whose scan
    /// is still pending; it stays selected even if that scan omits it.
    pub requested: Option<crate::operation::OperationId>,
    pub repair_plan: Option<crate::operation_journal::RepairPlan>,
    pub versions: Vec<crate::version_store::VersionRecord>,
    pub error: Option<String>,
    pub outcome: Option<RecoveryOutcome>,
    /// Set after Resume queues a transfer; the list is rescanned once the
    /// transfer queue drains so the resumed operation's status is current.
    pub rescan_after_resume: bool,
    pub scanning: bool,
    pub repair_loaded: bool,
    pub scan_rx: Option<std::sync::mpsc::Receiver<RecoveryScanResult>>,
    /// Background reconcile/build started by Resume. The workspace holds a
    /// mutation gate until this result is consumed.
    pub resume_rx: Option<std::sync::mpsc::Receiver<ResumePreparationResult>>,
    pub preparing_resume: Option<crate::operation::OperationId>,
    pub rollback_rx: Option<std::sync::mpsc::Receiver<RollbackPreparationResult>>,
    pub preparing_rollback: Option<crate::operation::OperationId>,
    /// Pending background load of the selected operation's repair plan.
    pub repair_rx: Option<std::sync::mpsc::Receiver<RepairLoadResult>>,
    /// Version-store usage line and when it was read from the on-disk
    /// manifest. Dropped when a scan completes (scans follow rollback and
    /// cleanup) and re-read after `VERSION_USAGE_REFRESH` so versions saved
    /// by a running transfer show up.
    pub version_usage_label: Option<(std::time::Instant, String)>,
}

/// The result line of the last cleanup or rollback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryOutcome {
    pub message: String,
    /// Items still need the user's attention, so the line is a warning.
    pub needs_review: bool,
}

pub struct RecoveryScanResult {
    pub inventory: Result<crate::operation_journal::RecoveryInventory, String>,
}

pub struct ResumePreparationResult {
    pub operation_id: crate::operation::OperationId,
    pub spec: Result<crate::transfer::TransferSpec, String>,
}

pub struct RollbackPreparationResult {
    pub operation_id: crate::operation::OperationId,
    pub plan: Result<crate::operation_journal::RepairPlan, String>,
}

pub struct RepairLoadResult {
    pub operation_id: crate::operation::OperationId,
    pub plan: Result<crate::operation_journal::RepairPlan, String>,
    pub versions: Result<Vec<crate::version_store::VersionRecord>, String>,
}

/// What a constructor supplies to [`App::assemble`]: the workspace, the
/// injected ports and the stores it loaded.
struct AppParts {
    ws: Workspace,
    context_menu: Rc<dyn crate::ports::ContextMenuPort>,
    clipboard: Rc<dyn crate::ports::ClipboardPort>,
    opener: Rc<dyn crate::ports::OpenerPort>,
    workload: crate::workload::WorkloadHandle,
    directory_probe: std::sync::Arc<dyn crate::pathname::DirectoryProbePort>,
    persistence: std::sync::Arc<dyn crate::persistence::Persist>,
    session_gate: crate::persistence::StoreGate,
    ui_scale: f32,
    theme_mode: ThemeMode,
    accessibility_preferences: crate::accessibility::Preferences,
    image_cache: crate::image_cache::ImageCache,
    content_index: crate::content_index::ContentIndex,
    recovery: RecoveryState,
    project_collections: crate::collections::ProjectCollections,
}

/// The panels' start folders and the entry each one focuses. Launch paths win
/// over the restored ones, and a single launch path opens in both panels so
/// the right one does not show an unrelated restored folder beside it. A file
/// launch argument puts the cursor on that file wherever its folder opens.
fn resolve_start_paths(
    restored: (PathBuf, PathBuf),
    launch: crate::launch::LaunchPaths,
) -> ((PathBuf, PathBuf), (Option<String>, Option<String>)) {
    match (launch.left, launch.right) {
        (Some(left), Some(right)) => ((left, right), (launch.left_focus, launch.right_focus)),
        (Some(left), None) => (
            (left.clone(), left),
            (launch.left_focus.clone(), launch.left_focus),
        ),
        (None, Some(right)) => ((restored.0, right), (None, launch.right_focus)),
        (None, None) => (restored, (None, None)),
    }
}

impl App {
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        services: AppServices,
        launch: crate::launch::LaunchPaths,
    ) -> Self {
        let AppServices {
            context_menu,
            clipboard,
            opener,
            trash,
            free_space,
            persistence,
            workload,
            directory_probe,
        } = services;
        let mut startup = crate::measurement::StartupTrace::start();
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let loaded_session = crate::session::load_with(persistence.as_ref());
        let session_gate = loaded_session.gate;
        let session = loaded_session.value;
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

        let ((left, right), (left_focus, right_focus)) = resolve_start_paths(
            session
                .as_ref()
                .map(|s| s.sanitized_paths(&home))
                .unwrap_or_else(|| (home.clone(), home.clone())),
            launch,
        );
        let views = session
            .as_ref()
            .map(crate::session::Session::view_configs)
            .unwrap_or([crate::panel::ViewConfig::default(); 2]);
        let mut ws = Workspace::with_ports_and_views(
            left,
            right,
            views,
            trash,
            free_space,
            persistence.clone(),
        );
        let ui_scale =
            crate::accessibility::sanitize_text_scale(session.as_ref().map_or(1.0, |s| s.ui_scale));
        cc.egui_ctx.set_zoom_factor(ui_scale);
        let restored_window_size =
            crate::session::restorable_window_size(session.as_ref().and_then(|s| s.window_size));
        if let Some([width, height]) = restored_window_size {
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(width, height)));
        }

        if let Some(s) = &session {
            ws.active = if s.active_left {
                ActivePanel::Left
            } else {
                ActivePanel::Right
            };
            ws.restore_tabs(&s.left_tabs, &s.right_tabs);
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
        crate::version_store::set_store_quota(crate::operation::VersionStoreQuota::default());
        let mut app = Self {
            show_tree: session.as_ref().is_some_and(|s| s.show_tree),
            tree_expanded: session.as_ref().map_or_else(Default::default, |s| {
                s.tree_expanded
                    .iter()
                    .take(crate::session::TREE_EXPANDED_CAP)
                    .cloned()
                    .collect()
            }),
            tree_width: session.as_ref().map_or(200.0, |s| s.tree_width),
            window_size: restored_window_size,
            show_size_bars: session.as_ref().is_some_and(|s| s.show_size_bars),
            show_compare: session.as_ref().is_some_and(|s| s.show_compare),
            show_key_bar: session.as_ref().is_some_and(|s| s.show_key_bar),
            // A first launch has no saved choice, so it follows the system.
            follow_system_theme: session.as_ref().is_none_or(|s| s.theme_follow_system),
            recent_order: session
                .as_ref()
                .map_or(crate::panel::RecentOrder::Frecency, |s| s.recent_order),
            rename_presets: session
                .as_ref()
                .map(|s| s.rename_presets.clone())
                .unwrap_or_default(),
            search_history: session
                .as_ref()
                .map(|s| s.search_history.clone())
                .unwrap_or_default(),
            palette_usage: session
                .as_ref()
                .map(|s| s.palette_usage.clone())
                .unwrap_or_default(),
            palette_tick: session.as_ref().map_or(0, |s| s.palette_tick),
            startup_trace: Some(startup),
            launch_focus: (left_focus, right_focus),
            ..Self::assemble(AppParts {
                ws,
                context_menu,
                clipboard,
                opener,
                workload,
                directory_probe,
                persistence,
                session_gate,
                ui_scale,
                theme_mode: mode,
                accessibility_preferences,
                image_cache,
                content_index,
                recovery,
                project_collections,
            })
        };
        app.theme_palette = session
            .as_ref()
            .and_then(|s| s.theme_palette.clone())
            .filter(|id| crate::theme_catalog::find(id).is_some());
        app.apply_current_palette(&cc.egui_ctx);
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
        let persistence = crate::persistence::ephemeral_persist();
        let ws = Workspace::with_ports_and_bookmarks(
            seed.left,
            seed.right,
            trash,
            free_space,
            crate::bookmarks::Bookmarks::default(),
            persistence.clone(),
        );
        Self {
            show_tree: seed.show_tree,
            show_size_bars: true,
            persistence_issue_seen: crate::persistence::issue_generation(),
            helper_failure_seen: crate::native_effect::helper_failure_generation(),
            quit_when_operations_settle: false,
            quit_confirmation: false,
            ..Self::assemble(AppParts {
                ws,
                context_menu,
                clipboard,
                opener,
                workload: crate::workload::global_handle(),
                directory_probe: std::sync::Arc::new(crate::pathname::FsDirectoryProbe),
                persistence,
                session_gate: crate::persistence::StoreGate::missing(),
                ui_scale: seed.ui_scale,
                theme_mode: seed.theme_mode,
                accessibility_preferences: seed.accessibility_preferences,
                image_cache: crate::image_cache::ImageCache::new(),
                content_index: crate::content_index::ContentIndex::empty(
                    crate::workload::global_handle(),
                ),
                recovery: RecoveryState::default(),
                project_collections: crate::collections::ProjectCollections::default(),
            })
        }
    }

    /// An App over `parts` with every view toggle, cache, log and transient
    /// surface at its empty default; each constructor overrides what its
    /// launch restores.
    fn assemble(parts: AppParts) -> Self {
        let AppParts {
            ws,
            context_menu,
            clipboard,
            opener,
            workload,
            directory_probe,
            persistence,
            session_gate,
            ui_scale,
            theme_mode,
            accessibility_preferences,
            image_cache,
            content_index,
            recovery,
            project_collections,
        } = parts;
        Self {
            ws,
            ui: ui_state::UiState::default(),
            context_menu,
            clipboard,
            opener,
            workload,
            directory_probe,
            persistence,
            session_gate,
            ui_scale,
            theme_mode,
            colors: ThemeColors::for_preferences(theme_mode, accessibility_preferences),
            accessibility_preferences,
            accessibility_watch: crate::accessibility::PreferenceWatch::default(),
            prev_window_width: 0.0,
            pdf_cache: crate::pdf_preview::Cache::default(),
            image_cache,
            show_tree: false,
            tree_expanded: std::collections::HashSet::new(),
            tree_children_cache: std::collections::HashMap::new(),
            tree_child_loader: tree::ChildLoader::default(),
            selection_hud_cache: None,
            drag_announcement: update::DragAnnouncementMemo::default(),
            tree_width: 200.0,
            window_size: None,
            show_size_bars: false,
            show_compare: false,
            show_key_bar: false,
            follow_system_theme: false,
            theme_palette: None,
            fallback_menu: None,
            show_operations_center: false,
            operations_tab: OperationsTab::default(),
            operations_search: String::new(),
            operation_failures: crate::operation_view::FailureInbox::default(),
            assistive_timeline: crate::assistive_timeline::AssistiveTimeline::default(),
            failure_notice_seen: std::collections::HashSet::new(),
            recent_order: crate::panel::RecentOrder::Frecency,
            search_engine: crate::search::SearchEngine::default(),
            search_history: crate::search::QueryHistory::default(),
            content_index,
            toasts: crate::toasts::ToastQueue::default(),
            receipts: crate::receipts::ReceiptLog::default(),
            recovery,
            palette_usage: crate::command::UsageStats::default(),
            palette_tick: 0,
            smart_folders: None,
            project_collections,
            command_templates: None,
            rename_presets: crate::rename_presets::Presets::default(),
            compare_cache: None,
            confirm_cache: confirm_dialog::ConfirmCache::default(),
            startup_trace: None,
            launch_focus: (None, None),
            show_developer_panel: false,
            developer_notice: None,
            developer_store_usage: developer_panel::StoreUsageCache::default(),
            persistence_issue_seen: 0,
            helper_failure_seen: crate::native_effect::helper_failure_generation(),
            quit_when_operations_settle: false,
            quit_confirmation: false,
            hex_viewer: None,
            checksum_task: None,
            checksum_report: None,
            checksum_generation: 0,
            checksum_expected: String::new(),
            checksum_algorithm: crate::checksum::Algorithm::default(),
        }
    }

    /// Show a transient notice. `undoable` toasts carry the Undo affordance.
    pub(crate) fn toast(
        &mut self,
        ctx: &egui::Context,
        kind: crate::toasts::ToastKind,
        message: impl Into<String>,
        undoable: bool,
    ) {
        self.toast_at(ctx.input(|input| input.time), kind, message, undoable);
    }

    /// [`Self::toast`] for callers that only hold the frame time.
    pub(crate) fn toast_at(
        &mut self,
        now: f64,
        kind: crate::toasts::ToastKind,
        message: impl Into<String>,
        undoable: bool,
    ) {
        self.toasts
            .push(crate::toasts::Toast::new(message, kind, undoable, now));
    }

    pub(crate) const fn issue_transient_nonce(&mut self) -> u64 {
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

    /// Note that pane tool `tool` just opened, so its first field takes focus.
    pub(crate) fn mark_tool_opened(ctx: &egui::Context, tool: &'static str) {
        ctx.data_mut(|data| {
            data.insert_temp(egui::Id::new(("tool_opened", tool)), true);
        });
    }

    pub(crate) fn take_tool_opened(ctx: &egui::Context, tool: &'static str) -> bool {
        ctx.data_mut(|data| {
            let id = egui::Id::new(("tool_opened", tool));
            let opened = data.get_temp::<bool>(id).unwrap_or(false);
            data.remove::<bool>(id);
            opened
        })
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
            left_tabs: self.ws.tab_snapshot(ActivePanel::Left),
            right_tabs: self.ws.tab_snapshot(ActivePanel::Right),
            theme_dark: self.theme_mode == ThemeMode::Dark,
            theme_follow_system: self.follow_system_theme,
            theme_palette: self.theme_palette.clone(),
            ui_scale: self.ui_scale,
            show_tree: self.show_tree,
            tree_width: self.tree_width,
            window_size: self.window_size,
            show_size_bars: self.show_size_bars,
            show_compare: self.show_compare,
            show_key_bar: self.show_key_bar,
            left_view: crate::session::PersistedLeftView::from(left_view),
            right_view: crate::session::PersistedRightView::from(right_view),
            palette_usage: self.palette_usage.clone(),
            palette_tick: self.palette_tick,
            recent_paths,
            recent_stats,
            recent_order: self.recent_order,
            search_history: self.search_history.clone(),
            rename_presets: self.rename_presets.clone(),
            durability_profile: self.ws.durability_profile,
            version_retention: self.ws.version_retention,
            sync_guard_policy: self.ws.sync_guard_policy.clone(),
            name_policy: self.ws.name_policy,
            symlink_policy: self.ws.symlink_policy,
            tree_expanded: crate::session::persisted_tree_expanded(&self.tree_expanded),
        }
    }

    /// Confirm the pending operation; all filesystem work completes through
    /// background controller polling.
    pub(crate) fn confirm_pending_op(&mut self, ctx: &egui::Context) {
        let ctx2 = ctx.clone();
        self.ws.confirm_pending_op(move || ctx2.request_repaint());
    }

    /// Reveal `path` in the tree by expanding its ancestors; see
    /// [`tree::expand_ancestors`].
    pub(crate) fn tree_expand_to_path(&mut self, path: &std::path::Path) {
        tree::expand_ancestors(&mut self.tree_expanded, path);
    }

    /// Reveal `path` and expand it as well, for showing the tree on the
    /// current folder (a click must not do this; see `tree_expand_to_path`).
    /// Show or hide the folder sidebar; it opens on the active folder.
    pub(crate) fn toggle_folder_sidebar(&mut self) {
        self.show_tree = !self.show_tree;
        if self.show_tree {
            let path = self.ws.active_panel_ref().current_path.clone();
            self.tree_reveal_path(&path);
        }
    }

    pub(crate) fn tree_reveal_path(&mut self, path: &std::path::Path) {
        tree::expand_through(&mut self.tree_expanded, path);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ScrollCommand, apply_scroll_command, drain_events, resolve_start_paths, secs_to_millis,
    };
    use crate::launch::LaunchPaths;
    use std::path::PathBuf;

    #[test]
    fn draining_keeps_events_sent_before_a_disconnect() {
        let (tx, rx) = std::sync::mpsc::channel();
        assert_eq!(drain_events(|| rx.try_recv()), (Vec::<u8>::new(), false));
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        assert_eq!(drain_events(|| rx.try_recv()), (vec![1, 2], false));
        tx.send(3).unwrap();
        drop(tx);
        assert_eq!(drain_events(|| rx.try_recv()), (vec![3], true));
        assert_eq!(drain_events(|| rx.try_recv()), (Vec::new(), true));
    }

    #[test]
    fn launch_paths_override_the_restored_start_folders() {
        let restored = || (PathBuf::from("/saved/l"), PathBuf::from("/saved/r"));
        let launch = |left: Option<&str>, right: Option<&str>| LaunchPaths {
            left: left.map(PathBuf::from),
            right: right.map(PathBuf::from),
            ..LaunchPaths::default()
        };
        let paths = |launch| resolve_start_paths(restored(), launch).0;
        assert_eq!(paths(launch(None, None)), restored());
        // One argument opens in both panels instead of beside a stale folder.
        assert_eq!(
            paths(launch(Some("/a"), None)),
            (PathBuf::from("/a"), PathBuf::from("/a"))
        );
        assert_eq!(
            paths(launch(Some("/a"), Some("/b"))),
            (PathBuf::from("/a"), PathBuf::from("/b"))
        );
        assert_eq!(
            paths(launch(None, Some("/b"))),
            (PathBuf::from("/saved/l"), PathBuf::from("/b"))
        );
    }

    #[test]
    fn a_file_launch_argument_focuses_it_where_its_folder_opens() {
        let restored = (PathBuf::from("/saved/l"), PathBuf::from("/saved/r"));
        let single = LaunchPaths {
            left: Some(PathBuf::from("/notes")),
            left_focus: Some("todo.txt".to_string()),
            ..LaunchPaths::default()
        };
        let focus = |name: &str| Some(name.to_string());
        assert_eq!(
            resolve_start_paths(restored.clone(), single).1,
            (focus("todo.txt"), focus("todo.txt"))
        );
        let pair = LaunchPaths {
            left: Some(PathBuf::from("/a")),
            right: Some(PathBuf::from("/b")),
            left_focus: None,
            right_focus: focus("b.txt"),
        };
        assert_eq!(
            resolve_start_paths(restored, pair).1,
            (None, focus("b.txt"))
        );
    }

    #[test]
    fn frame_seconds_convert_to_whole_millis_and_clamp_at_zero() {
        assert_eq!(secs_to_millis(0.0), 0);
        assert_eq!(secs_to_millis(1.2345), 1_234);
        assert_eq!(secs_to_millis(-3.0), 0);
        assert_eq!(secs_to_millis(f64::NAN), 0);
        assert_eq!(secs_to_millis(f64::INFINITY), u64::MAX);
    }

    #[test]
    fn keyboard_scroll_commands_clamp_at_the_start_and_reach_the_end() {
        let offset = egui::vec2(12.0, 40.0);
        assert_eq!(
            apply_scroll_command(offset, ScrollCommand::PageUp, 100.0),
            egui::vec2(12.0, 0.0)
        );
        assert_eq!(
            apply_scroll_command(offset, ScrollCommand::LineLeft, 100.0),
            egui::vec2(0.0, 40.0)
        );
        assert_eq!(
            apply_scroll_command(offset, ScrollCommand::End, 100.0).y,
            f32::MAX
        );
    }
}
