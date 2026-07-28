use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use super::{Notify, WATCHER_RETRY_BACKOFF, invalidate_size_cache};

enum DirectoryWatcher {
    Native(notify::RecommendedWatcher),
    Polling(notify::PollWatcher),
}

impl DirectoryWatcher {
    fn watch(
        &mut self,
        path: &Path,
        depth: crate::watcher_policy::WatchDepth,
    ) -> notify::Result<()> {
        use notify::Watcher as _;

        let mode = match depth {
            crate::watcher_policy::WatchDepth::Recursive => notify::RecursiveMode::Recursive,
            crate::watcher_policy::WatchDepth::DirectoryOnly => notify::RecursiveMode::NonRecursive,
        };
        match self {
            Self::Native(watcher) => watcher.watch(path, mode),
            Self::Polling(watcher) => watcher.watch(path, mode),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct WatchBinding {
    path: PathBuf,
    epoch: u64,
}

#[derive(Debug)]
enum WatchMessageKind {
    Changed { paths: Vec<PathBuf>, direct: bool },
    Gap,
    BackendError,
}

#[derive(Debug)]
struct WatchMessage {
    binding: WatchBinding,
    kind: WatchMessageKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReconciliationTicket {
    binding: WatchBinding,
    generation: u64,
}

#[derive(Debug, Default)]
pub(super) struct WatchPoll {
    pub(super) ticket: Option<ReconciliationTicket>,
    pub(super) recovered_gap: bool,
    pub(super) sizes_dirty: bool,
}

struct WatcherCallback {
    binding: WatchBinding,
    sender: mpsc::Sender<WatchMessage>,
    wake_scheduled: Arc<AtomicBool>,
    wake: Option<Notify>,
}

impl WatcherCallback {
    fn send(&self, kind: WatchMessageKind) {
        if self
            .sender
            .send(WatchMessage {
                binding: self.binding.clone(),
                kind,
            })
            .is_err()
        {
            return;
        }
        let coalesced = self.wake_scheduled.swap(true, Ordering::AcqRel);
        crate::watcher_health::record_event_batch(coalesced);
        if !coalesced && let Some(wake) = &self.wake {
            wake();
        }
    }

    fn into_handler(self) -> Box<dyn FnMut(Result<notify::Event, notify::Error>) + Send + 'static> {
        Box::new(move |result| {
            let event = match result {
                Ok(event) => event,
                Err(_) => {
                    crate::watcher_health::record_backend_error();
                    self.send(WatchMessageKind::BackendError);
                    return;
                }
            };

            crate::watcher_health::record_event();
            if event.need_rescan() {
                crate::watcher_health::record_rescan_signal();
                self.send(WatchMessageKind::Gap);
                return;
            }

            let mut direct = event.paths.is_empty();
            for path in &event.paths {
                if path == &self.binding.path || path.parent() == Some(self.binding.path.as_path())
                {
                    direct = true;
                }
            }
            if direct {
                crate::watcher_health::record_direct_event();
            } else {
                crate::watcher_health::record_deep_event();
            }
            self.send(WatchMessageKind::Changed {
                paths: event.paths,
                direct,
            });
        })
    }
}

/// Owns one exact directory subscription and its typed event inbox.
///
/// Callbacks carry both the watched pathname and a monotonic binding epoch.
/// Dropped watcher callbacks may still race with navigation, but their messages
/// cannot affect the new directory because `poll` rejects a non-current binding.
pub(super) struct DirectoryWatcherState {
    watcher: Option<DirectoryWatcher>,
    binding: Option<WatchBinding>,
    retry: Option<(PathBuf, Instant)>,
    notify: Option<Notify>,
    sender: mpsc::Sender<WatchMessage>,
    receiver: mpsc::Receiver<WatchMessage>,
    wake_scheduled: Arc<AtomicBool>,
    next_binding_epoch: u64,
    requested_generation: u64,
    applied_generation: u64,
    gap_generation: u64,
    applied_gap_generation: u64,
    retry_reconciliation_at: Option<Instant>,
}

impl Default for DirectoryWatcherState {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            watcher: None,
            binding: None,
            retry: None,
            notify: None,
            sender,
            receiver,
            wake_scheduled: Arc::new(AtomicBool::new(false)),
            next_binding_epoch: 1,
            requested_generation: 0,
            applied_generation: 0,
            gap_generation: 0,
            applied_gap_generation: 0,
            retry_reconciliation_at: None,
        }
    }
}

impl DirectoryWatcherState {
    pub(super) fn set_notify(&mut self, notify: Notify) {
        self.notify = Some(notify);
        self.watcher = None;
        self.binding = None;
        self.retry = None;
        self.retry_reconciliation_at = None;
    }

    pub(super) fn has_notify(&self) -> bool {
        self.notify.is_some()
    }

    pub(super) fn is_active(&self) -> bool {
        self.watcher.is_some()
    }

    pub(super) fn notify(&self) -> Option<Notify> {
        self.notify.clone()
    }

    pub(super) fn ensure_binding(&mut self, path: &Path) {
        if self
            .binding
            .as_ref()
            .is_some_and(|binding| binding.path == path)
            && self.watcher.is_some()
        {
            return;
        }
        if self
            .retry
            .as_ref()
            .is_some_and(|(retry_path, retry_at)| retry_path == path && Instant::now() < *retry_at)
        {
            return;
        }
        if self.notify.is_none() {
            return;
        }

        let recovering = self
            .retry
            .take()
            .is_some_and(|(retry_path, _)| retry_path == path);
        self.watcher = None;
        self.binding = None;
        self.wake_scheduled = Arc::new(AtomicBool::new(false));

        let binding = WatchBinding {
            path: path.to_path_buf(),
            epoch: self.next_binding_epoch,
        };
        self.next_binding_epoch = self.next_binding_epoch.wrapping_add(1).max(1);

        let profile = crate::volume_profile::profile(path);
        let policy = crate::watcher_policy::policy_for(profile.backend);
        let attempts = [
            (policy.backend, policy.depth, false),
            (
                crate::watcher_policy::WatcherBackend::Polling,
                crate::watcher_policy::WatchDepth::DirectoryOnly,
                true,
            ),
        ];
        let attempt_count =
            usize::from(policy.backend == crate::watcher_policy::WatcherBackend::Native) + 1;

        for &(backend, depth, fallback) in attempts.iter().take(attempt_count) {
            let callback = WatcherCallback {
                binding: binding.clone(),
                sender: self.sender.clone(),
                wake_scheduled: Arc::clone(&self.wake_scheduled),
                wake: self.notify.clone(),
            };
            let watcher = match backend {
                crate::watcher_policy::WatcherBackend::Native => {
                    notify::recommended_watcher(callback.into_handler())
                        .map(DirectoryWatcher::Native)
                }
                crate::watcher_policy::WatcherBackend::Polling => notify::PollWatcher::new(
                    callback.into_handler(),
                    notify::Config::default().with_poll_interval(policy.poll_interval),
                )
                .map(DirectoryWatcher::Polling),
            };
            let Ok(mut watcher) = watcher else {
                crate::watcher_health::record_start_failure();
                continue;
            };
            if watcher.watch(path, depth).is_err() {
                crate::watcher_health::record_watch_failure();
                continue;
            }

            self.binding = Some(binding.clone());
            self.watcher = Some(watcher);
            self.retry = None;
            self.request_reconciliation(true);
            crate::watcher_health::record_watcher_start(backend, depth, fallback);
            if recovering {
                crate::watcher_health::record_reconnect();
            }
            return;
        }

        self.mark_unavailable(path);
    }

    pub(super) fn snapshot_ticket(&self) -> Option<ReconciliationTicket> {
        self.binding.as_ref().map(|binding| ReconciliationTicket {
            binding: binding.clone(),
            generation: self.requested_generation,
        })
    }

    pub(super) fn snapshot_ticket_for(&self, path: &Path) -> Option<ReconciliationTicket> {
        self.binding
            .as_ref()
            .filter(|binding| binding.path == path)
            .map(|binding| ReconciliationTicket {
                binding: binding.clone(),
                generation: self.requested_generation,
            })
    }

    pub(super) fn acknowledge_snapshot(
        &mut self,
        ticket: Option<ReconciliationTicket>,
        listing_binding: &Path,
    ) {
        let Some(ticket) = ticket else {
            return;
        };
        if self.binding.as_ref() != Some(&ticket.binding) || ticket.binding.path != listing_binding
        {
            return;
        }
        self.applied_generation = self.applied_generation.max(ticket.generation);
        if self.applied_generation >= self.requested_generation {
            self.retry_reconciliation_at = None;
        }
        if ticket.generation >= self.gap_generation
            && self.applied_gap_generation < self.gap_generation
        {
            self.applied_gap_generation = self.gap_generation;
        }
    }

    pub(super) fn defer_snapshot(&mut self, ticket: Option<&ReconciliationTicket>) {
        if ticket.is_some_and(|ticket| self.binding.as_ref() == Some(&ticket.binding)) {
            self.retry_reconciliation_at = Some(Instant::now() + WATCHER_RETRY_BACKOFF);
            if let Some(wake) = self.notify.clone() {
                let _ = std::thread::Builder::new()
                    .name("commander-listing-retry".to_string())
                    .spawn(move || {
                        std::thread::sleep(WATCHER_RETRY_BACKOFF);
                        wake();
                    });
            }
        }
    }

    pub(super) fn poll(&mut self, path: &Path) -> WatchPoll {
        self.ensure_binding(path);
        let mut outcome = WatchPoll::default();
        let mut restart = false;

        loop {
            while let Ok(message) = self.receiver.try_recv() {
                restart |= self.apply_message(message, &mut outcome);
            }

            // Clear the coalescing gate, then check once more. An event racing
            // before the clear is already in the inbox; one racing after it
            // schedules its own wake-up.
            self.wake_scheduled.store(false, Ordering::Release);
            match self.receiver.try_recv() {
                Ok(message) => {
                    restart |= self.apply_message(message, &mut outcome);
                    continue;
                }
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
            }
        }

        if restart {
            self.watcher = None;
            self.binding = None;
            self.retry = Some((path.to_path_buf(), Instant::now() + WATCHER_RETRY_BACKOFF));
        }
        if self.watcher.is_none() {
            self.ensure_binding(path);
        }

        let due = self
            .retry_reconciliation_at
            .is_none_or(|retry_at| Instant::now() >= retry_at);
        if self.requested_generation > self.applied_generation && due {
            outcome.ticket = self.snapshot_ticket();
        }
        outcome.recovered_gap = self.gap_generation > self.applied_gap_generation;
        outcome
    }

    fn apply_message(&mut self, message: WatchMessage, outcome: &mut WatchPoll) -> bool {
        if self.binding.as_ref() != Some(&message.binding) {
            return false;
        }
        match message.kind {
            WatchMessageKind::Changed { paths, direct } => {
                if paths.is_empty() {
                    invalidate_size_cache(&message.binding.path);
                } else {
                    for changed in paths {
                        invalidate_size_cache(&changed);
                    }
                }
                if direct {
                    self.request_reconciliation(false);
                } else {
                    outcome.sizes_dirty = true;
                }
                false
            }
            WatchMessageKind::Gap => {
                invalidate_size_cache(&message.binding.path);
                self.request_reconciliation(true);
                false
            }
            WatchMessageKind::BackendError => {
                invalidate_size_cache(&message.binding.path);
                self.request_reconciliation(true);
                true
            }
        }
    }

    fn request_reconciliation(&mut self, gap: bool) {
        self.requested_generation = self.requested_generation.wrapping_add(1);
        if gap {
            self.gap_generation = self.requested_generation;
        }
        self.retry_reconciliation_at = None;
        if let Some(wake) = &self.notify {
            wake();
        }
    }

    fn mark_unavailable(&mut self, path: &Path) {
        invalidate_size_cache(path);
        self.watcher = None;
        self.binding = None;
        self.retry = Some((path.to_path_buf(), Instant::now() + WATCHER_RETRY_BACKOFF));
        self.request_reconciliation(true);
    }

    #[cfg(test)]
    pub(super) fn activate_test_binding(&mut self, path: &Path) -> WatchBinding {
        let binding = WatchBinding {
            path: path.to_path_buf(),
            epoch: self.next_binding_epoch,
        };
        self.next_binding_epoch += 1;
        self.binding = Some(binding.clone());
        self.requested_generation += 1;
        binding
    }

    #[cfg(test)]
    fn inject_test_message(&self, binding: WatchBinding, kind: WatchMessageKind) {
        self.sender.send(WatchMessage { binding, kind }).unwrap();
    }

    #[cfg(test)]
    pub(super) fn inject_current_change_for_test(&self, path: PathBuf) {
        let binding = self.binding.clone().expect("active test binding");
        let direct = path == binding.path || path.parent() == Some(binding.path.as_path());
        self.inject_test_message(
            binding,
            WatchMessageKind::Changed {
                paths: vec![path],
                direct,
            },
        );
    }

    #[cfg(test)]
    pub(super) fn inject_gap_for_test(&self) {
        let binding = self.binding.clone().expect("active test binding");
        self.inject_test_message(binding, WatchMessageKind::Gap);
    }

    #[cfg(test)]
    pub(super) fn has_pending_reconciliation_for_test(&self) -> bool {
        self.requested_generation > self.applied_generation
    }

    #[cfg(test)]
    pub(super) fn reconciliation_state_for_test(
        &self,
    ) -> (Option<(PathBuf, u64)>, u64, u64, u64, u64) {
        (
            self.binding
                .as_ref()
                .map(|binding| (binding.path.clone(), binding.epoch)),
            self.requested_generation,
            self.applied_generation,
            self.gap_generation,
            self.applied_gap_generation,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_callback_after_navigation_is_ignored() {
        let mut watcher = DirectoryWatcherState::default();
        let old = watcher.activate_test_binding(Path::new("/old"));
        let current = watcher.activate_test_binding(Path::new("/current"));
        let initial_generation = watcher.requested_generation;

        watcher.inject_test_message(
            old,
            WatchMessageKind::Changed {
                paths: vec![PathBuf::from("/old/late")],
                direct: true,
            },
        );
        let outcome = watcher.poll(Path::new("/current"));

        assert_eq!(watcher.binding.as_ref(), Some(&current));
        assert_eq!(watcher.requested_generation, initial_generation);
        assert!(outcome.ticket.is_some());
        assert!(!outcome.sizes_dirty);
    }

    #[test]
    fn event_between_subscription_and_snapshot_is_not_acknowledged_early() {
        let mut watcher = DirectoryWatcherState::default();
        let binding = watcher.activate_test_binding(Path::new("/watched"));
        let snapshot = watcher.snapshot_ticket().unwrap();

        // Deterministic barrier: subscription exists, the listing snapshot has
        // not yet been published, and a callback arrives in that exact window.
        watcher.inject_test_message(
            binding,
            WatchMessageKind::Changed {
                paths: vec![PathBuf::from("/watched/new")],
                direct: true,
            },
        );
        watcher.acknowledge_snapshot(Some(snapshot.clone()), Path::new("/watched"));
        let outcome = watcher.poll(Path::new("/watched"));

        let next = outcome.ticket.expect("the racing mutation must reconcile");
        assert!(next.generation > snapshot.generation);
    }

    #[test]
    fn snapshot_from_another_listing_binding_cannot_acknowledge_ticket() {
        let mut watcher = DirectoryWatcherState::default();
        watcher.activate_test_binding(Path::new("/watched"));
        let snapshot = watcher.snapshot_ticket().unwrap();

        watcher.acknowledge_snapshot(Some(snapshot.clone()), Path::new("/other"));
        assert!(watcher.has_pending_reconciliation_for_test());

        watcher.acknowledge_snapshot(Some(snapshot), Path::new("/watched"));
        assert!(!watcher.has_pending_reconciliation_for_test());
    }

    #[test]
    fn path_bound_snapshot_never_borrows_a_stale_subscription() {
        let mut watcher = DirectoryWatcherState::default();
        watcher.activate_test_binding(Path::new("/old"));

        assert!(watcher.snapshot_ticket_for(Path::new("/current")).is_none());
        assert!(watcher.snapshot_ticket_for(Path::new("/old")).is_some());
    }

    #[test]
    fn snapshot_older_than_gap_generation_cannot_acknowledge_gap() {
        let mut watcher = DirectoryWatcherState::default();
        let binding = watcher.activate_test_binding(Path::new("/watched"));
        let before_gap = watcher.snapshot_ticket().unwrap();
        watcher.inject_test_message(binding, WatchMessageKind::Gap);
        let gap = watcher.poll(Path::new("/watched"));
        assert!(gap.recovered_gap);

        watcher.acknowledge_snapshot(Some(before_gap), Path::new("/watched"));
        assert!(
            watcher.poll(Path::new("/watched")).recovered_gap,
            "a pre-gap snapshot cannot confirm gap recovery"
        );

        watcher.acknowledge_snapshot(gap.ticket, Path::new("/watched"));
        assert!(!watcher.poll(Path::new("/watched")).recovered_gap);
    }

    #[test]
    fn backend_error_drops_binding_and_observes_retry_backoff() {
        let mut watcher = DirectoryWatcherState::default();
        let binding = watcher.activate_test_binding(Path::new("/watched"));
        watcher.inject_test_message(binding, WatchMessageKind::BackendError);

        let outcome = watcher.poll(Path::new("/watched"));

        assert!(!watcher.is_active());
        assert!(outcome.recovered_gap);
        let (path, retry_at) = watcher.retry.as_ref().unwrap();
        assert_eq!(path, Path::new("/watched"));
        assert!(*retry_at > Instant::now());
    }
}
