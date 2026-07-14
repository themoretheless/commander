//! Shared workload admission, scheduling, cancellation, and result freshness.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskKind {
    Listing,
    Search,
    Preview,
    Hash,
    Transfer,
    Index,
}

impl TaskKind {
    const COUNT: usize = 6;

    fn index(self) -> usize {
        match self {
            Self::Listing => 0,
            Self::Search => 1,
            Self::Preview => 2,
            Self::Hash => 3,
            Self::Transfer => 4,
            Self::Index => 5,
        }
    }

    fn thread_label(self) -> &'static str {
        match self {
            Self::Listing => "listing",
            Self::Search => "search",
            Self::Preview => "preview",
            Self::Hash => "hash",
            Self::Transfer => "transfer",
            Self::Index => "index",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    Maintenance,
    Background,
    Interactive,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub id: TaskId,
    pub kind: TaskKind,
    pub root: PathBuf,
    pub generation: u64,
    pub priority: Priority,
    pub estimated_bytes: u64,
    pub submitted_tick: u64,
    pub replace_older_generation: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskSpec {
    pub kind: TaskKind,
    pub root: PathBuf,
    pub generation: u64,
    pub priority: Priority,
    pub estimated_bytes: u64,
    pub replace_older_generation: bool,
}

impl TaskSpec {
    pub fn new(kind: TaskKind, root: impl Into<PathBuf>, generation: u64) -> Self {
        Self {
            kind,
            root: root.into(),
            generation,
            priority: Priority::Background,
            estimated_bytes: 0,
            replace_older_generation: false,
        }
    }

    pub fn priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    pub fn estimated_bytes(mut self, bytes: u64) -> Self {
        self.estimated_bytes = bytes;
        self
    }

    pub fn replace_older_generation(mut self) -> Self {
        self.replace_older_generation = true;
        self
    }
}

#[derive(Clone, Debug)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TaskState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Stale,
}

#[derive(Clone, Debug)]
struct TaskRecord {
    snapshot: TaskSnapshot,
    token: CancellationToken,
    state: TaskState,
    started_tick: Option<u64>,
    cancel_requested_tick: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchedulerLimits {
    pub max_running: usize,
    pub max_queued: usize,
    pub max_inflight_bytes: u64,
    pub per_kind_running: [usize; TaskKind::COUNT],
}

impl Default for SchedulerLimits {
    fn default() -> Self {
        Self {
            max_running: 8,
            max_queued: 128,
            max_inflight_bytes: 2 * 1024 * 1024 * 1024,
            per_kind_running: [2, 2, 4, 2, 2, 1],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    QueueFull,
    ByteBudgetExceeded,
    RootDisconnected(PathBuf),
    StaleGeneration { active: u64, submitted: u64 },
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QueueFull => write!(formatter, "workload queue is full"),
            Self::ByteBudgetExceeded => write!(formatter, "workload byte budget is exhausted"),
            Self::RootDisconnected(root) => {
                write!(
                    formatter,
                    "workload root is disconnected: {}",
                    root.display()
                )
            }
            Self::StaleGeneration { active, submitted } => write!(
                formatter,
                "workload generation {submitted} is older than active generation {active}"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionDisposition {
    Accepted,
    Failed,
    Cancelled,
    Stale,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SchedulerStats {
    pub queued: usize,
    pub running: usize,
    pub inflight_bytes: u64,
    pub cancelled: u64,
    pub stale_results: u64,
    pub backpressured: u64,
    pub max_cancellation_latency_ticks: u64,
}

#[derive(Clone, Debug)]
pub struct StartedTask {
    pub snapshot: TaskSnapshot,
    pub token: CancellationToken,
}

pub struct Scheduler {
    limits: SchedulerLimits,
    next_id: u64,
    tasks: HashMap<TaskId, TaskRecord>,
    active_generations: HashMap<(TaskKind, PathBuf), u64>,
    disconnected_roots: HashSet<PathBuf>,
    counters: SchedulerStats,
}

impl Scheduler {
    pub fn new(limits: SchedulerLimits) -> Self {
        Self {
            limits,
            next_id: 1,
            tasks: HashMap::new(),
            active_generations: HashMap::new(),
            disconnected_roots: HashSet::new(),
            counters: SchedulerStats::default(),
        }
    }

    pub fn submit_at(&mut self, spec: TaskSpec, tick: u64) -> Result<TaskSnapshot, AdmissionError> {
        self.tasks
            .retain(|_, record| matches!(record.state, TaskState::Queued | TaskState::Running));
        if self
            .disconnected_roots
            .iter()
            .any(|root| spec.root.starts_with(root))
        {
            self.counters.backpressured = self.counters.backpressured.saturating_add(1);
            return Err(AdmissionError::RootDisconnected(spec.root));
        }
        if self.queued_count() >= self.limits.max_queued {
            self.counters.backpressured = self.counters.backpressured.saturating_add(1);
            return Err(AdmissionError::QueueFull);
        }
        if spec.estimated_bytes > self.limits.max_inflight_bytes {
            self.counters.backpressured = self.counters.backpressured.saturating_add(1);
            return Err(AdmissionError::ByteBudgetExceeded);
        }
        if spec.replace_older_generation {
            let key = (spec.kind, spec.root.clone());
            if let Some(active) = self.active_generations.get(&key).copied()
                && spec.generation < active
            {
                self.counters.backpressured = self.counters.backpressured.saturating_add(1);
                return Err(AdmissionError::StaleGeneration {
                    active,
                    submitted: spec.generation,
                });
            }
            self.active_generations.insert(key, spec.generation);
            for record in self.tasks.values_mut().filter(|record| {
                record.snapshot.replace_older_generation
                    && record.snapshot.kind == spec.kind
                    && record.snapshot.root == spec.root
                    && record.snapshot.generation < spec.generation
                    && matches!(record.state, TaskState::Queued | TaskState::Running)
            }) {
                record.token.cancel();
                record.cancel_requested_tick.get_or_insert(tick);
                if record.state == TaskState::Queued {
                    record.state = TaskState::Cancelled;
                    self.counters.cancelled = self.counters.cancelled.saturating_add(1);
                }
            }
        }
        let id = TaskId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        let snapshot = TaskSnapshot {
            id,
            kind: spec.kind,
            root: spec.root,
            generation: spec.generation,
            priority: spec.priority,
            estimated_bytes: spec.estimated_bytes,
            submitted_tick: tick,
            replace_older_generation: spec.replace_older_generation,
        };
        self.tasks.insert(
            id,
            TaskRecord {
                snapshot: snapshot.clone(),
                token: CancellationToken::new(),
                state: TaskState::Queued,
                started_tick: None,
                cancel_requested_tick: None,
            },
        );
        Ok(snapshot)
    }

    pub fn start_next_at(&mut self, tick: u64) -> Option<StartedTask> {
        if self.running_count() >= self.limits.max_running {
            return None;
        }
        let running_bytes = self.inflight_bytes();
        let mut candidates = self
            .tasks
            .values()
            .filter(|record| record.state == TaskState::Queued && !record.token.is_cancelled())
            .filter(|record| {
                self.running_of_kind(record.snapshot.kind)
                    < self.limits.per_kind_running[record.snapshot.kind.index()].max(1)
            })
            .filter(|record| {
                running_bytes.saturating_add(record.snapshot.estimated_bytes)
                    <= self.limits.max_inflight_bytes
            })
            .map(|record| record.snapshot.clone())
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.id.0.cmp(&right.id.0))
        });
        let snapshot = candidates.into_iter().next()?;
        let record = self.tasks.get_mut(&snapshot.id)?;
        record.state = TaskState::Running;
        record.started_tick = Some(tick);
        Some(StartedTask {
            snapshot,
            token: record.token.clone(),
        })
    }

    pub fn cancel_at(&mut self, id: TaskId, tick: u64) -> bool {
        let Some(record) = self.tasks.get_mut(&id) else {
            return false;
        };
        if !matches!(record.state, TaskState::Queued | TaskState::Running) {
            return false;
        }
        record.token.cancel();
        record.cancel_requested_tick.get_or_insert(tick);
        if record.state == TaskState::Queued {
            record.state = TaskState::Cancelled;
            self.counters.cancelled = self.counters.cancelled.saturating_add(1);
        }
        true
    }

    pub fn complete_at(&mut self, id: TaskId, tick: u64, succeeded: bool) -> CompletionDisposition {
        let Some(record) = self.tasks.get_mut(&id) else {
            return CompletionDisposition::Unknown;
        };
        if record.state != TaskState::Running {
            return if record.state == TaskState::Cancelled {
                CompletionDisposition::Cancelled
            } else {
                CompletionDisposition::Unknown
            };
        }
        let stale = record.snapshot.replace_older_generation
            && self
                .active_generations
                .get(&(record.snapshot.kind, record.snapshot.root.clone()))
                .is_some_and(|generation| *generation != record.snapshot.generation);
        if stale {
            record.state = TaskState::Stale;
            self.counters.stale_results = self.counters.stale_results.saturating_add(1);
            CompletionDisposition::Stale
        } else if record.token.is_cancelled() {
            record.state = TaskState::Cancelled;
            self.counters.cancelled = self.counters.cancelled.saturating_add(1);
            if let Some(requested) = record.cancel_requested_tick {
                self.counters.max_cancellation_latency_ticks = self
                    .counters
                    .max_cancellation_latency_ticks
                    .max(tick.saturating_sub(requested));
            }
            CompletionDisposition::Cancelled
        } else if succeeded {
            record.state = TaskState::Completed;
            CompletionDisposition::Accepted
        } else {
            record.state = TaskState::Failed;
            CompletionDisposition::Failed
        }
    }

    pub fn disconnect_at(&mut self, root: &Path, tick: u64) -> Vec<TaskId> {
        self.disconnected_roots.insert(root.to_path_buf());
        let ids = self
            .tasks
            .values()
            .filter(|record| {
                record.snapshot.root.starts_with(root)
                    && matches!(record.state, TaskState::Queued | TaskState::Running)
            })
            .map(|record| record.snapshot.id)
            .collect::<Vec<_>>();
        for id in &ids {
            self.cancel_at(*id, tick);
        }
        ids
    }

    pub fn reconnect(&mut self, root: &Path) {
        self.disconnected_roots.remove(root);
    }

    pub fn stats(&self) -> SchedulerStats {
        SchedulerStats {
            queued: self.queued_count(),
            running: self.running_count(),
            inflight_bytes: self.inflight_bytes(),
            ..self.counters
        }
    }

    fn queued_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|record| record.state == TaskState::Queued)
            .count()
    }

    fn running_count(&self) -> usize {
        self.tasks
            .values()
            .filter(|record| record.state == TaskState::Running)
            .count()
    }

    fn running_of_kind(&self, kind: TaskKind) -> usize {
        self.tasks
            .values()
            .filter(|record| record.state == TaskState::Running && record.snapshot.kind == kind)
            .count()
    }

    fn inflight_bytes(&self) -> u64 {
        self.tasks
            .values()
            .filter(|record| record.state == TaskState::Running)
            .map(|record| record.snapshot.estimated_bytes)
            .sum()
    }
}

type Work = Box<dyn FnOnce(CancellationToken) + Send + 'static>;

struct RuntimeState {
    scheduler: Scheduler,
    pending: HashMap<TaskId, Work>,
    tick: u64,
}

pub struct WorkloadRuntime {
    state: Mutex<RuntimeState>,
}

impl WorkloadRuntime {
    pub fn new(limits: SchedulerLimits) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(RuntimeState {
                scheduler: Scheduler::new(limits),
                pending: HashMap::new(),
                tick: 0,
            }),
        })
    }

    pub fn submit(
        self: &Arc<Self>,
        spec: TaskSpec,
        work: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> Result<TaskHandle, AdmissionError> {
        let snapshot = {
            let mut state = crate::lock_util::recover(&self.state);
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            let snapshot = state.scheduler.submit_at(spec, tick)?;
            state.pending.insert(snapshot.id, Box::new(work));
            snapshot
        };
        self.pump();
        Ok(TaskHandle {
            snapshot,
            runtime: Arc::downgrade(self),
        })
    }

    fn pump(self: &Arc<Self>) {
        loop {
            let next = {
                let mut state = crate::lock_util::recover(&self.state);
                state.tick = state.tick.saturating_add(1);
                let tick = state.tick;
                let Some(started) = state.scheduler.start_next_at(tick) else {
                    return;
                };
                let Some(work) = state.pending.remove(&started.snapshot.id) else {
                    state.scheduler.cancel_at(started.snapshot.id, tick);
                    continue;
                };
                (started, work)
            };
            let runtime = Arc::clone(self);
            let task_id = next.0.snapshot.id;
            let thread_name = format!(
                "work-{}-{}",
                next.0.snapshot.kind.thread_label(),
                next.0.snapshot.id.0
            );
            let spawn = std::thread::Builder::new()
                .name(thread_name)
                .spawn(move || {
                    let succeeded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        (next.1)(next.0.token);
                    }))
                    .is_ok();
                    runtime.finish(next.0.snapshot.id, succeeded);
                });
            if spawn.is_err() {
                self.finish(task_id, false);
            }
        }
    }

    fn finish(self: &Arc<Self>, id: TaskId, succeeded: bool) {
        {
            let mut state = crate::lock_util::recover(&self.state);
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            state.scheduler.complete_at(id, tick, succeeded);
        }
        self.pump();
    }

    fn cancel(self: &Arc<Self>, id: TaskId) -> bool {
        let cancelled = {
            let mut state = crate::lock_util::recover(&self.state);
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            let cancelled = state.scheduler.cancel_at(id, tick);
            state.pending.remove(&id);
            cancelled
        };
        self.pump();
        cancelled
    }

    pub fn disconnect_root(self: &Arc<Self>, root: &Path) -> usize {
        let count = {
            let mut state = crate::lock_util::recover(&self.state);
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            let ids = state.scheduler.disconnect_at(root, tick);
            for id in &ids {
                state.pending.remove(id);
            }
            ids.len()
        };
        self.pump();
        count
    }

    pub fn stats(&self) -> SchedulerStats {
        crate::lock_util::recover(&self.state).scheduler.stats()
    }
}

#[derive(Clone)]
pub struct TaskHandle {
    snapshot: TaskSnapshot,
    runtime: Weak<WorkloadRuntime>,
}

impl TaskHandle {
    pub fn snapshot(&self) -> &TaskSnapshot {
        &self.snapshot
    }

    pub fn cancel(&self) -> bool {
        self.runtime
            .upgrade()
            .is_some_and(|runtime| runtime.cancel(self.snapshot.id))
    }
}

fn global_runtime() -> &'static Arc<WorkloadRuntime> {
    static RUNTIME: OnceLock<Arc<WorkloadRuntime>> = OnceLock::new();
    RUNTIME.get_or_init(|| WorkloadRuntime::new(SchedulerLimits::default()))
}

pub fn submit(
    spec: TaskSpec,
    work: impl FnOnce(CancellationToken) + Send + 'static,
) -> Result<TaskHandle, AdmissionError> {
    global_runtime().submit(spec, work)
}

pub fn stats() -> SchedulerStats {
    global_runtime().stats()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: TaskKind, generation: u64, priority: Priority) -> TaskSpec {
        TaskSpec::new(kind, "/project", generation)
            .priority(priority)
            .estimated_bytes(10)
    }

    #[test]
    fn priority_quota_and_backpressure_are_deterministic() {
        let mut scheduler = Scheduler::new(SchedulerLimits {
            max_running: 1,
            max_queued: 3,
            max_inflight_bytes: 20,
            per_kind_running: [1; TaskKind::COUNT],
        });
        let background = scheduler
            .submit_at(spec(TaskKind::Index, 1, Priority::Background), 1)
            .unwrap();
        let critical = scheduler
            .submit_at(spec(TaskKind::Transfer, 1, Priority::Critical), 2)
            .unwrap();
        scheduler
            .submit_at(spec(TaskKind::Preview, 1, Priority::Interactive), 3)
            .unwrap();
        assert!(matches!(
            scheduler.submit_at(spec(TaskKind::Search, 1, Priority::Interactive), 4),
            Err(AdmissionError::QueueFull)
        ));
        assert_eq!(scheduler.start_next_at(5).unwrap().snapshot.id, critical.id);
        assert_eq!(
            scheduler.complete_at(critical.id, 6, true),
            CompletionDisposition::Accepted
        );
        assert_ne!(
            scheduler.start_next_at(7).unwrap().snapshot.id,
            background.id
        );
    }

    #[test]
    fn immutable_snapshots_serialize_and_stale_generations_are_rejected() {
        let mut scheduler = Scheduler::new(SchedulerLimits {
            max_running: 2,
            ..SchedulerLimits::default()
        });
        let first = scheduler
            .submit_at(
                spec(TaskKind::Search, 7, Priority::Interactive).replace_older_generation(),
                1,
            )
            .unwrap();
        let started = scheduler.start_next_at(2).unwrap();
        assert_eq!(started.snapshot, first);
        let encoded = serde_json::to_string(&first).unwrap();
        assert_eq!(
            serde_json::from_str::<TaskSnapshot>(&encoded).unwrap(),
            first
        );
        scheduler
            .submit_at(
                spec(TaskKind::Search, 8, Priority::Interactive).replace_older_generation(),
                3,
            )
            .unwrap();
        assert!(started.token.is_cancelled());
        assert_eq!(
            scheduler.complete_at(first.id, 4, true),
            CompletionDisposition::Stale
        );
        assert_eq!(scheduler.stats().stale_results, 1);
        assert!(matches!(
            scheduler.submit_at(
                spec(TaskKind::Search, 6, Priority::Interactive).replace_older_generation(),
                5,
            ),
            Err(AdmissionError::StaleGeneration {
                active: 8,
                submitted: 6
            })
        ));
    }

    #[test]
    fn simulated_latency_cancellation_and_disconnect_preserve_quotas() {
        let mut scheduler = Scheduler::new(SchedulerLimits {
            max_running: 2,
            max_queued: 8,
            max_inflight_bytes: 100,
            per_kind_running: [2; TaskKind::COUNT],
        });
        let slow = scheduler
            .submit_at(spec(TaskKind::Search, 1, Priority::Interactive), 10)
            .unwrap();
        let queued = scheduler
            .submit_at(spec(TaskKind::Index, 1, Priority::Background), 11)
            .unwrap();
        scheduler.start_next_at(12).unwrap();
        assert_eq!(scheduler.disconnect_at(Path::new("/project"), 15).len(), 2);
        assert_eq!(
            scheduler.complete_at(slow.id, 19, true),
            CompletionDisposition::Cancelled
        );
        let stats = scheduler.stats();
        assert_eq!(stats.running, 0);
        assert_eq!(stats.queued, 0);
        assert_eq!(stats.max_cancellation_latency_ticks, 4);
        assert!(stats.cancelled >= 2);
        scheduler.reconnect(Path::new("/project"));
        assert!(
            scheduler
                .submit_at(spec(TaskKind::Index, 2, Priority::Background), 20)
                .is_ok()
        );
        assert_ne!(slow.id, queued.id);
    }

    #[test]
    fn runtime_starts_queued_work_after_a_slot_is_released() {
        let runtime = WorkloadRuntime::new(SchedulerLimits {
            max_running: 1,
            max_queued: 4,
            max_inflight_bytes: 100,
            per_kind_running: [1; TaskKind::COUNT],
        });
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        runtime
            .submit(spec(TaskKind::Index, 1, Priority::Background), move |_| {
                let _ = release_rx.recv();
            })
            .unwrap();
        runtime
            .submit(
                spec(TaskKind::Search, 1, Priority::Interactive),
                move |_| {
                    let _ = done_tx.send(());
                },
            )
            .unwrap();
        assert!(done_rx.try_recv().is_err());
        release_tx.send(()).unwrap();
        done_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
    }
}
