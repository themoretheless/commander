//! Typed requests crossing from the UI-independent workspace into the app
//! shell. The queue deliberately contains no egui types: it records intent,
//! while the app decides how that intent is presented in a frame.

use std::collections::VecDeque;
use std::path::PathBuf;

use crate::clipboard::PathStyle;
use crate::transfer::TransferKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum UiModal {
    Recovery,
    History,
    Rename,
    BatchRename,
    Sync,
    Duplicates,
    Diff,
    Treemap,
    Find,
    Archive,
    SavedSearch,
    Collections,
    Mask,
    Path,
    Recent,
    RunCommand,
    Palette,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UiRequest {
    Rename(PathBuf),
    SelectMask,
    RunCommand,
    GatherIntoFolder,
    TransferIntoCursorFolder(TransferKind),
    GoToPath,
    Recent,
    Undo,
    Palette,
    BatchRename,
    Sync,
    FindDuplicates,
    DiffFiles,
    DiskTreemap,
    Find,
    Archive(PathBuf),
    SavedSearch,
    ProjectCollections,
    ToggleQueuePanel,
    OperationHistory,
    OpenRecoveryCenter,
    ReviewRecovery(crate::operation::OperationId),
    OpenExternal(crate::ports::OpenRequest),
    CopyPaths(PathStyle),
    CopyText { text: String, label: String },
    Redo,
    DrainShelf,
}

impl UiRequest {
    pub(crate) const fn modal(&self) -> Option<UiModal> {
        match self {
            Self::ReviewRecovery(_) => Some(UiModal::Recovery),
            Self::Undo | Self::Redo => Some(UiModal::History),
            Self::Rename(_) => Some(UiModal::Rename),
            Self::BatchRename => Some(UiModal::BatchRename),
            Self::Sync => Some(UiModal::Sync),
            Self::FindDuplicates => Some(UiModal::Duplicates),
            Self::DiffFiles => Some(UiModal::Diff),
            Self::DiskTreemap => Some(UiModal::Treemap),
            Self::Find => Some(UiModal::Find),
            Self::Archive(_) => Some(UiModal::Archive),
            Self::SavedSearch => Some(UiModal::SavedSearch),
            Self::ProjectCollections => Some(UiModal::Collections),
            Self::SelectMask => Some(UiModal::Mask),
            Self::GoToPath => Some(UiModal::Path),
            Self::Recent => Some(UiModal::Recent),
            Self::RunCommand => Some(UiModal::RunCommand),
            Self::Palette => Some(UiModal::Palette),
            Self::GatherIntoFolder
            | Self::TransferIntoCursorFolder(_)
            | Self::ToggleQueuePanel
            | Self::OperationHistory
            | Self::OpenRecoveryCenter
            | Self::OpenExternal(_)
            | Self::CopyPaths(_)
            | Self::CopyText { .. }
            | Self::DrainShelf => None,
        }
    }

    pub(crate) const fn coalesces_when_open(&self) -> bool {
        matches!(
            self,
            Self::SelectMask
                | Self::RunCommand
                | Self::GoToPath
                | Self::Recent
                | Self::Palette
                | Self::BatchRename
                | Self::Sync
                | Self::FindDuplicates
                | Self::DiffFiles
                | Self::DiskTreemap
                | Self::Find
                | Self::SavedSearch
                | Self::ProjectCollections
        )
    }
}

#[derive(Default)]
pub(crate) struct UiRequestQueue {
    pending: VecDeque<UiRequest>,
}

impl UiRequestQueue {
    pub(crate) fn emit(&mut self, request: UiRequest) {
        self.pending.push_back(request);
    }

    /// Drain only the requests that existed at the frame boundary. Requests
    /// emitted while this snapshot is being dispatched remain queued.
    pub(crate) fn drain_snapshot(&mut self) -> Vec<UiRequest> {
        self.pending.drain(..).collect()
    }

    pub(crate) fn prepend_deferred(&mut self, requests: Vec<UiRequest>) {
        for request in requests.into_iter().rev() {
            self.pending.push_front(request);
        }
    }

    #[cfg(test)]
    pub(crate) fn has_modal(&self, modal: UiModal) -> bool {
        self.pending
            .iter()
            .any(|request| request.modal() == Some(modal))
    }

    pub(crate) fn has_any_modal(&self) -> bool {
        self.pending.iter().any(|request| request.modal().is_some())
    }

    pub(crate) fn first_pending_modal(&self) -> Option<UiModal> {
        self.pending.iter().find_map(UiRequest::modal)
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Vec<UiRequest> {
        self.pending.iter().cloned().collect()
    }
}

pub(crate) trait UiRequestSink {
    fn any_modal_open(&self) -> bool;
    fn is_modal_open(&self, modal: UiModal) -> bool;
    fn can_transition_from_open_modal(&self, _request: &UiRequest) -> bool {
        false
    }
    fn apply(&mut self, request: UiRequest);
}

/// Apply one drained frame snapshot and return requests that must retain FIFO
/// ownership until the currently open modal closes.
pub(crate) fn dispatch_snapshot(
    requests: Vec<UiRequest>,
    sink: &mut impl UiRequestSink,
) -> Vec<UiRequest> {
    let mut deferred = Vec::new();
    for request in requests {
        if let Some(modal) = request.modal() {
            if sink.is_modal_open(modal) && request.coalesces_when_open() {
                continue;
            }
            if sink.any_modal_open() && !sink.can_transition_from_open_modal(&request) {
                deferred.push(request);
                continue;
            }
        }
        sink.apply(request);
    }
    deferred
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeSink {
        open_modal: Option<UiModal>,
        applied: Vec<UiRequest>,
        emitted: Vec<UiRequest>,
        emit_after: Option<(UiRequest, UiRequest)>,
        toggles: usize,
        safe_state_operation: Option<crate::operation::OperationId>,
        retained_error_transfer: Option<crate::operation::OperationId>,
    }

    impl UiRequestSink for FakeSink {
        fn any_modal_open(&self) -> bool {
            self.open_modal.is_some()
                || self.safe_state_operation.is_some()
                || self.retained_error_transfer.is_some()
        }

        fn is_modal_open(&self, modal: UiModal) -> bool {
            self.open_modal == Some(modal)
        }

        fn can_transition_from_open_modal(&self, request: &UiRequest) -> bool {
            let UiRequest::ReviewRecovery(requested) = request else {
                return false;
            };
            self.open_modal.is_none()
                && self.safe_state_operation.as_ref() == Some(requested)
                && self
                    .retained_error_transfer
                    .as_ref()
                    .is_none_or(|active| active == requested)
        }

        fn apply(&mut self, request: UiRequest) {
            if request == UiRequest::ToggleQueuePanel {
                self.toggles += 1;
            }
            if let Some((trigger, emitted)) = &self.emit_after
                && &request == trigger
            {
                self.emitted.push(emitted.clone());
            }
            if let Some(modal) = request.modal() {
                self.open_modal = Some(modal);
            }
            self.applied.push(request);
        }
    }

    fn dispatch_frame(queue: &mut UiRequestQueue, sink: &mut FakeSink) {
        let deferred = dispatch_snapshot(queue.drain_snapshot(), sink);
        for request in std::mem::take(&mut sink.emitted) {
            queue.emit(request);
        }
        queue.prepend_deferred(deferred);
    }

    #[test]
    fn fifo_payload_order_and_exactly_once_drain() {
        let first = PathBuf::from("/tmp/first.zip");
        let second = PathBuf::from("/tmp/second.txt");
        let mut queue = UiRequestQueue::default();
        queue.emit(UiRequest::Archive(first.clone()));
        queue.emit(UiRequest::Rename(second.clone()));
        queue.emit(UiRequest::CopyText {
            text: "body".into(),
            label: "listing".into(),
        });

        assert_eq!(
            queue.drain_snapshot(),
            vec![
                UiRequest::Archive(first),
                UiRequest::Rename(second),
                UiRequest::CopyText {
                    text: "body".into(),
                    label: "listing".into(),
                },
            ]
        );
        assert!(queue.drain_snapshot().is_empty());
    }

    #[test]
    fn snapshot_drain_leaves_requests_emitted_during_dispatch_for_next_frame() {
        let mut queue = UiRequestQueue::default();
        queue.emit(UiRequest::Palette);

        let frame = queue.drain_snapshot();
        queue.emit(UiRequest::Recent);

        assert_eq!(frame, vec![UiRequest::Palette]);
        assert_eq!(queue.snapshot(), vec![UiRequest::Recent]);
    }

    #[test]
    fn deferred_snapshot_items_stay_ahead_of_new_dispatch_emissions() {
        let mut queue = UiRequestQueue::default();
        queue.emit(UiRequest::Palette);
        queue.emit(UiRequest::Recent);

        let frame = queue.drain_snapshot();
        queue.emit(UiRequest::CopyPaths(PathStyle::NameOnly));
        queue.prepend_deferred(vec![frame[1].clone()]);

        assert_eq!(
            queue.snapshot(),
            vec![UiRequest::Recent, UiRequest::CopyPaths(PathStyle::NameOnly),]
        );
    }

    #[test]
    fn modal_inspection_distinguishes_surfaces_and_effects() {
        let mut queue = UiRequestQueue::default();
        queue.emit(UiRequest::CopyPaths(PathStyle::FullPath));
        queue.emit(UiRequest::BatchRename);

        assert!(queue.has_any_modal());
        assert!(queue.has_modal(UiModal::BatchRename));
        assert!(!queue.has_modal(UiModal::Palette));

        queue.drain_snapshot();
        assert!(!queue.has_any_modal());
    }

    #[test]
    fn first_pending_modal_uses_fifo_instead_of_modal_registry_priority() {
        let mut queue = UiRequestQueue::default();
        queue.emit(UiRequest::CopyPaths(PathStyle::FullPath));
        queue.emit(UiRequest::Recent);
        queue.emit(UiRequest::Palette);

        assert_eq!(queue.first_pending_modal(), Some(UiModal::Recent));
    }

    #[test]
    fn duplicate_requests_are_retained_for_the_dispatcher_policy() {
        let mut queue = UiRequestQueue::default();
        queue.emit(UiRequest::ToggleQueuePanel);
        queue.emit(UiRequest::ToggleQueuePanel);
        queue.emit(UiRequest::Palette);
        queue.emit(UiRequest::Palette);

        assert_eq!(
            queue.drain_snapshot(),
            vec![
                UiRequest::ToggleQueuePanel,
                UiRequest::ToggleQueuePanel,
                UiRequest::Palette,
                UiRequest::Palette,
            ]
        );
    }

    #[test]
    fn dispatcher_defers_then_opens_modals_across_frames() {
        let mut queue = UiRequestQueue::default();
        let mut sink = FakeSink::default();
        queue.emit(UiRequest::Recent);
        queue.emit(UiRequest::Palette);

        dispatch_frame(&mut queue, &mut sink);
        assert_eq!(sink.applied, vec![UiRequest::Recent]);
        assert_eq!(queue.snapshot(), vec![UiRequest::Palette]);

        sink.open_modal = None;
        dispatch_frame(&mut queue, &mut sink);
        assert_eq!(sink.applied, vec![UiRequest::Recent, UiRequest::Palette]);
    }

    #[test]
    fn dispatcher_applies_both_toggle_requests() {
        let mut queue = UiRequestQueue::default();
        let mut sink = FakeSink::default();
        queue.emit(UiRequest::ToggleQueuePanel);
        queue.emit(UiRequest::ToggleQueuePanel);

        dispatch_frame(&mut queue, &mut sink);

        assert_eq!(sink.toggles, 2);
        assert!(queue.snapshot().is_empty());
    }

    #[test]
    fn dispatcher_serializes_undo_then_redo() {
        let mut queue = UiRequestQueue::default();
        let mut sink = FakeSink::default();
        queue.emit(UiRequest::Undo);
        queue.emit(UiRequest::Redo);

        dispatch_frame(&mut queue, &mut sink);
        assert_eq!(sink.applied, vec![UiRequest::Undo]);
        assert_eq!(queue.snapshot(), vec![UiRequest::Redo]);

        sink.open_modal = None;
        dispatch_frame(&mut queue, &mut sink);
        assert_eq!(sink.applied, vec![UiRequest::Undo, UiRequest::Redo]);
    }

    #[test]
    fn dispatcher_preserves_all_rename_and_archive_payloads_in_order() {
        let requests = vec![
            UiRequest::Rename(PathBuf::from("/tmp/rename-a")),
            UiRequest::Rename(PathBuf::from("/tmp/rename-b")),
            UiRequest::Archive(PathBuf::from("/tmp/archive-a.zip")),
            UiRequest::Archive(PathBuf::from("/tmp/archive-b.zip")),
        ];
        let mut queue = UiRequestQueue::default();
        let mut sink = FakeSink::default();
        for request in requests.iter().cloned() {
            queue.emit(request);
        }

        for _ in 0..requests.len() {
            dispatch_frame(&mut queue, &mut sink);
            sink.open_modal = None;
        }

        assert_eq!(sink.applied, requests);
        assert!(queue.snapshot().is_empty());
    }

    #[test]
    fn emissions_during_dispatch_stay_behind_deferred_requests() {
        let emitted = UiRequest::Archive(PathBuf::from("/tmp/emitted.zip"));
        let mut queue = UiRequestQueue::default();
        let mut sink = FakeSink {
            emit_after: Some((UiRequest::Recent, emitted.clone())),
            ..Default::default()
        };
        queue.emit(UiRequest::Recent);
        queue.emit(UiRequest::Palette);

        dispatch_frame(&mut queue, &mut sink);

        assert_eq!(queue.snapshot(), vec![UiRequest::Palette, emitted]);
    }

    #[test]
    fn recovery_center_and_safe_state_review_have_distinct_transitions() {
        let operation_id = crate::operation::OperationId("safe-op".to_string());
        let mut queue = UiRequestQueue::default();
        let mut sink = FakeSink {
            safe_state_operation: Some(operation_id.clone()),
            retained_error_transfer: Some(operation_id.clone()),
            ..Default::default()
        };
        queue.emit(UiRequest::OpenRecoveryCenter);
        queue.emit(UiRequest::ReviewRecovery(operation_id.clone()));

        dispatch_frame(&mut queue, &mut sink);

        assert_eq!(
            sink.applied,
            vec![
                UiRequest::OpenRecoveryCenter,
                UiRequest::ReviewRecovery(operation_id),
            ]
        );
        assert_eq!(sink.open_modal, Some(UiModal::Recovery));
    }
}
