//! Transfer queue lifecycle and its narrow [`Workspace`] facade.
//!
//! [`TransferQueueController`] is the sole owner of queue state, the active
//! worker record, per-job history intent, and reviewed safe-state identity.
//! Workspace applies the controller's typed outcomes only after the controller
//! borrow has ended.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::{
    ActiveTransferView, TransferTerminalReport, TransferTerminalState, Workspace,
    faithfully_undoable,
};
use crate::operation::{ClassifiedFailure, FailureClass, OperationId, SafeState};
use crate::opqueue::{JobId, JobKind, JobState, Queue};
use crate::transfer::{self, TransferKind, TransferProgress, TransferSpec, TransferState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HistoryDirection {
    Undo,
    Redo,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum HistoryIntent {
    None,
    Record(crate::undo::Action),
    Replay {
        direction: HistoryDirection,
        cleanup_on_failure: Option<PathBuf>,
    },
}

impl HistoryIntent {
    pub(super) fn replay(direction: HistoryDirection, cleanup_on_failure: Option<PathBuf>) -> Self {
        Self::Replay {
            direction,
            cleanup_on_failure,
        }
    }
}

/// One transfer waiting in (or running from) the queue.
#[derive(Clone)]
pub(super) struct QueuedJob {
    attempt_id: crate::operation::TransferAttemptId,
    spec: TransferSpec,
    history: HistoryIntent,
    submitted: crate::operation_view::SubmittedSummary,
}

#[cfg(test)]
impl QueuedJob {
    pub(super) fn group_id(&self) -> Option<crate::operation::OperationGroupId> {
        self.spec.group_id.clone()
    }
}

struct ActiveTransfer {
    job_id: JobId,
    attempt_id: crate::operation::TransferAttemptId,
    operation_id: OperationId,
    submitted: crate::operation_view::SubmittedSummary,
    progress: TransferState,
    history: HistoryIntent,
    identity_failure: Option<ClassifiedFailure>,
}

/// One row for the queue panel: enough to label and act on a job without
/// exposing queue or transfer internals to the UI layer.
pub struct QueueRow {
    pub id: JobId,
    pub label: String,
    pub summary: crate::operation_view::SubmittedSummary,
    pub state: JobState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LaunchOutcome {
    Started {
        job_id: JobId,
        operation_id: OperationId,
    },
    AlreadyActive,
    BlockedBySafeState,
    NoRunnableJob,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum HistoryOutcome {
    None,
    Record {
        action: crate::undo::Action,
        placements: Vec<(PathBuf, PathBuf)>,
    },
    Commit(HistoryDirection),
    AbortReplay {
        cleanup_on_failure: Option<PathBuf>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RetirementOutcome {
    pub report: TransferTerminalReport,
    pub history: HistoryOutcome,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct PollOutcome {
    pub safe_state: Option<SafeState>,
    pub retirement: Option<RetirementOutcome>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DismissRejection {
    NoActive,
    JobMismatch,
    NotFinished,
    NotRetainedError,
    ReviewRequired(SafeState),
    InvariantViolation,
}

struct PreparedLaunch {
    outcome: LaunchOutcome,
    spec: TransferSpec,
    progress: TransferState,
}

struct ProgressSnapshot {
    operation_id: Option<OperationId>,
    finished: bool,
    cancelled: bool,
    stopped: bool,
    errors: Vec<String>,
    failures: Vec<ClassifiedFailure>,
    placements: Vec<(PathBuf, PathBuf)>,
}

impl ProgressSnapshot {
    fn clean(&self) -> bool {
        self.finished && self.errors.is_empty() && !self.cancelled && !self.stopped
    }

    fn closes_automatically(&self) -> bool {
        self.finished && (self.cancelled || self.stopped || self.errors.is_empty())
    }
}

/// Atomic owner of transfer queue lifecycle.
pub(super) struct TransferQueueController {
    queue: Queue<QueuedJob>,
    active: Option<ActiveTransfer>,
    reviewed_safe_operation: Option<OperationId>,
}

impl Default for TransferQueueController {
    fn default() -> Self {
        Self {
            queue: Queue::new(),
            active: None,
            reviewed_safe_operation: None,
        }
    }
}

impl TransferQueueController {
    pub(super) fn enqueue(&mut self, spec: TransferSpec, history: HistoryIntent) -> JobId {
        let (kind, verb) = match spec.kind {
            TransferKind::Copy => (JobKind::Copy, "Copy"),
            TransferKind::Move => (JobKind::Move, "Move"),
        };
        let paths = spec
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        let submitted =
            crate::operation_view::SubmittedSummary::capture(verb, &paths, spec.target.clone());
        self.queue.enqueue(
            kind,
            QueuedJob {
                attempt_id: crate::operation::TransferAttemptId::new(),
                spec,
                history,
                submitted,
            },
        )
    }

    fn prepare_launch(&mut self, blocked: bool) -> Result<PreparedLaunch, LaunchOutcome> {
        if self.active.is_some() {
            return Err(LaunchOutcome::AlreadyActive);
        }
        if blocked {
            return Err(LaunchOutcome::BlockedBySafeState);
        }
        let Some(job_id) = self.queue.dequeue_next() else {
            return Err(LaunchOutcome::NoRunnableJob);
        };
        let job = self
            .queue
            .get(job_id)
            .expect("a dequeued transfer must remain in the queue")
            .spec
            .clone();
        let operation_id = job.spec.operation_id.clone();
        let mut initial_progress = TransferProgress::unknown(job.spec.entries.len());
        // Establish the canonical identity before the worker can publish its
        // first update. None is never a valid active-operation identity.
        initial_progress.operation_id = Some(operation_id.clone());
        initial_progress.submitted = Some(job.submitted.clone());
        let progress = Arc::new(Mutex::new(initial_progress));
        self.reviewed_safe_operation = None;
        self.active = Some(ActiveTransfer {
            job_id,
            attempt_id: job.attempt_id,
            operation_id: operation_id.clone(),
            submitted: job.submitted,
            progress: Arc::clone(&progress),
            history: job.history,
            identity_failure: None,
        });
        Ok(PreparedLaunch {
            outcome: LaunchOutcome::Started {
                job_id,
                operation_id,
            },
            spec: job.spec,
            progress,
        })
    }

    pub(super) fn launch(
        &mut self,
        blocked: bool,
        notify: impl Fn() + Send + 'static,
    ) -> LaunchOutcome {
        let prepared = match self.prepare_launch(blocked) {
            Ok(prepared) => prepared,
            Err(outcome) => return outcome,
        };
        let outcome = prepared.outcome.clone();
        transfer::spawn_transfer(prepared.spec, prepared.progress, notify);
        outcome
    }

    #[cfg(test)]
    fn launch_without_worker(&mut self, blocked: bool) -> LaunchOutcome {
        match self.prepare_launch(blocked) {
            Ok(prepared) => prepared.outcome,
            Err(outcome) => outcome,
        }
    }

    #[cfg(test)]
    fn launch_with_workload(
        &mut self,
        blocked: bool,
        workload: crate::workload::WorkloadHandle,
        notify: impl Fn() + Send + 'static,
    ) -> LaunchOutcome {
        let prepared = match self.prepare_launch(blocked) {
            Ok(prepared) => prepared,
            Err(outcome) => return outcome,
        };
        let outcome = prepared.outcome.clone();
        transfer::spawn_transfer_with_workload(workload, prepared.spec, prepared.progress, notify);
        outcome
    }

    #[cfg(test)]
    pub(super) fn active_progress(&self) -> Option<&TransferState> {
        self.active.as_ref().map(|active| &active.progress)
    }

    pub(super) fn active_view(&self) -> Option<ActiveTransferView> {
        self.active.as_ref().map(|active| ActiveTransferView {
            attempt_id: active.attempt_id,
            operation_id: active.operation_id.clone(),
            submitted: active.submitted.clone(),
            progress: Arc::clone(&active.progress),
        })
    }

    pub(super) fn active_job_id(&self) -> Option<JobId> {
        self.active.as_ref().map(|active| active.job_id)
    }

    pub(super) fn queued_count(&self) -> usize {
        self.queue
            .jobs()
            .iter()
            .filter(|job| job.state == JobState::Pending)
            .count()
    }

    pub(super) fn unfinished_count(&self) -> usize {
        self.queue.unfinished_count()
    }

    pub(super) fn has_unfinished(&self) -> bool {
        self.active.is_some() || self.unfinished_count() > 0
    }

    pub(super) fn request_cancel(&mut self) {
        if let Some(active) = &self.active {
            transfer::request_cancel(&active.progress);
        }
    }

    pub(super) fn request_stop(&mut self) {
        if let Some(active) = &self.active {
            crate::lock_util::recover(&active.progress).request_stop();
        }
    }

    fn snapshot_active(&mut self) -> Option<ProgressSnapshot> {
        let mut should_cancel = false;
        let snapshot = {
            let active = self.active.as_mut()?;
            let mut progress = crate::lock_util::recover(&active.progress);
            if progress.operation_id.as_ref() != Some(&active.operation_id)
                && active.identity_failure.is_none()
            {
                let observed = progress
                    .operation_id
                    .as_ref()
                    .map_or_else(|| "<missing>".to_string(), |id| id.0.clone());
                let message = format!(
                    "Transfer identity mismatch: expected {}, observed {observed}",
                    active.operation_id.0
                );
                let failure =
                    ClassifiedFailure::message(FailureClass::IntegrityUncertain, None, message);
                progress
                    .errors
                    .push(format!("Operation: {}", failure.message));
                progress.failures.push(failure.clone());
                active.identity_failure = Some(failure);
                should_cancel = true;
            }
            ProgressSnapshot {
                operation_id: progress.operation_id.clone(),
                finished: progress.finished,
                cancelled: progress.cancelled,
                stopped: progress.stopped,
                errors: progress.errors.clone(),
                failures: progress.failures.clone(),
                placements: if progress.finished {
                    progress.placements.clone()
                } else {
                    Vec::new()
                },
            }
        };
        if should_cancel && let Some(active) = &self.active {
            transfer::request_cancel(&active.progress);
        }
        Some(snapshot)
    }

    fn safe_state_for(
        operation_id: OperationId,
        failures: Vec<ClassifiedFailure>,
    ) -> Option<SafeState> {
        let reason = failures.first()?.message.clone();
        let mut paths = failures
            .iter()
            .filter_map(|failure| failure.path.clone())
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        Some(SafeState {
            operation_id,
            reason,
            paths,
            failures,
        })
    }

    fn cancel_waiting(&mut self) {
        let waiting = self
            .queue
            .jobs()
            .iter()
            .filter(|job| job.state.is_waiting())
            .map(|job| job.id)
            .collect::<Vec<_>>();
        for job_id in waiting {
            self.queue.cancel(job_id);
        }
    }

    fn latch_integrity_failure(&mut self, failure: ClassifiedFailure) {
        let progress_to_cancel = {
            let Some(active) = self.active.as_mut() else {
                return;
            };
            if active.identity_failure.is_none() {
                let mut progress = crate::lock_util::recover(&active.progress);
                progress
                    .errors
                    .push(format!("Operation: {}", failure.message));
                progress.failures.push(failure.clone());
                active.identity_failure = Some(failure);
                Some(Arc::clone(&active.progress))
            } else {
                None
            }
        };
        if let Some(progress) = progress_to_cancel {
            transfer::request_cancel(&progress);
        }
    }

    fn history_outcome(
        history: HistoryIntent,
        clean: bool,
        placements: Vec<(PathBuf, PathBuf)>,
    ) -> HistoryOutcome {
        match (history, clean) {
            (HistoryIntent::None, _) => HistoryOutcome::None,
            (HistoryIntent::Record(action), true) => HistoryOutcome::Record { action, placements },
            (HistoryIntent::Record(_), false) => HistoryOutcome::None,
            (HistoryIntent::Replay { direction, .. }, true) => HistoryOutcome::Commit(direction),
            (
                HistoryIntent::Replay {
                    cleanup_on_failure, ..
                },
                false,
            ) => HistoryOutcome::AbortReplay { cleanup_on_failure },
        }
    }

    fn retire(
        &mut self,
        terminal: TransferTerminalState,
        snapshot: ProgressSnapshot,
    ) -> Result<RetirementOutcome, ClassifiedFailure> {
        let active_ref = self
            .active
            .as_ref()
            .expect("retirement requires an active transfer");
        if !self
            .queue
            .get(active_ref.job_id)
            .is_some_and(|job| job.state == JobState::Running)
        {
            return Err(ClassifiedFailure::message(
                FailureClass::IntegrityUncertain,
                None,
                "Active transfer is not bound to its Running queue row",
            ));
        }
        let active = self
            .active
            .take()
            .expect("retirement requires an active transfer");
        let transitioned = match terminal {
            TransferTerminalState::Done => self.queue.complete(active.job_id),
            TransferTerminalState::Failed => self.queue.fail(active.job_id),
            TransferTerminalState::Cancelled | TransferTerminalState::Stopped => {
                self.queue.cancel(active.job_id)
            }
        };
        if !transitioned {
            self.active = Some(active);
            return Err(ClassifiedFailure::message(
                FailureClass::IntegrityUncertain,
                None,
                "Running queue row rejected its terminal transition",
            ));
        }
        let history = Self::history_outcome(active.history, snapshot.clean(), snapshot.placements);
        let report = TransferTerminalReport {
            attempt_id: active.attempt_id,
            operation_id: active.operation_id,
            submitted: active.submitted,
            terminal,
            errors: snapshot.errors,
            failures: snapshot.failures,
        };
        self.queue.clear_finished();
        Ok(RetirementOutcome { report, history })
    }

    pub(super) fn poll(&mut self) -> PollOutcome {
        let Some(snapshot) = self.snapshot_active() else {
            return PollOutcome::default();
        };
        let active = self
            .active
            .as_ref()
            .expect("snapshot requires an active transfer");
        let operation_id = active.operation_id.clone();

        // Queue-row corruption and progress identity corruption both stop the
        // pipeline. In neither case may a foreign completion retire this job.
        let row_is_running = self
            .queue
            .get(active.job_id)
            .is_some_and(|job| job.state == JobState::Running);
        let identity_failure = active.identity_failure.clone().or_else(|| {
            (!row_is_running).then(|| {
                ClassifiedFailure::message(
                    FailureClass::IntegrityUncertain,
                    None,
                    "Active transfer is not bound to its Running queue row",
                )
            })
        });
        if let Some(failure) = identity_failure {
            self.latch_integrity_failure(failure.clone());
            let safe_state = (snapshot.finished
                && self.reviewed_safe_operation.as_ref() != Some(&operation_id))
            .then(|| Self::safe_state_for(operation_id, vec![failure]))
            .flatten();
            self.cancel_waiting();
            return PollOutcome {
                safe_state,
                retirement: None,
            };
        }

        debug_assert_eq!(snapshot.operation_id.as_ref(), Some(&operation_id));
        let integrity_failures = snapshot
            .failures
            .iter()
            .filter(|failure| failure.class == FailureClass::IntegrityUncertain)
            .cloned()
            .collect::<Vec<_>>();
        if !integrity_failures.is_empty() {
            self.request_cancel();
            self.cancel_waiting();
        }
        let safe_state = if !snapshot.finished
            || integrity_failures.is_empty()
            || self.reviewed_safe_operation.as_ref() == Some(&operation_id)
        {
            None
        } else {
            Self::safe_state_for(operation_id.clone(), integrity_failures)
        };

        if !snapshot.closes_automatically() {
            return PollOutcome {
                safe_state,
                retirement: None,
            };
        }

        let terminal = if snapshot.stopped {
            self.cancel_waiting();
            TransferTerminalState::Stopped
        } else if snapshot.cancelled {
            self.cancel_waiting();
            TransferTerminalState::Cancelled
        } else if snapshot.errors.is_empty() {
            TransferTerminalState::Done
        } else {
            TransferTerminalState::Failed
        };
        let finished = snapshot.finished;
        match self.retire(terminal, snapshot) {
            Ok(retirement) => PollOutcome {
                safe_state,
                retirement: Some(retirement),
            },
            Err(failure) => {
                if let Some(active) = self.active.as_mut() {
                    let mut progress = crate::lock_util::recover(&active.progress);
                    progress
                        .errors
                        .push(format!("Operation: {}", failure.message));
                    progress.failures.push(failure.clone());
                    active.identity_failure = Some(failure.clone());
                }
                self.request_cancel();
                self.cancel_waiting();
                PollOutcome {
                    safe_state: finished
                        .then(|| Self::safe_state_for(operation_id, vec![failure]))
                        .flatten(),
                    retirement: None,
                }
            }
        }
    }

    pub(super) fn dismiss(&mut self, job_id: JobId) -> Result<RetirementOutcome, DismissRejection> {
        let active_job_id = self
            .active
            .as_ref()
            .map(|active| active.job_id)
            .ok_or(DismissRejection::NoActive)?;
        if active_job_id != job_id {
            return Err(DismissRejection::JobMismatch);
        }
        let snapshot = self.snapshot_active().ok_or(DismissRejection::NoActive)?;
        if !snapshot.finished {
            return Err(DismissRejection::NotFinished);
        }
        let active = self
            .active
            .as_ref()
            .expect("snapshot preserves the active transfer");
        let operation_id = active.operation_id.clone();
        let review_failures = snapshot
            .failures
            .iter()
            .filter(|failure| failure.class == FailureClass::IntegrityUncertain)
            .cloned()
            .collect::<Vec<_>>();
        let has_review_failures = !review_failures.is_empty();
        if has_review_failures && self.reviewed_safe_operation.as_ref() != Some(&operation_id) {
            self.cancel_waiting();
            let safe_state = Self::safe_state_for(operation_id, review_failures)
                .expect("an integrity failure always creates safe state");
            return Err(DismissRejection::ReviewRequired(safe_state));
        }
        if active.identity_failure.is_none()
            && !has_review_failures
            && (snapshot.errors.is_empty() || snapshot.cancelled || snapshot.stopped)
        {
            return Err(DismissRejection::NotRetainedError);
        }
        self.retire(TransferTerminalState::Failed, snapshot)
            .map_err(|_| DismissRejection::InvariantViolation)
    }

    pub(super) fn acknowledge_safe_state(&mut self, operation_id: OperationId) {
        self.reviewed_safe_operation = Some(operation_id);
    }

    pub(super) fn cancel_pending(&mut self) {
        self.cancel_waiting();
        self.queue.clear_finished();
    }

    pub(super) fn snapshot(&self) -> Vec<QueueRow> {
        self.queue
            .jobs()
            .iter()
            .map(|job| {
                let summary = job.spec.submitted.clone();
                QueueRow {
                    id: job.id,
                    label: summary.label(),
                    summary,
                    state: job.state,
                }
            })
            .collect()
    }

    pub(super) fn pause(&mut self, job_id: JobId) {
        if self
            .queue
            .get(job_id)
            .is_some_and(|job| job.state == JobState::Pending)
        {
            self.queue.pause(job_id);
        }
    }

    pub(super) fn resume(&mut self, job_id: JobId) -> bool {
        self.queue.resume(job_id)
    }

    pub(super) fn promote(&mut self, job_id: JobId) {
        self.queue.promote(job_id);
    }

    pub(super) fn move_job(&mut self, job_id: JobId, offset: i32) {
        let Some(from) = self.queue.jobs().iter().position(|job| job.id == job_id) else {
            return;
        };
        let last = self.queue.jobs().len() as i32 - 1;
        let to = (from as i32 + offset).clamp(0, last.max(0)) as usize;
        self.queue.reorder(job_id, to);
    }

    pub(super) fn cancel(&mut self, job_id: JobId) {
        if self.active_job_id() == Some(job_id) {
            self.request_cancel();
        } else {
            self.queue.cancel(job_id);
            self.queue.clear_finished();
        }
    }

    pub(super) fn clear_finished(&mut self) {
        self.queue.clear_finished();
    }

    #[cfg(test)]
    fn group_ids(&self) -> Vec<crate::operation::OperationGroupId> {
        self.queue
            .jobs()
            .iter()
            .filter_map(|job| job.spec.group_id())
            .collect()
    }
}

impl Workspace {
    /// Clear a reviewed integrity stop while remembering its exact operation.
    pub fn acknowledge_safe_state(&mut self) {
        if let Some(state) = self.safe_state.take() {
            self.transfers
                .acknowledge_safe_state(state.operation_id.clone());
        }
    }

    pub(super) fn enqueue_only(&mut self, spec: TransferSpec, undo: Option<crate::undo::Action>) {
        let history = undo.map_or(HistoryIntent::None, HistoryIntent::Record);
        self.transfers.enqueue(spec, history);
    }

    pub(super) fn enqueue_with_history(&mut self, spec: TransferSpec, history: HistoryIntent) {
        self.transfers.enqueue(spec, history);
    }

    pub(super) fn pump_queue(&mut self, notify: impl Fn() + Send + 'static) {
        self.transfers.launch(self.mutations_blocked(), notify);
    }

    #[cfg(test)]
    pub fn active_transfer(&self) -> Option<&TransferState> {
        self.transfers.active_progress()
    }

    pub fn active_transfer_view(&self) -> Option<ActiveTransferView> {
        self.transfers.active_view()
    }

    pub fn queued_count(&self) -> usize {
        self.transfers.queued_count()
    }

    #[cfg(test)]
    pub(crate) fn unfinished_queue_count(&self) -> usize {
        self.transfers.unfinished_count()
    }

    pub(crate) fn has_unfinished_transfer_work(&self) -> bool {
        self.transfers.has_unfinished()
    }

    pub fn cancel_transfer(&mut self) {
        self.transfers.request_cancel();
    }

    pub fn stop_transfer_after_current(&mut self) {
        self.transfers.request_stop();
    }

    fn apply_history_outcome(&mut self, outcome: HistoryOutcome) -> bool {
        match outcome {
            HistoryOutcome::None => false,
            HistoryOutcome::Record { action, placements } => {
                let action = match action {
                    crate::undo::Action::Move { .. } => crate::undo::Action::Move {
                        pairs: faithfully_undoable(placements),
                    },
                    crate::undo::Action::Gather { folder, .. } => crate::undo::Action::Gather {
                        folder,
                        pairs: faithfully_undoable(placements),
                    },
                    action => action,
                };
                if action.item_count() == 0 {
                    false
                } else {
                    self.stack.push(action);
                    true
                }
            }
            HistoryOutcome::Commit(HistoryDirection::Undo) => {
                self.stack.commit_undo();
                false
            }
            HistoryOutcome::Commit(HistoryDirection::Redo) => {
                self.stack.commit_redo();
                false
            }
            HistoryOutcome::AbortReplay { cleanup_on_failure } => {
                if let Some(folder) = cleanup_on_failure {
                    // Only an empty replay-owned container is safe to remove.
                    let _ = std::fs::remove_dir(folder);
                }
                false
            }
        }
    }

    /// Poll the active worker and apply each terminal/history outcome once.
    pub fn poll_transfer(
        &mut self,
        notify: impl Fn() + Send + 'static,
    ) -> super::TransferPollOutcome {
        let outcome = self.transfers.poll();
        if self.safe_state.is_none()
            && let Some(safe_state) = outcome.safe_state
        {
            self.safe_state = Some(safe_state);
        }
        let Some(retirement) = outcome.retirement else {
            return super::TransferPollOutcome::default();
        };
        self.left.refresh();
        self.right.refresh();
        let raised = self.apply_history_outcome(retirement.history);
        // History and safe-state outcomes are committed before another worker
        // may observe the queue.
        self.pump_queue(notify);
        super::TransferPollOutcome {
            undo_recorded: raised,
            terminal: Some(retirement.report),
        }
    }

    pub fn cancel_pending_transfers(&mut self) {
        self.transfers.cancel_pending();
    }

    /// Dismiss only the exact active job when it is a retained terminal error.
    #[cfg(test)]
    pub fn dismiss_transfer(&mut self, notify: impl Fn() + Send + 'static) {
        let _ = self.try_dismiss_transfer(notify);
    }

    pub(crate) fn try_dismiss_transfer(
        &mut self,
        notify: impl Fn() + Send + 'static,
    ) -> Result<super::TransferTerminalReport, DismissRejection> {
        let Some(job_id) = self.transfers.active_job_id() else {
            return Err(DismissRejection::NoActive);
        };
        let retirement = match self.transfers.dismiss(job_id) {
            Ok(retirement) => retirement,
            Err(DismissRejection::ReviewRequired(safe_state)) => {
                if self.safe_state.is_none() {
                    self.safe_state = Some(safe_state.clone());
                }
                return Err(DismissRejection::ReviewRequired(safe_state));
            }
            Err(rejection) => return Err(rejection),
        };
        self.left.refresh();
        self.right.refresh();
        self.apply_history_outcome(retirement.history);
        self.pump_queue(notify);
        Ok(retirement.report)
    }

    pub fn queue_snapshot(&self) -> Vec<super::QueueRow> {
        self.transfers.snapshot()
    }

    pub fn queue_pause(&mut self, id: JobId) {
        self.transfers.pause(id);
    }

    pub fn queue_resume(&mut self, id: JobId, notify: impl Fn() + Send + 'static) {
        if self.transfers.resume(id) {
            self.pump_queue(notify);
        }
    }

    pub fn queue_promote(&mut self, id: JobId) {
        self.transfers.promote(id);
    }

    pub fn queue_move(&mut self, id: JobId, offset: i32) {
        self.transfers.move_job(id, offset);
    }

    pub fn queue_cancel(&mut self, id: JobId) {
        self.transfers.cancel(id);
    }

    pub fn queue_clear_finished(&mut self) {
        self.transfers.clear_finished();
    }

    #[cfg(test)]
    pub(super) fn launch_test_transfer(
        &mut self,
        spec: TransferSpec,
        history: HistoryIntent,
    ) -> TransferState {
        self.transfers.enqueue(spec, history);
        assert!(matches!(
            self.transfers.launch_without_worker(false),
            LaunchOutcome::Started { .. }
        ));
        self.active_transfer()
            .cloned()
            .expect("test transfer should be active")
    }

    #[cfg(test)]
    pub(super) fn transfer_group_ids(&self) -> Vec<crate::operation::OperationGroupId> {
        self.transfers.group_ids()
    }

    #[cfg(test)]
    pub(super) fn pump_queue_with_workload(
        &mut self,
        workload: crate::workload::WorkloadHandle,
        notify: impl Fn() + Send + 'static,
    ) -> LaunchOutcome {
        self.transfers
            .launch_with_workload(self.mutations_blocked(), workload, notify)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::{CopyMethod, OverwritePolicy};

    fn spec(operation_id: &str) -> TransferSpec {
        TransferSpec {
            operation_id: OperationId(operation_id.to_string()),
            group_id: None,
            kind: TransferKind::Move,
            entries: Vec::new(),
            expectations: Vec::new(),
            target: PathBuf::from("/tmp/commander-transfer-controller-test"),
            policy: OverwritePolicy::Ask,
            method: CopyMethod::Native,
            durability: crate::operation::DurabilityProfile::default(),
            version_retention: crate::operation::VersionRetentionPolicy::default(),
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            post_success: None,
            rollback_cleanup: None,
            mount_wait_override: None,
            before_commit: None,
            before_post_success: None,
            before_terminal_publish: None,
            journal_enabled: false,
        }
    }

    fn launch(controller: &mut TransferQueueController) -> (JobId, TransferState) {
        let outcome = controller.launch_without_worker(false);
        let LaunchOutcome::Started { job_id, .. } = outcome else {
            panic!("expected a started transfer, got {outcome:?}");
        };
        let progress = controller
            .active_progress()
            .cloned()
            .expect("active progress");
        (job_id, progress)
    }

    #[test]
    fn active_record_and_running_row_move_together() {
        let mut controller = TransferQueueController::default();
        let job_id = controller.enqueue(spec("active-running"), HistoryIntent::None);

        let (started_id, progress) = launch(&mut controller);

        assert_eq!(started_id, job_id);
        assert_eq!(controller.active_job_id(), Some(job_id));
        assert_eq!(
            crate::lock_util::recover(&progress).operation_id,
            Some(OperationId("active-running".to_string()))
        );
        assert_eq!(controller.queue.running_count(), 1);
        assert_eq!(
            controller.queue.get(job_id).map(|job| job.state),
            Some(JobState::Running)
        );
    }

    #[test]
    fn dismiss_rejects_running_and_clean_finished_workers_without_advancing() {
        let mut controller = TransferQueueController::default();
        let active_id = controller.enqueue(spec("dismiss-active"), HistoryIntent::None);
        let tail_id = controller.enqueue(spec("dismiss-tail"), HistoryIntent::None);
        let (_, progress) = launch(&mut controller);

        assert_eq!(
            controller.dismiss(active_id),
            Err(DismissRejection::NotFinished)
        );
        assert_eq!(controller.active_job_id(), Some(active_id));
        assert_eq!(
            controller.queue.get(tail_id).map(|job| job.state),
            Some(JobState::Pending)
        );

        crate::lock_util::recover(&progress).finished = true;
        assert_eq!(
            controller.dismiss(active_id),
            Err(DismissRejection::NotRetainedError)
        );
        assert_eq!(controller.active_job_id(), Some(active_id));
        assert_eq!(
            controller.queue.get(tail_id).map(|job| job.state),
            Some(JobState::Pending)
        );
    }

    #[test]
    fn retained_error_requires_the_matching_job_before_retirement() {
        let mut controller = TransferQueueController::default();
        let active_id = controller.enqueue(spec("retained"), HistoryIntent::None);
        let wrong_id = controller.enqueue(spec("tail"), HistoryIntent::None);
        let (_, progress) = launch(&mut controller);
        {
            let mut progress = crate::lock_util::recover(&progress);
            progress.finished = true;
            progress.errors.push("failed".to_string());
        }

        assert!(controller.poll().retirement.is_none());
        assert_eq!(
            controller.dismiss(wrong_id),
            Err(DismissRejection::JobMismatch)
        );
        assert_eq!(controller.active_job_id(), Some(active_id));

        let retired = controller.dismiss(active_id).expect("matching dismiss");
        assert_eq!(retired.report.terminal, TransferTerminalState::Failed);
        assert!(controller.active_progress().is_none());
    }

    #[test]
    fn progress_identity_mismatch_fails_closed_under_exact_spec_identity() {
        let mut controller = TransferQueueController::default();
        let active_id = controller.enqueue(spec("expected-operation"), HistoryIntent::None);
        let tail_id = controller.enqueue(spec("tail-operation"), HistoryIntent::None);
        let (_, progress) = launch(&mut controller);
        {
            let mut progress = crate::lock_util::recover(&progress);
            progress.operation_id = Some(OperationId("foreign-operation".to_string()));
        }

        let live = controller.poll();

        assert!(live.retirement.is_none());
        assert!(
            live.safe_state.is_none(),
            "a live worker cannot enter review"
        );
        assert!(crate::lock_util::recover(&progress).cancelled);
        assert_eq!(
            controller.queue.get(tail_id).map(|job| job.state),
            Some(JobState::Cancelled)
        );

        crate::lock_util::recover(&progress).finished = true;
        let terminal = controller.poll();
        assert!(terminal.retirement.is_none());
        let safe_state = terminal.safe_state.expect("terminal identity safe state");
        assert_eq!(safe_state.operation_id.0, "expected-operation");
        assert!(safe_state.reason.contains("foreign-operation"));
        controller.acknowledge_safe_state(safe_state.operation_id);
        assert_eq!(controller.active_job_id(), Some(active_id));
        assert_eq!(
            controller.dismiss(tail_id),
            Err(DismissRejection::JobMismatch)
        );
        assert_eq!(
            controller
                .dismiss(active_id)
                .expect("exact active dismiss")
                .report
                .terminal,
            TransferTerminalState::Failed
        );
    }

    #[test]
    fn integrity_safe_state_cancels_pending_and_paused_tail() {
        let mut controller = TransferQueueController::default();
        controller.enqueue(spec("safe-active"), HistoryIntent::None);
        let pending_id = controller.enqueue(spec("safe-pending"), HistoryIntent::None);
        let paused_id = controller.enqueue(spec("safe-paused"), HistoryIntent::None);
        controller.pause(paused_id);
        let (_, progress) = launch(&mut controller);
        {
            let mut progress = crate::lock_util::recover(&progress);
            progress.errors.push("placement uncertain".to_string());
            progress.failures.push(ClassifiedFailure::message(
                FailureClass::IntegrityUncertain,
                Some(PathBuf::from("/affected")),
                "placement uncertain",
            ));
        }

        let live = controller.poll();

        assert!(live.safe_state.is_none());
        assert!(live.retirement.is_none());
        assert!(crate::lock_util::recover(&progress).cancelled);
        assert_eq!(
            controller.queue.get(pending_id).map(|job| job.state),
            Some(JobState::Cancelled)
        );
        assert_eq!(
            controller.queue.get(paused_id).map(|job| job.state),
            Some(JobState::Cancelled)
        );

        crate::lock_util::recover(&progress).finished = true;
        let terminal = controller.poll();
        assert!(terminal.safe_state.is_some());
        assert!(terminal.retirement.is_some());
        assert!(controller.queue.jobs().is_empty());
    }

    #[test]
    fn clean_retirement_emits_history_exactly_once() {
        let mut controller = TransferQueueController::default();
        let action = crate::undo::Action::Move {
            pairs: vec![(PathBuf::from("/old"), PathBuf::from("/new"))],
        };
        controller.enqueue(spec("once"), HistoryIntent::Record(action.clone()));
        let (_, progress) = launch(&mut controller);
        {
            let mut progress = crate::lock_util::recover(&progress);
            progress.finished = true;
            progress
                .placements
                .push((PathBuf::from("/source"), PathBuf::from("/target")));
        }

        let first = controller.poll().retirement.expect("first retirement");
        assert_eq!(
            first.history,
            HistoryOutcome::Record {
                action,
                placements: vec![(PathBuf::from("/source"), PathBuf::from("/target"))],
            }
        );
        assert!(controller.poll().retirement.is_none());
    }

    #[test]
    fn late_cancel_does_not_demote_a_clean_replay_completion() {
        let mut controller = TransferQueueController::default();
        controller.enqueue(
            spec("late-cancel"),
            HistoryIntent::replay(HistoryDirection::Undo, None),
        );
        let (_, progress) = launch(&mut controller);
        crate::lock_util::recover(&progress).finished = true;

        controller.request_cancel();
        let retirement = controller.poll().retirement.expect("clean retirement");

        assert_eq!(retirement.report.terminal, TransferTerminalState::Done);
        assert_eq!(
            retirement.history,
            HistoryOutcome::Commit(HistoryDirection::Undo)
        );
    }

    #[test]
    fn a_new_attempt_with_the_same_operation_can_raise_safe_state_again() {
        let mut controller = TransferQueueController::default();
        let first_id = controller.enqueue(spec("retry-operation"), HistoryIntent::None);
        let (_, first) = launch(&mut controller);
        let first_attempt = controller
            .active_view()
            .expect("first active view")
            .attempt_id;
        {
            let mut progress = crate::lock_util::recover(&first);
            progress.finished = true;
            progress.errors.push("uncertain".to_string());
            progress.failures.push(ClassifiedFailure::message(
                FailureClass::IntegrityUncertain,
                None,
                "uncertain",
            ));
        }
        assert!(controller.poll().safe_state.is_some());
        controller.acknowledge_safe_state(OperationId("retry-operation".to_string()));
        assert!(controller.poll().safe_state.is_none());
        let first_report = controller
            .dismiss(first_id)
            .expect("dismiss first attempt")
            .report;
        assert_eq!(first_report.attempt_id, first_attempt);

        controller.enqueue(spec("retry-operation"), HistoryIntent::None);
        let (_, second) = launch(&mut controller);
        let retry_attempt = controller
            .active_view()
            .expect("retry active view")
            .attempt_id;
        assert_ne!(first_attempt, retry_attempt);
        {
            let mut progress = crate::lock_util::recover(&second);
            progress.finished = true;
            progress.errors.push("uncertain again".to_string());
            progress.failures.push(ClassifiedFailure::message(
                FailureClass::IntegrityUncertain,
                None,
                "uncertain again",
            ));
        }

        assert!(controller.poll().safe_state.is_some());
    }

    #[test]
    fn replay_commits_on_clean_and_aborts_with_cleanup_on_failure() {
        let mut clean = TransferQueueController::default();
        clean.enqueue(
            spec("clean-replay"),
            HistoryIntent::replay(HistoryDirection::Undo, None),
        );
        let (_, progress) = launch(&mut clean);
        crate::lock_util::recover(&progress).finished = true;
        assert_eq!(
            clean.poll().retirement.expect("clean retirement").history,
            HistoryOutcome::Commit(HistoryDirection::Undo)
        );

        let cleanup = PathBuf::from("/tmp/replay-cleanup");
        let mut failed = TransferQueueController::default();
        let failed_id = failed.enqueue(
            spec("failed-replay"),
            HistoryIntent::replay(HistoryDirection::Redo, Some(cleanup.clone())),
        );
        let (_, progress) = launch(&mut failed);
        {
            let mut progress = crate::lock_util::recover(&progress);
            progress.finished = true;
            progress.errors.push("failed".to_string());
        }
        assert!(failed.poll().retirement.is_none());
        assert_eq!(
            failed
                .dismiss(failed_id)
                .expect("dismiss failed replay")
                .history,
            HistoryOutcome::AbortReplay {
                cleanup_on_failure: Some(cleanup),
            }
        );
    }
}
