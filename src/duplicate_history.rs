//! In-memory duplicate proposal edits only; never filesystem undo.
use crate::dedup::KeepPolicy;
use std::collections::VecDeque;
const MAX_STEPS: usize = 32;
pub const MAX_CHOICES: usize = 65_536;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub keep: Vec<Option<usize>>,
    pub policy: KeepPolicy,
}
#[derive(Default)]
pub struct History {
    undo: VecDeque<Snapshot>,
    redo: VecDeque<Snapshot>,
}
impl History {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn record(&mut self, before: Snapshot, keep: &[Option<usize>], policy: KeepPolicy) {
        if before.keep == keep && before.policy == policy {
            return;
        }
        if before.keep.len() != keep.len() || keep.len() > MAX_CHOICES {
            self.clear();
            return;
        }
        self.redo.clear();
        self.undo.push_back(before);
        while self.undo.len() > MAX_STEPS
            || self.undo.iter().map(|s| s.keep.len()).sum::<usize>() > MAX_CHOICES
        {
            self.undo.pop_front();
        }
    }
    pub fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        if self
            .undo
            .back()
            .is_some_and(|s| s.keep.len() != current.keep.len())
        {
            self.clear();
            return None;
        }
        let previous = self.undo.pop_back()?;
        self.redo.push_back(current);
        Some(previous)
    }
    pub fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        if self
            .redo
            .back()
            .is_some_and(|s| s.keep.len() != current.keep.len())
        {
            self.clear();
            return None;
        }
        let next = self.redo.pop_back()?;
        self.undo.push_back(current);
        Some(next)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(keep: Vec<Option<usize>>, policy: KeepPolicy) -> Snapshot {
        Snapshot { keep, policy }
    }
    #[test]
    fn edits_round_trip_keep_all_policy_and_branch_without_recording_noops() {
        let mut h = History::default();
        let a = snapshot(vec![Some(0), None], KeepPolicy::Oldest);
        let b = snapshot(vec![Some(1), Some(0)], KeepPolicy::Newest);
        h.record(a.clone(), &b.keep, b.policy);
        assert_eq!(h.undo(b.clone()), Some(a.clone()));
        h.record(a.clone(), &a.keep, a.policy);
        assert!(h.can_redo());
        assert_eq!(h.redo(a.clone()), Some(b.clone()));
        assert_eq!(h.undo(b.clone()), Some(a.clone()));
        let c = snapshot(vec![None, None], KeepPolicy::Oldest);
        h.record(a.clone(), &c.keep, c.policy);
        assert!(!h.can_redo());
        assert_eq!(h.undo(c.clone()), Some(a));
        h.clear();
        assert!(!h.can_undo() && !h.can_redo());
    }
    #[test]
    fn bounded_history_refuses_unrecordable_and_mismatched_plans() {
        let mut h = History::default();
        for i in 0..40 {
            h.record(
                snapshot(vec![Some(i)], KeepPolicy::Oldest),
                &[Some(i + 1)],
                KeepPolicy::Oldest,
            );
        }
        assert_eq!(h.undo.len(), MAX_STEPS);
        assert!(
            h.undo(snapshot(vec![None, None], KeepPolicy::Oldest))
                .is_none()
        );
        assert!(!h.can_undo() && !h.can_redo());
        let big = vec![Some(0); MAX_CHOICES];
        h.record(
            snapshot(big.clone(), KeepPolicy::Oldest),
            &big,
            KeepPolicy::Newest,
        );
        h.record(
            snapshot(big.clone(), KeepPolicy::Newest),
            &big,
            KeepPolicy::Oldest,
        );
        assert_eq!(h.undo.len(), 1);
        h.record(
            snapshot(big, KeepPolicy::Oldest),
            &vec![None; MAX_CHOICES + 1],
            KeepPolicy::Newest,
        );
        assert!(!h.can_undo() && !h.can_redo());
    }
}
