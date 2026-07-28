//! Debounced, binding-checked directory probing for the go-to-path dialog.

use crate::pathname::{DirInputError, DirectoryProbePort, parse_dir_input};
use crate::workload::{
    AbandonReason, AdmissionError, Priority, TaskHandle, TaskKind, TaskSpec, WorkloadHandle,
};
use crate::workspace::ActivePanel;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

pub(crate) const PATH_PROBE_DEBOUNCE: Duration = Duration::from_millis(200);
const PATH_PROBE_POLL: Duration = Duration::from_millis(250);
const MAX_IN_FLIGHT_PROBES: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProbeStatus {
    Waiting,
    Checking,
    Valid,
    Error(DirInputError),
    WorkerFailed(String),
}

impl ProbeStatus {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Waiting => "Waiting to check folder".to_string(),
            Self::Checking => "Checking folder\u{2026}".to_string(),
            Self::Valid => "\u{2713} folder".to_string(),
            Self::Error(error) => error.to_string(),
            Self::WorkerFailed(message) => message.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProbeBinding {
    dialog_id: u64,
    generation: u64,
    raw_input: String,
    lexical_path: PathBuf,
}

#[derive(Debug)]
enum WorkerEvent {
    Completed {
        binding: ProbeBinding,
        outcome: Result<(), DirInputError>,
    },
    Cancelled {
        binding: ProbeBinding,
    },
    Abandoned {
        binding: ProbeBinding,
        reason: AbandonReason,
    },
}

impl WorkerEvent {
    fn binding(&self) -> &ProbeBinding {
        match self {
            Self::Completed { binding, .. }
            | Self::Cancelled { binding }
            | Self::Abandoned { binding, .. } => binding,
        }
    }
}

struct InFlightProbe {
    binding: ProbeBinding,
    task: TaskHandle,
    receiver: mpsc::Receiver<WorkerEvent>,
    cancellation_requested: bool,
}

impl InFlightProbe {
    fn cancel_once(&mut self) {
        if !self.cancellation_requested {
            self.task.cancel();
            self.cancellation_requested = true;
        }
    }
}

enum ProbeTerminal {
    Event(WorkerEvent),
    Disconnected,
}

pub(crate) struct PathProbeController {
    dialog_id: u64,
    generation: u64,
    home: PathBuf,
    raw_input: String,
    lexical_path: Option<PathBuf>,
    status: ProbeStatus,
    edited_at: f64,
    in_flight: Vec<InFlightProbe>,
}

impl PathProbeController {
    pub(crate) fn new(dialog_id: u64, raw_input: String, home: PathBuf, now: f64) -> Self {
        let parsed = parse_dir_input(&raw_input, &home);
        let (lexical_path, status) = match parsed {
            Ok(path) => (Some(path), ProbeStatus::Waiting),
            Err(error) => (None, ProbeStatus::Error(error)),
        };
        Self {
            dialog_id,
            generation: 1,
            home,
            raw_input,
            lexical_path,
            status,
            edited_at: now,
            in_flight: Vec::with_capacity(MAX_IN_FLIGHT_PROBES),
        }
    }

    #[cfg(test)]
    fn dialog_id(&self) -> u64 {
        self.dialog_id
    }

    pub(crate) fn input(&self) -> &str {
        &self.raw_input
    }

    pub(crate) fn input_mut(&mut self) -> &mut String {
        &mut self.raw_input
    }

    pub(crate) fn status(&self) -> &ProbeStatus {
        &self.status
    }

    pub(crate) fn input_changed(&mut self, now: f64) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("path probe generation space exhausted");
        for in_flight in &mut self.in_flight {
            in_flight.cancel_once();
        }
        match parse_dir_input(&self.raw_input, &self.home) {
            Ok(path) => {
                self.lexical_path = Some(path);
                self.status = ProbeStatus::Waiting;
            }
            Err(error) => {
                self.lexical_path = None;
                self.status = ProbeStatus::Error(error);
            }
        }
        self.edited_at = now;
    }

    #[cfg(test)]
    fn replace_input(&mut self, input: impl Into<String>, now: f64) {
        self.raw_input = input.into();
        self.input_changed(now);
    }

    pub(crate) fn validated_path(&self, exact_input: &str) -> Option<&Path> {
        (self.raw_input == exact_input && self.status == ProbeStatus::Valid)
            .then_some(self.lexical_path.as_deref())
            .flatten()
    }

    pub(crate) fn drive(
        &mut self,
        now: f64,
        workload: &WorkloadHandle,
        probe: Arc<dyn DirectoryProbePort>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.poll_terminals();
        if self.status != ProbeStatus::Waiting {
            return;
        }
        if now < self.edited_at + PATH_PROBE_DEBOUNCE.as_secs_f64() {
            return;
        }
        let Some(binding) = self.current_binding() else {
            return;
        };
        if self
            .in_flight
            .iter()
            .any(|in_flight| in_flight.binding == binding)
        {
            self.status = ProbeStatus::Checking;
            return;
        }
        if self.in_flight.len() >= MAX_IN_FLIGHT_PROBES {
            return;
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker_sender = sender.clone();
        let abandoned_sender = sender;
        let worker_binding = binding.clone();
        let abandoned_binding = binding.clone();
        let worker_notify = Arc::clone(&notify);
        let abandoned_notify = notify;
        let spec = TaskSpec::new(
            TaskKind::PathProbe,
            binding.lexical_path.clone(),
            self.generation,
        )
        .priority(Priority::Interactive);

        let submitted = workload.submit_with_abandonment(
            spec,
            move |token| {
                let event = if token.is_cancelled() {
                    WorkerEvent::Cancelled {
                        binding: worker_binding,
                    }
                } else {
                    let outcome = probe.probe(&worker_binding.lexical_path);
                    if token.is_cancelled() {
                        WorkerEvent::Cancelled {
                            binding: worker_binding,
                        }
                    } else {
                        WorkerEvent::Completed {
                            binding: worker_binding,
                            outcome,
                        }
                    }
                };
                let _ = worker_sender.try_send(event);
                worker_notify();
            },
            move |reason| {
                let _ = abandoned_sender.try_send(WorkerEvent::Abandoned {
                    binding: abandoned_binding,
                    reason,
                });
                abandoned_notify();
            },
        );

        match submitted {
            Ok(task) => {
                self.status = ProbeStatus::Checking;
                self.in_flight.push(InFlightProbe {
                    binding,
                    task,
                    receiver,
                    cancellation_requested: false,
                });
                self.poll_terminals();
            }
            Err(error) => {
                self.status = ProbeStatus::WorkerFailed(admission_message(&error));
            }
        }
    }

    pub(crate) fn repaint_after(&self, now: f64) -> Option<Duration> {
        match self.status {
            ProbeStatus::Waiting if self.in_flight.is_empty() => {
                let remaining = (self.edited_at + PATH_PROBE_DEBOUNCE.as_secs_f64() - now).max(0.0);
                Some(Duration::from_secs_f64(remaining))
            }
            ProbeStatus::Checking | ProbeStatus::Waiting => Some(PATH_PROBE_POLL),
            ProbeStatus::Valid | ProbeStatus::Error(_) | ProbeStatus::WorkerFailed(_) => None,
        }
    }

    fn current_binding(&self) -> Option<ProbeBinding> {
        Some(ProbeBinding {
            dialog_id: self.dialog_id,
            generation: self.generation,
            raw_input: self.raw_input.clone(),
            lexical_path: self.lexical_path.clone()?,
        })
    }

    fn poll_terminals(&mut self) {
        let current = self.current_binding();
        let mut current_terminal = None;
        let mut index = 0;
        while index < self.in_flight.len() {
            let terminal = match self.in_flight[index].receiver.try_recv() {
                Ok(event) => Some(ProbeTerminal::Event(event)),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(ProbeTerminal::Disconnected),
            };
            let Some(terminal) = terminal else {
                index += 1;
                continue;
            };
            let retired = self.in_flight.swap_remove(index);
            if current.as_ref() == Some(&retired.binding) {
                current_terminal = Some((retired.binding, terminal));
            }
        }

        let Some((binding, terminal)) = current_terminal else {
            return;
        };
        self.status = match terminal {
            ProbeTerminal::Event(event) if event.binding() == &binding => match event {
                WorkerEvent::Completed {
                    outcome: Ok(()), ..
                } => ProbeStatus::Valid,
                WorkerEvent::Completed {
                    outcome: Err(error),
                    ..
                } => ProbeStatus::Error(error),
                WorkerEvent::Cancelled { .. } => {
                    ProbeStatus::WorkerFailed("Folder check was cancelled".to_string())
                }
                WorkerEvent::Abandoned { reason, .. } => {
                    ProbeStatus::WorkerFailed(format!("Folder check could not run: {reason}"))
                }
            },
            ProbeTerminal::Event(_) => ProbeStatus::WorkerFailed(
                "Folder check worker returned a mismatched result".to_string(),
            ),
            ProbeTerminal::Disconnected => {
                ProbeStatus::WorkerFailed("Folder check worker stopped unexpectedly".to_string())
            }
        }
    }
}

impl Drop for PathProbeController {
    fn drop(&mut self) {
        for in_flight in &mut self.in_flight {
            in_flight.cancel_once();
        }
    }
}

fn admission_message(error: &AdmissionError) -> String {
    format!("Folder check could not start: {error}")
}

pub(crate) struct PathDialogState {
    pub(crate) opening_panel: ActivePanel,
    pub(crate) probe: PathProbeController,
}

impl PathDialogState {
    pub(crate) fn new(
        dialog_id: u64,
        opening_panel: ActivePanel,
        raw_input: String,
        home: PathBuf,
        now: f64,
    ) -> Self {
        Self {
            opening_panel,
            probe: PathProbeController::new(dialog_id, raw_input, home, now),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workload::{
        DeterministicWorkload, SchedulerLimits, SchedulerStats, TaskSnapshot, WorkloadBackend,
        WorkloadJob,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Condvar, Mutex, mpsc};
    use std::thread::ThreadId;
    use std::time::Instant;

    fn silent_notify() -> Arc<dyn Fn() + Send + Sync> {
        Arc::new(|| {})
    }

    #[derive(Default)]
    struct RecordingProbe {
        calls: AtomicUsize,
        thread: Mutex<Option<ThreadId>>,
    }

    impl DirectoryProbePort for RecordingProbe {
        fn probe(&self, _path: &Path) -> Result<(), DirInputError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            *crate::lock_util::recover(&self.thread) = Some(std::thread::current().id());
            Ok(())
        }
    }

    struct PanicProbe;

    impl DirectoryProbePort for PanicProbe {
        fn probe(&self, _path: &Path) -> Result<(), DirInputError> {
            panic!("scripted probe failure");
        }
    }

    #[derive(Default)]
    struct ProbeGate {
        open: Mutex<bool>,
        ready: Condvar,
    }

    impl ProbeGate {
        fn wait(&self) {
            let open = crate::lock_util::recover(&self.open);
            let (_open, timeout) = self
                .ready
                .wait_timeout_while(open, Duration::from_secs(2), |open| !*open)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert!(!timeout.timed_out(), "scripted path probe was not released");
        }

        fn release(&self) {
            *crate::lock_util::recover(&self.open) = true;
            self.ready.notify_all();
        }
    }

    struct BlockingProbe {
        calls: mpsc::Sender<PathBuf>,
        first: ProbeGate,
        second: ProbeGate,
    }

    impl BlockingProbe {
        fn new() -> (Arc<Self>, mpsc::Receiver<PathBuf>) {
            let (calls, receiver) = mpsc::channel();
            (
                Arc::new(Self {
                    calls,
                    first: ProbeGate::default(),
                    second: ProbeGate::default(),
                }),
                receiver,
            )
        }
    }

    impl DirectoryProbePort for BlockingProbe {
        fn probe(&self, path: &Path) -> Result<(), DirInputError> {
            let _ = self.calls.send(path.to_path_buf());
            if path == Path::new("first") {
                self.first.wait();
            } else if path == Path::new("second") {
                self.second.wait();
            }
            Ok(())
        }
    }

    fn controller(input: &str) -> PathProbeController {
        PathProbeController::new(41, input.to_string(), PathBuf::from("/home/test"), 0.0)
    }

    #[test]
    fn debounce_submits_once_and_unchanged_frames_do_no_more_io() {
        let runtime = DeterministicWorkload::new(SchedulerLimits::default());
        let probe = Arc::new(RecordingProbe::default());
        let mut controller = controller(".");

        controller.drive(0.199, &runtime.handle(), probe.clone(), silent_notify());
        assert_eq!(runtime.stats().queued, 0);
        controller.drive(0.2, &runtime.handle(), probe.clone(), silent_notify());
        controller.drive(0.9, &runtime.handle(), probe.clone(), silent_notify());
        assert_eq!(runtime.stats().queued, 1);
        assert!(runtime.run_next());
        controller.drive(0.9, &runtime.handle(), probe.clone(), silent_notify());
        assert_eq!(controller.status(), &ProbeStatus::Valid);

        for frame in 0..100 {
            controller.drive(
                1.0 + f64::from(frame),
                &runtime.handle(),
                probe.clone(),
                silent_notify(),
            );
        }
        assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn blocked_probe_allows_one_latest_wins_replacement_and_bounds_rapid_edits() {
        let runtime = crate::workload::WorkloadRuntime::new(SchedulerLimits::default());
        let workload = WorkloadHandle::from_runtime(runtime.clone());
        let (probe, calls) = BlockingProbe::new();
        let mut controller = controller("first");
        controller.drive(0.2, &workload, probe.clone(), silent_notify());
        assert_eq!(
            calls.recv_timeout(Duration::from_secs(1)).unwrap(),
            PathBuf::from("first")
        );

        controller.replace_input("second", 0.21);
        controller.drive(0.42, &workload, probe.clone(), silent_notify());
        assert_eq!(
            calls.recv_timeout(Duration::from_secs(1)).unwrap(),
            PathBuf::from("second"),
            "the second workload lane must not wait behind blocked metadata"
        );
        for edit in 0..100 {
            controller.replace_input(format!("latest-{edit}"), 0.42 + f64::from(edit) / 10_000.0);
        }
        controller.drive(1.0, &workload, probe.clone(), silent_notify());
        assert_eq!(controller.in_flight.len(), MAX_IN_FLIGHT_PROBES);
        assert_eq!(controller.repaint_after(1.0), Some(PATH_PROBE_POLL));
        assert!(matches!(
            calls.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        probe.second.release();
        let deadline = Instant::now() + Duration::from_secs(1);
        let submitted = loop {
            controller.drive(1.0, &workload, probe.clone(), silent_notify());
            match calls.recv_timeout(Duration::from_millis(10)) {
                Ok(path) => break path,
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
                Err(error) => panic!("latest path probe was not submitted: {error}"),
            }
        };
        assert_eq!(submitted, PathBuf::from("latest-99"));

        let deadline = Instant::now() + Duration::from_secs(1);
        while controller.status() != &ProbeStatus::Valid && Instant::now() < deadline {
            controller.drive(1.0, &workload, probe.clone(), silent_notify());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            controller.validated_path("latest-99"),
            Some(Path::new("latest-99"))
        );
        assert_eq!(controller.repaint_after(1.0), None);

        probe.first.release();
        let deadline = Instant::now() + Duration::from_secs(1);
        while (!controller.in_flight.is_empty() || runtime.stats().running != 0)
            && Instant::now() < deadline
        {
            controller.drive(1.0, &workload, probe.clone(), silent_notify());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(controller.in_flight.is_empty());
        assert_eq!(runtime.stats().running, 0);
        assert!(calls.try_recv().is_err());
        assert_eq!(controller.status(), &ProbeStatus::Valid);
    }

    #[test]
    fn stale_a_b_a_result_cannot_validate_the_new_a() {
        let runtime = DeterministicWorkload::new(SchedulerLimits::default());
        let probe = Arc::new(RecordingProbe::default());
        let mut controller = controller("A");
        controller.drive(0.2, &runtime.handle(), probe.clone(), silent_notify());

        assert!(runtime.run_next_after_dequeue(|| {
            controller.replace_input("B", 0.31);
            controller.replace_input("A", 0.32);
        }));
        controller.drive(0.4, &runtime.handle(), probe.clone(), silent_notify());
        assert_eq!(controller.status(), &ProbeStatus::Waiting);
        assert!(controller.validated_path("A").is_none());

        controller.drive(0.521, &runtime.handle(), probe.clone(), silent_notify());
        assert_eq!(controller.status(), &ProbeStatus::Checking);
        assert!(controller.validated_path("A").is_none());
        assert!(runtime.run_next());
        controller.drive(0.521, &runtime.handle(), probe, silent_notify());
        assert_eq!(controller.status(), &ProbeStatus::Valid);
        assert_eq!(controller.validated_path("A"), Some(Path::new("A")));
    }

    #[test]
    fn dropping_and_reopening_rejects_the_old_dialog_task() {
        let runtime = DeterministicWorkload::new(SchedulerLimits::default());
        let probe = Arc::new(RecordingProbe::default());
        let mut old = controller("old");
        old.drive(0.2, &runtime.handle(), probe.clone(), silent_notify());
        drop(old);
        assert!(!runtime.run_next());

        let mut reopened =
            PathProbeController::new(42, "new".to_string(), PathBuf::from("/home/test"), 0.0);
        reopened.drive(0.2, &runtime.handle(), probe.clone(), silent_notify());
        assert!(runtime.run_next());
        reopened.drive(0.2, &runtime.handle(), probe, silent_notify());
        assert_eq!(reopened.dialog_id(), 42);
        assert_eq!(reopened.status(), &ProbeStatus::Valid);
    }

    #[test]
    fn dialog_context_keeps_its_opening_panel_and_home_snapshot() {
        let external_active_panel = ActivePanel::Right;
        let mut state = PathDialogState::new(
            7,
            ActivePanel::Left,
            "~".to_string(),
            PathBuf::from("/captured-home"),
            0.0,
        );
        assert_eq!(state.opening_panel, ActivePanel::Left);
        assert_ne!(state.opening_panel, external_active_panel);
        assert_eq!(
            state.probe.current_binding().unwrap().lexical_path,
            PathBuf::from("/captured-home")
        );

        state.probe.replace_input("~/Documents", 1.0);
        assert_eq!(
            state.probe.current_binding().unwrap().lexical_path,
            PathBuf::from("/captured-home/Documents")
        );
    }

    #[test]
    fn admission_failure_and_worker_panic_are_terminal() {
        let blocked = DeterministicWorkload::new(SchedulerLimits {
            max_queued: 0,
            ..SchedulerLimits::default()
        });
        let mut rejected = controller(".");
        rejected.drive(
            0.2,
            &blocked.handle(),
            Arc::new(RecordingProbe::default()),
            silent_notify(),
        );
        assert!(matches!(
            rejected.status(),
            ProbeStatus::WorkerFailed(message) if message.contains("queue is full")
        ));

        let runtime = DeterministicWorkload::new(SchedulerLimits::default());
        let mut panicked = controller(".");
        panicked.drive(
            0.2,
            &runtime.handle(),
            Arc::new(PanicProbe),
            silent_notify(),
        );
        assert!(runtime.run_next());
        panicked.drive(
            0.3,
            &runtime.handle(),
            Arc::new(PanicProbe),
            silent_notify(),
        );
        assert!(matches!(
            panicked.status(),
            ProbeStatus::WorkerFailed(message) if message.contains("stopped unexpectedly")
        ));
    }

    struct DisconnectingBackend;

    impl WorkloadBackend for DisconnectingBackend {
        fn submit_boxed(
            self: Arc<Self>,
            spec: TaskSpec,
            work: WorkloadJob,
            on_abandoned: Option<crate::workload::AbandonmentCallback>,
        ) -> Result<TaskSnapshot, AdmissionError> {
            drop(work);
            drop(on_abandoned);
            Ok(TaskSnapshot {
                id: crate::workload::TaskId(1),
                kind: spec.kind,
                root: spec.root,
                generation: spec.generation,
                priority: spec.priority,
                estimated_bytes: spec.estimated_bytes,
                submitted_tick: 1,
                replace_older_generation: spec.replace_older_generation,
            })
        }

        fn cancel_task(self: Arc<Self>, _id: crate::workload::TaskId) -> bool {
            false
        }

        fn stats(&self) -> SchedulerStats {
            SchedulerStats::default()
        }
    }

    #[derive(Default)]
    struct ImmediateBackend {
        next_id: AtomicUsize,
        submitted: Mutex<Vec<TaskSpec>>,
    }

    impl WorkloadBackend for ImmediateBackend {
        fn submit_boxed(
            self: Arc<Self>,
            spec: TaskSpec,
            work: WorkloadJob,
            on_abandoned: Option<crate::workload::AbandonmentCallback>,
        ) -> Result<TaskSnapshot, AdmissionError> {
            let id = self.next_id.fetch_add(1, Ordering::SeqCst) as u64 + 1;
            let snapshot = TaskSnapshot {
                id: crate::workload::TaskId(id),
                kind: spec.kind,
                root: spec.root.clone(),
                generation: spec.generation,
                priority: spec.priority,
                estimated_bytes: spec.estimated_bytes,
                submitted_tick: id,
                replace_older_generation: spec.replace_older_generation,
            };
            crate::lock_util::recover(&self.submitted).push(spec);

            // This backend intentionally completes before submit returns.
            work(crate::workload::CancellationToken::new());
            drop(on_abandoned);
            Ok(snapshot)
        }

        fn cancel_task(self: Arc<Self>, _id: crate::workload::TaskId) -> bool {
            false
        }

        fn stats(&self) -> SchedulerStats {
            SchedulerStats::default()
        }
    }

    #[test]
    fn disconnected_worker_channel_is_terminal() {
        let workload = WorkloadHandle::new(Arc::new(DisconnectingBackend));
        let mut controller = controller(".");
        controller.drive(
            0.2,
            &workload,
            Arc::new(RecordingProbe::default()),
            silent_notify(),
        );
        assert!(matches!(
            controller.status(),
            ProbeStatus::WorkerFailed(message) if message.contains("stopped unexpectedly")
        ));
    }

    #[test]
    fn synchronous_completion_is_observed_without_scheduler_freshness_keys() {
        let backend = Arc::new(ImmediateBackend::default());
        let workload = WorkloadHandle::new(backend.clone());
        let probe = Arc::new(RecordingProbe::default());

        for dialog_id in 1..=100 {
            let mut controller = PathProbeController::new(
                dialog_id,
                format!("path-{dialog_id}"),
                PathBuf::from("/home/test"),
                0.0,
            );
            controller.drive(0.2, &workload, probe.clone(), silent_notify());
            assert_eq!(controller.status(), &ProbeStatus::Valid);
            assert!(controller.in_flight.is_empty());
        }

        let submitted = crate::lock_util::recover(&backend.submitted);
        assert_eq!(submitted.len(), 100);
        assert!(
            submitted.iter().all(|spec| !spec.replace_older_generation),
            "dialog-local bindings must not grow Scheduler::active_generations"
        );
    }

    #[test]
    fn production_runtime_runs_probe_off_the_caller_thread() {
        let runtime = crate::workload::WorkloadRuntime::new(SchedulerLimits::default());
        let workload = WorkloadHandle::from_runtime(runtime);
        let probe = Arc::new(RecordingProbe::default());
        let mut controller = controller(".");
        let caller = std::thread::current().id();
        let (notify_tx, notify_rx) = mpsc::channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = notify_tx.send(());
        });
        controller.drive(0.2, &workload, probe.clone(), notify);
        notify_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker notification");

        let deadline = Instant::now() + Duration::from_secs(1);
        while controller.status() == &ProbeStatus::Checking && Instant::now() < deadline {
            controller.drive(0.3, &workload, probe.clone(), silent_notify());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(controller.status(), &ProbeStatus::Valid);
        assert_ne!(
            *crate::lock_util::recover(&probe.thread),
            Some(caller),
            "filesystem probe must not execute on the caller/UI thread"
        );
    }

    #[test]
    fn same_frame_edit_invalidates_validity_before_enter_is_read() {
        let runtime = DeterministicWorkload::new(SchedulerLimits::default());
        let probe = Arc::new(RecordingProbe::default());
        let mut controller = controller("valid");
        controller.drive(0.2, &runtime.handle(), probe.clone(), silent_notify());
        assert!(runtime.run_next());
        controller.drive(0.2, &runtime.handle(), probe, silent_notify());
        assert!(controller.validated_path("valid").is_some());

        *controller.input_mut() = "edited".to_string();
        controller.input_changed(0.21);
        assert!(controller.validated_path("edited").is_none());
        assert_eq!(controller.status(), &ProbeStatus::Waiting);
    }
}
