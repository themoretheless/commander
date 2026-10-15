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

    const fn index(self) -> usize {
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

    const fn thread_label(self) -> &'static str {
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

    #[must_use]
    pub const fn priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    #[must_use]
    pub const fn estimated_bytes(mut self, bytes: u64) -> Self {
        self.estimated_bytes = bytes;
        self
    }

    #[must_use]
    pub const fn replace_older_generation(mut self) -> Self {
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
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(CancellationState {
                cancelled: AtomicBool::new(false),
                requested_at: Mutex::new(None),
            }),
        }
    }

    #[must_use]
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

/// Cancelled-but-still-running tasks of one kind that stop holding that
/// kind's running slot (see [`Scheduler::slots_in_use`]).
const MAX_DETACHED_PER_KIND: usize = 4;
/// Cancelled-but-still-running tasks overall that stop holding a
/// `max_running` slot.
const MAX_DETACHED_TOTAL: usize = 8;

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
                write!(
                    formatter,
                    "workload deferred under machine pressure: {reason}"
                )
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
    #[must_use]
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
                retire_if_queued(record, &mut self.counters);
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
        if self.slots_in_use(None) >= self.limits.max_running {
            return None;
        }
        let running_bytes = self.inflight_bytes();
        let mut candidates = self
            .tasks
            .values()
            .filter(|record| record.state == TaskState::Queued && !record.token.is_cancelled())
            .filter(|record| {
                self.slots_in_use(Some(record.snapshot.kind))
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
        retire_if_queued(record, &mut self.counters);
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

    #[must_use]
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

    /// Running slots counted against `max_running` (`kind: None`) or a
    /// per-kind limit. A task whose cancellation was requested but whose job
    /// has not returned yet (for example a `read_dir` blocked on a hung
    /// mount) is detached: it stops holding a slot, so it cannot keep fresh
    /// work of its kind queued forever. Detached tasks still count once there
    /// are more of them than the detached allowance, which bounds the extra
    /// threads a stuck backend can pin.
    fn slots_in_use(&self, kind: Option<TaskKind>) -> usize {
        let (live, detached) = self
            .tasks
            .values()
            .filter(|record| {
                record.state == TaskState::Running
                    && kind.is_none_or(|kind| record.snapshot.kind == kind)
            })
            .fold((0_usize, 0_usize), |(live, detached), record| {
                if record.token.is_cancelled() {
                    (live, detached + 1)
                } else {
                    (live + 1, detached)
                }
            });
        let allowance = if kind.is_some() {
            MAX_DETACHED_PER_KIND
        } else {
            MAX_DETACHED_TOTAL
        };
        live + detached.saturating_sub(allowance)
    }

    fn inflight_bytes(&self) -> u64 {
        self.tasks
            .values()
            .filter(|record| record.state == TaskState::Running)
            .map(|record| record.snapshot.estimated_bytes)
            .sum()
    }
}

/// Cancels a task that never started. It records no latency sample: the
/// tick and wall-clock percentiles both measure how fast *running* work
/// honours cancellation, and instant queued cancels would drag them to zero.
fn retire_if_queued(record: &mut TaskRecord, counters: &mut SchedulerStats) {
    if record.state == TaskState::Queued {
        record.state = TaskState::Cancelled;
        counters.cancelled = counters.cancelled.saturating_add(1);
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
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(reason))).is_err() {
        log::warn!(
            target: "commander::workload",
            "{}",
            caught_panic_message(&format!("abandonment callback ({reason})"))
        );
    }
}

/// App-log line for a panic the runtime caught, naming what panicked.
/// The panic hook (`logging::install_panic_hook`) has already logged the
/// thread, location and message, so this line only adds the runtime context
/// and does not repeat the payload.
fn caught_panic_message(what: &str) -> String {
    format!("{what} panicked (see the preceding panic entry); it was retired as failed")
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

    const fn with_fallback_reason(mut self, reason: AbandonReason) -> Self {
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

    #[must_use]
    pub fn stats(&self) -> SchedulerStats {
        self.backend.stats()
    }
}

pub struct WorkloadRuntime {
    state: Mutex<RuntimeState>,
    #[cfg(test)]
    failing_spawns: std::sync::atomic::AtomicUsize,
}

impl WorkloadRuntime {
    #[must_use]
    pub fn new(limits: SchedulerLimits) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(RuntimeState {
                scheduler: Scheduler::new(limits),
                pending: HashMap::new(),
                tick: 0,
            }),
            #[cfg(test)]
            failing_spawns: std::sync::atomic::AtomicUsize::new(0),
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
                    // Nothing to run: retire the Running record so its slot
                    // is released (cancel_at alone leaves it Running).
                    state
                        .scheduler
                        .complete_at(started.snapshot.id, tick, false);
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
            let task_label = thread_name.clone();
            #[cfg(test)]
            let injected_failure = {
                let left = self.failing_spawns.load(Ordering::Acquire);
                self.failing_spawns
                    .store(left.saturating_sub(1), Ordering::Release);
                left > 0
            };
            #[cfg(not(test))]
            let injected_failure = false;
            let spawned = !injected_failure
                && {
                    let worker_name = thread_name.clone();
                    match std::thread::Builder::new()
                        .name(thread_name)
                        .spawn(move || {
                            let outcome =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    next.1.run(next.0.token);
                                }));
                            if outcome.is_err() {
                                log::warn!(
                                    target: "commander::workload",
                                    "{}",
                                    caught_panic_message(&format!("task {worker_name}"))
                                );
                            }
                            runtime.finish(next.0.snapshot.id, outcome.is_ok());
                        }) {
                        Ok(_) => true,
                        Err(error) => {
                            log::warn!(
                                target: "commander::workload",
                                "could not start worker thread for task {}: {error}; the task was abandoned",
                                task_label
                            );
                            false
                        }
                    }
                };
            if !spawned {
                // The unspawned job was dropped above and abandoned as
                // SpawnFailed. Retire it, but do not burn through the rest of
                // the queue on a failing spawn: while other work is running,
                // its completion pumps again; only an idle runtime retries now.
                let others_running = {
                    let mut state = crate::lock_util::recover(&self.state);
                    state.tick = state.tick.saturating_add(1);
                    let tick = state.tick;
                    state.scheduler.complete_at(task_id, tick, false);
                    state.scheduler.running_count() > 0
                };
                if others_running {
                    return;
                }
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
        self.fail_spawns_for_test(1);
    }

    #[cfg(test)]
    fn fail_spawns_for_test(&self, count: usize) {
        self.failing_spawns.store(count, Ordering::Release);
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
        Self::submit_boxed(&self, spec, work, on_abandoned)
    }

    fn cancel_task(self: Arc<Self>, id: TaskId) -> bool {
        Self::cancel(&self, id)
    }

    fn stats(&self) -> SchedulerStats {
        Self::stats(self)
    }
}

#[derive(Clone)]
pub struct TaskHandle {
    snapshot: TaskSnapshot,
    owner: Weak<dyn WorkloadBackend>,
}

impl TaskHandle {
    #[must_use]
    pub const fn snapshot(&self) -> &TaskSnapshot {
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

#[must_use]
pub fn global_handle() -> WorkloadHandle {
    WorkloadHandle::from_runtime(Arc::clone(global_runtime()))
}

pub fn submit(
    spec: TaskSpec,
    work: impl FnOnce(CancellationToken) + Send + 'static,
) -> Result<TaskHandle, AdmissionError> {
    global_handle().submit(spec, work)
}

/// Like [`submit`], but `on_abandoned` runs if the job is dropped before it
/// starts (cancelled, superseded, spawn failure), so callers can release
/// bookkeeping they reserved for it.
pub fn submit_with_abandonment(
    spec: TaskSpec,
    work: impl FnOnce(CancellationToken) + Send + 'static,
    on_abandoned: impl FnOnce(AbandonReason) + Send + 'static,
) -> Result<TaskHandle, AdmissionError> {
    global_handle().submit_with_abandonment(spec, work, on_abandoned)
}

#[must_use]
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
                state
                    .scheduler
                    .complete_at(started.snapshot.id, tick, false);
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

    #[test]
    fn caught_panic_message_names_task_without_repeating_payload() {
        assert_eq!(
            caught_panic_message("task work-list-7"),
            "task work-list-7 panicked (see the preceding panic entry); it was retired as failed"
        );
    }

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
    fn cancelled_running_tasks_release_their_kind_slot_up_to_a_bound() {
        let mut scheduler = Scheduler::new(SchedulerLimits {
            per_kind_running: [1; TaskKind::COUNT],
            ..SchedulerLimits::default()
        });
        let submit = |scheduler: &mut Scheduler, tick| {
            scheduler
                .submit_at(spec(TaskKind::Listing, tick, Priority::Interactive), tick)
                .unwrap()
        };
        let mut stuck = Vec::new();
        for tick in 1..=u64::try_from(MAX_DETACHED_PER_KIND).unwrap() {
            let task = submit(&mut scheduler, tick * 10);
            // Each earlier cancelled-but-running read no longer holds the slot.
            assert_eq!(
                scheduler.start_next_at(tick * 10 + 1).unwrap().snapshot,
                task
            );
            assert!(scheduler.cancel_at(task.id, tick * 10 + 2));
            stuck.push(task.id);
        }
        let live = submit(&mut scheduler, 100);
        assert_eq!(scheduler.start_next_at(101).unwrap().snapshot, live);
        submit(&mut scheduler, 102);
        assert!(
            scheduler.start_next_at(103).is_none(),
            "a live task holds it"
        );
        // Past the detached allowance the stuck tasks hold the slot again.
        assert!(scheduler.cancel_at(live.id, 104));
        assert!(scheduler.start_next_at(105).is_none());
        scheduler.complete_at(stuck[0], 106, false);
        assert!(scheduler.start_next_at(107).is_some());
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
        // Only the running task's cancellation is a latency sample; the
        // queued one was retired before it ever started.
        assert_eq!(stats.cancellation_latency_samples, 1);
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

    #[test]
    fn queued_cancels_and_supersession_record_no_latency_sample() {
        let mut scheduler = Scheduler::new(SchedulerLimits::default());
        let queued = scheduler
            .submit_at(spec(TaskKind::Index, 1, Priority::Background), 1)
            .unwrap();
        assert!(scheduler.cancel_at(queued.id, 2));
        scheduler
            .submit_at(
                spec(TaskKind::Search, 1, Priority::Interactive).replace_older_generation(),
                3,
            )
            .unwrap();
        scheduler
            .submit_at(
                spec(TaskKind::Search, 2, Priority::Interactive).replace_older_generation(),
                4,
            )
            .unwrap();
        let stats = scheduler.stats();
        assert_eq!(stats.cancelled, 2);
        assert_eq!(stats.cancellation_latency_samples, 0);
    }

    #[test]
    fn started_task_without_work_releases_its_run_slot() {
        let workload = DeterministicWorkload::new(SchedulerLimits::default());
        {
            let mut state = crate::lock_util::recover(&workload.backend.state);
            state
                .scheduler
                .submit_at(spec(TaskKind::Index, 1, Priority::Background), 1)
                .unwrap();
        }
        assert!(!workload.run_next());
        assert_eq!(workload.stats().running, 0);

        let runtime = WorkloadRuntime::new(SchedulerLimits::default());
        {
            let mut state = crate::lock_util::recover(&runtime.state);
            state
                .scheduler
                .submit_at(spec(TaskKind::Index, 1, Priority::Background), 1)
                .unwrap();
        }
        runtime.pump();
        assert_eq!(runtime.stats().running, 0);
    }

    #[test]
    fn spawn_failure_does_not_abandon_the_rest_of_the_queue_while_work_runs() {
        let runtime = WorkloadRuntime::new(SchedulerLimits::default());
        let abandoned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        {
            let mut state = crate::lock_util::recover(&runtime.state);
            // A task already running elsewhere; its completion would re-pump.
            state
                .scheduler
                .submit_at(spec(TaskKind::Transfer, 1, Priority::Critical), 1)
                .unwrap();
            state.scheduler.start_next_at(2).unwrap();
            for _ in 0..2 {
                let snapshot = state
                    .scheduler
                    .submit_at(spec(TaskKind::Index, 1, Priority::Background), 3)
                    .unwrap();
                let abandoned = Arc::clone(&abandoned);
                state.pending.insert(
                    snapshot.id,
                    PendingWork::new(
                        Box::new(|_| {}),
                        Some(Box::new(move |_| {
                            abandoned.fetch_add(1, Ordering::SeqCst);
                        })),
                    ),
                );
            }
        }
        runtime.fail_spawns_for_test(usize::MAX);
        runtime.pump();

        assert_eq!(abandoned.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.stats().queued, 1);
    }

    fn interactive(kind: TaskKind, root: &str, generation: u64) -> TaskSpec {
        TaskSpec::new(kind, root, generation).priority(Priority::Interactive)
    }

    #[test]
    fn over_budget_estimate_is_backpressured() {
        let mut scheduler = Scheduler::new(SchedulerLimits {
            max_inflight_bytes: 100,
            ..SchedulerLimits::default()
        });
        assert_eq!(
            scheduler.submit_at(
                spec(TaskKind::Search, 1, Priority::Interactive).estimated_bytes(101),
                1
            ),
            Err(AdmissionError::ByteBudgetExceeded)
        );
        assert_eq!(scheduler.stats().backpressured, 1);
        assert_eq!(scheduler.stats().queued, 0);
        assert!(
            scheduler
                .submit_at(
                    spec(TaskKind::Search, 1, Priority::Interactive).estimated_bytes(100),
                    2
                )
                .is_ok()
        );
    }

    #[test]
    fn newer_generation_abandons_queued_work_as_superseded_once() {
        let runtime = DeterministicWorkload::new(SchedulerLimits::default());
        let handle = runtime.handle();
        let reasons = Arc::new(Mutex::new(Vec::new()));
        let reasons_for_job = Arc::clone(&reasons);
        let ran = Arc::new(AtomicBool::new(false));
        let ran_for_job = Arc::clone(&ran);
        let search = |generation| {
            spec(TaskKind::Search, generation, Priority::Interactive).replace_older_generation()
        };

        handle
            .submit_with_abandonment(
                search(1),
                |_| panic!("superseded queued work must not execute"),
                move |reason| crate::lock_util::recover(&reasons_for_job).push(reason),
            )
            .unwrap();
        handle
            .submit(search(2), move |_| {
                ran_for_job.store(true, Ordering::Release)
            })
            .unwrap();

        assert_eq!(
            *crate::lock_util::recover(&reasons),
            vec![AbandonReason::Superseded]
        );
        assert_eq!(runtime.stats().queued, 1);
        assert!(runtime.run_next());
        assert!(ran.load(Ordering::Acquire));
        assert!(!runtime.run_next());
        assert_eq!(crate::lock_util::recover(&reasons).len(), 1);
    }

    #[test]
    fn complete_at_ignores_queued_unknown_and_repeated_completions() {
        let mut scheduler = Scheduler::new(SchedulerLimits {
            max_running: 1,
            ..SchedulerLimits::default()
        });
        let first = scheduler
            .submit_at(spec(TaskKind::Search, 1, Priority::Interactive), 1)
            .unwrap();
        let queued = scheduler
            .submit_at(spec(TaskKind::Listing, 1, Priority::Interactive), 2)
            .unwrap();
        assert_eq!(scheduler.start_next_at(3).unwrap().snapshot, first);
        assert_eq!(
            scheduler.complete_at(queued.id, 4, true),
            CompletionDisposition::Unknown
        );
        assert_eq!(
            scheduler.complete_at(TaskId(999), 4, true),
            CompletionDisposition::Unknown
        );
        assert_eq!(
            scheduler.complete_at(first.id, 5, true),
            CompletionDisposition::Accepted
        );
        assert_eq!(
            scheduler.complete_at(first.id, 6, true),
            CompletionDisposition::Unknown
        );
        // The single slot was freed exactly once and the queued task is
        // still startable.
        assert_eq!(scheduler.start_next_at(7).unwrap().snapshot, queued);
        assert!(scheduler.start_next_at(8).is_none());
    }

    #[test]
    fn full_kind_lets_another_kind_start() {
        let mut limits = SchedulerLimits {
            max_running: 4,
            per_kind_running: [4; TaskKind::COUNT],
            ..SchedulerLimits::default()
        };
        limits.per_kind_running[TaskKind::Listing.index()] = 1;
        let mut scheduler = Scheduler::new(limits);
        let listing = scheduler
            .submit_at(interactive(TaskKind::Listing, "/a", 1), 1)
            .unwrap();
        let second_listing = scheduler
            .submit_at(interactive(TaskKind::Listing, "/b", 1), 2)
            .unwrap();
        let search = scheduler
            .submit_at(interactive(TaskKind::Search, "/a", 1), 3)
            .unwrap();
        assert_eq!(scheduler.start_next_at(4).unwrap().snapshot, listing);
        assert_eq!(scheduler.start_next_at(5).unwrap().snapshot, search);
        assert!(scheduler.start_next_at(6).is_none());
        scheduler.complete_at(listing.id, 7, true);
        assert_eq!(scheduler.start_next_at(8).unwrap().snapshot, second_listing);
    }

    #[test]
    fn equal_priorities_start_in_submission_order() {
        let mut scheduler = Scheduler::new(SchedulerLimits {
            max_running: 3,
            per_kind_running: [3; TaskKind::COUNT],
            ..SchedulerLimits::default()
        });
        let ids = (1..=3)
            .map(|tick| {
                scheduler
                    .submit_at(interactive(TaskKind::Search, "/a", tick), tick)
                    .unwrap()
                    .id
            })
            .collect::<Vec<_>>();
        let started = (4..=6)
            .map(|tick| scheduler.start_next_at(tick).unwrap().snapshot.id)
            .collect::<Vec<_>>();
        assert_eq!(started, ids);
    }

    #[test]
    fn supersession_is_scoped_to_kind_root_and_replace_flag() {
        let mut scheduler = Scheduler::new(SchedulerLimits::default());
        let mut submit = |spec: TaskSpec, tick| scheduler.submit_at(spec, tick).unwrap();
        let same = submit(
            interactive(TaskKind::Search, "/a", 1).replace_older_generation(),
            1,
        );
        let other_kind = submit(
            interactive(TaskKind::Preview, "/a", 1).replace_older_generation(),
            2,
        );
        let other_root = submit(
            interactive(TaskKind::Search, "/b", 1).replace_older_generation(),
            3,
        );
        let not_replacing = submit(interactive(TaskKind::Search, "/a", 1), 4);
        submit(
            interactive(TaskKind::Search, "/a", 2).replace_older_generation(),
            5,
        );

        let cancelled =
            |scheduler: &Scheduler, id: TaskId| scheduler.tasks[&id].state == TaskState::Cancelled;
        assert!(cancelled(&scheduler, same.id));
        assert!(!cancelled(&scheduler, other_kind.id));
        assert!(!cancelled(&scheduler, other_root.id));
        assert!(!cancelled(&scheduler, not_replacing.id));
        assert_eq!(scheduler.stats().cancelled, 1);
        assert_eq!(scheduler.stats().queued, 4);
    }

    #[test]
    fn equal_generation_resubmission_is_admitted_without_cancelling() {
        let mut scheduler = Scheduler::new(SchedulerLimits::default());
        let search = || spec(TaskKind::Search, 5, Priority::Interactive).replace_older_generation();
        let first = scheduler.submit_at(search(), 1).unwrap();
        let started = scheduler.start_next_at(2).unwrap();
        let second = scheduler.submit_at(search(), 3).unwrap();
        assert_ne!(first.id, second.id);
        assert!(!started.token.is_cancelled());
        assert_eq!(
            scheduler.complete_at(first.id, 4, true),
            CompletionDisposition::Accepted
        );
    }

    #[test]
    fn disconnected_root_rejects_descendants_but_not_siblings() {
        let mut scheduler = Scheduler::new(SchedulerLimits::default());
        scheduler.disconnect_at(Path::new("/project"), 1);
        assert_eq!(
            scheduler.submit_at(interactive(TaskKind::Listing, "/project/sub", 1), 2),
            Err(AdmissionError::RootDisconnected(PathBuf::from(
                "/project/sub"
            )))
        );
        assert!(
            scheduler
                .submit_at(interactive(TaskKind::Listing, "/project2", 1), 3)
                .is_ok()
        );
        assert_eq!(scheduler.stats().backpressured, 1);
    }

    #[test]
    fn panicking_job_fails_and_frees_its_slot_for_queued_work() {
        let runtime = WorkloadRuntime::new(SchedulerLimits {
            max_running: 1,
            ..SchedulerLimits::default()
        });
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        runtime
            .submit(
                spec(TaskKind::Search, 1, Priority::Interactive),
                move |_| {
                    let _ = release_rx.recv();
                    panic!("job panics");
                },
            )
            .unwrap();
        runtime
            .submit(
                spec(TaskKind::Listing, 1, Priority::Interactive),
                move |_| {
                    let _ = done_tx.send(());
                },
            )
            .unwrap();
        assert_eq!(runtime.stats().queued, 1);
        drop(release_tx);
        done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("queued job runs after the panic frees the slot");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while runtime.stats().running > 0 {
            assert!(std::time::Instant::now() < deadline, "slot never freed");
            std::thread::yield_now();
        }
        assert_eq!(runtime.stats().queued, 0);
    }
}
