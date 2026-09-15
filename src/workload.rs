//! Shared workload admission, scheduling, cancellation, and result freshness.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

const MAX_LATENCY_SAMPLES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskKind {
    Listing,
    PathProbe,
    Search,
    Preview,
    Hash,
    Transfer,
    Index,
}

impl TaskKind {
    const COUNT: usize = 7;

    fn index(self) -> usize {
        match self {
            Self::Listing => 0,
            Self::PathProbe => 1,
            Self::Search => 2,
            Self::Preview => 3,
            Self::Hash => 4,
            Self::Transfer => 5,
            Self::Index => 6,
        }
    }

    fn thread_label(self) -> &'static str {
        match self {
            Self::Listing => "listing",
            Self::PathProbe => "path-probe",
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
    state: Arc<CancellationState>,
}

#[derive(Debug)]
struct CancellationState {
    cancelled: AtomicBool,
    requested_at: Mutex<Option<std::time::Instant>>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            state: Arc::new(CancellationState {
                cancelled: AtomicBool::new(false),
                requested_at: Mutex::new(None),
            }),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }

    fn cancel(&self) {
        let mut requested_at = crate::lock_util::recover(&self.state.requested_at);
        if !self.state.cancelled.load(Ordering::Acquire) {
            *requested_at = Some(std::time::Instant::now());
            self.state.cancelled.store(true, Ordering::Release);
        }
    }

    fn cancellation_elapsed(&self) -> Option<std::time::Duration> {
        crate::lock_util::recover(&self.state.requested_at)
            .as_ref()
            .map(std::time::Instant::elapsed)
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
            per_kind_running: [2, 2, 2, 4, 2, 2, 1],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    QueueFull,
    ByteBudgetExceeded,
    RootDisconnected(PathBuf),
    StaleGeneration { active: u64, submitted: u64 },
    MachinePressure(&'static str),
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
            Self::MachinePressure(reason) => {
                write!(formatter, "workload deferred under machine pressure: {reason}")
            }
        }
    }
}

fn background_pressure_block(kind: TaskKind, priority: Priority) -> Option<&'static str> {
    if !matches!(priority, Priority::Background | Priority::Maintenance) {
        return None;
    }
    let pressure = crate::machine_pressure::snapshot();
    match kind {
        TaskKind::Preview if !pressure.allows_background_preview() => {
            Some("preview admission blocked")
        }
        TaskKind::Index if !pressure.allows_background_index() => Some("index admission blocked"),
        TaskKind::Hash if !pressure.allows_background_hash() => Some("hash admission blocked"),
        _ => None,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerStats {
    pub queued: usize,
    pub running: usize,
    pub inflight_bytes: u64,
    pub cancelled: u64,
    pub stale_results: u64,
    pub backpressured: u64,
    pub cancellation_latency_p50_ticks: u64,
    pub cancellation_latency_p95_ticks: u64,
    pub cancellation_latency_p99_ticks: u64,
    pub cancellation_latency_samples: usize,
    pub cancellation_latency_p50_micros: u64,
    pub cancellation_latency_p95_micros: u64,
    pub cancellation_latency_p99_micros: u64,
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
    cancellation_latencies: VecDeque<u64>,
    cancellation_latencies_micros: VecDeque<u64>,
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
            cancellation_latencies: VecDeque::new(),
            cancellation_latencies_micros: VecDeque::new(),
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
        if let Some(reason) = background_pressure_block(spec.kind, spec.priority) {
            self.counters.backpressured = self.counters.backpressured.saturating_add(1);
            return Err(AdmissionError::MachinePressure(reason));
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
        let was_queued = record.state == TaskState::Queued;
        if was_queued {
            record.state = TaskState::Cancelled;
            self.counters.cancelled = self.counters.cancelled.saturating_add(1);
        }
        if was_queued {
            self.cancellation_latencies_micros.push_back(0);
            trim_samples(&mut self.cancellation_latencies_micros);
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
        let mut cancellation_micros = None;
        let disposition = if stale {
            record.state = TaskState::Stale;
            self.counters.stale_results = self.counters.stale_results.saturating_add(1);
            cancellation_micros = record.token.cancellation_elapsed().map(duration_micros);
            CompletionDisposition::Stale
        } else if record.token.is_cancelled() {
            record.state = TaskState::Cancelled;
            self.counters.cancelled = self.counters.cancelled.saturating_add(1);
            if let Some(requested) = record.cancel_requested_tick {
                let latency = tick.saturating_sub(requested);
                self.counters.max_cancellation_latency_ticks =
                    self.counters.max_cancellation_latency_ticks.max(latency);
                self.cancellation_latencies.push_back(latency);
                trim_samples(&mut self.cancellation_latencies);
            }
            cancellation_micros = record.token.cancellation_elapsed().map(duration_micros);
            CompletionDisposition::Cancelled
        } else if succeeded {
            record.state = TaskState::Completed;
            CompletionDisposition::Accepted
        } else {
            record.state = TaskState::Failed;
            CompletionDisposition::Failed
        };
        if let Some(micros) = cancellation_micros {
            self.cancellation_latencies_micros.push_back(micros);
            trim_samples(&mut self.cancellation_latencies_micros);
        }
        disposition
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
        let (tick_p50, tick_p95, tick_p99) =
            crate::measurement::percentiles_u64(&self.cancellation_latencies);
        let (micros_p50, micros_p95, micros_p99) =
            crate::measurement::percentiles_u64(&self.cancellation_latencies_micros);
        SchedulerStats {
            queued: self.queued_count(),
            running: self.running_count(),
            inflight_bytes: self.inflight_bytes(),
            cancellation_latency_p50_ticks: tick_p50,
            cancellation_latency_p95_ticks: tick_p95,
            cancellation_latency_p99_ticks: tick_p99,
            cancellation_latency_samples: self.cancellation_latencies_micros.len(),
            cancellation_latency_p50_micros: micros_p50,
            cancellation_latency_p95_micros: micros_p95,
            cancellation_latency_p99_micros: micros_p99,
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

fn duration_micros(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn trim_samples(samples: &mut VecDeque<u64>) {
    while samples.len() > MAX_LATENCY_SAMPLES {
        samples.pop_front();
    }
}

pub type WorkloadJob = Box<dyn FnOnce(CancellationToken) + Send + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbandonReason {
    Cancelled,
    Disconnected,
    Superseded,
    SpawnFailed,
    BackendDropped,
}

impl std::fmt::Display for AbandonReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Cancelled => "cancelled while queued",
            Self::Disconnected => "root disconnected",
            Self::Superseded => "superseded before execution",
            Self::SpawnFailed => "worker thread spawn failed",
            Self::BackendDropped => "workload backend dropped",
        };
        formatter.write_str(label)
    }
}

pub type AbandonmentCallback = Box<dyn FnOnce(AbandonReason) + Send + 'static>;

fn invoke_abandonment(callback: AbandonmentCallback, reason: AbandonReason) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(reason)));
}

struct PendingWork {
    work: Option<WorkloadJob>,
    on_abandoned: Option<AbandonmentCallback>,
    fallback_reason: AbandonReason,
}

impl PendingWork {
    fn new(work: WorkloadJob, on_abandoned: Option<AbandonmentCallback>) -> Self {
        Self {
            work: Some(work),
            on_abandoned,
            fallback_reason: AbandonReason::BackendDropped,
        }
    }

    fn with_fallback_reason(mut self, reason: AbandonReason) -> Self {
        self.fallback_reason = reason;
        self
    }

    fn run(mut self, token: CancellationToken) {
        self.on_abandoned = None;
        let work = self.work.take().expect("pending work runs at most once");
        work(token);
    }

    fn abandon(mut self, reason: AbandonReason) {
        self.work = None;
        if let Some(callback) = self.on_abandoned.take() {
            invoke_abandonment(callback, reason);
        }
    }
}

impl Drop for PendingWork {
    fn drop(&mut self) {
        if let Some(callback) = self.on_abandoned.take() {
            invoke_abandonment(callback, self.fallback_reason);
        }
    }
}

struct RuntimeState {
    scheduler: Scheduler,
    pending: HashMap<TaskId, PendingWork>,
    tick: u64,
}

pub trait WorkloadBackend: Send + Sync + 'static {
    fn submit_boxed(
        self: Arc<Self>,
        spec: TaskSpec,
        work: WorkloadJob,
        on_abandoned: Option<AbandonmentCallback>,
    ) -> Result<TaskSnapshot, AdmissionError>;

    fn cancel_task(self: Arc<Self>, id: TaskId) -> bool;

    fn stats(&self) -> SchedulerStats;
}

/// Cloneable ownership boundary for workload admission and task cancellation.
///
/// A handle keeps its backend alive. Submitted task handles hold only a weak
/// reference, so a task cannot extend the backend lifecycle on its own.
#[derive(Clone)]
pub struct WorkloadHandle {
    backend: Arc<dyn WorkloadBackend>,
}

impl WorkloadHandle {
    pub fn new(backend: Arc<dyn WorkloadBackend>) -> Self {
        Self { backend }
    }

    pub fn from_runtime(runtime: Arc<WorkloadRuntime>) -> Self {
        Self::new(runtime)
    }

    pub fn submit(
        &self,
        spec: TaskSpec,
        work: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> Result<TaskHandle, AdmissionError> {
        self.submit_inner(spec, Box::new(work), None)
    }

    pub fn submit_with_abandonment(
        &self,
        spec: TaskSpec,
        work: impl FnOnce(CancellationToken) + Send + 'static,
        on_abandoned: impl FnOnce(AbandonReason) + Send + 'static,
    ) -> Result<TaskHandle, AdmissionError> {
        self.submit_inner(spec, Box::new(work), Some(Box::new(on_abandoned)))
    }

    fn submit_inner(
        &self,
        spec: TaskSpec,
        work: WorkloadJob,
        on_abandoned: Option<AbandonmentCallback>,
    ) -> Result<TaskHandle, AdmissionError> {
        let snapshot = Arc::clone(&self.backend).submit_boxed(spec, work, on_abandoned)?;
        Ok(TaskHandle {
            snapshot,
            owner: Arc::downgrade(&self.backend),
        })
    }

    pub fn stats(&self) -> SchedulerStats {
        self.backend.stats()
    }
}

pub struct WorkloadRuntime {
    state: Mutex<RuntimeState>,
    #[cfg(test)]
    fail_next_spawn: AtomicBool,
}

impl WorkloadRuntime {
    pub fn new(limits: SchedulerLimits) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(RuntimeState {
                scheduler: Scheduler::new(limits),
                pending: HashMap::new(),
                tick: 0,
            }),
            #[cfg(test)]
            fail_next_spawn: AtomicBool::new(false),
        })
    }

    pub fn submit(
        self: &Arc<Self>,
        spec: TaskSpec,
        work: impl FnOnce(CancellationToken) + Send + 'static,
    ) -> Result<TaskHandle, AdmissionError> {
        WorkloadHandle::from_runtime(Arc::clone(self)).submit(spec, work)
    }

    fn submit_boxed(
        self: &Arc<Self>,
        spec: TaskSpec,
        work: WorkloadJob,
        on_abandoned: Option<AbandonmentCallback>,
    ) -> Result<TaskSnapshot, AdmissionError> {
        let (snapshot, superseded) = {
            let mut state = crate::lock_util::recover(&self.state);
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            let snapshot = state.scheduler.submit_at(spec, tick)?;
            let superseded_ids = state
                .pending
                .keys()
                .copied()
                .filter(|id| {
                    !state
                        .scheduler
                        .tasks
                        .get(id)
                        .is_some_and(|record| record.state == TaskState::Queued)
                })
                .collect::<Vec<_>>();
            let superseded = superseded_ids
                .into_iter()
                .filter_map(|id| state.pending.remove(&id))
                .collect::<Vec<_>>();
            state
                .pending
                .insert(snapshot.id, PendingWork::new(work, on_abandoned));
            (snapshot, superseded)
        };
        for pending in superseded {
            pending.abandon(AbandonReason::Superseded);
        }
        self.pump();
        Ok(snapshot)
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
                (
                    started,
                    work.with_fallback_reason(AbandonReason::SpawnFailed),
                )
            };
            let runtime = Arc::clone(self);
            let task_id = next.0.snapshot.id;
            let thread_name = format!(
                "work-{}-{}",
                next.0.snapshot.kind.thread_label(),
                next.0.snapshot.id.0
            );
            #[cfg(test)]
            if self.fail_next_spawn.swap(false, Ordering::AcqRel) {
                drop(next);
                self.finish(task_id, false);
                continue;
            }
            let spawn = std::thread::Builder::new()
                .name(thread_name)
                .spawn(move || {
                    let succeeded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        next.1.run(next.0.token);
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
        let (cancelled, abandoned) = {
            let mut state = crate::lock_util::recover(&self.state);
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            let cancelled = state.scheduler.cancel_at(id, tick);
            let abandoned = state.pending.remove(&id);
            (cancelled, abandoned)
        };
        if let Some(pending) = abandoned {
            pending.abandon(AbandonReason::Cancelled);
        }
        self.pump();
        cancelled
    }

    pub fn disconnect_root(self: &Arc<Self>, root: &Path) -> usize {
        let (count, abandoned) = {
            let mut state = crate::lock_util::recover(&self.state);
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            let ids = state.scheduler.disconnect_at(root, tick);
            let abandoned = ids
                .iter()
                .filter_map(|id| state.pending.remove(id))
                .collect::<Vec<_>>();
            (ids.len(), abandoned)
        };
        for pending in abandoned {
            pending.abandon(AbandonReason::Disconnected);
        }
        self.pump();
        count
    }

    #[cfg(test)]
    fn fail_next_spawn_for_test(&self) {
        self.fail_next_spawn.store(true, Ordering::Release);
    }

    pub fn stats(&self) -> SchedulerStats {
        crate::lock_util::recover(&self.state).scheduler.stats()
    }
}

impl WorkloadBackend for WorkloadRuntime {
    fn submit_boxed(
        self: Arc<Self>,
        spec: TaskSpec,
        work: WorkloadJob,
        on_abandoned: Option<AbandonmentCallback>,
    ) -> Result<TaskSnapshot, AdmissionError> {
        WorkloadRuntime::submit_boxed(&self, spec, work, on_abandoned)
    }

    fn cancel_task(self: Arc<Self>, id: TaskId) -> bool {
        WorkloadRuntime::cancel(&self, id)
    }

    fn stats(&self) -> SchedulerStats {
        WorkloadRuntime::stats(self)
    }
}

#[derive(Clone)]
pub struct TaskHandle {
    snapshot: TaskSnapshot,
    owner: Weak<dyn WorkloadBackend>,
}

impl TaskHandle {
    pub fn snapshot(&self) -> &TaskSnapshot {
        &self.snapshot
    }

    pub fn cancel(&self) -> bool {
        self.owner
            .upgrade()
            .is_some_and(|runtime| runtime.cancel_task(self.snapshot.id))
    }
}

fn global_runtime() -> &'static Arc<WorkloadRuntime> {
    static RUNTIME: OnceLock<Arc<WorkloadRuntime>> = OnceLock::new();
    RUNTIME.get_or_init(|| WorkloadRuntime::new(SchedulerLimits::default()))
}

pub fn global_handle() -> WorkloadHandle {
    WorkloadHandle::from_runtime(Arc::clone(global_runtime()))
}

pub fn submit(
    spec: TaskSpec,
    work: impl FnOnce(CancellationToken) + Send + 'static,
) -> Result<TaskHandle, AdmissionError> {
    global_handle().submit(spec, work)
}

pub fn stats() -> SchedulerStats {
    global_handle().stats()
}

#[cfg(test)]
struct DeterministicBackend {
    state: Mutex<RuntimeState>,
}

#[cfg(test)]
impl WorkloadBackend for DeterministicBackend {
    fn submit_boxed(
        self: Arc<Self>,
        spec: TaskSpec,
        work: WorkloadJob,
        on_abandoned: Option<AbandonmentCallback>,
    ) -> Result<TaskSnapshot, AdmissionError> {
        let (snapshot, superseded) = {
            let mut state = crate::lock_util::recover(&self.state);
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            let snapshot = state.scheduler.submit_at(spec, tick)?;
            let superseded_ids = state
                .pending
                .keys()
                .copied()
                .filter(|id| {
                    !state
                        .scheduler
                        .tasks
                        .get(id)
                        .is_some_and(|record| record.state == TaskState::Queued)
                })
                .collect::<Vec<_>>();
            let superseded = superseded_ids
                .into_iter()
                .filter_map(|id| state.pending.remove(&id))
                .collect::<Vec<_>>();
            state
                .pending
                .insert(snapshot.id, PendingWork::new(work, on_abandoned));
            (snapshot, superseded)
        };
        for pending in superseded {
            pending.abandon(AbandonReason::Superseded);
        }
        Ok(snapshot)
    }

    fn cancel_task(self: Arc<Self>, id: TaskId) -> bool {
        let (cancelled, abandoned) = {
            let mut state = crate::lock_util::recover(&self.state);
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            let cancelled = state.scheduler.cancel_at(id, tick);
            let abandoned = state.pending.remove(&id);
            (cancelled, abandoned)
        };
        if let Some(pending) = abandoned {
            pending.abandon(AbandonReason::Cancelled);
        }
        cancelled
    }

    fn stats(&self) -> SchedulerStats {
        crate::lock_util::recover(&self.state).scheduler.stats()
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct DeterministicWorkload {
    backend: Arc<DeterministicBackend>,
}

#[cfg(test)]
impl DeterministicWorkload {
    pub(crate) fn new(limits: SchedulerLimits) -> Self {
        Self {
            backend: Arc::new(DeterministicBackend {
                state: Mutex::new(RuntimeState {
                    scheduler: Scheduler::new(limits),
                    pending: HashMap::new(),
                    tick: 0,
                }),
            }),
        }
    }

    pub(crate) fn handle(&self) -> WorkloadHandle {
        WorkloadHandle::new(self.backend.clone())
    }

    pub(crate) fn run_next(&self) -> bool {
        self.run_next_after_dequeue(|| {})
    }

    pub(crate) fn run_next_after_dequeue(&self, before_run: impl FnOnce()) -> bool {
        let (started, work) = {
            let mut state = crate::lock_util::recover(&self.backend.state);
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            let Some(started) = state.scheduler.start_next_at(tick) else {
                return false;
            };
            let Some(work) = state.pending.remove(&started.snapshot.id) else {
                state.scheduler.cancel_at(started.snapshot.id, tick);
                return false;
            };
            (started, work)
        };
        before_run();
        let task_id = started.snapshot.id;
        let succeeded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            work.run(started.token);
        }))
        .is_ok();
        let mut state = crate::lock_util::recover(&self.backend.state);
        state.tick = state.tick.saturating_add(1);
        let tick = state.tick;
        state.scheduler.complete_at(task_id, tick, succeeded);
        true
    }

    pub(crate) fn stats(&self) -> SchedulerStats {
        self.backend.stats()
    }
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
        assert_eq!(stats.cancellation_latency_p50_ticks, 4);
        assert_eq!(stats.cancellation_latency_p95_ticks, 4);
        assert_eq!(stats.cancellation_latency_p99_ticks, 4);
        assert_eq!(stats.cancellation_latency_samples, 2);
        assert!(stats.cancellation_latency_p50_micros <= stats.cancellation_latency_p95_micros);
        assert!(stats.cancellation_latency_p95_micros <= stats.cancellation_latency_p99_micros);
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

    #[test]
    fn injected_handle_runs_deterministically_without_global_state() {
        let runtime = DeterministicWorkload::new(SchedulerLimits {
            max_running: 1,
            max_queued: 4,
            max_inflight_bytes: 100,
            per_kind_running: [1; TaskKind::COUNT],
        });
        let handle = runtime.handle();
        let ran = Arc::new(AtomicBool::new(false));
        let worker_ran = Arc::clone(&ran);

        let task = handle
            .submit(spec(TaskKind::Index, 1, Priority::Background), move |_| {
                worker_ran.store(true, Ordering::Release);
            })
            .unwrap();

        assert_eq!(handle.stats().queued, 1);
        assert!(!ran.load(Ordering::Acquire));
        assert!(runtime.run_next());
        assert!(ran.load(Ordering::Acquire));
        assert_eq!(runtime.stats().queued, 0);
        assert_eq!(runtime.stats().running, 0);
        assert!(!task.cancel());
    }

    #[test]
    fn queued_cancellation_runs_abandonment_callback_exactly_once() {
        let runtime = DeterministicWorkload::new(SchedulerLimits::default());
        let handle = runtime.handle();
        let callback_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let callback_count_for_job = Arc::clone(&callback_count);
        let observed_reason = Arc::new(Mutex::new(None));
        let observed_reason_for_job = Arc::clone(&observed_reason);

        let task = handle
            .submit_with_abandonment(
                spec(TaskKind::Transfer, 1, Priority::Critical),
                |_| panic!("cancelled queued work must not execute"),
                move |reason| {
                    callback_count_for_job.fetch_add(1, Ordering::SeqCst);
                    *crate::lock_util::recover(&observed_reason_for_job) = Some(reason);
                },
            )
            .unwrap();

        assert!(task.cancel());
        assert!(!task.cancel());
        assert_eq!(callback_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            *crate::lock_util::recover(&observed_reason),
            Some(AbandonReason::Cancelled)
        );
        assert_eq!(runtime.stats().queued, 0);
        assert!(!runtime.run_next());
        assert_eq!(callback_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn spawn_failure_abandons_admitted_work_without_running_it() {
        let runtime = WorkloadRuntime::new(SchedulerLimits::default());
        runtime.fail_next_spawn_for_test();
        let handle = WorkloadHandle::from_runtime(Arc::clone(&runtime));
        let callback_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let callback_count_for_job = Arc::clone(&callback_count);
        let observed_reason = Arc::new(Mutex::new(None));
        let observed_reason_for_job = Arc::clone(&observed_reason);
        let work_ran = Arc::new(AtomicBool::new(false));
        let work_ran_for_job = Arc::clone(&work_ran);

        let task = handle
            .submit_with_abandonment(
                spec(TaskKind::Transfer, 1, Priority::Critical),
                move |_| {
                    work_ran_for_job.store(true, Ordering::Release);
                },
                move |reason| {
                    callback_count_for_job.fetch_add(1, Ordering::SeqCst);
                    *crate::lock_util::recover(&observed_reason_for_job) = Some(reason);
                },
            )
            .unwrap();

        assert!(!work_ran.load(Ordering::Acquire));
        assert_eq!(callback_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            *crate::lock_util::recover(&observed_reason),
            Some(AbandonReason::SpawnFailed)
        );
        assert_eq!(runtime.stats().running, 0);
        assert!(!task.cancel());
    }

    #[test]
    fn disconnect_abandons_each_queued_job_once() {
        let runtime = WorkloadRuntime::new(SchedulerLimits {
            max_running: 0,
            ..SchedulerLimits::default()
        });
        let handle = WorkloadHandle::from_runtime(Arc::clone(&runtime));
        let callback_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_reason = Arc::new(Mutex::new(None));
        let callback_count_for_job = Arc::clone(&callback_count);
        let observed_reason_for_job = Arc::clone(&observed_reason);

        handle
            .submit_with_abandonment(
                spec(TaskKind::Index, 1, Priority::Background),
                |_| panic!("disconnected queued work must not execute"),
                move |reason| {
                    callback_count_for_job.fetch_add(1, Ordering::SeqCst);
                    *crate::lock_util::recover(&observed_reason_for_job) = Some(reason);
                },
            )
            .unwrap();

        assert_eq!(runtime.disconnect_root(Path::new("/project")), 1);
        assert_eq!(callback_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            *crate::lock_util::recover(&observed_reason),
            Some(AbandonReason::Disconnected)
        );
        assert_eq!(runtime.stats().queued, 0);
    }
}
