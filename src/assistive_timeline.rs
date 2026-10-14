//! Rate-limited assistive operation timeline (research **J006**).
use crate::operation::OperationId;
use crate::operation_view::OperationPhase;
use std::collections::VecDeque;

const DEFAULT_MIN_INTERVAL_MILLIS: u64 = 750;
const MAX_EVENTS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineKind {
    Phase,
    Failure,
    Recovery,
    Pause,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineEvent {
    pub kind: TimelineKind,
    pub operation_id: Option<OperationId>,
    pub message: String,
    pub at_millis: u64,
}

#[derive(Clone, Debug)]
pub struct AssistiveTimeline {
    events: VecDeque<TimelineEvent>,
    last_emit_millis: u64,
    min_interval_millis: u64,
    last_phase: Option<(OperationId, OperationPhase)>,
    /// The pause last announced; the operations panel re-notes the pause on
    /// every repaint, which must not flood (and evict) the event history.
    last_pause: Option<(Option<OperationId>, String)>,
    current: String,
}

impl Default for AssistiveTimeline {
    fn default() -> Self {
        Self {
            events: VecDeque::new(),
            last_emit_millis: 0,
            min_interval_millis: DEFAULT_MIN_INTERVAL_MILLIS,
            last_phase: None,
            last_pause: None,
            current: String::new(),
        }
    }
}

impl AssistiveTimeline {
    pub fn current(&self) -> &str {
        &self.current
    }
    pub fn events(&self) -> impl Iterator<Item = &TimelineEvent> {
        self.events.iter()
    }

    pub fn note_phase(
        &mut self,
        operation_id: OperationId,
        phase: OperationPhase,
        detail: impl Into<String>,
        now_millis: u64,
    ) {
        // Phase notes are only made while not paused, so the pause is over.
        self.last_pause = None;
        if self
            .last_phase
            .as_ref()
            .is_some_and(|(id, prev)| id == &operation_id && *prev == phase)
        {
            return;
        }
        self.last_phase = Some((operation_id.clone(), phase));
        self.push(
            TimelineKind::Phase,
            Some(operation_id),
            format!("{}: {}", phase.label(), detail.into()),
            now_millis,
        );
    }

    pub fn note_failure(
        &mut self,
        operation_id: Option<OperationId>,
        message: impl Into<String>,
        now_millis: u64,
    ) {
        self.push(
            TimelineKind::Failure,
            operation_id,
            message.into(),
            now_millis,
        );
    }

    pub fn note_recovery(
        &mut self,
        operation_id: Option<OperationId>,
        message: impl Into<String>,
        now_millis: u64,
    ) {
        self.push(
            TimelineKind::Recovery,
            operation_id,
            message.into(),
            now_millis,
        );
    }

    pub fn note_pause(
        &mut self,
        operation_id: Option<OperationId>,
        message: impl Into<String>,
        now_millis: u64,
    ) {
        let pause = (operation_id, message.into());
        if self.last_pause.as_ref() == Some(&pause) {
            return;
        }
        if self.push(
            TimelineKind::Pause,
            pause.0.clone(),
            pause.1.clone(),
            now_millis,
        ) {
            self.last_pause = Some(pause);
        }
    }

    fn push(
        &mut self,
        kind: TimelineKind,
        operation_id: Option<OperationId>,
        message: String,
        now_millis: u64,
    ) -> bool {
        let force = matches!(kind, TimelineKind::Failure | TimelineKind::Recovery);
        if !force
            && now_millis.saturating_sub(self.last_emit_millis) < self.min_interval_millis
            && !self.current.is_empty()
        {
            return false;
        }
        self.last_emit_millis = now_millis;
        self.current = message.clone();
        self.events.push_front(TimelineEvent {
            kind,
            operation_id,
            message,
            at_millis: now_millis,
        });
        while self.events.len() > MAX_EVENTS {
            self.events.pop_back();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rate_limits_phase_updates_but_always_emits_failures() {
        let mut timeline = AssistiveTimeline::default();
        let op = OperationId("op".into());
        timeline.note_phase(op.clone(), OperationPhase::Transfer, "copying", 1000);
        timeline.note_phase(op.clone(), OperationPhase::Verify, "checking", 1100);
        assert_eq!(timeline.events().count(), 1);
        timeline.note_failure(Some(op), "disk full", 1150);
        assert_eq!(timeline.events().count(), 2);
    }

    #[test]
    fn repeated_pause_note_is_announced_once_per_pause() {
        let mut timeline = AssistiveTimeline::default();
        let op = OperationId("op".into());
        for tick in 0..10 {
            timeline.note_pause(Some(op.clone()), "Quiet hours", 1000 + tick * 1000);
        }
        assert_eq!(timeline.events().count(), 1);
        // Resumed, then paused again for the same reason: announced again.
        timeline.note_phase(op.clone(), OperationPhase::Transfer, "copying", 20_000);
        timeline.note_pause(Some(op), "Quiet hours", 21_000);
        assert_eq!(timeline.events().count(), 3);
    }

    #[test]
    fn rate_limited_pause_is_retried() {
        let mut timeline = AssistiveTimeline::default();
        let op = OperationId("op".into());
        timeline.note_phase(op.clone(), OperationPhase::Transfer, "copying", 1000);
        timeline.note_pause(Some(op.clone()), "Quiet hours", 1100);
        assert_eq!(timeline.events().count(), 1);
        timeline.note_pause(Some(op), "Quiet hours", 2000);
        assert_eq!(timeline.events().count(), 2);
    }
}
