//! Operation queue: a pure, UI-independent state machine that stacks file
//! operations (copy / move / delete) and decides which may run, while
//! honouring a concurrency cap.
//!
//! This module holds NO threads and does NO I/O. It is the brain the transfer
//! worker will be driven by: the worker asks [`Queue::dequeue_next`] for the
//! next job to start and reports back with [`Queue::complete`] / [`Queue::fail`].
//! Keeping it pure makes the lifecycle (legal transitions, ordering under
//! reorder, never exceeding the cap) exhaustively unit-testable.
//!
//! The queue is generic over the spec payload `S` so the core never depends on
//! transfer types; the app instantiates `Queue<TransferSpec>`.
//!
//! The transfer worker drives this queue: every copy/move enqueues a job and
//! `poll_transfer` drains the next when a slot frees. The job-management ops
//! (pause/resume/cancel/reorder/promote/concurrency) are exercised by the unit
//! tests and wired to the keyboard queue panel in a follow-up iteration; they
//! carry a narrow allow below until then.

/// Stable identifier for a queued job, assigned at enqueue and unchanged by
/// reordering, so the UI can refer to a job across frames.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct JobId(pub u64);

/// The kind of file operation a job performs. (Deletes are synchronous and not
/// queued, so there is no `Delete` variant yet.)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobKind {
    Copy,
    Move,
}

/// Lifecycle state of a single job.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobState {
    /// Queued, waiting for a free slot.
    Pending,
    /// Currently executing (occupies a concurrency slot).
    Running,
    /// Held back by the user; will not be dequeued until resumed.
    Paused,
    /// Finished successfully (terminal).
    Done,
    /// Finished with an error (terminal).
    Failed,
    /// Abandoned by the user (terminal).
    Cancelled,
}

impl JobState {
    /// Not started yet, whether runnable now or explicitly held by the user.
    pub fn is_waiting(self) -> bool {
        matches!(self, JobState::Pending | JobState::Paused)
    }

    /// Terminal states accept no further transitions.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Done | JobState::Failed | JobState::Cancelled
        )
    }

    /// Work that can still run or is currently running.
    pub fn is_unfinished(self) -> bool {
        !self.is_terminal()
    }
}

/// One queued operation.
#[derive(Clone, Debug)]
pub struct Job<S> {
    pub id: JobId,
    /// What the job does. Carried for the queue panel's labels (next iteration);
    /// the cap-1 scheduler does not branch on it.
    #[allow(dead_code)]
    pub kind: JobKind,
    pub spec: S,
    pub state: JobState,
}

/// An ordered queue of operations with a concurrency cap.
///
/// Jobs are stored in priority order; [`dequeue_next`](Queue::dequeue_next)
/// always starts the earliest `Pending` job, so reordering pending jobs changes
/// what runs next.
#[derive(Debug)]
pub struct Queue<S> {
    jobs: Vec<Job<S>>,
    /// Maximum number of jobs allowed in `Running` at once (at least 1).
    concurrency: usize,
    next_id: u64,
}

impl<S> Default for Queue<S> {
    fn default() -> Self {
        Self::with_concurrency(1)
    }
}

impl<S> Queue<S> {
    /// A queue running one job at a time.
    pub fn new() -> Self {
        Self::default()
    }

    /// A queue running up to `concurrency` jobs at once (clamped to >= 1, so a
    /// zero can never wedge the queue into never starting anything).
    pub fn with_concurrency(concurrency: usize) -> Self {
        Queue {
            jobs: Vec::new(),
            concurrency: concurrency.max(1),
            next_id: 0,
        }
    }

    /// Current concurrency cap.
    // Concurrency-cap API: used once parallel transfers / the queue panel land.
    #[allow(dead_code)]
    pub fn concurrency(&self) -> usize {
        self.concurrency
    }

    /// Change the concurrency cap (clamped to >= 1). Raising it lets more
    /// pending jobs start on the next `dequeue_next`; lowering it never stops a
    /// job already running, it only throttles future starts.
    #[allow(dead_code)]
    pub fn set_concurrency(&mut self, concurrency: usize) {
        self.concurrency = concurrency.max(1);
    }

    /// All jobs in priority order.
    pub fn jobs(&self) -> &[Job<S>] {
        &self.jobs
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn get(&self, id: JobId) -> Option<&Job<S>> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// How many jobs are currently `Running`.
    pub fn running_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| j.state == JobState::Running)
            .count()
    }

    /// Number of jobs that are pending, paused, or running.
    pub fn unfinished_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|job| job.state.is_unfinished())
            .count()
    }

    /// Append a new `Pending` job, returning its stable id.
    pub fn enqueue(&mut self, kind: JobKind, spec: S) -> JobId {
        let id = JobId(self.next_id);
        self.next_id += 1;
        self.jobs.push(Job {
            id,
            kind,
            spec,
            state: JobState::Pending,
        });
        id
    }

    /// The job that would start next if a slot were free: the earliest
    /// `Pending` job, or `None` if there is none.
    fn next_pending(&self) -> Option<JobId> {
        self.jobs
            .iter()
            .find(|j| j.state == JobState::Pending)
            .map(|j| j.id)
    }

    /// The job that *could* start right now: the earliest `Pending` job, but
    /// only while the running count is below the cap. `None` when the cap is
    /// full or nothing is pending.
    pub fn runnable(&self) -> Option<JobId> {
        if self.running_count() >= self.concurrency {
            return None;
        }
        self.next_pending()
    }

    /// Start the next runnable job: transition it `Pending -> Running` and
    /// return its id. Returns `None` (changing nothing) when the cap is full or
    /// nothing is pending, so a caller can loop it to fill every free slot.
    pub fn dequeue_next(&mut self) -> Option<JobId> {
        let id = self.runnable()?;
        self.set_state(id, JobState::Running);
        Some(id)
    }

    /// Mark a `Running` job finished successfully.
    pub fn complete(&mut self, id: JobId) -> bool {
        self.transition(id, JobState::Running, JobState::Done)
    }

    /// Mark a `Running` job finished with an error.
    pub fn fail(&mut self, id: JobId) -> bool {
        self.transition(id, JobState::Running, JobState::Failed)
    }

    // Job-management API (pause/resume/cancel/reorder/promote): exercised by
    // the unit tests and wired to the keyboard queue panel in a follow-up
    // iteration; cap-1 auto-draining does not call them yet.

    /// Hold a job back. A `Pending` or `Running` job becomes `Paused`; anything
    /// else is left untouched. (Pausing a running job marks intent; the worker
    /// shell is responsible for actually stopping it.)
    #[allow(dead_code)]
    pub fn pause(&mut self, id: JobId) -> bool {
        match self.state_of(id) {
            Some(JobState::Pending) | Some(JobState::Running) => {
                self.set_state(id, JobState::Paused);
                true
            }
            _ => false,
        }
    }

    /// Return a `Paused` job to the back-of-mind `Pending` pool so it can be
    /// dequeued again.
    #[allow(dead_code)]
    pub fn resume(&mut self, id: JobId) -> bool {
        self.transition(id, JobState::Paused, JobState::Pending)
    }

    /// Abandon a job. Any non-terminal job (`Pending`/`Running`/`Paused`)
    /// becomes `Cancelled`; a job that already finished cannot be cancelled.
    /// Used by the transfer shell when the user cancels (the running job and any
    /// jobs queued behind it).
    pub fn cancel(&mut self, id: JobId) -> bool {
        match self.state_of(id) {
            Some(s) if !s.is_terminal() => {
                self.set_state(id, JobState::Cancelled);
                true
            }
            _ => false,
        }
    }

    /// Move a `Pending` job to `to_index` in the queue order, preserving the
    /// relative order of every other job (a remove-then-insert). Only pending
    /// jobs may be reordered; reordering a running or finished job is refused.
    /// `to_index` is clamped into range.
    #[allow(dead_code)]
    pub fn reorder(&mut self, id: JobId, to_index: usize) -> bool {
        let Some(from) = self.jobs.iter().position(|j| j.id == id) else {
            return false;
        };
        if self.jobs[from].state != JobState::Pending {
            return false;
        }
        let to = to_index.min(self.jobs.len() - 1);
        if from == to {
            return true;
        }
        let job = self.jobs.remove(from);
        self.jobs.insert(to, job);
        true
    }

    /// Move a `Pending` job to the front of the queue so it is dequeued next.
    #[allow(dead_code)]
    pub fn promote(&mut self, id: JobId) -> bool {
        self.reorder(id, 0)
    }

    /// Drop terminal jobs (Done/Failed/Cancelled) from the queue, e.g. when the
    /// user clears finished entries. Returns how many were removed.
    pub fn clear_finished(&mut self) -> usize {
        let before = self.jobs.len();
        self.jobs.retain(|j| !j.state.is_terminal());
        before - self.jobs.len()
    }

    fn state_of(&self, id: JobId) -> Option<JobState> {
        self.get(id).map(|j| j.state)
    }

    fn set_state(&mut self, id: JobId, state: JobState) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            job.state = state;
        }
    }

    /// Apply a transition only if the job is currently in `from`.
    fn transition(&mut self, id: JobId, from: JobState, to: JobState) -> bool {
        if self.state_of(id) == Some(from) {
            self.set_state(id, to);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A queue whose payload is just a label, so tests stay readable.
    fn q() -> Queue<&'static str> {
        Queue::new()
    }

    fn states(queue: &Queue<&'static str>) -> Vec<JobState> {
        queue.jobs().iter().map(|j| j.state).collect()
    }

    fn specs(queue: &Queue<&'static str>) -> Vec<&'static str> {
        queue.jobs().iter().map(|j| j.spec).collect()
    }

    #[test]
    fn enqueue_assigns_unique_stable_ids_and_pending_state() {
        let mut queue = q();
        let a = queue.enqueue(JobKind::Copy, "a");
        let b = queue.enqueue(JobKind::Move, "b");
        assert_ne!(a, b);
        assert_eq!(queue.len(), 2);
        assert_eq!(states(&queue), vec![JobState::Pending, JobState::Pending]);
        assert_eq!(queue.get(a).unwrap().kind, JobKind::Copy);
        assert_eq!(queue.get(b).unwrap().kind, JobKind::Move);
    }

    #[test]
    fn dequeue_next_starts_the_earliest_pending_job() {
        let mut queue = q();
        let a = queue.enqueue(JobKind::Copy, "a");
        let _b = queue.enqueue(JobKind::Copy, "b");
        assert_eq!(queue.dequeue_next(), Some(a));
        assert_eq!(queue.get(a).unwrap().state, JobState::Running);
    }

    #[test]
    fn concurrency_cap_of_one_is_never_exceeded() {
        let mut queue = q();
        queue.enqueue(JobKind::Copy, "a");
        queue.enqueue(JobKind::Copy, "b");
        queue.enqueue(JobKind::Copy, "c");

        assert!(queue.dequeue_next().is_some());
        // Cap is 1 and one job is running: nothing more may start.
        assert_eq!(queue.runnable(), None);
        assert_eq!(queue.dequeue_next(), None);
        assert_eq!(queue.running_count(), 1);
    }

    #[test]
    fn raising_concurrency_lets_more_start_but_never_over_cap() {
        let mut queue = q();
        for s in ["a", "b", "c"] {
            queue.enqueue(JobKind::Copy, s);
        }
        queue.set_concurrency(2);
        assert!(queue.dequeue_next().is_some());
        assert!(queue.dequeue_next().is_some());
        assert_eq!(queue.running_count(), 2);
        // Third start is blocked by the cap of 2.
        assert_eq!(queue.dequeue_next(), None);
        assert_eq!(queue.running_count(), 2);
    }

    #[test]
    fn completing_a_job_frees_a_slot_for_the_next() {
        let mut queue = q();
        let a = queue.enqueue(JobKind::Copy, "a");
        let b = queue.enqueue(JobKind::Copy, "b");
        assert_eq!(queue.dequeue_next(), Some(a));
        assert_eq!(queue.dequeue_next(), None); // cap full
        assert!(queue.complete(a));
        assert_eq!(queue.dequeue_next(), Some(b)); // slot freed
        assert_eq!(queue.get(a).unwrap().state, JobState::Done);
    }

    #[test]
    fn concurrency_is_clamped_to_at_least_one() {
        let mut queue = q();
        queue.enqueue(JobKind::Copy, "a");
        queue.set_concurrency(0);
        assert_eq!(queue.concurrency(), 1);
        assert!(queue.dequeue_next().is_some());
    }

    #[test]
    fn pause_blocks_dequeue_and_resume_requeues() {
        let mut queue = q();
        let a = queue.enqueue(JobKind::Copy, "a");
        let b = queue.enqueue(JobKind::Copy, "b");
        // Pause the front job: the next dequeue must skip it.
        assert!(queue.pause(a));
        assert_eq!(queue.dequeue_next(), Some(b));
        // Resume puts a back to Pending; with b running and cap 1 it waits.
        assert!(queue.resume(a));
        assert_eq!(queue.get(a).unwrap().state, JobState::Pending);
        assert_eq!(queue.runnable(), None);
        assert!(queue.complete(b));
        assert_eq!(queue.dequeue_next(), Some(a));
    }

    #[test]
    fn pause_can_target_a_running_job() {
        let mut queue = q();
        let a = queue.enqueue(JobKind::Copy, "a");
        queue.dequeue_next();
        assert_eq!(queue.get(a).unwrap().state, JobState::Running);
        assert!(queue.pause(a));
        assert_eq!(queue.get(a).unwrap().state, JobState::Paused);
        assert_eq!(queue.running_count(), 0);
    }

    #[test]
    fn cancel_works_from_any_non_terminal_state_only() {
        // Enqueue the three to-be-started jobs first and the pending one last,
        // so starting three (cap 3) leaves `pending` genuinely Pending.
        let mut queue = q();
        let running = queue.enqueue(JobKind::Copy, "r");
        let paused = queue.enqueue(JobKind::Copy, "x");
        let done = queue.enqueue(JobKind::Copy, "d");
        let pending = queue.enqueue(JobKind::Copy, "p");

        queue.set_concurrency(3);
        queue.dequeue_next(); // r -> Running
        queue.dequeue_next(); // x -> Running
        queue.dequeue_next(); // d -> Running
        assert!(queue.pause(paused)); // x -> Paused
        assert!(queue.complete(done)); // d -> Done (terminal)
        assert_eq!(queue.get(pending).unwrap().state, JobState::Pending);

        // Cancel succeeds from each non-terminal state: Pending, Running, Paused.
        assert!(queue.cancel(pending));
        assert!(queue.cancel(running));
        assert!(queue.cancel(paused));
        assert_eq!(queue.get(pending).unwrap().state, JobState::Cancelled);
        assert_eq!(queue.get(running).unwrap().state, JobState::Cancelled);
        assert_eq!(queue.get(paused).unwrap().state, JobState::Cancelled);

        // Cancel is refused from a terminal state (Done, or already-Cancelled).
        assert!(!queue.cancel(done));
        assert!(!queue.cancel(running));
    }

    #[test]
    fn unfinished_count_includes_pending_running_and_paused() {
        let mut queue = q();
        let running = queue.enqueue(JobKind::Copy, "running");
        let paused = queue.enqueue(JobKind::Copy, "paused");
        let done = queue.enqueue(JobKind::Copy, "done");
        let pending = queue.enqueue(JobKind::Copy, "pending");
        queue.set_concurrency(3);
        assert_eq!(queue.dequeue_next(), Some(running));
        assert_eq!(queue.dequeue_next(), Some(paused));
        assert_eq!(queue.dequeue_next(), Some(done));
        assert!(queue.pause(paused));
        assert!(queue.complete(done));

        assert!(queue.get(pending).unwrap().state.is_waiting());
        assert!(queue.get(paused).unwrap().state.is_waiting());
        assert!(!queue.get(running).unwrap().state.is_waiting());
        assert_eq!(queue.unfinished_count(), 3);
    }

    #[test]
    fn terminal_states_reject_further_transitions() {
        let mut queue = q();
        let a = queue.enqueue(JobKind::Copy, "a");
        queue.dequeue_next();
        assert!(queue.complete(a));
        // Done is terminal: complete/fail/pause/resume/cancel all refused.
        assert!(!queue.complete(a));
        assert!(!queue.fail(a));
        assert!(!queue.pause(a));
        assert!(!queue.resume(a));
        assert!(!queue.cancel(a));
        assert_eq!(queue.get(a).unwrap().state, JobState::Done);
    }

    #[test]
    fn fail_only_applies_to_running_jobs() {
        let mut queue = q();
        let a = queue.enqueue(JobKind::Copy, "a");
        assert!(!queue.fail(a)); // still Pending
        queue.dequeue_next();
        assert!(queue.fail(a));
        assert_eq!(queue.get(a).unwrap().state, JobState::Failed);
    }

    #[test]
    fn reorder_moves_a_pending_job_and_preserves_others_order() {
        let mut queue = q();
        queue.enqueue(JobKind::Copy, "a");
        queue.enqueue(JobKind::Copy, "b");
        let c = queue.enqueue(JobKind::Copy, "c");
        // Move c to the front; a and b keep their relative order.
        assert!(queue.reorder(c, 0));
        assert_eq!(specs(&queue), vec!["c", "a", "b"]);
        // Dequeue now starts c first.
        assert_eq!(queue.dequeue_next(), Some(c));
    }

    #[test]
    fn promote_sends_a_pending_job_to_the_front() {
        let mut queue = q();
        queue.enqueue(JobKind::Copy, "a");
        queue.enqueue(JobKind::Copy, "b");
        let c = queue.enqueue(JobKind::Copy, "c");
        assert!(queue.promote(c));
        assert_eq!(specs(&queue), vec!["c", "a", "b"]);
    }

    #[test]
    fn reorder_clamps_out_of_range_target() {
        let mut queue = q();
        let a = queue.enqueue(JobKind::Copy, "a");
        queue.enqueue(JobKind::Copy, "b");
        // Target past the end is clamped to the last index.
        assert!(queue.reorder(a, 99));
        assert_eq!(specs(&queue), vec!["b", "a"]);
    }

    #[test]
    fn reorder_refuses_non_pending_jobs() {
        let mut queue = q();
        let a = queue.enqueue(JobKind::Copy, "a");
        queue.enqueue(JobKind::Copy, "b");
        queue.dequeue_next(); // a -> Running
        // A running job cannot be reordered; order is unchanged.
        assert!(!queue.reorder(a, 1));
        assert_eq!(specs(&queue), vec!["a", "b"]);
    }

    #[test]
    fn clear_finished_drops_only_terminal_jobs() {
        let mut queue = q();
        let a = queue.enqueue(JobKind::Copy, "a");
        let b = queue.enqueue(JobKind::Copy, "b");
        let c = queue.enqueue(JobKind::Copy, "c");
        queue.set_concurrency(3);
        queue.dequeue_next(); // a running
        queue.dequeue_next(); // b running
        queue.dequeue_next(); // c running
        queue.complete(a);
        queue.cancel(b);
        // c stays Running; a (Done) and b (Cancelled) are cleared.
        assert_eq!(queue.clear_finished(), 2);
        assert_eq!(specs(&queue), vec!["c"]);
        assert_eq!(queue.get(c).unwrap().state, JobState::Running);
    }

    #[test]
    fn runnable_reports_the_next_startable_job() {
        let mut queue = q();
        assert_eq!(queue.runnable(), None); // empty
        let a = queue.enqueue(JobKind::Copy, "a");
        assert_eq!(queue.runnable(), Some(a));
        queue.dequeue_next();
        assert_eq!(queue.runnable(), None); // cap full
    }
}
