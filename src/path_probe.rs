//! Debounced, generation-bound directory probing for the go-to-path dialog.

use crate::pathname::{DirInputError, DirectoryProbePort, parse_dir_input};
use crate::workload::{
    AbandonReason, AdmissionError, Priority, TaskHandle, TaskKind, TaskSpec, WorkloadHandle,
};
use crate::workspace::ActivePanel;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

pub(crate) const PATH_PROBE_DEBOUNCE: Duration = Duration::from_millis(200);
const PATH_PROBE_POLL: Duration = Duration::from_millis(50);

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
}

pub(crate) struct PathProbeController {
    dialog_id: u64,
    generation: u64,
    home: PathBuf,
    raw_input: String,
    lexical_path: Option<PathBuf>,
    status: ProbeStatus,
    edited_at: f64,
    in_flight: Option<InFlightProbe>,
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
            in_flight: None,
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
        if let Some(in_flight) = &self.in_flight {
            in_flight.task.cancel();
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
        self.poll_terminal();
        if self.in_flight.is_some() || self.status != ProbeStatus::Waiting {
            return;
        }
        if now < self.edited_at + PATH_PROBE_DEBOUNCE.as_secs_f64() {
            return;
        }
        let Some(binding) = self.current_binding() else {
            return;
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker_sender = sender.clone();
        let abandoned_sender = sender;
        let worker_binding = binding.clone();
        let abandoned_binding = binding.clone();
        let worker_notify = Arc::clone(&notify);
        let abandoned_notify = notify;
        let spec = TaskSpec::new(
            TaskKind::PathProbe,
            PathBuf::from(format!(".commander-path-probe/{}", self.dialog_id)),
            self.generation,
        )
        .priority(Priority::Interactive)
        .replace_older_generation();

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
                self.in_flight = Some(InFlightProbe {
                    binding,
                    task,
                    receiver,
                });
                self.poll_terminal();
            }
            Err(error) => {
                self.status = ProbeStatus::WorkerFailed(admission_message(&error));
            }
        }
    }

    pub(crate) fn repaint_after(&self, now: f64) -> Option<Duration> {
        match self.status {
            ProbeStatus::Waiting if self.in_flight.is_none() => {
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

    fn poll_terminal(&mut self) {
        let Some(in_flight) = self.in_flight.as_ref() else {
            return;
        };
        let terminal = match in_flight.receiver.try_recv() {
            Ok(event) => Some(Ok(event)),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(())),
        };
        let Some(terminal) = terminal else {
            return;
        };
        let in_flight = self.in_flight.take().expect("in-flight probe exists");
        let is_current = self.current_binding().as_ref() == Some(&in_flight.binding);
        match terminal {
            Ok(event) if is_current && event.binding() == &in_flight.binding => {
                self.status = match event {
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
                };
            }
            Err(()) if is_current => {
                self.status = ProbeStatus::WorkerFailed(
                    "Folder check worker stopped unexpectedly".to_string(),
                );
            }
            Ok(_) | Err(()) => {}
        }
    }
}

impl Drop for PathProbeController {
    fn drop(&mut self) {
        if let Some(in_flight) = &self.in_flight {
            in_flight.task.cancel();
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
    use std::sync::{Mutex, mpsc};
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
    fn rapid_edits_keep_one_trailing_submission() {
        let runtime = DeterministicWorkload::new(SchedulerLimits::default());
        let probe = Arc::new(RecordingProbe::default());
        let mut controller = controller("first");
        controller.drive(0.2, &runtime.handle(), probe.clone(), silent_notify());
        assert_eq!(runtime.stats().queued, 1);

        for edit in 0..100 {
            controller.replace_input(format!("path-{edit}"), 0.21 + f64::from(edit) / 1_000.0);
        }
        assert_eq!(runtime.stats().queued, 0);
        controller.drive(1.0, &runtime.handle(), probe.clone(), silent_notify());
        assert_eq!(runtime.stats().queued, 1);
        controller.drive(1.1, &runtime.handle(), probe, silent_notify());
        assert_eq!(runtime.stats().queued, 1);
    }

    #[test]
    fn stale_a_b_a_result_cannot_validate_the_new_a() {
        let runtime = DeterministicWorkload::new(SchedulerLimits::default());
        let probe = Arc::new(RecordingProbe::default());
        let mut controller = controller("A");
        controller.drive(0.2, &runtime.handle(), probe.clone(), silent_notify());

        assert!(runtime.run_next_after_dequeue(|| {
            controller.replace_input("B", 0.21);
            controller.replace_input("A", 0.22);
        }));
        controller.drive(0.5, &runtime.handle(), probe.clone(), silent_notify());
        assert_eq!(controller.status(), &ProbeStatus::Checking);
        assert!(controller.validated_path("A").is_none());
        assert!(runtime.run_next());
        controller.drive(0.5, &runtime.handle(), probe, silent_notify());
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
