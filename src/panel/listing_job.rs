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
}

pub(super) struct ListingJobController {
    generation: u64,
    desired: Option<ListingBinding>,
    ticket: Option<ReconciliationTicket>,
    focus: PendingFocus,
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
        self.generation = self
            .generation
            .checked_add(1)
            .expect("listing generation space exhausted");
        for in_flight in &mut self.in_flight {
            in_flight.cancel_once();
        }
        let binding = ListingBinding {
            path,
            show_hidden,
            generation: self.generation,
        };
        self.desired = Some(binding.clone());
        self.ticket = ticket;
        self.focus = focus;
        self.ready = None;
        self.awaiting = true;
        binding
    }

    pub(super) fn set_focus(&mut self, focus: PendingFocus) {
        self.focus = focus;
    }

    pub(super) fn is_awaiting(&self) -> bool {
        self.awaiting
    }

    pub(super) fn desired_binding(&self) -> Option<&ListingBinding> {
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
        if self.in_flight.len() >= MAX_IN_FLIGHT_LISTINGS {
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
            Err(_) => {
                self.ready = Some(ListingReady {
                    binding,
                    read: DirectoryRead::Incomplete(DirStatus::Denied),
                    ticket: self.ticket.take(),
                    focus: std::mem::replace(&mut self.focus, PendingFocus::None),
                });
                self.awaiting = false;
            }
        }
    }

    pub(super) fn take_ready(&mut self) -> Option<ListingReady> {
        self.poll_terminals();
        self.ready.take()
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
                    self.ready = Some(ListingReady {
                        binding,
                        read,
                        ticket: self.ticket.take(),
                        focus: std::mem::replace(&mut self.focus, PendingFocus::None),
                    });
                    self.awaiting = false;
                }
                WorkerEvent::Completed { .. } => {
                    self.ready = Some(ListingReady {
                        binding: retired.binding,
                        read: DirectoryRead::Incomplete(DirStatus::Denied),
                        ticket: self.ticket.take(),
                        focus: std::mem::replace(&mut self.focus, PendingFocus::None),
                    });
                    self.awaiting = false;
                }
                WorkerEvent::Cancelled { .. } => {
                    // Keep awaiting so drive() can admit a fresh attempt for
                    // the still-desired binding (or a newer request already
                    // replaced desired and will submit itself).
                    if self.desired.as_ref() == Some(&retired.binding) {
                        self.awaiting = true;
                    }
                }
                WorkerEvent::Abandoned { .. } => {
                    self.ready = Some(ListingReady {
                        binding: retired.binding,
                        read: DirectoryRead::Incomplete(DirStatus::Denied),
                        ticket: self.ticket.take(),
                        focus: std::mem::replace(&mut self.focus, PendingFocus::None),
                    });
                    self.awaiting = false;
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
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            DirectoryRead::Incomplete(DirStatus::Gone)
                        }
                        Err(_) => DirectoryRead::Incomplete(DirStatus::Denied),
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
}
