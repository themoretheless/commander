//! Transfer queue lifecycle owned by [`Workspace`].
//!
//! Transfer specifications are still created by their feature-specific callers
//! in the parent module. This module owns only queue admission, sequencing,
//! worker retirement, cancellation, and the queue-facing UI snapshot.

use std::sync::{Arc, Mutex};

use super::{Workspace, faithfully_undoable};
use crate::transfer::{self, TransferKind, TransferProgress, TransferSpec};

/// One transfer waiting in (or running from) the queue: the fully-built spec
/// plus the undo action to record if it finishes cleanly (a user Move) or
/// `None` for copies and undo/redo-driven transfers.
pub(super) struct QueuedJob {
    spec: TransferSpec,
    undo: Option<crate::undo::Action>,
    submitted: crate::operation_view::SubmittedSummary,
}

#[cfg(test)]
impl QueuedJob {
    pub(super) fn group_id(&self) -> Option<crate::operation::OperationGroupId> {
        self.spec.group_id.clone()
    }
}

/// One row for the queue panel: enough to label and act on a job without
/// exposing the opqueue/transfer internals to the UI layer.
pub struct QueueRow {
    pub id: crate::opqueue::JobId,
    pub label: String,
    pub summary: crate::operation_view::SubmittedSummary,
    pub state: crate::opqueue::JobState,
}

impl Workspace {
    /// Clear a reviewed integrity stop while remembering its operation so a
    /// still-visible failed transfer cannot immediately reopen the same state.
    pub fn acknowledge_safe_state(&mut self) {
        if let Some(state) = self.safe_state.take() {
            self.reviewed_safe_operation = Some(state.operation_id);
        }
    }

    /// Append a transfer to the queue without starting it.
    pub(super) fn enqueue_only(&mut self, spec: TransferSpec, undo: Option<crate::undo::Action>) {
        let (kind, verb) = match spec.kind {
            TransferKind::Copy => (crate::opqueue::JobKind::Copy, "Copy"),
            TransferKind::Move => (crate::opqueue::JobKind::Move, "Move"),
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
                spec,
                undo,
                submitted,
            },
        );
    }

    /// Start the next queued job if no transfer is active (concurrency cap 1).
    /// The single place that spawns the worker, so the running job, its undo
    /// action and `active_transfer` always move together.
    pub(super) fn pump_queue(&mut self, notify: impl Fn() + Send + 'static) {
        if self.active_transfer.is_some() || self.mutations_blocked() {
            return;
        }
        let Some(id) = self.queue.dequeue_next() else {
            return;
        };
        // Clone the spec/undo out of the (now Running) job to launch it.
        // `job.spec` is the opqueue payload (a QueuedJob); its `.spec` is the
        // TransferSpec and `.undo` the recorded action.
        let Some(job) = self.queue.get(id) else {
            return;
        };
        let spec = job.spec.spec.clone();
        let submitted = job.spec.submitted.clone();
        self.reviewed_safe_operation = None;
        self.pending_undo_action = job.spec.undo.clone();
        self.running_job = Some(id);
        // The worker sizes the entries once and fills in `total_bytes`; passing
        // 0 here keeps a same-volume move from walking the tree twice (once for
        // the denominator, once for the rename's progress).
        let mut initial_progress = TransferProgress::unknown(spec.entries.len());
        initial_progress.submitted = Some(submitted);
        let progress = Arc::new(Mutex::new(initial_progress));
        self.active_transfer = Some(progress.clone());
        transfer::spawn_transfer(spec, progress, notify);
    }

    /// Number of transfers waiting behind the active one (for a queued-count
    /// indicator).
    pub fn queued_count(&self) -> usize {
        self.queue
            .jobs()
            .iter()
            .filter(|j| j.state == crate::opqueue::JobState::Pending)
            .count()
    }

    /// Number of queue jobs that can still run or are currently running.
    pub(crate) fn unfinished_queue_count(&self) -> usize {
        self.queue.unfinished_count()
    }

    /// True while either a live transfer or any non-terminal queue job exists.
    /// Checking both sides keeps guards sound while a worker is being attached
    /// to or retired from its queue row.
    pub(crate) fn has_unfinished_transfer_work(&self) -> bool {
        self.active_transfer.is_some() || self.unfinished_queue_count() > 0
    }

    /// Cancel active transfer.
    pub fn cancel_transfer(&mut self) {
        if let Some(ref state) = self.active_transfer {
            // A poisoned progress mutex (worker thread panicked) must not panic
            // the UI thread in turn; recover the guard and flag cancellation.
            let mut s = crate::lock_util::recover(state);
            // Ignore a cancel that races in after the worker already finished
            // cleanly: flagging it would demote a completed Move to "not clean"
            // in poll_transfer and silently drop its undo entry.
            s.request_cancel();
        }
    }

    /// Finish the current top-level entry, then checkpoint and stop before the
    /// worker accepts another entry from this transfer.
    pub fn stop_transfer_after_current(&mut self) {
        if let Some(state) = &self.active_transfer {
            let mut progress = crate::lock_util::recover(state);
            progress.request_stop();
        }
    }

    /// Auto-close finished transfers. A transfer that finished with errors
    /// stays open so the user can read the error list (dismissed via OK).
    /// Returns `true` when a clean Move just finished, so the UI can raise the
    /// undo toast.
    pub fn poll_transfer(&mut self, notify: impl Fn() + Send + 'static) -> bool {
        let (close, clean, had_errors, cancelled, placements, safe_state) = self
            .active_transfer
            .as_ref()
            .map(|s| {
                // Recover from a poisoned lock rather than panicking the UI.
                let s = crate::lock_util::recover(s);
                // Close only once the worker has set `finished` (it now does so
                // even on cancel, after its cleanup), so we never tear the
                // shared state out from under a still-running cleanup pass. A
                // finished run with errors stays open so the user can read them.
                let clean = s.finished && s.errors.is_empty() && !s.cancelled && !s.stopped;
                let errs = !s.errors.is_empty();
                // Only a clean Move needs its placements (to record undo).
                let placements = if clean {
                    s.placements.clone()
                } else {
                    Vec::new()
                };
                let failures = if s.finished {
                    s.failures
                        .iter()
                        .filter(|failure| {
                            failure.class == crate::operation::FailureClass::IntegrityUncertain
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                let safe_state = if failures.is_empty() {
                    None
                } else {
                    let mut paths = failures
                        .iter()
                        .filter_map(|failure| failure.path.clone())
                        .collect::<Vec<_>>();
                    paths.sort();
                    paths.dedup();
                    Some(crate::operation::SafeState {
                        operation_id: s.operation_id.clone().unwrap_or_else(|| {
                            crate::operation::OperationId("unknown-operation".to_string())
                        }),
                        reason: failures[0].message.clone(),
                        paths,
                        failures,
                    })
                };
                (
                    s.finished && (s.cancelled || s.stopped || s.errors.is_empty()),
                    clean,
                    errs,
                    s.cancelled || s.stopped,
                    placements,
                    safe_state,
                )
            })
            .unwrap_or((false, false, false, false, Vec::new(), None));

        if self.safe_state.is_none()
            && let Some(safe_state) = safe_state
            && self.reviewed_safe_operation.as_ref() != Some(&safe_state.operation_id)
        {
            self.safe_state = Some(safe_state);
            self.cancel_waiting_jobs();
        }

        if !close {
            return false;
        }
        self.active_transfer = None;
        // Retire the finished job from the queue so a free slot opens up.
        if let Some(id) = self.running_job.take() {
            if cancelled {
                // Record the truthful terminal state, and stop the rest of the
                // pipeline: a user Cancel means "stop", not "skip to the next
                // queued op" (e.g. the second pass of a two-way sync).
                self.queue.cancel(id);
                self.cancel_waiting_jobs();
            } else if had_errors {
                self.queue.fail(id);
            } else {
                self.queue.complete(id);
            }
            self.queue.clear_finished();
        }
        self.left.refresh();
        self.right.refresh();
        self.finish_history_transition(clean);
        // Record the move on the history stack on a clean run, built from where
        // the files ACTUALLY landed: a KeepBoth conflict renames to "name copy",
        // which is not faithfully reversible, so those entries are dropped (and
        // an all-KeepBoth move raises no undo toast). Read this BEFORE pumping
        // the next job (which overwrites `pending_undo_action`).
        let raised = if clean {
            match self.pending_undo_action.take() {
                Some(crate::undo::Action::Move { .. }) => {
                    let pairs = faithfully_undoable(placements);
                    if pairs.is_empty() {
                        false
                    } else {
                        self.stack.push(crate::undo::Action::Move { pairs });
                        true
                    }
                }
                Some(crate::undo::Action::Gather { folder, .. }) => {
                    let pairs = faithfully_undoable(placements);
                    if pairs.is_empty() {
                        false
                    } else {
                        self.stack
                            .push(crate::undo::Action::Gather { folder, pairs });
                        true
                    }
                }
                Some(action) => {
                    self.stack.push(action);
                    true
                }
                None => false,
            }
        } else {
            self.pending_undo_action = None;
            false
        };
        // Start the next queued transfer, if any.
        self.pump_queue(notify);
        raised
    }

    /// Cancel every not-started job, including work held in `Paused`. Used when
    /// the current pipeline is cancelled or enters safe-state so no resumable
    /// tail survives the stop.
    fn cancel_waiting_jobs(&mut self) {
        let waiting: Vec<crate::opqueue::JobId> = self
            .queue
            .jobs()
            .iter()
            .filter(|job| job.state.is_waiting())
            .map(|j| j.id)
            .collect();
        for id in waiting {
            self.queue.cancel(id);
        }
    }

    /// Cancel all not-started work, whether runnable or paused. The active
    /// worker keeps running, matching the Operations Center action label.
    pub fn cancel_pending_transfers(&mut self) {
        self.cancel_waiting_jobs();
        self.queue.clear_finished();
    }

    /// Dismiss a finished transfer the user is acknowledging via the OK button.
    /// `poll_transfer` deliberately leaves a finished-with-errors transfer open
    /// (so the error list can be read) and does NOT retire its queue job; this
    /// does that retirement and starts the next queued job, so acknowledging an
    /// errored transfer can never wedge the queue (running_job stuck Running,
    /// `runnable()` then forever blocked at the concurrency cap).
    pub fn dismiss_transfer(&mut self, notify: impl Fn() + Send + 'static) {
        self.active_transfer = None;
        if let Some(id) = self.running_job.take() {
            // It is shown via OK only because it finished with errors.
            self.queue.fail(id);
            self.queue.clear_finished();
        }
        // An errored/aborted run records no undo history.
        self.pending_undo_action = None;
        self.finish_history_transition(false);
        self.left.refresh();
        self.right.refresh();
        self.pump_queue(notify);
    }

    /// Snapshot of every job in the transfer queue, in priority order, for
    /// the queue panel to render.
    pub fn queue_snapshot(&self) -> Vec<super::QueueRow> {
        self.queue
            .jobs()
            .iter()
            .map(|j| {
                let summary = j.spec.submitted.clone();
                QueueRow {
                    id: j.id,
                    label: summary.label(),
                    summary,
                    state: j.state,
                }
            })
            .collect()
    }

    /// Hold a `Pending` job back so it waits for an explicit resume.
    pub fn queue_pause(&mut self, id: crate::opqueue::JobId) {
        if self
            .queue
            .get(id)
            .is_some_and(|job| job.state == crate::opqueue::JobState::Pending)
        {
            self.queue.pause(id);
        }
    }

    /// Return a held job to the `Pending` pool and immediately fill an idle
    /// worker slot. The callback lets the newly-started worker repaint the UI.
    pub fn queue_resume(&mut self, id: crate::opqueue::JobId, notify: impl Fn() + Send + 'static) {
        if self.queue.resume(id) {
            self.pump_queue(notify);
        }
    }

    /// Move a `Pending` job to the front so it runs next.
    pub fn queue_promote(&mut self, id: crate::opqueue::JobId) {
        self.queue.promote(id);
    }

    /// Swap a `Pending` job with its immediate neighbour in queue order.
    /// `offset` is `-1` (move up / earlier) or `1` (move down / later).
    pub fn queue_move(&mut self, id: crate::opqueue::JobId, offset: i32) {
        let Some(from) = self.queue.jobs().iter().position(|j| j.id == id) else {
            return;
        };
        let last = self.queue.jobs().len() as i32 - 1;
        let to = (from as i32 + offset).clamp(0, last.max(0)) as usize;
        self.queue.reorder(id, to);
    }

    /// Cancel a queued job. The running job is stopped through the live
    /// transfer (so its worker thread actually stops, same as the transfer
    /// dialog's own Cancel); a pending/paused job is simply dropped from the
    /// queue, since no worker exists for it yet.
    pub fn queue_cancel(&mut self, id: crate::opqueue::JobId) {
        if self.running_job == Some(id) {
            self.cancel_transfer();
        } else {
            self.queue.cancel(id);
            self.queue.clear_finished();
        }
    }

    /// Drop every finished (Done/Failed/Cancelled) job from the queue panel.
    pub fn queue_clear_finished(&mut self) {
        self.queue.clear_finished();
    }
}
