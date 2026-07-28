//! UI-agnostic application core: two panels, the active-panel marker,
//! pending operations and the running transfer. All file-manager behaviour
//! lives here so it can be driven and tested without a GUI. The UI layer
//! only renders this state and forwards [`Command`]s / notify callbacks.

use std::path::{Path, PathBuf};

mod delete;
mod space_probe;
mod transfer_queue;

pub use transfer_queue::QueueRow;

use crate::command::Command;
use crate::panel::{self, FileEntry, PanelState, PreviewContent};
use crate::scan::{self, FlatList};
#[cfg(test)]
use crate::transfer;
use crate::transfer::{
    CopyMethod, OverwritePolicy, PostTransferAction, TransferKind, TransferSpec, TransferState,
};
use crate::ui_request::{UiModal, UiRequest};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ActivePanel {
    Left,
    Right,
}

/// Immutable target captured when the batch-rename sheet opens. Keeping the
/// panel side, directory, names, and collision set together prevents a later
/// click in the other panel from silently retargeting the operation.
#[derive(Clone)]
pub struct BatchRenameContext {
    pub panel: ActivePanel,
    pub dir: PathBuf,
    pub targets: Vec<String>,
    pub existing: std::collections::HashSet<String>,
}

/// Immutable disk-usage view captured when the treemap opens. The directory
/// and sized rows travel together so later panel navigation cannot relabel the
/// existing layout.
pub struct TreemapSnapshot {
    pub dir: PathBuf,
    pub items: Vec<(FileEntry, u64)>,
}

/// A copy/move awaiting user confirmation in the dialog.
pub struct PendingTransfer {
    pub kind: TransferKind,
    pub entries: Vec<FileEntry>,
    pub expectations: Vec<crate::transfer::TransferExpectation>,
    pub target: PathBuf,
    pub conflicts: Vec<String>,
    pub policy: OverwritePolicy,
    pub method: CopyMethod,
    pub durability: crate::operation::DurabilityProfile,
    pub version_retention: crate::operation::VersionRetentionPolicy,
    pub name_policy: crate::filesystem_policy::NamePolicy,
    pub symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    pub filesystem: Box<crate::filesystem_policy::OperationPreflight>,
    pub flat: FlatList,
    /// One generation-bound snapshot produced off the UI thread. Size, free
    /// space, and volume relation are never mixed across different plans.
    pub space: TransferSpaceState,
    /// Conflict-free drag/drop preserves its immediate-run UX, but only after
    /// the background preflight has published a trustworthy size snapshot.
    pub start_when_ready: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferSpaceState {
    Pending {
        generation: u64,
    },
    Ready {
        generation: u64,
        need_bytes: u64,
        free: crate::ports::SpaceProbeOutcome,
        relation: crate::ports::VolumeRelation,
    },
    Failed {
        generation: u64,
        failure: crate::ports::NativeFailure,
        free: crate::ports::SpaceProbeOutcome,
        relation: crate::ports::VolumeRelation,
    },
}

impl PendingTransfer {
    /// A same-volume move is an instant rename. Every copy reserves its full
    /// logical size because a native clone attempt may fall back to byte copy.
    fn op_class(&self) -> crate::fs_util::OpClass {
        use crate::fs_util::OpClass;
        match self.kind {
            TransferKind::Move => OpClass::Move {
                same_volume: self.symlink_policy != crate::filesystem_policy::SymlinkPolicy::Follow
                    && matches!(
                        self.space,
                        TransferSpaceState::Ready {
                            relation: crate::ports::VolumeRelation::Same,
                            ..
                        }
                    ),
            },
            TransferKind::Copy => OpClass::Copy,
        }
    }

    pub fn space_verdict(&self) -> crate::fs_util::SpaceVerdict {
        let TransferSpaceState::Ready {
            need_bytes, free, ..
        } = &self.space
        else {
            return crate::fs_util::SpaceVerdict::Indeterminate;
        };
        let free = match free {
            crate::ports::SpaceProbeOutcome::Known { bytes, .. } => Some(*bytes),
            crate::ports::SpaceProbeOutcome::Unknown(_) => None,
        };
        crate::fs_util::space_verdict(*need_bytes, free, self.op_class(), 0, 0)
    }

    pub fn needs_no_space(&self) -> bool {
        matches!(
            self.op_class(),
            crate::fs_util::OpClass::Move { same_volume: true }
        )
    }

    pub fn space_ready(&self) -> bool {
        matches!(self.space, TransferSpaceState::Ready { .. })
    }

    pub fn need_bytes(&self) -> Option<u64> {
        match self.space {
            TransferSpaceState::Pending { .. } => None,
            TransferSpaceState::Ready { need_bytes, .. } => Some(need_bytes),
            TransferSpaceState::Failed { .. } => None,
        }
    }

    pub fn free_space(&self) -> Option<&crate::ports::SpaceProbeOutcome> {
        match &self.space {
            TransferSpaceState::Pending { .. } => None,
            TransferSpaceState::Ready { free, .. } | TransferSpaceState::Failed { free, .. } => {
                Some(free)
            }
        }
    }

    pub fn space_failure(&self) -> Option<&crate::ports::NativeFailure> {
        match &self.space {
            TransferSpaceState::Failed { failure, .. } => Some(failure),
            TransferSpaceState::Pending { .. } | TransferSpaceState::Ready { .. } => None,
        }
    }

    /// True when the operation cannot fit on the target volume.
    pub fn overflows(&self) -> bool {
        matches!(
            self.space_verdict(),
            crate::fs_util::SpaceVerdict::WontFit { .. }
        )
    }
}

fn should_auto_start_after_preflight(transfer: &PendingTransfer) -> bool {
    transfer.start_when_ready
        && (transfer.needs_no_space()
            || matches!(
                transfer.space_verdict(),
                crate::fs_util::SpaceVerdict::Fits | crate::fs_util::SpaceVerdict::Tight
            ))
}

/// Pending file operation awaiting user confirmation.
pub enum PendingOp {
    Transfer(PendingTransfer),
    Delete {
        entries: Vec<FileEntry>,
        targets: Vec<crate::ports::TrashBatchItem>,
        flat: FlatList,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteOrigin {
    Confirmation,
    Duplicates,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteItemResult {
    pub path: PathBuf,
    pub outcome: crate::ports::TrashItemOutcome,
}

/// Ordered, per-path result of one background delete batch.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeleteOutcome {
    pub attempt_id: crate::operation::TransferAttemptId,
    pub operation_id: crate::operation::OperationId,
    pub submitted: crate::operation_view::SubmittedSummary,
    pub origin: DeleteOrigin,
    pub items: Vec<DeleteItemResult>,
    pub trashed: usize,
    pub failed: usize,
    pub failures: Vec<crate::operation::ClassifiedFailure>,
    /// The worker stopped without a terminal report, so some mutations may
    /// have committed even though no per-path success could be confirmed.
    pub indeterminate: bool,
    pub cancelled: bool,
}

impl DeleteOutcome {
    pub fn refresh_required(&self) -> bool {
        self.trashed > 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeleteActivity {
    pub completed: usize,
    pub total: usize,
    pub cancel_requested: bool,
}

/// How a shelf drain turned out: how many copies started, and how many items
/// could not be read and so were kept on the shelf rather than discarded.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct ShelfDrainOutcome {
    pub started: usize,
    pub unavailable: usize,
}

#[derive(Clone)]
pub struct ActiveTransferView {
    pub attempt_id: crate::operation::TransferAttemptId,
    pub operation_id: crate::operation::OperationId,
    pub submitted: crate::operation_view::SubmittedSummary,
    pub progress: TransferState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferTerminalState {
    Done,
    Failed,
    Cancelled,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferTerminalReport {
    pub attempt_id: crate::operation::TransferAttemptId,
    pub operation_id: crate::operation::OperationId,
    pub submitted: crate::operation_view::SubmittedSummary,
    pub terminal: TransferTerminalState,
    pub errors: Vec<String>,
    pub failures: Vec<crate::operation::ClassifiedFailure>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransferPollOutcome {
    pub undo_recorded: bool,
    pub terminal: Option<TransferTerminalReport>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ActionExecution {
    Completed,
    Started,
}

#[derive(Debug)]
struct RenameExecutionError {
    message: String,
    integrity_uncertain: bool,
    paths: Vec<PathBuf>,
}

impl RenameExecutionError {
    fn unchanged(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            integrity_uncertain: false,
            paths: Vec::new(),
        }
    }

    fn uncertain(message: impl Into<String>, paths: Vec<PathBuf>) -> Self {
        Self {
            message: message.into(),
            integrity_uncertain: true,
            paths,
        }
    }
}

impl std::fmt::Display for RenameExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

struct CommandCapabilityCache {
    left_path: PathBuf,
    right_path: PathBuf,
    left_read_only: bool,
    right_read_only: bool,
    expires_at: std::time::Instant,
}

pub struct Workspace {
    pub left: PanelState,
    pub right: PanelState,
    pub active: ActivePanel,
    pub pending_op: Option<PendingOp>,
    pub safe_state: Option<crate::operation::SafeState>,
    pub durability_profile: crate::operation::DurabilityProfile,
    pub version_retention: crate::operation::VersionRetentionPolicy,
    pub sync_guard_policy: crate::sync_guard::GuardPolicy,
    pub name_policy: crate::filesystem_policy::NamePolicy,
    pub symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    /// Typed intents waiting for the app shell. This replaces the former
    /// collection of one-shot booleans and optional payload slots.
    ui_requests: crate::ui_request::UiRequestQueue,
    /// The drop stack: paths gathered across folders to copy in one go.
    pub shelf: crate::shelf::Shelf,
    /// Persisted directory bookmarks (favorites + quick-jump slots 1..9).
    pub bookmarks: crate::bookmarks::Bookmarks,
    persistence: std::sync::Arc<dyn crate::persistence::Persist>,
    bookmark_gate: crate::persistence::StoreGate,
    /// A stashed selection for set-algebra combinations (union/intersect/...).
    pub selection_stash: std::collections::HashSet<PathBuf>,
    /// Atomic owner of the transfer queue, active worker, and per-job history
    /// intent. Workspace applies only the controller's typed outcomes.
    transfers: transfer_queue::TransferQueueController,
    space_probes: space_probe::SpaceProbeController,
    free_space_port: std::sync::Arc<dyn crate::ports::FreeSpacePort>,
    deletes: delete::DeleteController,
    /// Sole owner of reversible-operation history and replay reservations.
    undo: crate::undo::UndoCenter,
    command_capabilities: std::cell::RefCell<Option<CommandCapabilityCache>>,
}

/// `(from, to)` pairs for a Move: each entry goes from its current path to
/// `target/name`. Recorded as an [`undo::Action::Move`] so the move is
/// reversible. Pure/testable.
pub fn move_pairs(entries: &[FileEntry], target: &Path) -> Vec<(PathBuf, PathBuf)> {
    entries
        .iter()
        .map(|e| (e.path.clone(), target.join(&e.name)))
        .collect()
}

/// Keep only the move placements that landed under their original name, so undo
/// reverses them faithfully. A KeepBoth conflict lands at "name copy.ext"; a
/// move there cannot be undone by relocating the file under the original name
/// (that would be a different operation), so it is dropped. Pure/testable.
fn faithfully_undoable(placements: Vec<(PathBuf, PathBuf)>) -> Vec<(PathBuf, PathBuf)> {
    placements
        .into_iter()
        .filter(|(src, dst)| src.file_name() == dst.file_name())
        .collect()
}

fn filesystem_preflight(
    entries: &[FileEntry],
    target: &Path,
    name_policy: crate::filesystem_policy::NamePolicy,
) -> Box<crate::filesystem_policy::OperationPreflight> {
    let profile = crate::volume_profile::profile(target);
    let names = entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();
    Box::new(crate::filesystem_policy::OperationPreflight {
        normalization: crate::filesystem_policy::preview_normalization(
            names.iter().copied(),
            name_policy.normalization,
            profile.case_sensitive.unwrap_or(false),
        ),
        portability: crate::filesystem_policy::audit_names(names.iter().copied()),
        capabilities: crate::filesystem_policy::matrix_for_profile(
            &profile,
            std::fs::metadata(target).is_ok(),
        ),
    })
}

pub(crate) fn trash_batch_item_from_listing(
    path: PathBuf,
    identity: &crate::panel::ListingIdentity,
) -> crate::ports::TrashBatchItem {
    match identity {
        crate::panel::ListingIdentity::Captured(expected) => {
            crate::ports::TrashBatchItem::Ready(crate::ports::TrashTarget {
                path,
                expected: expected.clone(),
            })
        }
        crate::panel::ListingIdentity::CaptureFailed(failure) => {
            crate::ports::TrashBatchItem::CaptureFailed {
                path,
                failure: failure.clone(),
            }
        }
        crate::panel::ListingIdentity::Unavailable => crate::ports::TrashBatchItem::CaptureFailed {
            path,
            failure: crate::ports::NativeFailure {
                kind: crate::ports::NativeFailureKind::Unsupported,
                message: "the visible listing did not capture a lexical filesystem identity"
                    .to_string(),
            },
        },
    }
}

fn trash_batch_item(entry: &FileEntry) -> crate::ports::TrashBatchItem {
    trash_batch_item_from_listing(entry.path.clone(), &entry.identity)
}

impl Workspace {
    #[cfg(test)]
    pub fn new(left: PathBuf, right: PathBuf) -> Self {
        Self::with_ports(
            left,
            right,
            std::sync::Arc::new(TestTrashPort),
            std::sync::Arc::new(TestFreeSpacePort),
        )
    }

    #[cfg(test)]
    pub fn with_ports(
        left: PathBuf,
        right: PathBuf,
        trash: std::sync::Arc<dyn crate::ports::TrashPort>,
        free_space: std::sync::Arc<dyn crate::ports::FreeSpacePort>,
    ) -> Self {
        Self::with_ports_and_views(
            left,
            right,
            [crate::panel::ViewConfig::default(); 2],
            trash,
            free_space,
            crate::persistence::fs_persist(),
        )
    }

    pub(crate) fn with_ports_and_views(
        left: PathBuf,
        right: PathBuf,
        views: [crate::panel::ViewConfig; 2],
        trash: std::sync::Arc<dyn crate::ports::TrashPort>,
        free_space: std::sync::Arc<dyn crate::ports::FreeSpacePort>,
        persistence: std::sync::Arc<dyn crate::persistence::Persist>,
    ) -> Self {
        let loaded = crate::bookmarks::load_with(persistence.as_ref());
        Self::with_ports_bookmarks_and_views(
            left,
            right,
            views,
            trash,
            free_space,
            loaded,
            persistence,
        )
    }

    #[cfg(feature = "visual-qa")]
    pub(crate) fn with_ports_and_bookmarks(
        left: PathBuf,
        right: PathBuf,
        trash: std::sync::Arc<dyn crate::ports::TrashPort>,
        free_space: std::sync::Arc<dyn crate::ports::FreeSpacePort>,
        bookmarks: crate::bookmarks::Bookmarks,
        persistence: std::sync::Arc<dyn crate::persistence::Persist>,
    ) -> Self {
        Self::with_ports_bookmarks_and_views(
            left,
            right,
            [crate::panel::ViewConfig::default(); 2],
            trash,
            free_space,
            crate::bookmarks::LoadedBookmarks {
                store: bookmarks,
                gate: crate::persistence::StoreGate::missing(),
            },
            persistence,
        )
    }

    fn with_ports_bookmarks_and_views(
        left: PathBuf,
        right: PathBuf,
        views: [crate::panel::ViewConfig; 2],
        trash: std::sync::Arc<dyn crate::ports::TrashPort>,
        free_space: std::sync::Arc<dyn crate::ports::FreeSpacePort>,
        bookmarks: crate::bookmarks::LoadedBookmarks,
        persistence: std::sync::Arc<dyn crate::persistence::Persist>,
    ) -> Self {
        let [left_view, right_view] = views;
        Workspace {
            left: PanelState::new_with_view(left, left_view),
            right: PanelState::new_with_view(right, right_view),
            active: ActivePanel::Left,
            pending_op: None,
            safe_state: None,
            durability_profile: crate::operation::DurabilityProfile::default(),
            version_retention: crate::operation::VersionRetentionPolicy::default(),
            sync_guard_policy: crate::sync_guard::GuardPolicy::default(),
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            ui_requests: crate::ui_request::UiRequestQueue::default(),
            shelf: crate::shelf::Shelf::default(),
            bookmarks: bookmarks.store,
            persistence,
            bookmark_gate: bookmarks.gate,
            selection_stash: std::collections::HashSet::new(),
            transfers: transfer_queue::TransferQueueController::default(),
            space_probes: space_probe::SpaceProbeController::default(),
            free_space_port: free_space,
            deletes: delete::DeleteController::new(trash),
            undo: crate::undo::UndoCenter::default(),
            command_capabilities: std::cell::RefCell::new(None),
        }
    }

    fn save_bookmarks(&mut self) {
        match crate::bookmarks::save_with(
            self.persistence.as_ref(),
            &self.bookmarks,
            &mut self.bookmark_gate,
        ) {
            Ok(crate::persistence::AtomicWriteOutcome::Durable) => {}
            Ok(crate::persistence::AtomicWriteOutcome::CommittedButNotDurable(error)) => {
                crate::persistence::record_durability_warning("Bookmarks", &error);
            }
            Err(error) => crate::persistence::record_json_save_failure("Bookmarks", &error),
        }
    }

    pub fn active_panel(&mut self) -> &mut PanelState {
        match self.active {
            ActivePanel::Left => &mut self.left,
            ActivePanel::Right => &mut self.right,
        }
    }

    pub fn active_panel_ref(&self) -> &PanelState {
        match self.active {
            ActivePanel::Left => &self.left,
            ActivePanel::Right => &self.right,
        }
    }

    pub fn inactive_panel(&self) -> &PanelState {
        match self.active {
            ActivePanel::Left => &self.right,
            ActivePanel::Right => &self.left,
        }
    }

    pub fn inactive_panel_mut(&mut self) -> &mut PanelState {
        match self.active {
            ActivePanel::Left => &mut self.right,
            ActivePanel::Right => &mut self.left,
        }
    }

    pub(crate) fn emit_ui_request(&mut self, request: UiRequest) {
        self.ui_requests.emit(request);
    }

    pub(crate) fn drain_ui_requests(&mut self) -> Vec<UiRequest> {
        self.ui_requests.drain_snapshot()
    }

    pub(crate) fn defer_ui_requests(&mut self, requests: Vec<UiRequest>) {
        self.ui_requests.prepend_deferred(requests);
    }

    pub(crate) fn first_pending_ui_modal(&self) -> Option<UiModal> {
        self.ui_requests.first_pending_modal()
    }

    pub(crate) fn has_any_pending_ui_modal(&self) -> bool {
        self.ui_requests.has_any_modal()
    }

    #[cfg(test)]
    fn pending_ui_requests(&self) -> Vec<UiRequest> {
        self.ui_requests.snapshot()
    }

    fn pane_read_only(&self) -> (bool, bool) {
        const TTL: std::time::Duration = std::time::Duration::from_secs(5);
        let now = std::time::Instant::now();
        if let Some(cached) = self.command_capabilities.borrow().as_ref()
            && cached.left_path == self.left.current_path
            && cached.right_path == self.right.current_path
            && cached.expires_at > now
        {
            return (cached.left_read_only, cached.right_read_only);
        }

        let read_only = |path: &Path| {
            let profile = crate::volume_profile::profile(path);
            let matrix = crate::filesystem_policy::matrix_for_profile(&profile, true);
            matrix.state(crate::filesystem_policy::Capability::Write)
                == crate::filesystem_policy::CapabilityState::Unavailable
        };
        let left_read_only = read_only(&self.left.current_path);
        let right_read_only = read_only(&self.right.current_path);
        self.command_capabilities
            .replace(Some(CommandCapabilityCache {
                left_path: self.left.current_path.clone(),
                right_path: self.right.current_path.clone(),
                left_read_only,
                right_read_only,
                expires_at: now + TTL,
            }));
        (left_read_only, right_read_only)
    }

    /// Capture the complete command-relevant state for the palette and other
    /// broad command surfaces. The filtered view is visited once without a
    /// temporary allocation.
    pub fn command_context(&self) -> crate::command::CommandContext {
        let (active, inactive) = match self.active {
            ActivePanel::Left => (&self.left, &self.right),
            ActivePanel::Right => (&self.right, &self.left),
        };
        let visible_entries = active.filtered_count();
        let cursor = active.cursor_entry();
        let mut visible_files = 0usize;
        let mut selected_entries = 0usize;
        let mut selected_file_count = 0usize;
        let mut first_selected_file_index = None;
        let mut has_transfer_source = false;
        active.visit_filtered(|index, entry| {
            if !entry.is_dir {
                visible_files = visible_files.saturating_add(1);
            }
            if active.is_selected(&entry.path) {
                selected_entries = selected_entries.saturating_add(1);
                has_transfer_source |= cursor.is_some_and(|target| entry.path != target.path);
                if !entry.is_dir {
                    selected_file_count = selected_file_count.saturating_add(1);
                    first_selected_file_index.get_or_insert(index);
                }
            }
            true
        });
        let picked_entries = if active.selection_is_empty() {
            usize::from(cursor.is_some())
        } else {
            selected_entries
        };
        let listing_entries = if active.selection_is_empty() {
            visible_entries
        } else {
            selected_entries
        };

        let can_diff = if selected_file_count == 2 {
            true
        } else {
            let candidate = if selected_file_count == 1 {
                first_selected_file_index.and_then(|index| active.entries().get(index))
            } else {
                cursor.filter(|entry| !entry.is_dir)
            };
            candidate.is_some_and(|entry| {
                inactive
                    .entries()
                    .iter()
                    .any(|other| !other.is_dir && other.name_lower == entry.name_lower)
            })
        };

        let active_transfer = self.active_transfer_view().is_some();
        let transfer_queue_busy = self.has_unfinished_transfer_work();
        let pending_operation = self.pending_op.is_some();
        let safe_state = self.safe_state.is_some();
        let active_mutation = self.deletes.is_active();
        let (left_read_only, right_read_only) = self.pane_read_only();
        let (active_read_only, inactive_read_only) = match self.active {
            ActivePanel::Left => (left_read_only, right_read_only),
            ActivePanel::Right => (right_read_only, left_read_only),
        };
        let can_transfer_into_cursor_folder = !transfer_queue_busy
            && !pending_operation
            && !self.mutations_blocked()
            && cursor.is_some_and(|target| target.is_dir)
            && has_transfer_source;

        let (marked_entries, stashed_entries) =
            active
                .entries()
                .iter()
                .fold((0usize, 0usize), |(marked, stashed), entry| {
                    (
                        marked.saturating_add(usize::from(active.is_marked(&entry.path))),
                        stashed.saturating_add(usize::from(
                            self.selection_stash.contains(&entry.path),
                        )),
                    )
                });

        crate::command::CommandContext {
            visible_entries,
            visible_files,
            other_entries: inactive.entries().len(),
            picked_entries,
            selected_entries,
            listing_entries,
            marked_entries,
            stashed_entries,
            shelf_entries: self.shelf.len(),
            cursor_entry: cursor.is_some(),
            cursor_is_dir: cursor.is_some_and(|entry| entry.is_dir),
            cursor_is_file: cursor.is_some_and(|entry| !entry.is_dir),
            cursor_has_extension: cursor
                .is_some_and(|entry| !entry.is_dir && !entry.extension.is_empty()),
            can_go_up: active.current_path.parent().is_some(),
            can_go_back: active.can_go_back(),
            can_go_forward: active.can_go_forward(),
            can_diff,
            can_transfer_into_cursor_folder,
            can_undo: self.can_undo(),
            can_redo: self.can_redo(),
            preview_open: inactive.preview.is_some(),
            info_open: matches!(inactive.preview, Some(PreviewContent::Info(_))),
            safe_state,
            active_mutation,
            pending_operation,
            active_transfer,
            transfer_queue_busy,
            active_read_only,
            inactive_read_only,
        }
    }

    /// Minimal context for always-visible action surfaces. It reuses the
    /// shared availability policy but avoids an O(directory size) scan on
    /// ordinary frames with no selection.
    pub fn action_bar_command_context(&self) -> crate::command::CommandContext {
        let active = self.active_panel_ref();
        let cursor = active.cursor_entry();
        let mut has_selected = false;
        let mut has_transfer_source = false;
        if !active.selection_is_empty() {
            active.visit_filtered(|_, entry| {
                if active.is_selected(&entry.path) {
                    has_selected = true;
                    has_transfer_source |= cursor.is_some_and(|target| entry.path != target.path);
                }
                !(has_selected
                    && (!cursor.is_some_and(|target| target.is_dir) || has_transfer_source))
            });
        }

        let safe_state = self.safe_state.is_some();
        let active_mutation = self.deletes.is_active();
        let pending_operation = self.pending_op.is_some();
        let active_transfer = self.active_transfer_view().is_some();
        let transfer_queue_busy = self.has_unfinished_transfer_work();
        let (left_read_only, right_read_only) = self.pane_read_only();
        let (active_read_only, inactive_read_only) = match self.active {
            ActivePanel::Left => (left_read_only, right_read_only),
            ActivePanel::Right => (right_read_only, left_read_only),
        };
        let selected_entries = usize::from(has_selected);
        crate::command::CommandContext {
            picked_entries: if active.selection_is_empty() {
                usize::from(cursor.is_some())
            } else {
                selected_entries
            },
            selected_entries,
            cursor_entry: cursor.is_some(),
            cursor_is_dir: cursor.is_some_and(|entry| entry.is_dir),
            cursor_is_file: cursor.is_some_and(|entry| !entry.is_dir),
            can_go_up: active.current_path.parent().is_some(),
            can_go_back: active.can_go_back(),
            can_go_forward: active.can_go_forward(),
            can_transfer_into_cursor_folder: !self.mutations_blocked()
                && !pending_operation
                && !transfer_queue_busy
                && cursor.is_some_and(|target| target.is_dir)
                && has_transfer_source,
            can_undo: self.can_undo(),
            can_redo: self.can_redo(),
            preview_open: self.inactive_panel().preview.is_some(),
            info_open: matches!(self.inactive_panel().preview, Some(PreviewContent::Info(_))),
            safe_state,
            active_mutation,
            pending_operation,
            active_transfer,
            transfer_queue_busy,
            active_read_only,
            inactive_read_only,
            ..crate::command::CommandContext::default()
        }
    }

    /// Add to the active panel's selection every visible entry whose name also
    /// exists in the inactive panel (by lowercased name). Builds on top of any
    /// existing selection so it composes with mask/manual picks.
    pub fn select_same_named(&mut self) {
        let names: std::collections::HashSet<String> = self
            .inactive_panel()
            .filtered_entries()
            .iter()
            .map(|e| e.name_lower.clone())
            .collect();
        let picks = {
            let active = self.active_panel_ref();
            crate::compare::matching_name_paths(active.filtered_entries().into_iter(), &names)
        };
        self.active_panel().extend_selection(picks);
    }

    /// Replace the active panel's selection with the entries a cross-pane
    /// relation (vs the inactive panel) picks: only-here, differing, or
    /// identical. Distinct from [`select_same_named`](Self::select_same_named),
    /// which adds all same-named entries regardless of content.
    fn select_by_relation(&mut self, pick: fn(&crate::sync::PaneRelation) -> &Vec<usize>) {
        // Operate over the active panel's VISIBLE (filtered) entries, like the
        // other selectors (select_all / invert / same-named), so the picks are
        // actionable and the on-screen count matches what an op will touch.
        // Compare against the other directory's full contents.
        let active: Vec<FileEntry> = self
            .active_panel_ref()
            .filtered_entries()
            .into_iter()
            .cloned()
            .collect();
        let rel = crate::sync::pane_relation(&active, self.inactive_panel().entries());
        let paths: Vec<PathBuf> = pick(&rel)
            .iter()
            .filter_map(|&i| active.get(i).map(|e| e.path.clone()))
            .collect();
        self.active_panel().replace_selection(paths);
    }

    /// Copy the active panel's current selection into the stash.
    pub fn stash_selection(&mut self) {
        self.selection_stash = self.active_panel_ref().selected_paths().clone();
    }

    /// Replace the active panel's selection with `op(current, stash)`, dropping
    /// any paths no longer present in the panel so the result stays valid.
    fn combine_with_stash(
        &mut self,
        op: fn(
            &std::collections::HashSet<PathBuf>,
            &std::collections::HashSet<PathBuf>,
        ) -> std::collections::HashSet<PathBuf>,
    ) {
        let stash = self.selection_stash.clone();
        let panel = self.active_panel();
        let present: std::collections::HashSet<PathBuf> =
            panel.entries().iter().map(|e| e.path.clone()).collect();
        let combined = op(panel.selected_paths(), &stash);
        panel.replace_selection(combined.intersection(&present).cloned());
    }

    pub fn stash_union(&mut self) {
        self.combine_with_stash(crate::selset::union);
    }
    pub fn stash_intersect(&mut self) {
        self.combine_with_stash(crate::selset::intersect);
    }
    pub fn stash_subtract(&mut self) {
        self.combine_with_stash(crate::selset::difference);
    }
    pub fn stash_symmetric_diff(&mut self) {
        self.combine_with_stash(crate::selset::symmetric_difference);
    }

    /// Toggle the mark on the cursor entry and advance the cursor,
    /// mirroring `Command::ToggleSelect`.
    pub fn toggle_mark(&mut self) {
        let panel = self.active_panel();
        if let Some(path) = panel.cursor_entry().map(|entry| entry.path.clone()) {
            panel.toggle_mark(path);
        }
        let max = panel.filtered_count();
        if panel.cursor() < max {
            panel.set_cursor(panel.cursor() + 1);
        }
    }

    /// Replace the active panel's selection with `op(current, marked)`,
    /// dropping any paths no longer present in the panel.
    fn combine_with_marked(
        &mut self,
        op: fn(
            &std::collections::HashSet<PathBuf>,
            &std::collections::HashSet<PathBuf>,
        ) -> std::collections::HashSet<PathBuf>,
    ) {
        let panel = self.active_panel();
        let marked = panel.marked_paths().clone();
        let present: std::collections::HashSet<PathBuf> =
            panel.entries().iter().map(|e| e.path.clone()).collect();
        let combined = op(panel.selected_paths(), &marked);
        panel.replace_selection(combined.intersection(&present).cloned());
    }

    pub fn marked_union(&mut self) {
        self.combine_with_marked(crate::selset::union);
    }
    pub fn marked_intersect(&mut self) {
        self.combine_with_marked(crate::selset::intersect);
    }
    pub fn marked_subtract(&mut self) {
        self.combine_with_marked(crate::selset::difference);
    }
    pub fn marked_symmetric_diff(&mut self) {
        self.combine_with_marked(crate::selset::symmetric_difference);
    }

    // ── Command dispatch ────────────────────────────────────────────────

    pub fn execute(&mut self, cmd: Command) {
        if self.mutation_commits_blocked() && cmd.mutates_filesystem() {
            return;
        }
        match cmd {
            Command::SwitchPanel => {
                self.active = match self.active {
                    ActivePanel::Left => ActivePanel::Right,
                    ActivePanel::Right => ActivePanel::Left,
                };
            }
            Command::CursorUp => {
                let panel = self.active_panel();
                if panel.cursor() > 0 {
                    panel.set_cursor(panel.cursor() - 1);
                    panel.set_scroll_to_cursor(true);
                }
            }
            Command::CursorDown => {
                let panel = self.active_panel();
                let max = panel.filtered_count();
                if panel.cursor() < max {
                    panel.set_cursor(panel.cursor() + 1);
                    panel.set_scroll_to_cursor(true);
                }
            }
            Command::CursorHome => {
                let panel = self.active_panel();
                panel.set_cursor(0);
                panel.set_scroll_to_cursor(true);
            }
            Command::CursorEnd => {
                let panel = self.active_panel();
                panel.set_cursor(panel.filtered_count());
                panel.set_scroll_to_cursor(true);
            }
            Command::CursorPageUp => {
                let panel = self.active_panel();
                let page = panel.page_rows().max(1);
                panel.set_cursor(panel.cursor().saturating_sub(page));
                panel.set_scroll_to_cursor(true);
            }
            Command::CursorPageDown => {
                let panel = self.active_panel();
                let page = panel.page_rows().max(1);
                let max = panel.filtered_count();
                panel.set_cursor((panel.cursor() + page).min(max));
                panel.set_scroll_to_cursor(true);
            }
            Command::CursorMove(delta) => {
                let panel = self.active_panel();
                let max = panel.filtered_count() as i32;
                panel.set_cursor((panel.cursor() as i32 + delta).clamp(0, max) as usize);
                panel.set_scroll_to_cursor(true);
            }
            Command::ExtendSelectDown => {
                let panel = self.active_panel();
                panel.select_cursor();
                let max = panel.filtered_count();
                if panel.cursor() < max {
                    panel.set_cursor(panel.cursor() + 1);
                }
                panel.select_cursor();
                panel.set_scroll_to_cursor(true);
            }
            Command::ExtendSelectUp => {
                let panel = self.active_panel();
                panel.select_cursor();
                if panel.cursor() > 1 {
                    panel.set_cursor(panel.cursor() - 1);
                }
                panel.select_cursor();
                panel.set_scroll_to_cursor(true);
            }
            Command::Activate => {
                // Cursor 0 is the ".." row, real files start at cursor 1.
                if self.active_panel_ref().cursor() == 0 {
                    self.active_panel().go_up();
                } else if let Some(entry) = {
                    let panel = self.active_panel_ref();
                    panel.cursor_entry().cloned()
                } {
                    if entry.is_dir {
                        self.active_panel().navigate_to(entry.path);
                    } else if crate::archive::is_supported(&entry.path) {
                        self.emit_ui_request(UiRequest::Archive(entry.path));
                    } else {
                        self.emit_ui_request(UiRequest::OpenExternal(
                            crate::ports::OpenRequest::OpenPath(entry.path),
                        ));
                    }
                }
            }
            Command::GoUp => {
                self.active_panel().go_up();
            }
            Command::JumpBack => {
                self.active_panel().go_back();
            }
            Command::JumpForward => {
                self.active_panel().go_forward();
            }
            Command::JumpSlot(n) => {
                if let Some(path) = self.bookmarks.by_slot(n).map(|b| b.path.clone())
                    && path.is_dir()
                {
                    self.active_panel().navigate_to(path);
                }
            }
            Command::AssignSlot(n) => {
                let dir = self.active_panel_ref().current_path.clone();
                self.bookmark_dir(dir.clone());
                self.bookmarks.assign_slot(&dir, n);
                self.save_bookmarks();
            }
            Command::BookmarkCurrentDir => {
                // Toggle: bookmark the active directory, or un-bookmark it if it
                // is already a favorite.
                let dir = self.active_panel_ref().current_path.clone();
                if self.bookmarks.contains(&dir) {
                    self.bookmarks.remove(&dir);
                } else {
                    self.bookmark_dir(dir);
                }
                self.save_bookmarks();
            }
            Command::ToggleSelect => {
                let panel = self.active_panel();
                if let Some(path) = panel.cursor_entry().map(|entry| entry.path.clone()) {
                    panel.toggle_select(path);
                }
                let max = panel.filtered_count();
                if panel.cursor() < max {
                    panel.set_cursor(panel.cursor() + 1);
                }
            }
            Command::MoveIntoCursorFolder => {
                self.emit_ui_request(UiRequest::TransferIntoCursorFolder(TransferKind::Move))
            }
            Command::CopyIntoCursorFolder => {
                self.emit_ui_request(UiRequest::TransferIntoCursorFolder(TransferKind::Copy))
            }
            Command::TogglePreview => {
                if self.inactive_panel().preview.is_some() {
                    self.inactive_panel_mut().preview = None;
                } else {
                    let preview = {
                        let panel = self.active_panel_ref();
                        panel.cursor_entry().and_then(panel::make_preview)
                    };
                    self.inactive_panel_mut().preview = preview;
                }
            }
            Command::RequestCopy => self.request_copy(),
            Command::RequestMove => self.request_move(),
            Command::CreateDir => self.create_dir(),
            Command::RequestDelete => self.request_delete(),
            Command::BeginRename => {
                let panel = self.active_panel_ref();
                if let Some(path) = panel.cursor_entry().map(|entry| entry.path.clone()) {
                    self.emit_ui_request(UiRequest::Rename(path));
                }
            }
            Command::EqualizePanels => {
                let target = self.active_panel_ref().current_path.clone();
                self.inactive_panel_mut().navigate_to(target);
            }
            Command::SwapPanels => {
                std::mem::swap(&mut self.left, &mut self.right);
                self.active = match self.active {
                    ActivePanel::Left => ActivePanel::Right,
                    ActivePanel::Right => ActivePanel::Left,
                };
            }
            Command::BeginBatchRename => self.emit_ui_request(UiRequest::BatchRename),
            Command::BeginSync => self.emit_ui_request(UiRequest::Sync),
            Command::FindDuplicates => self.emit_ui_request(UiRequest::FindDuplicates),
            Command::DiffFiles => self.emit_ui_request(UiRequest::DiffFiles),
            Command::DiskTreemap => self.emit_ui_request(UiRequest::DiskTreemap),
            Command::BeginFind => self.emit_ui_request(UiRequest::Find),
            Command::OpenSavedSearch => self.emit_ui_request(UiRequest::SavedSearch),
            Command::OpenProjectCollections => self.emit_ui_request(UiRequest::ProjectCollections),
            Command::CopyPath => {
                self.emit_ui_request(UiRequest::CopyPaths(crate::clipboard::PathStyle::FullPath))
            }
            Command::CopyName => {
                self.emit_ui_request(UiRequest::CopyPaths(crate::clipboard::PathStyle::NameOnly))
            }
            Command::CopyParentPath => self.emit_ui_request(UiRequest::CopyPaths(
                crate::clipboard::PathStyle::ParentPath,
            )),
            Command::CopyFileUrl => {
                self.emit_ui_request(UiRequest::CopyPaths(crate::clipboard::PathStyle::FileUrl))
            }
            Command::CopyShellPath => self.emit_ui_request(UiRequest::CopyPaths(
                crate::clipboard::PathStyle::ShellEscaped,
            )),
            Command::CopyRelativePath => self.emit_ui_request(UiRequest::CopyPaths(
                crate::clipboard::PathStyle::RelativeToOther,
            )),
            Command::CycleDensity => {
                let panel = self.active_panel();
                panel.set_density(crate::density::cycle(panel.density(), 1));
            }
            Command::ShelfAdd => {
                let paths: Vec<PathBuf> = self
                    .active_panel_ref()
                    .selected_or_cursor()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|e| e.path)
                    .collect();
                self.shelf.add_all(paths);
            }
            Command::ShelfDrain => {
                if !self.shelf.is_empty() {
                    self.emit_ui_request(UiRequest::DrainShelf);
                }
            }
            Command::BeginSelectMask => self.emit_ui_request(UiRequest::SelectMask),
            Command::BeginRunBar => self.emit_ui_request(UiRequest::RunCommand),
            Command::GatherIntoFolder => self.emit_ui_request(UiRequest::GatherIntoFolder),
            Command::BeginGoToPath => self.emit_ui_request(UiRequest::GoToPath),
            Command::BeginRecent => self.emit_ui_request(UiRequest::Recent),
            Command::BeginPalette => self.emit_ui_request(UiRequest::Palette),
            Command::Undo => self.emit_ui_request(UiRequest::Undo),
            Command::Redo => self.emit_ui_request(UiRequest::Redo),
            Command::ToggleInfo => self.toggle_info(),
            Command::SelectAll => self.active_panel().select_all(),
            Command::InvertSelection => self.active_panel().invert_selection(),
            Command::SelectSameNamed => self.select_same_named(),
            Command::SelectOnlyHere => self.select_by_relation(|r| &r.only_here),
            Command::SelectDiffering => self.select_by_relation(|r| &r.differing),
            Command::SelectIdentical => self.select_by_relation(|r| &r.identical),
            Command::StashSelection => self.stash_selection(),
            Command::StashUnion => self.stash_union(),
            Command::StashIntersect => self.stash_intersect(),
            Command::StashSubtract => self.stash_subtract(),
            Command::StashSymmetricDiff => self.stash_symmetric_diff(),
            Command::ToggleMark => self.toggle_mark(),
            Command::ClearMarks => self.active_panel().clear_marks(),
            Command::MarkedUnion => self.marked_union(),
            Command::MarkedIntersect => self.marked_intersect(),
            Command::MarkedSubtract => self.marked_subtract(),
            Command::MarkedSymmetricDiff => self.marked_symmetric_diff(),
            Command::ToggleQueuePanel => self.emit_ui_request(UiRequest::ToggleQueuePanel),
            Command::OpenReceipts => self.emit_ui_request(UiRequest::OperationHistory),
            Command::OpenRecoveryCenter => self.emit_ui_request(UiRequest::OpenRecoveryCenter),
            Command::ToggleHidden => {
                let outcome = self.active_panel().toggle_hidden();
                self.emit_ui_request(UiRequest::HiddenFilesOutcome(outcome));
            }
            Command::ToggleFoldersFirst => self.active_panel().toggle_folders_first(),
            Command::ToggleNaturalSort => self.active_panel().toggle_natural_sort(),
            Command::SortByExtension => self
                .active_panel()
                .set_sort(crate::panel::SortColumn::Extension),
            Command::SortByKind => self.active_panel().set_sort(crate::panel::SortColumn::Kind),
            Command::SelectJunk => {
                self.active_panel().select_junk();
            }
            Command::ReverseSort => self.active_panel().reverse_sort(),
            Command::SelectLargest => {
                self.active_panel().select_largest(10);
            }
            Command::SelectLikeCursor => {
                self.active_panel().select_same_extension_as_cursor();
            }
            Command::SelectEmptyFiles => {
                self.active_panel().select_empty_files();
            }
            Command::CopyListingText => {
                self.request_listing_copy(crate::listing_export::ListingFormat::Text)
            }
            Command::CopyListingCsv => {
                self.request_listing_copy(crate::listing_export::ListingFormat::Csv)
            }
            Command::CopyListingMarkdown => {
                self.request_listing_copy(crate::listing_export::ListingFormat::Markdown)
            }
        }
    }

    /// Build the active panel's filtered listing in `fmt` and stage it for the
    /// UI to copy to the clipboard (with a row-count toast label).
    fn request_listing_copy(&mut self, fmt: crate::listing_export::ListingFormat) {
        let (text, count) = {
            let entries = self.active_panel_ref().listing_entries();
            (crate::listing_export::format(&entries, fmt), entries.len())
        };
        let label = format!("listing ({count} rows as {})", fmt.label());
        self.emit_ui_request(UiRequest::CopyText { text, label });
    }

    // ── File operations ─────────────────────────────────────────────────

    pub fn request_copy(&mut self) {
        self.request_transfer(TransferKind::Copy);
    }

    pub fn request_move(&mut self) {
        self.request_transfer(TransferKind::Move);
    }

    /// Copy/Move may queue behind an active transfer, but must never replace an
    /// operation that is already waiting for confirmation.
    pub fn can_request_transfer(&self) -> bool {
        self.pending_op.is_none() && !self.mutation_commits_blocked()
    }

    /// Delete is not queue-backed, so it is available only while no operation
    /// is pending and no queued, paused, or running transfer exists.
    pub fn can_request_delete(&self) -> bool {
        self.pending_op.is_none()
            && !self.has_unfinished_transfer_work()
            && !self.mutation_commits_blocked()
    }

    /// One fail-closed gate for every filesystem mutation entry point.
    /// Callers that present a reason should use [`Self::mutation_block_reason`]
    /// so recovery review and ordinary background activity remain distinct.
    pub fn mutations_blocked(&self) -> bool {
        self.safe_state.is_some() || self.deletes.is_active() || self.undo.has_pending_replay()
    }

    pub fn delete_active(&self) -> bool {
        self.deletes.is_active()
    }

    pub fn delete_activity(&self) -> Option<DeleteActivity> {
        self.deletes
            .activity()
            .map(|(completed, total, cancel_requested)| DeleteActivity {
                completed,
                total,
                cancel_requested,
            })
    }

    pub fn cancel_delete(&self) -> bool {
        self.deletes.cancel()
    }

    fn mutation_commits_blocked(&self) -> bool {
        self.mutations_blocked()
    }

    pub fn mutation_block_reason(&self, action: &str) -> Option<String> {
        if self.safe_state.is_some() {
            Some(format!("Safe-state review is required before {action}"))
        } else if self.deletes.is_active() {
            Some(format!(
                "Wait for the current Trash operation before {action}"
            ))
        } else if self.undo.has_pending_replay() {
            Some(format!(
                "Wait for the current history replay before {action}"
            ))
        } else {
            None
        }
    }

    fn ensure_matching_recovery_review(
        &self,
        operation_id: &crate::operation::OperationId,
    ) -> Result<(), String> {
        if let Some(state) = &self.safe_state
            && &state.operation_id != operation_id
        {
            return Err(format!(
                "Safe-state review belongs to operation {}",
                state.operation_id.0
            ));
        }
        Ok(())
    }

    pub fn recovery_seed_roots(&self) -> Vec<PathBuf> {
        vec![
            self.left.current_path.clone(),
            self.right.current_path.clone(),
        ]
    }

    pub fn resume_recovery(
        &mut self,
        operation_id: &crate::operation::OperationId,
        notify: impl Fn() + Send + 'static,
    ) -> Result<usize, String> {
        self.ensure_matching_recovery_review(operation_id)?;
        if self.deletes.is_active() {
            return Err(
                "Wait for the current Trash operation before resuming recovery".to_string(),
            );
        }
        if self.has_unfinished_transfer_work() {
            return Err("Wait for the transfer queue before resuming recovery".to_string());
        }
        let replay = self
            .undo
            .interrupted_reservation_for(operation_id)
            .map_err(|error| error.to_string())?;
        let spec = crate::operation_journal::build_resume_spec(operation_id)?;
        let count = spec.entries.len();
        self.acknowledge_safe_state();
        if let Some(reservation) = replay {
            self.enqueue_bound_replay(spec, transfer_queue::HistoryIntent::recovery(reservation))?;
        } else {
            self.enqueue_only(spec, None);
        }
        self.pump_queue(notify);
        Ok(count)
    }

    pub fn rollback_recovery(
        &mut self,
        operation_id: &crate::operation::OperationId,
    ) -> Result<crate::operation_journal::RepairPlan, String> {
        self.ensure_matching_recovery_review(operation_id)?;
        if self.deletes.is_active() {
            return Err(
                "Wait for the current Trash operation before rolling back recovery".to_string(),
            );
        }
        if self.has_unfinished_transfer_work() {
            return Err("Wait for the transfer queue before rolling back recovery".to_string());
        }
        let replay = self
            .undo
            .interrupted_reservation_for(operation_id)
            .map_err(|error| error.to_string())?;
        crate::operation_journal::repair_plan(operation_id)?;
        let plan = crate::operation_journal::rollback(operation_id)?;
        if plan.remaining.is_empty() {
            if let Some(reservation) = replay {
                self.undo
                    .abort_interrupted(reservation, operation_id)
                    .map_err(|error| self.history_invariant_error(error))?;
            }
            self.acknowledge_safe_state();
        } else if replay.is_some() {
            self.latch_interrupted_replay(operation_id.clone());
        }
        self.left.refresh();
        self.right.refresh();
        Ok(plan)
    }

    pub fn clean_recovery_orphan(
        &mut self,
        orphan: &crate::operation_journal::OrphanStaging,
    ) -> Result<(), String> {
        if let Some(reason) = self.mutation_block_reason("cleaning staging") {
            return Err(reason);
        }
        if self.has_unfinished_transfer_work() {
            return Err("Wait for the transfer queue before cleaning staging".to_string());
        }
        crate::operation_journal::clean_orphan(orphan)?;
        self.left.refresh();
        self.right.refresh();
        Ok(())
    }

    fn request_transfer(&mut self, kind: TransferKind) {
        if !self.can_request_transfer() {
            return;
        }
        let target = self.inactive_panel().current_path.clone();
        let Ok(entries) = self.active_panel_ref().selected_or_cursor() else {
            return;
        };
        if entries.is_empty() {
            return;
        }
        let flat = scan::pending_flat_list();
        let conflicts = scan::find_conflicts(&entries, &target);
        let wake = self.active_panel_ref().notify_callback();
        let generation = self.start_space_probe(
            entries.clone(),
            target.clone(),
            flat.clone(),
            self.symlink_policy,
            move || {
                if let Some(wake) = &wake {
                    wake();
                }
            },
        );
        let filesystem = filesystem_preflight(&entries, &target, self.name_policy);
        let policy = match self.name_policy.collision {
            crate::filesystem_policy::CollisionPolicy::Ask => OverwritePolicy::Ask,
            crate::filesystem_policy::CollisionPolicy::KeepBoth => OverwritePolicy::KeepBoth,
            crate::filesystem_policy::CollisionPolicy::Skip => OverwritePolicy::SkipAll,
        };
        self.pending_op = Some(PendingOp::Transfer(PendingTransfer {
            kind,
            entries,
            expectations: Vec::new(),
            target,
            conflicts,
            policy,
            method: CopyMethod::Native,
            durability: self.durability_profile,
            version_retention: self.version_retention,
            name_policy: self.name_policy,
            symlink_policy: self.symlink_policy,
            filesystem,
            flat,
            space: TransferSpaceState::Pending { generation },
            start_when_ready: false,
        }));
    }

    fn start_space_probe(
        &mut self,
        entries: Vec<FileEntry>,
        target: PathBuf,
        flat: FlatList,
        symlink_policy: crate::filesystem_policy::SymlinkPolicy,
        notify: impl Fn() + Send + 'static,
    ) -> u64 {
        let port = std::sync::Arc::clone(&self.free_space_port);
        self.space_probes
            .start(entries, target, flat, symlink_policy, port, notify)
    }

    #[cfg(test)]
    fn set_space_probe_before_scan(&mut self, hook: std::sync::Arc<dyn Fn() + Send + Sync>) {
        self.space_probes.set_before_scan(hook);
    }

    /// Publish one complete preflight snapshot. Reports only bind to the exact
    /// pending target and generation that launched them.
    pub fn poll_space_probe(&mut self, notify: impl Fn() + Send + 'static) {
        let Some(report) = self.space_probes.poll() else {
            return;
        };
        self.apply_space_probe(report, notify);
    }

    fn apply_space_probe(
        &mut self,
        report: space_probe::SpaceProbeReport,
        notify: impl Fn() + Send + 'static,
    ) {
        let mut start_when_ready = false;
        if let Some(PendingOp::Transfer(transfer)) = &mut self.pending_op {
            let matches_binding = matches!(
                transfer.space,
                TransferSpaceState::Pending { generation }
                    if generation == report.generation && transfer.target == report.target
            );
            if matches_binding {
                let (space, expectations) = match (report.need_bytes, report.expectations) {
                    (Ok(need_bytes), Ok(expectations)) => (
                        TransferSpaceState::Ready {
                            generation: report.generation,
                            need_bytes,
                            free: report.free,
                            relation: report.relation,
                        },
                        Some(expectations),
                    ),
                    (Err(failure), _) | (_, Err(failure)) => (
                        TransferSpaceState::Failed {
                            generation: report.generation,
                            failure,
                            free: report.free,
                            relation: report.relation,
                        },
                        None,
                    ),
                };
                transfer.expectations = expectations.unwrap_or_default();
                transfer.space = space;
                start_when_ready = should_auto_start_after_preflight(transfer);
            }
        }
        if start_when_ready {
            self.start_transfer(notify);
        }
    }

    #[cfg(test)]
    fn finish_space_probe(&mut self) {
        if let Some(report) = self.space_probes.finish() {
            self.apply_space_probe(report, || {});
        }
    }

    pub fn dismiss_pending_op(&mut self) {
        if matches!(self.pending_op, Some(PendingOp::Transfer(_))) {
            self.space_probes.cancel();
        }
        self.pending_op = None;
    }

    /// Rich conflict list for the pending Copy/Move: source entries whose name
    /// already exists in the destination folder, with both sides' size/mtime.
    pub fn pending_conflicts(&self) -> Vec<crate::conflict::Conflict> {
        let Some(PendingOp::Transfer(tr)) = &self.pending_op else {
            return Vec::new();
        };
        // Prefer an already-loaded destination panel. A drop into a subfolder
        // that neither panel displays still needs rich conflict metadata, so
        // fall back to the known conflicting names on disk.
        let disk_dest;
        let dest: &[FileEntry] = if self.left.current_path == tr.target {
            self.left.entries()
        } else if self.right.current_path == tr.target {
            self.right.entries()
        } else {
            disk_dest = tr
                .conflicts
                .iter()
                .filter_map(|name| {
                    let path = tr.target.join(name);
                    let meta = std::fs::symlink_metadata(&path).ok()?;
                    FileEntry::from_meta(path, &meta)
                })
                .collect::<Vec<_>>();
            &disk_dest
        };
        crate::conflict::detect(&tr.entries, dest)
    }

    /// Apply a relation policy to the pending Copy: filter its entries to the
    /// resolution's keep-set and set the matching overwrite policy. Returns
    /// `true` if anything remains to transfer.
    pub fn resolve_pending_conflicts(&mut self, policy: crate::conflict::RelationPolicy) -> bool {
        let conflicts = self.pending_conflicts();
        let resolved = {
            let Some(PendingOp::Transfer(tr)) = &mut self.pending_op else {
                return false;
            };
            let res = crate::conflict::resolve(&tr.entries, &conflicts, policy);
            let keep: std::collections::HashSet<PathBuf> = res.keep.into_iter().collect();
            tr.entries.retain(|entry| keep.contains(&entry.path));
            tr.expectations.clear();
            tr.policy = match res.decision {
                crate::conflict::Decision::Overwrite => OverwritePolicy::OverwriteAll,
                crate::conflict::Decision::KeepBoth => OverwritePolicy::KeepBoth,
                crate::conflict::Decision::Skip => OverwritePolicy::SkipAll,
            };
            // Choosing a conflict policy is the user's confirmation. The
            // generation-bound preflight may finish later; it auto-starts only
            // for a proven space verdict.
            tr.start_when_ready = true;
            tr.conflicts = scan::find_conflicts(&tr.entries, &tr.target);
            tr.flat = scan::pending_flat_list();
            if tr.entries.is_empty() {
                None
            } else {
                Some((
                    tr.entries.clone(),
                    tr.target.clone(),
                    tr.flat.clone(),
                    tr.symlink_policy,
                ))
            }
        };
        let Some((entries, target, flat, symlink_policy)) = resolved else {
            self.space_probes.cancel();
            self.pending_op = None;
            return false;
        };
        let wake = self.active_panel_ref().notify_callback();
        let generation = self.start_space_probe(entries, target, flat, symlink_policy, move || {
            if let Some(wake) = &wake {
                wake();
            }
        });
        if let Some(PendingOp::Transfer(tr)) = &mut self.pending_op {
            tr.space = TransferSpaceState::Pending { generation };
        }
        true
    }

    pub fn set_pending_symlink_policy(
        &mut self,
        policy: crate::filesystem_policy::SymlinkPolicy,
        notify: impl Fn() + Send + 'static,
    ) -> bool {
        let (entries, target, flat) = {
            let Some(PendingOp::Transfer(transfer)) = &mut self.pending_op else {
                return false;
            };
            if transfer.symlink_policy == policy {
                return false;
            }
            transfer.symlink_policy = policy;
            transfer.expectations.clear();
            transfer.flat = scan::pending_flat_list();
            (
                transfer.entries.clone(),
                transfer.target.clone(),
                transfer.flat.clone(),
            )
        };
        let generation = self.start_space_probe(entries, target, flat, policy, notify);
        if let Some(PendingOp::Transfer(transfer)) = &mut self.pending_op {
            transfer.space = TransferSpaceState::Pending { generation };
        }
        true
    }

    pub fn request_delete(&mut self) {
        if !self.can_request_delete() {
            return;
        }
        let Ok(entries) = self.active_panel_ref().selected_or_cursor() else {
            return;
        };
        self.request_delete_entries(entries);
    }

    pub fn request_context_delete(&mut self, panel: ActivePanel, path: &Path) {
        if !self.can_request_delete() {
            return;
        }
        self.active = panel;
        let source = self.active_panel_ref();
        let entries = if source.is_selected(path) {
            source.selected_or_cursor().unwrap_or_default()
        } else {
            source
                .entries()
                .iter()
                .find(|entry| entry.path == path)
                .cloned()
                .into_iter()
                .collect()
        };
        self.request_delete_entries(entries);
    }

    fn request_delete_entries(&mut self, entries: Vec<FileEntry>) {
        if entries.is_empty() {
            return;
        }
        let targets = entries.iter().map(trash_batch_item).collect();
        let flat = scan::spawn_scan(entries.clone());
        self.pending_op = Some(PendingOp::Delete {
            entries,
            targets,
            flat,
        });
    }

    /// Start background copy/move with progress tracking.
    /// `notify` is invoked when visible progress changes (UI passes a
    /// repaint request).
    pub fn start_transfer(&mut self, notify: impl Fn() + Send + 'static) {
        if self.mutation_commits_blocked() {
            return;
        }
        let ready = matches!(
            &self.pending_op,
            Some(PendingOp::Transfer(transfer))
                if transfer.space_ready()
                    && transfer.expectations.len() == transfer.entries.len()
                    && !transfer.overflows()
        );
        if !ready {
            return;
        }
        let Some(PendingOp::Transfer(t)) = self.pending_op.take() else {
            return;
        };
        self.durability_profile = t.durability;
        self.version_retention = t.version_retention;
        self.name_policy = t.name_policy;
        self.symlink_policy = t.symlink_policy;
        let preflight_bytes = t.need_bytes();
        // A Move is undoable, promoted onto the history stack when it finishes
        // cleanly (see `poll_transfer`); a Copy records no history.
        let undo = if t.kind == TransferKind::Move {
            Some(crate::undo::Action::Move {
                pairs: move_pairs(&t.entries, &t.target),
            })
        } else {
            None
        };
        let spec = TransferSpec {
            operation_id: crate::operation::OperationId::new(),
            group_id: None,
            kind: t.kind,
            entries: t.entries,
            expectations: t.expectations,
            target: t.target,
            policy: t.policy,
            method: t.method,
            durability: t.durability,
            version_retention: t.version_retention,
            name_policy: t.name_policy,
            symlink_policy: t.symlink_policy,
            preflight_bytes,
            post_success: None,
            rollback_cleanup: None,
            rollback_cleanup_identity: None,
            #[cfg(test)]
            mount_wait_override: None,
            #[cfg(test)]
            before_commit: None,
            #[cfg(test)]
            before_post_success: None,
            #[cfg(test)]
            before_terminal_publish: None,
            #[cfg(test)]
            journal_enabled: false,
        };
        self.enqueue_only(spec, undo);
        self.pump_queue(notify);
    }

    /// Undo the most recent reversible action (Cmd+Z): execute its inverse and
    /// move it onto the redo stack only after the filesystem commit succeeds.
    pub fn preview_undo(&self) -> Option<crate::undo::ReplayPreview> {
        self.undo
            .preview_undo_action()
            .map(|action| crate::undo::preview(&action))
    }

    pub fn preview_redo(&self) -> Option<crate::undo::ReplayPreview> {
        self.undo
            .preview_redo_action()
            .map(|action| crate::undo::preview(&action))
    }

    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    pub fn top_undo_action(&self) -> Option<&crate::undo::Action> {
        self.undo.top_undo()
    }

    #[cfg(test)]
    pub(crate) fn record_history_for_test(&mut self, action: crate::undo::Action) {
        self.undo.record(action).expect("test history record");
    }

    #[cfg(test)]
    pub(crate) fn begin_history_replay_for_test(
        &mut self,
        direction: crate::undo::ReplayDirection,
    ) -> crate::undo::ReplayPlan {
        self.undo
            .begin(direction)
            .expect("test history replay begin")
            .expect("test history replay plan")
    }

    #[cfg(test)]
    pub(crate) fn commit_history_replay_for_test(
        &mut self,
        reservation: crate::undo::ReplayReservation,
    ) {
        self.undo
            .commit_immediate(reservation)
            .expect("test history replay commit");
    }

    pub fn redo_unavailable_reason(&self) -> Option<String> {
        self.undo.redo_invalidation().map(|invalidation| {
            format!(
                "Redo was invalidated by {} after {} undone action{}",
                invalidation.caused_by,
                invalidation.abandoned_actions,
                if invalidation.abandoned_actions == 1 {
                    ""
                } else {
                    "s"
                }
            )
        })
    }

    pub(crate) fn history_replay_blocker(&self) -> Option<String> {
        if let Some(reason) = self.mutation_block_reason("history replay") {
            return Some(reason);
        }
        if self.has_unfinished_transfer_work() {
            return Some("Wait for the transfer queue before replaying history".to_string());
        }
        None
    }

    fn ensure_history_replay_ready(&self) -> Result<(), String> {
        self.history_replay_blocker().map_or(Ok(()), Err)
    }

    fn replay_preflight(preview: &crate::undo::ReplayPreview) -> Result<(), String> {
        if preview.can_execute() {
            return Ok(());
        }
        let detail = preview
            .paths
            .iter()
            .find_map(|path| match &path.eligibility {
                crate::undo::ReplayEligibility::Ready => None,
                crate::undo::ReplayEligibility::Blocked(reason) => Some(format!(
                    "{} -> {}: {reason}",
                    path.from.display(),
                    path.to.display()
                )),
            })
            .unwrap_or_else(|| "No replayable paths remain".to_string());
        Err(format!(
            "History replay is blocked for {} path{}; {detail}",
            preview.blocked_count(),
            if preview.blocked_count() == 1 {
                ""
            } else {
                "s"
            }
        ))
    }

    pub fn perform_undo(&mut self, notify: impl Fn() + Send + 'static) -> Result<(), String> {
        self.ensure_history_replay_ready()?;
        let Some(preview) = self.preview_undo() else {
            return Ok(());
        };
        Self::replay_preflight(&preview)?;
        self.perform_history_replay(crate::undo::ReplayDirection::Undo, notify)
    }

    /// Redo the most recently undone action (Cmd+Shift+Z): re-apply it and move
    /// it back onto the undo stack. Returns the same error surface as
    /// [`perform_undo`].
    pub fn perform_redo(&mut self, notify: impl Fn() + Send + 'static) -> Result<(), String> {
        self.ensure_history_replay_ready()?;
        let Some(preview) = self.preview_redo() else {
            return self.redo_unavailable_reason().map_or(Ok(()), Err);
        };
        Self::replay_preflight(&preview)?;
        self.perform_history_replay(crate::undo::ReplayDirection::Redo, notify)
    }

    fn perform_history_replay(
        &mut self,
        direction: crate::undo::ReplayDirection,
        notify: impl Fn() + Send + 'static,
    ) -> Result<(), String> {
        let Some(plan) = self
            .undo
            .begin(direction)
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        let crate::undo::ReplayPlan {
            action,
            reservation,
        } = plan;
        match self.execute_action(action, reservation, notify) {
            Ok(ActionExecution::Completed) => self
                .undo
                .commit_immediate(reservation)
                .map_err(|error| self.history_invariant_error(error)),
            Ok(ActionExecution::Started) => Ok(()),
            Err(execution_error) => match self.undo.abort_immediate(reservation) {
                Ok(()) => Err(execution_error),
                Err(history_error) => Err(format!(
                    "{execution_error}; {}",
                    self.history_invariant_error(history_error)
                )),
            },
        }
    }

    fn history_invariant_error(&mut self, error: crate::undo::HistoryError) -> String {
        let reason = format!("History settlement failed: {error}");
        if self.safe_state.is_none() {
            let failure = crate::operation::ClassifiedFailure::message(
                crate::operation::FailureClass::IntegrityUncertain,
                None,
                reason.clone(),
            );
            self.safe_state = Some(crate::operation::SafeState {
                operation_id: crate::operation::OperationId::new(),
                reason: reason.clone(),
                paths: Vec::new(),
                failures: vec![failure],
            });
        }
        reason
    }

    fn latch_rename_execution_error(&mut self, error: &RenameExecutionError) {
        if !error.integrity_uncertain || self.safe_state.is_some() {
            return;
        }
        let reason = format!(
            "Rename rollback did not restore the original namespace: {}",
            error.message
        );
        let failure = crate::operation::ClassifiedFailure::message(
            crate::operation::FailureClass::IntegrityUncertain,
            error.paths.first().cloned(),
            reason.clone(),
        );
        self.safe_state = Some(crate::operation::SafeState {
            operation_id: crate::operation::OperationId::new(),
            reason,
            paths: error.paths.clone(),
            failures: vec![failure],
        });
    }

    /// Execute `action` forward against the filesystem. Used by undo (with an
    /// inverted action) and redo (with the original). It records no new history
    /// of its own; the matching reservation settles only after execution.
    fn execute_action(
        &mut self,
        action: crate::undo::Action,
        reservation: crate::undo::ReplayReservation,
        notify: impl Fn() + Send + 'static,
    ) -> Result<ActionExecution, String> {
        Self::replay_preflight(&crate::undo::preview(&action))?;
        match action {
            crate::undo::Action::Move { pairs } => {
                // Each pair is (from, to): move the file at `from` into dir(to).
                // Move errors surface through the transfer engine's error list,
                // not here, so a started move is reported as Ok.
                let Some(dest_dir) = pairs
                    .first()
                    .and_then(|(_, to)| to.parent())
                    .map(Path::to_path_buf)
                else {
                    return Ok(ActionExecution::Completed);
                };
                let sources: Vec<PathBuf> = pairs.into_iter().map(|(from, _)| from).collect();
                self.start_move_silent(
                    sources,
                    dest_dir,
                    None,
                    transfer_queue::HistoryIntent::replay(reservation),
                    notify,
                )
                .map(|started| {
                    if started {
                        ActionExecution::Started
                    } else {
                        ActionExecution::Completed
                    }
                })
            }
            crate::undo::Action::BatchRename { dir, pairs } => {
                // Undo/redo replays recorded pairs without re-planning, so order
                // them against the live directory (the inverse of a swap or a
                // case-only rename is itself a swap/case-only and needs staging).
                let existing = Self::dir_names(&dir);
                let result = Self::apply_rename_order(&dir, &pairs, &existing);
                self.left.refresh();
                self.right.refresh();
                if let Err(error) = &result {
                    self.latch_rename_execution_error(error);
                }
                // A failed rename undo/redo leaves the filesystem out of step
                // with the stack: surface it rather than swallowing the error.
                result
                    .map(|_| ActionExecution::Completed)
                    .map_err(|error| error.to_string())
            }
            crate::undo::Action::Rename { from, to } => {
                let result = Self::rename_path_no_clobber(&from, &to);
                self.left.refresh();
                self.right.refresh();
                if let Err(error) = &result {
                    self.latch_rename_execution_error(error);
                }
                result
                    .map(|_| ActionExecution::Completed)
                    .map_err(|error| error.to_string())
            }
            crate::undo::Action::Gather { folder, pairs } => {
                let sources: Vec<PathBuf> = pairs.into_iter().map(|(from, _)| from).collect();
                let entries = Self::entries_for_paths(&sources)?;
                if entries.is_empty() {
                    return Ok(ActionExecution::Completed);
                }
                std::fs::create_dir(&folder)
                    .map_err(|error| format!("Could not recreate {}: {error}", folder.display()))?;
                self.enqueue_silent_move(
                    entries,
                    folder.clone(),
                    None,
                    Some(folder.clone()),
                    transfer_queue::HistoryIntent::replay(reservation),
                    notify,
                )?;
                Ok(ActionExecution::Started)
            }
            crate::undo::Action::Ungather { folder, pairs } => {
                let Some(dest_dir) = pairs
                    .first()
                    .and_then(|(_, to)| to.parent())
                    .map(Path::to_path_buf)
                else {
                    return Ok(ActionExecution::Completed);
                };
                let sources = pairs.into_iter().map(|(from, _)| from).collect();
                self.start_move_silent(
                    sources,
                    dest_dir,
                    Some(PostTransferAction::RemoveEmptyDir(folder)),
                    transfer_queue::HistoryIntent::replay(reservation),
                    notify,
                )
                .map(|started| {
                    if started {
                        ActionExecution::Started
                    } else {
                        ActionExecution::Completed
                    }
                })
            }
        }
    }

    fn entries_for_paths(sources: &[PathBuf]) -> Result<Vec<FileEntry>, String> {
        sources
            .iter()
            .map(|path| {
                let meta = std::fs::symlink_metadata(path)
                    .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
                FileEntry::from_meta(path.clone(), &meta)
                    .ok_or_else(|| format!("Invalid source path: {}", path.display()))
            })
            .collect()
    }

    fn enqueue_silent_move(
        &mut self,
        entries: Vec<FileEntry>,
        dest_dir: PathBuf,
        post_success: Option<PostTransferAction>,
        rollback_cleanup: Option<PathBuf>,
        history: transfer_queue::HistoryIntent,
        notify: impl Fn() + Send + 'static,
    ) -> Result<(), String> {
        let rollback_cleanup_identity = rollback_cleanup
            .as_ref()
            .map(|path| {
                crate::path_identity::PathIdentity::observe_deep(path).map_err(|error| {
                    format!(
                        "Could not prove replay-owned folder {}: {error}",
                        path.display()
                    )
                })
            })
            .transpose()?;
        let spec = TransferSpec {
            operation_id: crate::operation::OperationId::new(),
            group_id: None,
            kind: TransferKind::Move,
            entries,
            expectations: Vec::new(),
            target: dest_dir,
            policy: OverwritePolicy::Ask,
            method: CopyMethod::Native,
            durability: self.durability_profile,
            version_retention: self.version_retention,
            name_policy: self.name_policy,
            symlink_policy: self.symlink_policy,
            preflight_bytes: None,
            post_success,
            rollback_cleanup,
            rollback_cleanup_identity,
            #[cfg(test)]
            mount_wait_override: None,
            #[cfg(test)]
            before_commit: None,
            #[cfg(test)]
            before_post_success: None,
            #[cfg(test)]
            before_terminal_publish: None,
            #[cfg(test)]
            journal_enabled: false,
        };
        self.enqueue_bound_replay(spec, history)?;
        self.pump_queue(notify);
        Ok(())
    }

    /// Move the files at `sources` into `dest_dir` without recording a new undo
    /// entry. One source dir, one destination dir, matching how user moves are
    /// shaped.
    fn start_move_silent(
        &mut self,
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
        post_success: Option<PostTransferAction>,
        history: transfer_queue::HistoryIntent,
        notify: impl Fn() + Send + 'static,
    ) -> Result<bool, String> {
        let entries = Self::entries_for_paths(&sources)?;
        if entries.is_empty() {
            return Ok(false);
        }
        self.enqueue_silent_move(entries, dest_dir, post_success, None, history, notify)?;
        Ok(true)
    }

    /// Add `dir` to the bookmarks if it is not already present, naming it after
    /// its final path component (the volume root falls back to its display
    /// path). A no-op when already bookmarked, so a slot assign keeps the name.
    fn bookmark_dir(&mut self, dir: PathBuf) {
        if self.bookmarks.contains(&dir) {
            return;
        }
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| dir.display().to_string());
        self.bookmarks.add(name, dir);
    }

    /// Every name currently in `dir` (best-effort), so a rename batch can be
    /// ordered against the live directory at apply/replay time.
    fn dir_names(dir: &Path) -> std::collections::HashSet<String> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect()
    }

    /// Apply a `(from, to)` rename map in `dir` by computing a mid-batch-safe
    /// order (swaps, rotations and case-only renames are staged through a temp;
    /// see [`crate::rename_order`]) and executing it, rolling back on an OS
    /// failure. Returns how many entries were renamed, or a user-facing error
    /// (an unresolvable conflict, or the first OS error).
    fn apply_rename_order(
        dir: &Path,
        map: &[(String, String)],
        existing: &std::collections::HashSet<String>,
    ) -> Result<usize, RenameExecutionError> {
        Self::apply_rename_order_using(dir, map, existing, |from, to| {
            crate::native_copy::rename_noreplace(&dir.join(from), &dir.join(to))
        })
    }

    fn apply_rename_order_using<E: std::fmt::Display>(
        dir: &Path,
        map: &[(String, String)],
        existing: &std::collections::HashSet<String>,
        rename: impl FnMut(&str, &str) -> Result<(), E>,
    ) -> Result<usize, RenameExecutionError> {
        use crate::rename_order::{RenameOrder, apply_steps, safe_rename_order};
        match safe_rename_order(map, existing) {
            RenameOrder::Conflict(why) => Err(RenameExecutionError::unchanged(why)),
            RenameOrder::Steps(steps) => apply_steps(&steps, rename).map_err(|error| {
                let message = error.to_string();
                if error.rollback_failures.is_empty() {
                    RenameExecutionError::unchanged(message)
                } else {
                    let mut paths = error
                        .rollback_failures
                        .iter()
                        .flat_map(|failure| [dir.join(&failure.from), dir.join(&failure.to)])
                        .collect::<Vec<_>>();
                    paths.sort();
                    paths.dedup();
                    RenameExecutionError::uncertain(message, paths)
                }
            }),
        }
    }

    /// Confirm the pending op. Both transfers and deletes report completion
    /// asynchronously through their controller poll methods.
    pub fn confirm_pending_op(&mut self, notify: impl Fn() + Send + 'static) -> bool {
        if self.mutation_commits_blocked() {
            return false;
        }
        match &self.pending_op {
            Some(PendingOp::Delete { .. }) => {
                if self.has_unfinished_transfer_work() || self.deletes.is_active() {
                    return false;
                }
                if let Some(PendingOp::Delete { targets, .. }) = self.pending_op.take() {
                    return self.deletes.start(
                        targets,
                        DeleteOrigin::Confirmation,
                        self.durability_profile,
                        self.version_retention,
                        notify,
                    );
                }
                false
            }
            Some(PendingOp::Transfer(_)) => {
                self.start_transfer(notify);
                self.pending_op.is_none()
            }
            None => false,
        }
    }

    pub fn create_dir(&mut self) {
        if self.mutation_commits_blocked() {
            return;
        }
        let base = self.active_panel_ref().current_path.clone();
        let path = crate::fs_util::first_available(|i| {
            if i == 0 {
                base.join("New Folder")
            } else {
                base.join(format!("New Folder {}", i))
            }
        });
        let _ = std::fs::create_dir(&path);
        self.left.refresh();
        self.right.refresh();
    }

    /// Gather the active panel's selection into a fresh subfolder (Finder's
    /// "New Folder with Selection"): create a uniquely-named folder under the
    /// active directory and move the selection into it as one undoable Move
    /// (queued through the transfer pipeline, so it takes the same-volume rename
    /// fast path). A no-op on an empty selection or if the folder can't be made.
    pub fn gather_into_folder(&mut self, notify: impl Fn() + Send + 'static) {
        if self.mutation_commits_blocked() {
            return;
        }
        let Ok(entries) = self.active_panel_ref().selected_or_cursor() else {
            return;
        };
        if entries.is_empty() {
            return;
        }
        let base = self.active_panel_ref().current_path.clone();
        let name = crate::selection_summary::suggest_folder_name(&entries);
        // A collision-free folder name (never clobbers an existing sibling,
        // including one of the entries being gathered).
        let folder = crate::fs_util::first_available(|i| {
            if i == 0 {
                base.join(&name)
            } else {
                base.join(format!("{name} {}", i + 1))
            }
        });
        if std::fs::create_dir(&folder).is_err() {
            return;
        }
        // Record the folder lifecycle separately from an ordinary Move so undo
        // can remove it and redo can recreate it.
        let undo = crate::undo::Action::Gather {
            folder: folder.clone(),
            pairs: move_pairs(&entries, &folder),
        };
        let rollback_cleanup_identity =
            crate::path_identity::PathIdentity::observe_deep(&folder).ok();
        let spec = TransferSpec {
            operation_id: crate::operation::OperationId::new(),
            group_id: None,
            kind: TransferKind::Move,
            entries,
            expectations: Vec::new(),
            target: folder.clone(),
            policy: OverwritePolicy::Ask,
            method: CopyMethod::Native,
            durability: self.durability_profile,
            version_retention: self.version_retention,
            name_policy: self.name_policy,
            symlink_policy: self.symlink_policy,
            preflight_bytes: None,
            post_success: None,
            rollback_cleanup: Some(folder),
            rollback_cleanup_identity,
            #[cfg(test)]
            mount_wait_override: None,
            #[cfg(test)]
            before_commit: None,
            #[cfg(test)]
            before_post_success: None,
            #[cfg(test)]
            before_terminal_publish: None,
            #[cfg(test)]
            journal_enabled: false,
        };
        self.enqueue_only(spec, Some(undo));
        self.pump_queue(notify);
    }

    /// Names beside `old`, excluding `old` itself. Captured by the rename UI at
    /// open time; commit performs the same check again against the live disk.
    pub fn rename_siblings(old: &Path) -> Vec<String> {
        let old_name = old.file_name().map(|n| n.to_string_lossy().to_string());
        old.parent()
            .map(Self::dir_names)
            .unwrap_or_default()
            .into_iter()
            .filter(|name| Some(name) != old_name.as_ref())
            .collect()
    }

    /// Rename one path without replacing an unrelated destination. Case-only
    /// changes stage through a temporary name and roll back if the second move
    /// fails, preserving the recovery path in a composite error if rollback
    /// itself also fails.
    fn rename_path_no_clobber(from: &Path, to: &Path) -> Result<(), RenameExecutionError> {
        if from == to {
            return Ok(());
        }
        let dest_meta = to.symlink_metadata().ok();
        let same_file = match &dest_meta {
            Some(dest) => from.symlink_metadata().ok().is_some_and(|source| {
                use std::os::unix::fs::MetadataExt;
                source.ino() == dest.ino() && source.dev() == dest.dev()
            }),
            None => false,
        };
        if dest_meta.is_some() && !same_file {
            return Err(RenameExecutionError::unchanged("Name already in use"));
        }
        if !same_file {
            return crate::native_copy::rename_noreplace(from, to)
                .map_err(|error| RenameExecutionError::unchanged(error.to_string()));
        }

        let parent = to
            .parent()
            .ok_or_else(|| RenameExecutionError::unchanged("Path has no parent"))?;
        let tmp = crate::fs_util::first_available(|i| parent.join(format!(".cmdr-rename.{i}")));
        crate::native_copy::rename_noreplace(from, &tmp)
            .map_err(|error| RenameExecutionError::unchanged(error.to_string()))?;
        match crate::native_copy::rename_noreplace(&tmp, to) {
            Ok(()) => Ok(()),
            Err(rename_error) => match crate::native_copy::rename_noreplace(&tmp, from) {
                Ok(()) => Err(RenameExecutionError::unchanged(rename_error.to_string())),
                Err(rollback_error) => Err(RenameExecutionError::uncertain(
                    format!(
                        "{rename_error}; rollback failed: {rollback_error}; file preserved at {}",
                        tmp.display()
                    ),
                    vec![from.to_path_buf(), to.to_path_buf(), tmp],
                )),
            },
        }
    }

    /// Rename `old` to `new_name` in the same directory. A no-op (unchanged
    /// name) succeeds silently. Successful changes are recorded for undo/redo.
    pub fn commit_rename(&mut self, old: &Path, new_name: &str) -> Result<(), String> {
        if let Some(reason) = self.mutation_block_reason("rename") {
            return Err(reason);
        }
        let new_name = new_name.trim();
        let old_name = old
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if new_name == old_name {
            return Ok(()); // nothing to do
        }
        let siblings = Self::rename_siblings(old);
        crate::pathname::validate_new_name(new_name, &siblings)
            .map_err(|error| error.to_string())?;
        let dest = old
            .parent()
            .map(|p| p.join(new_name))
            .ok_or("Path has no parent")?;
        if let Err(error) = Self::rename_path_no_clobber(old, &dest) {
            self.latch_rename_execution_error(&error);
            return Err(error.to_string());
        }
        self.undo
            .record(crate::undo::Action::Rename {
                from: old.to_path_buf(),
                to: dest,
            })
            .map_err(|error| self.history_invariant_error(error))?;
        self.left.refresh();
        self.right.refresh();
        Ok(())
    }

    // ── Batch rename ────────────────────────────────────────────────────

    pub fn batch_rename_context(&self) -> Option<BatchRenameContext> {
        let panel = self.active_panel_ref();
        let targets: Vec<String> = panel
            .selected_or_cursor()
            .ok()?
            .into_iter()
            .map(|e| e.name)
            .collect();
        if targets.is_empty() {
            return None;
        }
        Some(BatchRenameContext {
            panel: self.active,
            dir: panel.current_path.clone(),
            targets,
            existing: panel.entries().iter().map(|e| e.name.clone()).collect(),
        })
    }

    /// Apply a batch rename to the active panel. Genuine swaps, rotations and
    /// case-only renames are now allowed: they are ordered safely (staging
    /// through a temp where needed) by [`Self::apply_rename_order`]. Only
    /// invalid target names and unresolvable conflicts (a target landing on an
    /// untouched sibling, or two rows clashing) are refused. Returns the number
    /// of entries renamed, or a user-facing error.
    /// Apply a batch rename to the exact context captured when its dialog
    /// opened. The live directory is re-read at commit time so newly-created
    /// siblings still participate in collision checks.
    pub fn apply_batch_rename_in(
        &mut self,
        context: &BatchRenameContext,
        rule: &crate::rename::RenameRule,
    ) -> Result<usize, String> {
        if let Some(reason) = self.mutation_block_reason("batch rename") {
            return Err(reason);
        }
        if context.targets.is_empty() {
            return Err("Nothing selected to rename".into());
        }
        if let Some(error) = crate::rename::regex_error(rule) {
            return Err(format!("Invalid regex: {error}"));
        }
        let existing = Self::dir_names(&context.dir);
        let plans = crate::rename::plan_batch_rename(&context.targets, &existing, rule);
        if plans
            .iter()
            .any(|p| p.status == crate::rename::PlanStatus::Invalid)
        {
            return Err("Fix the invalid names first".into());
        }
        // Every row whose name actually changes (Ok or a resolvable collision).
        let changes: Vec<(String, String)> = plans
            .iter()
            .filter(|p| p.to != p.from)
            .map(|p| (p.from.clone(), p.to.clone()))
            .collect();
        if changes.is_empty() {
            return Ok(0);
        }
        let dir = context.dir.clone();

        let done = match Self::apply_rename_order(&dir, &changes, &existing) {
            Ok(done) => done,
            Err(error) => {
                self.latch_rename_execution_error(&error);
                let panel = match context.panel {
                    ActivePanel::Left => &mut self.left,
                    ActivePanel::Right => &mut self.right,
                };
                if panel.current_path == context.dir {
                    panel.refresh();
                }
                return Err(error.to_string());
            }
        };
        // Record the batch as one undoable unit (Cmd+Z reverts the whole run).
        if done > 0 {
            self.undo
                .record(crate::undo::Action::BatchRename {
                    dir,
                    pairs: changes,
                })
                .map_err(|error| self.history_invariant_error(error))?;
        }
        let panel = match context.panel {
            ActivePanel::Left => &mut self.left,
            ActivePanel::Right => &mut self.right,
        };
        if panel.current_path == context.dir {
            panel.clear_selection();
            panel.refresh();
        }
        Ok(done)
    }

    // ── Duplicates ──────────────────────────────────────────────────────

    /// Find duplicate files in the active panel's directory (files only,
    /// non-recursive for now). Size-prefilters so only files whose size
    /// collides are content-hashed, groups by (size, hash), then confirms each
    /// group is byte-identical (a 64-bit hash match is not proof) before
    /// reporting it, so the dialog never offers to trash a false positive.
    pub fn find_duplicates(&self) -> Vec<crate::dedup::DupGroup> {
        use std::collections::HashMap;
        let files: Vec<&FileEntry> = self
            .active_panel_ref()
            .entries()
            .iter()
            .filter(|e| !e.is_dir)
            .collect();
        let mut size_counts: HashMap<u64, usize> = HashMap::new();
        for f in &files {
            *size_counts.entry(f.size).or_insert(0) += 1;
        }
        let mut keys: Vec<crate::dedup::FileKey> = Vec::new();
        for f in &files {
            if size_counts.get(&f.size).copied().unwrap_or(0) < 2 {
                continue; // a unique size cannot have a duplicate
            }
            if let Some(hash) = crate::fs_util::content_hash(&f.path) {
                keys.push(crate::dedup::FileKey {
                    path: f.path.clone(),
                    identity: f.identity.clone(),
                    size: f.size,
                    hash,
                    modified: f.modified,
                });
            }
        }
        crate::dedup::group_duplicates(&keys)
            .into_iter()
            .flat_map(Self::confirm_dup_group)
            .collect()
    }

    /// Split a hash-matched group into byte-identical clusters, keeping only
    /// those with two or more members. Guards against 64-bit hash collisions
    /// being trashed as duplicates.
    fn confirm_dup_group(group: crate::dedup::DupGroup) -> Vec<crate::dedup::DupGroup> {
        let size = group.size;
        let mut clusters: Vec<Vec<crate::dedup::FileKey>> = Vec::new();
        for file in group.files {
            if let Some(c) = clusters
                .iter_mut()
                .find(|c| crate::fs_util::files_equal(&c[0].path, &file.path))
            {
                c.push(file);
            } else {
                clusters.push(vec![file]);
            }
        }
        clusters
            .into_iter()
            .filter(|c| c.len() >= 2)
            .map(|files| crate::dedup::DupGroup { size, files })
            .collect()
    }

    pub fn trash_entries(
        &mut self,
        items: Vec<crate::ports::TrashBatchItem>,
        notify: impl Fn() + Send + 'static,
    ) -> bool {
        if items.is_empty()
            || self.mutation_commits_blocked()
            || self.has_unfinished_transfer_work()
            || self.deletes.is_active()
            || self.pending_op.is_some()
        {
            return false;
        }
        self.deletes.start(
            items,
            DeleteOrigin::Duplicates,
            self.durability_profile,
            self.version_retention,
            notify,
        )
    }

    pub fn poll_delete(&mut self) -> Option<DeleteOutcome> {
        let outcome = self.deletes.poll()?;
        if outcome.refresh_required() {
            self.left.refresh();
            self.right.refresh();
        }
        Some(outcome)
    }

    #[cfg(test)]
    fn finish_delete(&mut self) -> Option<DeleteOutcome> {
        let outcome = self.deletes.finish()?;
        if outcome.refresh_required() {
            self.left.refresh();
            self.right.refresh();
        }
        Some(outcome)
    }

    // ── Disk usage treemap ──────────────────────────────────────────────

    /// Capture the active folder and its direct children as (entry, bytes),
    /// sized by file length or cached recursive directory size (0 if not ready),
    /// sorted largest first. Reads the existing cache; never walks.
    pub fn treemap_snapshot(&self) -> TreemapSnapshot {
        let active = self.active_panel_ref();
        let sizes = active.size_snapshot();
        let mut items: Vec<(FileEntry, u64)> = active
            .entries()
            .iter()
            .map(|e| {
                let bytes = if e.is_dir {
                    sizes.size_of(&e.path).unwrap_or(0)
                } else {
                    e.size
                };
                (e.clone(), bytes)
            })
            .collect();
        items.sort_by_key(|i| std::cmp::Reverse(i.1));
        TreemapSnapshot {
            dir: active.current_path.clone(),
            items,
        }
    }

    /// Reveal `path` in the active panel: navigate to its parent folder and put
    /// the cursor on it (used by find results).
    pub fn reveal(&mut self, path: &Path) {
        let Some(parent) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        let name = path.file_name().map(|n| n.to_string_lossy().to_string());
        let panel = self.active_panel();
        panel.navigate_to(parent);
        if let Some(name) = name
            && let Some(idx) = panel.filtered_entries().iter().position(|e| e.name == name)
        {
            panel.set_cursor(idx + 1);
            panel.set_scroll_to_cursor(true);
        }
    }

    // ── Diff ────────────────────────────────────────────────────────────

    /// Pick the file pair to diff: two selected files in the active panel (in
    /// the panel's order), or one active file paired with a same-named file in
    /// the other panel. Returns `None` when no sensible pair exists.
    pub fn diff_targets(&self) -> Option<(PathBuf, PathBuf)> {
        let active = self.active_panel_ref();
        let files: Vec<PathBuf> = active
            .selected_entries()
            .into_iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.path)
            .collect();
        if files.len() == 2 {
            return Some((files[0].clone(), files[1].clone()));
        }
        // One file (selected, else under the cursor) vs the same name opposite.
        let one = if files.len() == 1 {
            files.into_iter().next()
        } else {
            active
                .cursor_entry()
                .filter(|e| !e.is_dir)
                .map(|e| e.path.clone())
        }?;
        let name = one.file_name()?.to_string_lossy().to_lowercase();
        let other = self
            .inactive_panel()
            .entries()
            .iter()
            .find(|e| !e.is_dir && e.name_lower == name)?;
        Some((other.path.clone(), one))
    }

    // ── Shelf (drop stack) ──────────────────────────────────────────────

    /// Copy every shelved item into the active panel's directory in one pass,
    /// dropping self-copies and keeping both on name collisions, then clear the
    /// shelf. Routes through the transfer engine. Items whose source can no
    /// longer be read are kept on the shelf (not silently discarded), and the
    /// outcome reports both how many copies started and how many were left.
    pub fn drain_shelf(&mut self, notify: impl Fn() + Send + 'static) -> ShelfDrainOutcome {
        if self.shelf.is_empty()
            || self.has_unfinished_transfer_work()
            || self.mutation_commits_blocked()
        {
            return ShelfDrainOutcome::default();
        }
        let dest = self.active_panel_ref().current_path.clone();
        let existing: std::collections::HashSet<String> = self
            .active_panel_ref()
            .entries()
            .iter()
            .map(|e| e.name.clone())
            .collect();
        let plan = crate::shelf::drain_plan(self.shelf.items(), &dest, &existing);

        // Build sources for the planned copies, keeping any whose source could
        // not be read so they stay on the shelf rather than vanishing.
        let mut entries: Vec<FileEntry> = Vec::new();
        let mut unavailable: Vec<PathBuf> = Vec::new();
        for (src, _) in &plan {
            match std::fs::metadata(src)
                .ok()
                .and_then(|m| FileEntry::from_meta(src.clone(), &m))
            {
                Some(fe) => entries.push(fe),
                None => unavailable.push(src.clone()),
            }
        }

        // Self-copies (excluded by the plan) and started copies leave the
        // shelf; only the unreadable items remain for the user to retry.
        self.shelf.clear();
        self.shelf.add_all(unavailable.iter().cloned());

        let outcome = ShelfDrainOutcome {
            started: entries.len(),
            unavailable: unavailable.len(),
        };
        if !entries.is_empty() {
            // KeepBoth so a drained file never clobbers an existing one.
            self.start_copy(entries, dest, OverwritePolicy::KeepBoth, notify);
        }
        outcome
    }

    // ── Directory sync ──────────────────────────────────────────────────

    /// Compute the synchronisation plan between the two panels for `policy`.
    #[cfg(test)]
    pub fn build_sync_actions(
        &self,
        policy: crate::sync::SyncPolicy,
    ) -> Vec<crate::sync::SyncAction> {
        crate::sync::sync_diff(self.left.entries(), self.right.entries(), policy)
    }

    /// Build a sync plan and baseline from the same fresh directory reads.
    pub fn build_guarded_sync_plan(
        &self,
        policy: crate::sync::SyncPolicy,
    ) -> Result<(Vec<crate::sync::SyncAction>, crate::sync_guard::PlanStamp), String> {
        crate::sync_guard::build_plan(
            &self.left.current_path,
            &self.right.current_path,
            self.left.show_hidden(),
            self.right.show_hidden(),
            policy,
        )
    }

    /// Resolve and start a synchronisation plan: copy each `ToRight` row's left
    /// file into the right directory and each `ToLeft` row's right file into the
    /// left directory. Both passes are enqueued on the transfer queue and run in
    /// order (the left-bound pass starts when the right-bound one finishes), so
    /// the engine stays single-transfer with no special follow-up handling.
    /// Apply a synchronization snapshot to the directories it was opened for.
    /// Sources are resolved by their captured paths, never by a lowercased
    /// display name or by whichever panels happen to be active now.
    #[cfg(test)]
    pub fn apply_sync_between(
        &mut self,
        actions: &[crate::sync::SyncAction],
        left_dir: &Path,
        right_dir: &Path,
        notify: impl Fn() + Send + 'static,
    ) {
        if self.mutation_commits_blocked() {
            return;
        }
        self.enqueue_sync_between(actions, left_dir, right_dir, notify);
    }

    /// Revalidate every fail-closed sync guard before the first transfer is
    /// queued. A rejected plan cannot leave a partially enqueued operation.
    pub fn apply_sync_guarded(
        &mut self,
        plan: crate::sync_guard::GuardedPlan<'_>,
        notify: impl Fn() + Send + 'static,
    ) -> Result<crate::sync_guard::Assessment, String> {
        if let Some(reason) = self.mutation_block_reason("synchronization") {
            return Err(reason);
        }
        let assessment = crate::sync_guard::validate(&plan)?;
        self.sync_guard_policy = plan.guard.clone();
        self.enqueue_sync_between(
            plan.actions,
            &plan.stamp.left_root,
            &plan.stamp.right_root,
            notify,
        );
        Ok(assessment)
    }

    fn enqueue_sync_between(
        &mut self,
        actions: &[crate::sync::SyncAction],
        left_dir: &Path,
        right_dir: &Path,
        notify: impl Fn() + Send + 'static,
    ) {
        use crate::sync::SyncDirection;
        let load = |path: &Path| {
            let meta = std::fs::metadata(path).ok()?;
            FileEntry::from_meta(path.to_path_buf(), &meta)
        };
        let mut to_right = Vec::new();
        let mut to_left = Vec::new();
        for a in actions {
            match a.direction {
                SyncDirection::ToRight => {
                    if let Some(e) = a.source_path().and_then(load) {
                        to_right.push(e);
                    }
                }
                SyncDirection::ToLeft => {
                    if let Some(e) = a.source_path().and_then(load) {
                        to_left.push(e);
                    }
                }
                SyncDirection::Skip => {}
            }
        }
        // Enqueue both passes; the queue runs them in order (the second starts
        // when the first finishes), so a two-way sync needs no special casing.
        let group_id = crate::operation::OperationGroupId::new();
        if !to_right.is_empty() {
            self.enqueue_copy_grouped(
                to_right,
                right_dir.to_path_buf(),
                OverwritePolicy::OverwriteAll,
                Some(group_id.clone()),
            );
        }
        if !to_left.is_empty() {
            self.enqueue_copy_grouped(
                to_left,
                left_dir.to_path_buf(),
                OverwritePolicy::OverwriteAll,
                Some(group_id),
            );
        }
        self.pump_queue(notify);
    }

    /// Queue a background Copy of `entries` into `target` under `policy` without
    /// starting it. Used by the sync sheet and shelf drain, which already
    /// served as the review step (no confirmation dialog). Copies record no
    /// undo history.
    fn enqueue_copy(&mut self, entries: Vec<FileEntry>, target: PathBuf, policy: OverwritePolicy) {
        self.enqueue_copy_grouped(entries, target, policy, None);
    }

    fn enqueue_copy_grouped(
        &mut self,
        entries: Vec<FileEntry>,
        target: PathBuf,
        policy: OverwritePolicy,
        group_id: Option<crate::operation::OperationGroupId>,
    ) {
        if entries.is_empty() {
            return;
        }
        let spec = TransferSpec {
            operation_id: crate::operation::OperationId::new(),
            group_id,
            kind: TransferKind::Copy,
            entries,
            expectations: Vec::new(),
            target,
            policy,
            method: CopyMethod::Native,
            durability: self.durability_profile,
            version_retention: self.version_retention,
            name_policy: self.name_policy,
            symlink_policy: self.symlink_policy,
            preflight_bytes: None,
            post_success: None,
            rollback_cleanup: None,
            rollback_cleanup_identity: None,
            #[cfg(test)]
            mount_wait_override: None,
            #[cfg(test)]
            before_commit: None,
            #[cfg(test)]
            before_post_success: None,
            #[cfg(test)]
            before_terminal_publish: None,
            #[cfg(test)]
            journal_enabled: false,
        };
        self.enqueue_only(spec, None);
    }

    /// Queue a copy and start it if idle (single-copy callers, e.g. shelf drain).
    fn start_copy(
        &mut self,
        entries: Vec<FileEntry>,
        target: PathBuf,
        policy: OverwritePolicy,
        notify: impl Fn() + Send + 'static,
    ) {
        self.enqueue_copy(entries, target, policy);
        self.pump_queue(notify);
    }

    // ── Preview ─────────────────────────────────────────────────────────

    /// Keep the preview (shown in the opposite panel) in sync with the
    /// cursor. The preview is cached by path: while the cursor stays on
    /// the same file nothing touches the filesystem.
    pub fn sync_preview(&mut self) {
        let (source, target) = if self.right.preview.is_some() {
            (&self.left, &mut self.right)
        } else if self.left.preview.is_some() {
            (&self.right, &mut self.left)
        } else {
            return;
        };

        let Some(entry) = source.cursor_entry() else {
            return;
        };

        let shown_matches = match &target.preview {
            Some(PreviewContent::Image(path)) => {
                entry.is_image() && path.as_path() == entry.path.as_path()
            }
            Some(PreviewContent::Pending(identity))
            | Some(PreviewContent::Text { identity, .. }) => {
                !entry.is_image() && identity.matches_entry(entry)
            }
            // The Get-Info card is a deliberate snapshot; don't auto-follow it.
            Some(PreviewContent::Info(_)) => return,
            None => false,
        };
        if shown_matches {
            return;
        }
        target.preview = panel::make_preview(entry);
    }

    /// Toggle the Get-Info inspector for the cursor entry, shown in the
    /// opposite panel (like preview).
    pub fn toggle_info(&mut self) {
        if matches!(self.inactive_panel().preview, Some(PreviewContent::Info(_))) {
            self.inactive_panel_mut().preview = None;
            return;
        }
        let panel = self.active_panel_ref();
        let Some(entry) = panel.cursor_entry().cloned() else {
            return;
        };
        let sizes = panel.size_snapshot();
        let dir_size = if entry.is_dir {
            sizes.size_of(&entry.path)
        } else {
            Some(entry.size)
        };
        let children = if entry.is_dir {
            sizes.count_of(&entry.path)
        } else {
            None
        };
        let card = panel::make_info(&entry, dir_size, children);
        self.inactive_panel_mut().preview = Some(PreviewContent::Info(card));
    }

    // ── Drag and drop ───────────────────────────────────────────────────

    /// Handle a completed drag (mouse released). The dragged entries are
    /// routed through the same Move engine as F6 instead of a raw rename, so
    /// conflicts are confirmed, cross-volume moves work, self/descendant drops
    /// are rejected and errors surface. A clean, conflict-free drop runs
    /// immediately; a conflicting one opens the confirmation dialog.
    pub fn drop_dragged(&mut self, notify: impl Fn() + Send + 'static) {
        self.drop_dragged_as(TransferKind::Move, notify);
    }

    /// Complete a drag using its announced effect. Option-drag copies; the
    /// default and keyboard equivalent move. Both share identical preflight.
    pub fn drop_dragged_as(&mut self, kind: TransferKind, notify: impl Fn() + Send + 'static) {
        // Ignore drops while a transfer or another dialog is in flight, so we
        // never stack a second operation over the first.
        if self.has_unfinished_transfer_work()
            || self.pending_op.is_some()
            || self.mutation_commits_blocked()
        {
            self.cancel_drag();
            return;
        }
        let Some((paths, target)) = self.take_drop_plan() else {
            return;
        };
        let entries: Vec<FileEntry> = paths
            .iter()
            .filter_map(|p| {
                let meta = std::fs::metadata(p).ok()?;
                FileEntry::from_meta(p.clone(), &meta)
            })
            .collect();
        if entries.is_empty() {
            return;
        }
        let conflicts = scan::find_conflicts(&entries, &target);
        let flat = scan::pending_flat_list();
        let policy = match self.name_policy.collision {
            crate::filesystem_policy::CollisionPolicy::Ask => OverwritePolicy::Ask,
            crate::filesystem_policy::CollisionPolicy::KeepBoth => OverwritePolicy::KeepBoth,
            crate::filesystem_policy::CollisionPolicy::Skip => OverwritePolicy::SkipAll,
        };
        let has_conflicts = !conflicts.is_empty() && policy == OverwritePolicy::Ask;
        let generation = self.start_space_probe(
            entries.clone(),
            target.clone(),
            flat.clone(),
            self.symlink_policy,
            notify,
        );
        let filesystem = filesystem_preflight(&entries, &target, self.name_policy);
        self.pending_op = Some(PendingOp::Transfer(PendingTransfer {
            kind,
            entries,
            expectations: Vec::new(),
            target,
            conflicts,
            policy,
            method: CopyMethod::Native,
            durability: self.durability_profile,
            version_retention: self.version_retention,
            name_policy: self.name_policy,
            symlink_policy: self.symlink_policy,
            filesystem,
            flat,
            space: TransferSpaceState::Pending { generation },
            start_when_ready: !has_conflicts,
        }));
    }

    /// Keyboard equivalent of dropping the active selection onto the folder
    /// under the cursor. The synthesized drag plan deliberately enters the
    /// normal drop pipeline, preserving every safety check and confirmation.
    pub fn transfer_selection_into_cursor_folder(
        &mut self,
        kind: TransferKind,
        notify: impl Fn() + Send + 'static,
    ) {
        if self.has_unfinished_transfer_work()
            || self.pending_op.is_some()
            || self.mutation_commits_blocked()
        {
            return;
        }
        let Some((paths, target)) = self.keyboard_drop_plan() else {
            return;
        };
        let panel = self.active_panel();
        panel.drag_entries = paths;
        panel.drop_target = Some(target);
        self.drop_dragged_as(kind, notify);
    }

    fn keyboard_drop_plan(&self) -> Option<(Vec<PathBuf>, PathBuf)> {
        let panel = self.active_panel_ref();
        let target = panel.cursor_entry()?;
        if !target.is_dir || panel.selection_is_empty() {
            return None;
        }
        let paths = panel
            .selected_entries()
            .into_iter()
            .map(|entry| entry.path)
            .filter(|path| path != &target.path)
            .collect::<Vec<_>>();
        (!paths.is_empty()).then(|| (paths, target.path.clone()))
    }

    /// Resolve which panel is the drag source and where the drop lands,
    /// consuming the drag/drop state. A target hovered in the source panel
    /// itself (drag onto its own subdirectory) takes priority over the other
    /// panel. With no explicit target the drag is consumed as a cancellation.
    fn take_drop_plan(&mut self) -> Option<(Vec<PathBuf>, PathBuf)> {
        let (source, other) = if !self.left.drag_entries.is_empty() {
            (&mut self.left, &mut self.right)
        } else if !self.right.drag_entries.is_empty() {
            (&mut self.right, &mut self.left)
        } else {
            return None;
        };
        let target = source
            .drop_target
            .take()
            .or_else(|| other.drop_target.take());
        let paths = std::mem::take(&mut source.drag_entries);
        source.drop_target = None;
        other.drop_target = None;
        if paths.is_empty() {
            return None;
        }
        Some((paths, target?))
    }

    pub(crate) fn cancel_drag(&mut self) {
        self.left.drag_entries.clear();
        self.right.drag_entries.clear();
        self.left.drop_target = None;
        self.right.drop_target = None;
    }
}

#[cfg(test)]
struct TestTrashPort;

#[cfg(test)]
impl crate::ports::TrashPort for TestTrashPort {
    fn move_to_trash(&self, _target: &crate::ports::TrashTarget) -> crate::ports::TrashItemOutcome {
        crate::ports::TrashItemOutcome::Trashed
    }
}

#[cfg(test)]
struct TestFreeSpacePort;

#[cfg(test)]
impl crate::ports::FreeSpacePort for TestFreeSpacePort {
    fn probe(&self, _path: &Path) -> crate::ports::SpaceProbeOutcome {
        crate::ports::SpaceProbeOutcome::Known {
            bytes: u64::MAX,
            precision: crate::ports::SpacePrecision::Exact,
        }
    }

    fn volume_relation(&self, _source: &Path, _target: &Path) -> crate::ports::VolumeRelation {
        crate::ports::VolumeRelation::Same
    }
}

#[cfg(test)]
mod tests;
