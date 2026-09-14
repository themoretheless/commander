//! Atomic owner of logical undo/redo history.
//!
//! Replay reservations bind a completion to one entry and one timeline
//! revision. Filesystem execution, cleanup, panels, and operation-journal
//! recovery deliberately remain outside this module.
//!
//! Two larger guarantees require a future schema change: actions are still
//! path-based rather than bound to a recorded `PathIdentity`, and history is
//! intentionally empty after restart. Durable replay must persist entry and
//! replay identity alongside the operation journal before either guarantee can
//! be claimed.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{Action, HistoryEntry, HistoryEntryId, RedoInvalidation, UndoStack, invert};
use crate::operation::OperationId;

static NEXT_OWNER_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReplayDirection {
    Undo,
    Redo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReplayToken(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct UndoOwnerId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReplayReservation {
    owner: UndoOwnerId,
    token: ReplayToken,
    direction: ReplayDirection,
    expected_revision: u64,
    entry_id: HistoryEntryId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReplayPlan {
    pub(crate) action: Action,
    pub(crate) reservation: ReplayReservation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HistoryError {
    ReplayAlreadyPending,
    RecordWhileReplayPending,
    NoPendingReplay,
    ReservationMismatch,
    RevisionMismatch,
    EntryMismatch,
    TimelineRejected,
    ExecutionAlreadyBound,
    ExecutionNotBound,
    ReplayNotInterrupted,
    OperationMismatch,
}

impl fmt::Display for HistoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ReplayAlreadyPending => "another history replay is already pending",
            Self::RecordWhileReplayPending => {
                "a new history action cannot be recorded during replay"
            }
            Self::NoPendingReplay => "no history replay is pending",
            Self::ReservationMismatch => "history replay reservation does not match",
            Self::RevisionMismatch => "history changed after replay was reserved",
            Self::EntryMismatch => "history replay no longer targets the expected entry",
            Self::TimelineRejected => "history timeline rejected its expected transition",
            Self::ExecutionAlreadyBound => "history replay is already bound to an execution",
            Self::ExecutionNotBound => "history replay is not bound to an execution",
            Self::ReplayNotInterrupted => "history replay is not awaiting recovery",
            Self::OperationMismatch => "history replay belongs to another operation",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PendingReplayState {
    Reserved,
    Running(OperationId),
    Interrupted(OperationId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingReplay {
    reservation: ReplayReservation,
    state: PendingReplayState,
}

pub(crate) struct UndoCenter {
    owner: UndoOwnerId,
    stack: UndoStack,
    revision: u64,
    pending: Option<PendingReplay>,
    next_entry_id: u64,
    next_replay_token: u64,
}

impl Default for UndoCenter {
    fn default() -> Self {
        let owner = NEXT_OWNER_ID
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .unwrap_or_else(|_| panic!("history owner identity exhausted"));
        Self {
            owner: UndoOwnerId(owner),
            stack: UndoStack::default(),
            revision: 0,
            pending: None,
            next_entry_id: 0,
            next_replay_token: 0,
        }
    }
}

impl UndoCenter {
    pub(crate) fn can_undo(&self) -> bool {
        self.stack.can_undo()
    }

    pub(crate) fn can_redo(&self) -> bool {
        self.stack.can_redo()
    }

    pub(crate) fn top_undo(&self) -> Option<&Action> {
        self.stack.peek_undo()
    }

    pub(crate) fn preview_undo_action(&self) -> Option<Action> {
        self.stack.peek_undo_inverse()
    }

    pub(crate) fn preview_redo_action(&self) -> Option<Action> {
        self.stack.peek_redo_action()
    }

    pub(crate) fn redo_invalidation(&self) -> Option<&RedoInvalidation> {
        self.stack.redo_invalidation()
    }

    pub(crate) fn has_pending_replay(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn pending_reservation(&self) -> Option<ReplayReservation> {
        self.pending.as_ref().map(|pending| pending.reservation)
    }

    pub(crate) fn record(&mut self, action: Action) -> Result<(), HistoryError> {
        if self.pending.is_some() {
            return Err(HistoryError::RecordWhileReplayPending);
        }
        let entry_id = HistoryEntryId(Self::allocate(&mut self.next_entry_id, "history entry"));
        self.stack.push_entry(HistoryEntry {
            id: entry_id,
            action,
        });
        self.bump_revision();
        Ok(())
    }

    pub(crate) fn begin(
        &mut self,
        direction: ReplayDirection,
    ) -> Result<Option<ReplayPlan>, HistoryError> {
        if self.pending.is_some() {
            return Err(HistoryError::ReplayAlreadyPending);
        }
        let (entry_id, action) = match direction {
            ReplayDirection::Undo => {
                let Some(entry) = self.stack.peek_undo_entry() else {
                    return Ok(None);
                };
                let Some(action) = invert(&entry.action) else {
                    return Ok(None);
                };
                (entry.id, action)
            }
            ReplayDirection::Redo => {
                let Some(entry) = self.stack.peek_redo_entry() else {
                    return Ok(None);
                };
                (entry.id, entry.action.clone())
            }
        };
        let reservation = ReplayReservation {
            owner: self.owner,
            token: ReplayToken(Self::allocate(
                &mut self.next_replay_token,
                "history replay",
            )),
            direction,
            expected_revision: self.revision,
            entry_id,
        };
        self.pending = Some(PendingReplay {
            reservation,
            state: PendingReplayState::Reserved,
        });
        Ok(Some(ReplayPlan {
            action,
            reservation,
        }))
    }

    pub(crate) fn commit_immediate(
        &mut self,
        reservation: ReplayReservation,
    ) -> Result<(), HistoryError> {
        self.validate_state(reservation, |state| {
            matches!(state, PendingReplayState::Reserved)
        })
        .map_err(|error| match error {
            HistoryError::ExecutionNotBound => HistoryError::ExecutionAlreadyBound,
            error => error,
        })?;
        self.commit_timeline(reservation)
    }

    pub(crate) fn abort_immediate(
        &mut self,
        reservation: ReplayReservation,
    ) -> Result<(), HistoryError> {
        self.validate_state(reservation, |state| {
            matches!(state, PendingReplayState::Reserved)
        })
        .map_err(|error| match error {
            HistoryError::ExecutionNotBound => HistoryError::ExecutionAlreadyBound,
            error => error,
        })?;
        self.pending = None;
        Ok(())
    }

    pub(crate) fn bind_execution(
        &mut self,
        reservation: ReplayReservation,
        operation_id: &OperationId,
    ) -> Result<(), HistoryError> {
        self.validate(reservation)?;
        let pending = self
            .pending
            .as_mut()
            .expect("validated history replay remains pending");
        match &pending.state {
            PendingReplayState::Reserved => {
                pending.state = PendingReplayState::Running(operation_id.clone());
                Ok(())
            }
            PendingReplayState::Interrupted(expected) if expected == operation_id => {
                pending.state = PendingReplayState::Running(operation_id.clone());
                Ok(())
            }
            PendingReplayState::Interrupted(_) => Err(HistoryError::OperationMismatch),
            PendingReplayState::Running(_) => Err(HistoryError::ExecutionAlreadyBound),
        }
    }

    pub(crate) fn commit_execution(
        &mut self,
        reservation: ReplayReservation,
        operation_id: &OperationId,
    ) -> Result<(), HistoryError> {
        self.validate_running(reservation, operation_id)?;
        self.commit_timeline(reservation)
    }

    pub(crate) fn abort_execution(
        &mut self,
        reservation: ReplayReservation,
        operation_id: &OperationId,
    ) -> Result<(), HistoryError> {
        self.validate_running(reservation, operation_id)?;
        self.pending = None;
        Ok(())
    }

    pub(crate) fn interrupt_execution(
        &mut self,
        reservation: ReplayReservation,
        operation_id: &OperationId,
    ) -> Result<(), HistoryError> {
        self.validate_running(reservation, operation_id)?;
        self.pending
            .as_mut()
            .expect("validated history replay remains pending")
            .state = PendingReplayState::Interrupted(operation_id.clone());
        Ok(())
    }

    pub(crate) fn interrupted_reservation_for(
        &self,
        operation_id: &OperationId,
    ) -> Result<Option<ReplayReservation>, HistoryError> {
        let Some(pending) = &self.pending else {
            return Ok(None);
        };
        self.validate(pending.reservation)?;
        match &pending.state {
            PendingReplayState::Interrupted(expected) if expected == operation_id => {
                Ok(Some(pending.reservation))
            }
            PendingReplayState::Interrupted(_) => Err(HistoryError::OperationMismatch),
            PendingReplayState::Reserved | PendingReplayState::Running(_) => {
                Err(HistoryError::ReplayNotInterrupted)
            }
        }
    }

    pub(crate) fn abort_interrupted(
        &mut self,
        reservation: ReplayReservation,
        operation_id: &OperationId,
    ) -> Result<(), HistoryError> {
        self.validate_state(reservation, |state| {
            matches!(state, PendingReplayState::Interrupted(expected) if expected == operation_id)
        })?;
        self.pending = None;
        Ok(())
    }

    fn commit_timeline(&mut self, reservation: ReplayReservation) -> Result<(), HistoryError> {
        self.validate(reservation)?;
        let committed = match reservation.direction {
            ReplayDirection::Undo => self.stack.commit_undo_entry(reservation.entry_id),
            ReplayDirection::Redo => self.stack.commit_redo_entry(reservation.entry_id),
        };
        if !committed {
            return Err(HistoryError::TimelineRejected);
        }
        self.pending = None;
        self.bump_revision();
        Ok(())
    }

    fn validate_state(
        &self,
        reservation: ReplayReservation,
        expected: impl FnOnce(&PendingReplayState) -> bool,
    ) -> Result<(), HistoryError> {
        self.validate(reservation)?;
        let state = &self
            .pending
            .as_ref()
            .expect("validated history replay remains pending")
            .state;
        if expected(state) {
            Ok(())
        } else {
            Err(HistoryError::ExecutionNotBound)
        }
    }

    fn validate_running(
        &self,
        reservation: ReplayReservation,
        operation_id: &OperationId,
    ) -> Result<(), HistoryError> {
        self.validate(reservation)?;
        match &self
            .pending
            .as_ref()
            .expect("validated history replay remains pending")
            .state
        {
            PendingReplayState::Running(expected) if expected == operation_id => Ok(()),
            PendingReplayState::Running(_) | PendingReplayState::Interrupted(_) => {
                Err(HistoryError::OperationMismatch)
            }
            PendingReplayState::Reserved => Err(HistoryError::ExecutionNotBound),
        }
    }

    fn validate(&self, reservation: ReplayReservation) -> Result<(), HistoryError> {
        let Some(pending) = &self.pending else {
            return Err(HistoryError::NoPendingReplay);
        };
        if pending.reservation != reservation || reservation.owner != self.owner {
            return Err(HistoryError::ReservationMismatch);
        }
        if reservation.expected_revision != self.revision {
            return Err(HistoryError::RevisionMismatch);
        }
        let top_id = match reservation.direction {
            ReplayDirection::Undo => self.stack.peek_undo_entry().map(|entry| entry.id),
            ReplayDirection::Redo => self.stack.peek_redo_entry().map(|entry| entry.id),
        };
        if top_id != Some(reservation.entry_id) {
            return Err(HistoryError::EntryMismatch);
        }
        Ok(())
    }

    fn bump_revision(&mut self) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("history revision exhausted");
    }

    fn allocate(counter: &mut u64, label: &str) -> u64 {
        *counter = counter
            .checked_add(1)
            .unwrap_or_else(|| panic!("{label} identity exhausted"));
        *counter
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn rename(from: &str, to: &str) -> Action {
        Action::Rename {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
        }
    }

    fn seeded() -> UndoCenter {
        let mut center = UndoCenter::default();
        center
            .record(rename("/a/old", "/a/new"))
            .expect("seed history");
        center
    }

    #[test]
    fn prepare_does_not_advance_and_matching_commit_advances_once() {
        let mut center = seeded();
        let plan = center
            .begin(ReplayDirection::Undo)
            .expect("begin")
            .expect("plan");

        assert!(center.can_undo());
        assert!(!center.can_redo());
        assert_eq!(center.top_undo(), Some(&rename("/a/old", "/a/new")));

        center
            .commit_immediate(plan.reservation)
            .expect("matching commit");
        assert!(!center.can_undo());
        assert!(center.can_redo());
        assert_eq!(
            center.commit_immediate(plan.reservation),
            Err(HistoryError::NoPendingReplay)
        );
        assert!(!center.can_undo());
    }

    #[test]
    fn stale_and_wrong_direction_reservations_do_not_advance() {
        let mut center = seeded();
        let first = center
            .begin(ReplayDirection::Undo)
            .expect("begin")
            .expect("plan");
        center
            .abort_immediate(first.reservation)
            .expect("abort first");
        let current = center
            .begin(ReplayDirection::Undo)
            .expect("begin current")
            .expect("current plan");

        assert_eq!(
            center.commit_immediate(first.reservation),
            Err(HistoryError::ReservationMismatch)
        );
        let wrong_direction = ReplayReservation {
            direction: ReplayDirection::Redo,
            ..current.reservation
        };
        assert_eq!(
            center.commit_immediate(wrong_direction),
            Err(HistoryError::ReservationMismatch)
        );
        assert!(center.can_undo());
        assert!(!center.can_redo());
        center
            .abort_immediate(current.reservation)
            .expect("abort current");
    }

    #[test]
    fn record_is_rejected_while_replay_is_pending() {
        let mut center = seeded();
        let plan = center
            .begin(ReplayDirection::Undo)
            .expect("begin")
            .expect("plan");

        assert_eq!(
            center.record(rename("/b/old", "/b/new")),
            Err(HistoryError::RecordWhileReplayPending)
        );
        assert_eq!(center.top_undo(), Some(&rename("/a/old", "/a/new")));
        center.abort_immediate(plan.reservation).expect("abort");
    }

    #[test]
    fn revision_change_rejects_settlement_without_advancing() {
        let mut center = seeded();
        let plan = center
            .begin(ReplayDirection::Undo)
            .expect("begin")
            .expect("plan");
        center.revision += 1;

        assert_eq!(
            center.commit_immediate(plan.reservation),
            Err(HistoryError::RevisionMismatch)
        );
        assert!(center.can_undo());
        assert!(!center.can_redo());
        assert_eq!(center.pending_reservation(), Some(plan.reservation));
    }

    #[test]
    fn abort_preserves_cursor_and_allows_a_later_replay() {
        let mut center = seeded();
        let plan = center
            .begin(ReplayDirection::Undo)
            .expect("begin")
            .expect("plan");
        center.abort_immediate(plan.reservation).expect("abort");

        assert!(center.can_undo());
        assert!(!center.can_redo());
        let retry = center
            .begin(ReplayDirection::Undo)
            .expect("retry")
            .expect("retry plan");
        center
            .commit_immediate(retry.reservation)
            .expect("retry commit");
        assert!(center.can_redo());
    }

    #[test]
    fn stale_async_completion_cannot_advance_a_newer_entry() {
        let mut center = seeded();
        let stale = center
            .begin(ReplayDirection::Undo)
            .expect("begin stale")
            .expect("stale plan");
        center
            .abort_immediate(stale.reservation)
            .expect("abort stale");
        center
            .record(rename("/b/old", "/b/new"))
            .expect("record newer");
        let current = center
            .begin(ReplayDirection::Undo)
            .expect("begin current")
            .expect("current plan");

        assert_eq!(
            center.commit_immediate(stale.reservation),
            Err(HistoryError::ReservationMismatch)
        );
        assert_eq!(center.top_undo(), Some(&rename("/b/old", "/b/new")));
        center
            .abort_immediate(current.reservation)
            .expect("abort current");
    }

    #[test]
    fn reservation_from_another_center_cannot_commit_identical_local_counters() {
        let mut current = seeded();
        let current_plan = current
            .begin(ReplayDirection::Undo)
            .expect("begin current")
            .expect("current plan");
        let mut foreign = seeded();
        let foreign_plan = foreign
            .begin(ReplayDirection::Undo)
            .expect("begin foreign")
            .expect("foreign plan");

        assert_ne!(current_plan.reservation, foreign_plan.reservation);
        assert_eq!(
            current.commit_immediate(foreign_plan.reservation),
            Err(HistoryError::ReservationMismatch)
        );
        assert_eq!(
            current.pending_reservation(),
            Some(current_plan.reservation)
        );
        assert!(current.can_undo());
    }

    #[test]
    fn interrupted_execution_can_only_resume_and_settle_for_its_operation() {
        let mut center = seeded();
        let plan = center
            .begin(ReplayDirection::Undo)
            .expect("begin")
            .expect("plan");
        let operation = OperationId("replay-operation".to_string());
        let foreign = OperationId("foreign-operation".to_string());

        center
            .bind_execution(plan.reservation, &operation)
            .expect("bind execution");
        assert_eq!(
            center.commit_execution(plan.reservation, &foreign),
            Err(HistoryError::OperationMismatch)
        );
        center
            .interrupt_execution(plan.reservation, &operation)
            .expect("interrupt execution");
        assert_eq!(
            center.interrupted_reservation_for(&foreign),
            Err(HistoryError::OperationMismatch)
        );
        assert_eq!(
            center
                .interrupted_reservation_for(&operation)
                .expect("matching recovery"),
            Some(plan.reservation)
        );
        center
            .bind_execution(plan.reservation, &operation)
            .expect("resume execution");
        center
            .commit_execution(plan.reservation, &operation)
            .expect("commit resumed execution");
        assert!(center.can_redo());
    }
}
