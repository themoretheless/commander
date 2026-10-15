//! Off-thread directory listing for navigation and refresh.
//!
//! Follows the path-probe controller shape: generation-checked bindings,
//! workload admission via `TaskKind::Listing`, and publish-only-when-current.

use crate::workload::{AbandonReason, Priority, TaskHandle, TaskKind, TaskSpec, WorkloadHandle};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use super::watcher::ReconciliationTicket;
use super::{DirStatus, DirectoryRead, Notify};

const MAX_IN_FLIGHT_LISTINGS: usize = 2;

/// Superseded listings still blocked in `read_dir` (a hung mount) do not
/// count toward [`MAX_IN_FLIGHT_LISTINGS`], so they cannot leave a new
/// navigation stuck on Loading; this separately bounds how many may pile up.
const MAX_CANCELLED_LISTINGS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ListingBinding {
    pub path: PathBuf,
    pub show_hidden: bool,
    pub generation: u64,
}

#[derive(Debug, Clone)]
pub(super) enum PendingFocus {
    None,
    Remembered {
        cursor_path: Option<PathBuf>,
        scroll_anchor: usize,
    },
    NamedChild(String),
}

#[allow(dead_code)]
enum WorkerEvent {
    Completed {
        binding: ListingBinding,
        read: DirectoryRead,
    },
    Cancelled {
        binding: ListingBinding,
    },
    Abandoned {
        binding: ListingBinding,
        reason: AbandonReason,
    },
}

struct InFlightListing {
    binding: ListingBinding,
    task: TaskHandle,
    receiver: mpsc::Receiver<WorkerEvent>,
    cancellation_requested: bool,
}

impl InFlightListing {
    fn cancel_once(&mut self) {
        if !self.cancellation_requested {
            self.task.cancel();
            self.cancellation_requested = true;
        }
    }
}

pub(super) struct ListingReady {
    pub binding: ListingBinding,
    pub read: DirectoryRead,
    pub ticket: Option<ReconciliationTicket>,
    pub focus: PendingFocus,
    /// The user asked for this listing (navigation, refresh): its size
    /// recompute may re-walk expensive folders. Watcher re-lists may not.
    pub forced_sizes: bool,
    /// `Some(recovered_gap)` when a watcher reconciliation asked for this
    /// listing, so the panel can record that provenance once it applies.
    pub watcher_gap: Option<bool>,
}

pub(super) struct ListingJobController {
    generation: u64,
    desired: Option<ListingBinding>,
    ticket: Option<ReconciliationTicket>,
    focus: PendingFocus,
    forced_sizes: bool,
    /// See [`ListingReady::watcher_gap`].
    watcher_gap: Option<bool>,
    in_flight: Vec<InFlightListing>,
    ready: Option<ListingReady>,
    /// True while the desired binding still needs a worker result.
    awaiting: bool,
}

impl Default for ListingJobController {
    fn default() -> Self {
        Self {
            generation: 0,
            desired: None,
            ticket: None,
            focus: PendingFocus::None,
            forced_sizes: false,
            watcher_gap: None,
            in_flight: Vec::new(),
            ready: None,
            awaiting: false,
        }
    }
}

impl ListingJobController {
    pub(super) fn request(
        &mut self,
        path: PathBuf,
        show_hidden: bool,
        ticket: Option<ReconciliationTicket>,
        focus: PendingFocus,
    ) -> ListingBinding {
        self.request_with(path, show_hidden, ticket, focus, true)
    }

    fn request_with(
        &mut self,
        path: PathBuf,
        show_hidden: bool,
        ticket: Option<ReconciliationTicket>,
        focus: PendingFocus,
        forced_sizes: bool,
    ) -> ListingBinding {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("listing generation space exhausted");
        for in_flight in &mut self.in_flight {
            in_flight.cancel_once();
        }
        // A plain re-read of the folder still being loaded (refresh, watcher)
        // keeps where navigation wanted the cursor to land.
        let focus = match focus {
            PendingFocus::None
                if self
                    .desired
                    .as_ref()
                    .is_some_and(|desired| desired.path == path) =>
            {
                match self.ready.take() {
                    Some(ready) => ready.focus,
                    None => std::mem::replace(&mut self.focus, PendingFocus::None),
                }
            }
            focus => focus,
        };
        let binding = ListingBinding {
            path,
            show_hidden,
            generation: self.generation,
        };
        self.desired = Some(binding.clone());
        self.ticket = ticket;
        self.focus = focus;
        self.forced_sizes = forced_sizes;
        self.watcher_gap = None;
        self.ready = None;
        self.awaiting = true;
        binding
    }

    /// A watcher-driven re-list of `path`. The watcher repeats its ticket
    /// every poll until a snapshot acknowledges it, so while a read of the
    /// same binding is still running (or its result is not yet published)
    /// this leaves that read, its ticket and its pending focus alone:
    /// restarting would cancel slow reads every frame. A change that raced
    /// the running read keeps the watcher's generation ahead of the old
    /// ticket, so the next poll re-lists once this result lands.
    /// `recovered_gap` marks a re-list after the watcher lost events.
    pub(super) fn reconcile(
        &mut self,
        path: PathBuf,
        show_hidden: bool,
        ticket: ReconciliationTicket,
        recovered_gap: bool,
    ) {
        let pending_same = (self.awaiting || self.ready.is_some())
            && self
                .desired
                .as_ref()
                .is_some_and(|desired| desired.path == path && desired.show_hidden == show_hidden);
        if !pending_same {
            self.request_with(path, show_hidden, Some(ticket), PendingFocus::None, false);
            self.watcher_gap = Some(recovered_gap);
        } else {
            // A user-requested listing (no watcher attribution yet) still
            // answers a gap recovery: claim it so the recovery is counted.
            let slot = match self.ready.as_mut() {
                Some(ready) => &mut ready.watcher_gap,
                None => &mut self.watcher_gap,
            };
            match slot {
                Some(gap) => *gap |= recovered_gap,
                None if recovered_gap => *slot = Some(true),
                None => {}
            }
        }
    }

    /// Where the cursor should land once the pending listing publishes.
    pub(super) fn pending_focus(&self) -> &PendingFocus {
        self.ready
            .as_ref()
            .map_or(&self.focus, |ready| &ready.focus)
    }

    pub(super) fn set_focus(&mut self, focus: PendingFocus) {
        self.focus = focus;
    }

    pub(super) const fn is_awaiting(&self) -> bool {
        self.awaiting
    }

    pub(super) const fn desired_binding(&self) -> Option<&ListingBinding> {
        self.desired.as_ref()
    }

    pub(super) fn drive(
        &mut self,
        workload: &WorkloadHandle,
        notify: Notify,
        read_dir: impl Fn(&Path, bool) -> DirectoryRead + Send + Sync + Clone + 'static,
    ) {
        self.poll_terminals();
        let Some(binding) = self.desired.clone() else {
            return;
        };
        if !self.awaiting {
            return;
        }
        if self
            .in_flight
            .iter()
            .any(|in_flight| in_flight.binding == binding)
        {
            return;
        }
        let cancelled = self
            .in_flight
            .iter()
            .filter(|in_flight| in_flight.cancellation_requested)
            .count();
        if self.in_flight.len() - cancelled >= MAX_IN_FLIGHT_LISTINGS
            || cancelled >= MAX_CANCELLED_LISTINGS
        {
            return;
        }

        let (sender, receiver) = mpsc::sync_channel(1);
        let worker_sender = sender.clone();
        let abandoned_sender = sender;
        let worker_binding = binding.clone();
        let abandoned_binding = binding.clone();
        let worker_notify = Arc::clone(&notify);
        let abandoned_notify = notify;
        let worker_read = read_dir;
        let spec = TaskSpec::new(TaskKind::Listing, binding.path.clone(), binding.generation)
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
                    let read = worker_read(&worker_binding.path, worker_binding.show_hidden);
                    if token.is_cancelled() {
                        WorkerEvent::Cancelled {
                            binding: worker_binding,
                        }
                    } else {
                        WorkerEvent::Completed {
                            binding: worker_binding,
                            read,
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
                self.in_flight.push(InFlightListing {
                    binding,
                    task,
                    receiver,
                    cancellation_requested: false,
                });
                self.poll_terminals();
            }
            Err(error) => {
                log::warn!(
                    target: "commander::listing",
                    "listing {} was not admitted: {error:?}",
                    binding.path.display()
                );
                self.finish(binding, DirectoryRead::Incomplete(DirStatus::Unavailable));
            }
        }
    }

    pub(super) fn take_ready(&mut self) -> Option<ListingReady> {
        self.poll_terminals();
        self.ready.take()
    }

    /// The result for the desired binding, carrying its pending ticket,
    /// focus and size policy.
    fn settle(&mut self, binding: ListingBinding, read: DirectoryRead) -> ListingReady {
        ListingReady {
            binding,
            read,
            ticket: self.ticket.take(),
            focus: std::mem::replace(&mut self.focus, PendingFocus::None),
            forced_sizes: self.forced_sizes,
            watcher_gap: self.watcher_gap.take(),
        }
    }

    /// Publish `read` as the desired binding's result; nothing more is
    /// awaited for it.
    fn finish(&mut self, binding: ListingBinding, read: DirectoryRead) {
        self.ready = Some(self.settle(binding, read));
        self.awaiting = false;
    }

    fn poll_terminals(&mut self) {
        let current = self.desired.clone();
        let mut index = 0;
        while index < self.in_flight.len() {
            let terminal = match self.in_flight[index].receiver.try_recv() {
                Ok(event) => Some(event),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(WorkerEvent::Abandoned {
                    binding: self.in_flight[index].binding.clone(),
                    reason: AbandonReason::Disconnected,
                }),
            };
            let Some(event) = terminal else {
                index += 1;
                continue;
            };
            let retired = self.in_flight.swap_remove(index);
            if current.as_ref() != Some(&retired.binding) {
                continue;
            }
            match event {
                WorkerEvent::Completed { binding, read } if binding == retired.binding => {
                    self.finish(binding, read);
                }
                WorkerEvent::Completed { .. } | WorkerEvent::Abandoned { .. } => {
                    if let WorkerEvent::Abandoned { reason, .. } = &event {
                        log::warn!(
                            target: "commander::listing",
                            "listing {} was abandoned: {reason}",
                            retired.binding.path.display()
                        );
                    }
                    self.finish(
                        retired.binding,
                        DirectoryRead::Incomplete(DirStatus::Unavailable),
                    );
                }
                WorkerEvent::Cancelled { .. } => {
                    // Keep awaiting so drive() can admit a fresh attempt for
                    // the still-desired binding (or a newer request already
                    // replaced desired and will submit itself).
                    if self.desired.as_ref() == Some(&retired.binding) {
                        self.awaiting = true;
                    }
                }
            }
        }
    }
}

impl Drop for ListingJobController {
    fn drop(&mut self) {
        for in_flight in &mut self.in_flight {
            in_flight.cancel_once();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use crate::workload::{SchedulerLimits, WorkloadRuntime};
    use std::time::{Duration, Instant};

    #[test]
    fn newer_request_replaces_desired_binding() {
        let mut job = ListingJobController::default();
        let first = job.request(PathBuf::from("/tmp"), false, None, PendingFocus::None);
        let second = job.request(
            PathBuf::from("/var"),
            false,
            None,
            PendingFocus::NamedChild("x".into()),
        );
        assert_ne!(first.generation, second.generation);
        assert_eq!(
            job.desired_binding().map(|binding| binding.path.as_path()),
            Some(Path::new("/var"))
        );
        assert!(job.is_awaiting());
    }

    fn named(focus: &PendingFocus) -> Option<&str> {
        match focus {
            PendingFocus::NamedChild(name) => Some(name),
            _ => None,
        }
    }

    #[test]
    fn plain_rerequest_of_pending_folder_keeps_its_focus() {
        let mut job = ListingJobController::default();
        job.request(
            PathBuf::from("/parent"),
            false,
            None,
            PendingFocus::NamedChild("child".into()),
        );
        job.request(PathBuf::from("/parent"), false, None, PendingFocus::None);
        assert_eq!(named(&job.focus), Some("child"));

        // A different folder starts clean.
        job.request(PathBuf::from("/other"), false, None, PendingFocus::None);
        assert!(matches!(job.focus, PendingFocus::None));
    }

    #[test]
    fn repeated_watcher_ticket_leaves_the_pending_read_running() {
        let mut watcher = super::super::watcher::DirectoryWatcherState::default();
        watcher.activate_test_binding(Path::new("/parent"));
        let ticket = watcher.snapshot_ticket().unwrap();

        let mut job = ListingJobController::default();
        let first = job.request(
            PathBuf::from("/parent"),
            false,
            Some(ticket.clone()),
            PendingFocus::NamedChild("child".into()),
        );
        // The watcher hands out the same unacknowledged ticket every frame.
        job.reconcile(PathBuf::from("/parent"), false, ticket.clone(), false);
        job.reconcile(PathBuf::from("/parent"), false, ticket.clone(), false);
        assert_eq!(job.desired_binding(), Some(&first));
        assert_eq!(named(&job.focus), Some("child"));
        assert_eq!(job.ticket.as_ref(), Some(&ticket));

        // Hidden-file toggles are a different binding and do re-list.
        job.reconcile(PathBuf::from("/parent"), true, ticket, false);
        assert_ne!(job.desired_binding(), Some(&first));
    }

    #[test]
    fn completed_listing_for_current_binding_is_ready() {
        let runtime = WorkloadRuntime::new(SchedulerLimits::default());
        let workload = WorkloadHandle::from_runtime(runtime);
        let notify: Notify = Arc::new(|| {});
        let mut job = ListingJobController::default();
        let dir = TempDir::new();
        job.request(
            dir.path().to_path_buf(),
            true,
            None,
            PendingFocus::Remembered {
                cursor_path: None,
                scroll_anchor: 0,
            },
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let ready =
            loop {
                job.drive(&workload, Arc::clone(&notify), |path, _show_hidden| {
                    match std::fs::read_dir(path) {
                        Ok(_) => DirectoryRead::Complete(Vec::new()),
                        Err(error) => {
                            DirectoryRead::Incomplete(super::super::status_for_read_error(&error))
                        }
                    }
                });
                if let Some(ready) = job.take_ready() {
                    break ready;
                }
                assert!(Instant::now() < deadline, "timeout waiting for listing");
                std::thread::sleep(Duration::from_millis(5));
            };
        assert_eq!(ready.binding.path, dir.path());
        assert!(matches!(ready.read, DirectoryRead::Complete(_)));
        assert!(matches!(ready.focus, PendingFocus::Remembered { .. }));
    }

    #[test]
    fn only_user_requested_listings_force_size_rewalks() {
        let mut watcher = super::super::watcher::DirectoryWatcherState::default();
        watcher.activate_test_binding(Path::new("/parent"));
        let ticket = watcher.snapshot_ticket().unwrap();
        let mut job = ListingJobController::default();

        job.reconcile(PathBuf::from("/parent"), false, ticket.clone(), false);
        assert!(!job.forced_sizes, "a watcher re-list is background work");

        job.request(PathBuf::from("/parent"), false, None, PendingFocus::None);
        assert!(job.forced_sizes);
        // A watcher ticket arriving while the user's read runs keeps it.
        job.reconcile(PathBuf::from("/parent"), false, ticket, false);
        assert!(job.forced_sizes);
        let binding = job.desired_binding().unwrap().clone();
        let ready = job.settle(binding, DirectoryRead::Incomplete(DirStatus::Gone));
        assert!(ready.forced_sizes);
    }

    #[test]
    fn watcher_listing_carries_its_provenance_to_the_result() {
        let mut watcher = super::super::watcher::DirectoryWatcherState::default();
        watcher.activate_test_binding(Path::new("/parent"));
        let ticket = watcher.snapshot_ticket().unwrap();
        let mut job = ListingJobController::default();

        // The worker result lands frames after poll_fs_changes asked for it,
        // so the result itself must say the watcher requested it.
        job.reconcile(PathBuf::from("/parent"), false, ticket.clone(), false);
        job.reconcile(PathBuf::from("/parent"), false, ticket.clone(), true);
        let binding = job.desired_binding().unwrap().clone();
        let ready = job.settle(binding, DirectoryRead::Incomplete(DirStatus::Gone));
        assert_eq!(ready.watcher_gap, Some(true));

        // A user-requested listing is not attributed to the watcher, even
        // when a watcher ticket arrives while it runs.
        job.request(PathBuf::from("/parent"), false, None, PendingFocus::None);
        job.reconcile(PathBuf::from("/parent"), false, ticket.clone(), false);
        let binding = job.desired_binding().unwrap().clone();
        let ready = job.settle(binding, DirectoryRead::Incomplete(DirStatus::Gone));
        assert_eq!(ready.watcher_gap, None);

        // ...but a gap recovery it answers is still counted.
        job.request(PathBuf::from("/parent"), false, None, PendingFocus::None);
        job.reconcile(PathBuf::from("/parent"), false, ticket, true);
        let binding = job.desired_binding().unwrap().clone();
        let ready = job.settle(binding, DirectoryRead::Incomplete(DirStatus::Gone));
        assert_eq!(ready.watcher_gap, Some(true));
    }

    #[test]
    fn superseded_reads_stuck_on_a_hung_mount_do_not_block_navigation() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let runtime = WorkloadRuntime::new(SchedulerLimits::default());
        let workload = WorkloadHandle::from_runtime(runtime);
        let notify: Notify = Arc::new(|| {});
        let started = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicBool::new(false));
        let read_dir = {
            let started = Arc::clone(&started);
            let released = Arc::clone(&released);
            move |_: &Path, _: bool| {
                started.fetch_add(1, Ordering::SeqCst);
                let deadline = Instant::now() + Duration::from_secs(10);
                while !released.load(Ordering::SeqCst) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(2));
                }
                DirectoryRead::Complete(Vec::new())
            }
        };
        let wait_started = |count: usize| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while started.load(Ordering::SeqCst) < count {
                assert!(Instant::now() < deadline, "listing worker never started");
                std::thread::sleep(Duration::from_millis(2));
            }
        };

        let mut job = ListingJobController::default();
        for (index, path) in ["/hung/a", "/hung/b"].into_iter().enumerate() {
            job.request(PathBuf::from(path), false, None, PendingFocus::None);
            job.drive(&workload, Arc::clone(&notify), read_dir.clone());
            wait_started(index + 1);
        }
        job.request(PathBuf::from("/fresh"), false, None, PendingFocus::None);
        job.drive(&workload, Arc::clone(&notify), read_dir.clone());

        // Both blocked reads were superseded; the new one is still admitted
        // and actually runs, although the scheduler caps listings at two.
        assert_eq!(job.in_flight.len(), 3);
        wait_started(3);
        assert!(
            job.in_flight
                .iter()
                .any(|in_flight| in_flight.binding.path == Path::new("/fresh"))
        );
        released.store(true, Ordering::SeqCst);
    }

    /// A listing worker driven by hand, with a read that counts its calls
    /// and finds every folder empty.
    struct Harness {
        workload: crate::workload::DeterministicWorkload,
        notify: Notify,
        reads: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Harness {
        fn new(limits: SchedulerLimits) -> Self {
            Self {
                workload: crate::workload::DeterministicWorkload::new(limits),
                notify: Arc::new(|| {}),
                reads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }

        fn drive(&self, job: &mut ListingJobController) {
            let reads = Arc::clone(&self.reads);
            job.drive(
                &self.workload.handle(),
                Arc::clone(&self.notify),
                move |_: &Path, _: bool| {
                    reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    DirectoryRead::Complete(Vec::new())
                },
            );
        }

        fn reads(&self) -> usize {
            self.reads.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[test]
    fn ready_listing_carries_its_ticket_and_is_not_read_again() {
        let mut watcher = super::super::watcher::DirectoryWatcherState::default();
        watcher.activate_test_binding(Path::new("/parent"));
        let ticket = watcher.snapshot_ticket().unwrap();
        let harness = Harness::new(SchedulerLimits::default());
        let mut job = ListingJobController::default();

        job.request(
            PathBuf::from("/parent"),
            false,
            Some(ticket.clone()),
            PendingFocus::None,
        );
        harness.drive(&mut job);
        assert!(harness.workload.run_next());
        let ready = job.take_ready().expect("the listing completed");
        assert_eq!(ready.ticket, Some(ticket));
        assert!(!job.is_awaiting());

        // Later frames keep driving the controller: no second read.
        harness.drive(&mut job);
        harness.drive(&mut job);
        assert!(!harness.workload.run_next(), "nothing was resubmitted");
        assert_eq!(harness.reads(), 1);
        assert!(job.take_ready().is_none());
    }

    #[test]
    fn completion_of_a_superseded_request_never_becomes_ready() {
        let harness = Harness::new(SchedulerLimits::default());
        let mut job = ListingJobController::default();
        job.request(PathBuf::from("/a"), false, None, PendingFocus::None);
        harness.drive(&mut job);
        // A's read finishes, but B is asked for before A's result is polled.
        assert!(harness.workload.run_next());
        let b = job.request(PathBuf::from("/b"), false, None, PendingFocus::None);

        harness.drive(&mut job);
        assert!(job.take_ready().is_none(), "A's completion is dropped");
        assert!(job.is_awaiting());
        assert!(harness.workload.run_next());
        let ready = job.take_ready().expect("B completed");
        assert_eq!(ready.binding, b);
        assert!(job.take_ready().is_none());
    }

    #[test]
    fn a_read_cancelled_while_still_desired_is_retried() {
        let harness = Harness::new(SchedulerLimits::default());
        let mut job = ListingJobController::default();
        let binding = job.request(PathBuf::from("/a"), false, None, PendingFocus::None);
        harness.drive(&mut job);
        // The worker sees its token cancelled once it starts.
        assert!(
            harness
                .workload
                .run_next_after_dequeue(|| assert!(job.in_flight[0].task.cancel()))
        );

        assert!(job.take_ready().is_none());
        assert!(job.is_awaiting(), "the binding is still wanted");
        harness.drive(&mut job);
        assert!(harness.workload.run_next(), "a fresh attempt was admitted");
        let ready = job.take_ready().expect("the retry completed");
        assert_eq!(ready.binding, binding);
        assert!(matches!(ready.read, DirectoryRead::Complete(_)));
        assert_eq!(harness.reads(), 1, "the cancelled attempt never read");
    }

    #[test]
    fn an_abandoned_read_settles_as_unreadable() {
        let harness = Harness::new(SchedulerLimits::default());
        let mut job = ListingJobController::default();
        let binding = job.request(
            PathBuf::from("/a"),
            false,
            None,
            PendingFocus::NamedChild("child".into()),
        );
        harness.drive(&mut job);
        // Cancelled while queued: the runtime abandons it without running.
        assert!(job.in_flight[0].task.cancel());

        let ready = job.take_ready().expect("an abandoned read settles");
        assert_eq!(ready.binding, binding);
        assert!(matches!(
            ready.read,
            DirectoryRead::Incomplete(DirStatus::Unavailable)
        ));
        assert_eq!(named(&ready.focus), Some("child"));
        assert!(!job.is_awaiting());
        assert_eq!(harness.reads(), 0);
    }

    #[test]
    fn a_refused_submission_settles_as_unreadable() {
        let harness = Harness::new(SchedulerLimits {
            max_queued: 0,
            ..SchedulerLimits::default()
        });
        let mut job = ListingJobController::default();
        let binding = job.request(PathBuf::from("/a"), false, None, PendingFocus::None);
        harness.drive(&mut job);

        assert!(job.in_flight.is_empty());
        assert!(!job.is_awaiting());
        let ready = job.take_ready().expect("a refused read settles at once");
        assert_eq!(ready.binding, binding);
        assert!(matches!(
            ready.read,
            DirectoryRead::Incomplete(DirStatus::Unavailable)
        ));
    }
}
