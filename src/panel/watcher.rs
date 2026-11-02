use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::time::Instant;

use super::{Notify, invalidate_size_cache, invalidate_size_cache_many};

pub const WATCHER_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

/// Longest wait between retries for a directory no watcher backend can
/// subscribe to (the base backoff doubles per consecutive failure).
const WATCHER_RETRY_BACKOFF_MAX_SHIFT: u32 = 4;

/// Messages the callbacks may queue before further events collapse into a
/// single gap (a full re-list): frames can stop while the window is hidden,
/// and an event burst must not grow the inbox without bound.
const INBOX_CAPACITY: usize = 4_096;
/// `overflowed` bit: an event was dropped on a full inbox (re-list).
const DROPPED_EVENT: u8 = 1;
/// `overflowed` bit: the dropped event was a backend error that needs the
/// watcher torn down and resubscribed.
const DROPPED_RESTART: u8 = 2;

/// Messages one `poll` (one frame) applies; the rest wait for the next frame
/// so an event flood cannot stall a single frame.
const MAX_MESSAGES_PER_POLL: usize = 1_024;

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

#[derive(Clone)]
struct WatcherCallback {
    binding: WatchBinding,
    /// The binding path as the backend may report it: macOS `FSEvents`
    /// canonicalizes watched paths (`/tmp` -> `/private/tmp`). `None` when
    /// it equals the binding path.
    canonical: Option<PathBuf>,
    sender: mpsc::SyncSender<WatchMessage>,
    wake_scheduled: Arc<AtomicBool>,
    /// [`DROPPED_EVENT`] (and [`DROPPED_RESTART`] for a backend error) when
    /// the inbox was full and a message was dropped; the next poll treats it
    /// as a gap, or as a backend error.
    overflowed: Arc<AtomicU8>,
    wake: Option<Notify>,
}

/// Whether a backend error invalidates the subscription itself (the watched
/// directory, the backend or its limits), as opposed to a per-path error
/// below it, such as a `PollWatcher` scan racing a deleted child. Only the
/// former tears the watcher down; the latter just forces a re-list.
fn error_needs_restart(error: &notify::Error, is_root: impl Fn(&Path) -> bool) -> bool {
    match error.kind {
        notify::ErrorKind::Io(_) | notify::ErrorKind::PathNotFound => {
            error.paths.is_empty() || error.paths.iter().any(|path| is_root(path))
        }
        notify::ErrorKind::Generic(_)
        | notify::ErrorKind::WatchNotFound
        | notify::ErrorKind::InvalidConfig(_)
        | notify::ErrorKind::MaxFilesWatch => true,
    }
}

impl WatcherCallback {
    fn send(&self, kind: WatchMessageKind) {
        match self.sender.try_send(WatchMessage {
            binding: self.binding.clone(),
            kind,
        }) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(message)) => {
                let dropped = if matches!(message.kind, WatchMessageKind::BackendError) {
                    DROPPED_EVENT | DROPPED_RESTART
                } else {
                    DROPPED_EVENT
                };
                self.overflowed.fetch_or(dropped, Ordering::AcqRel);
            }
            Err(mpsc::TrySendError::Disconnected(_)) => return,
        }
        let coalesced = self.wake_scheduled.swap(true, Ordering::AcqRel);
        crate::watcher_health::record_event_batch(coalesced);
        if !coalesced && let Some(wake) = &self.wake {
            wake();
        }
    }

    fn is_root(&self, path: &Path) -> bool {
        path == self.binding.path || self.canonical.as_deref() == Some(path)
    }

    /// Rewrite a path the backend reported under the canonical form onto the
    /// binding's own form, so the listing, size-cache invalidation and every
    /// later use compare against the paths the panel actually shows.
    fn to_binding_form(&self, path: PathBuf) -> PathBuf {
        match self
            .canonical
            .as_deref()
            .and_then(|canonical| path.strip_prefix(canonical).ok())
        {
            Some(rest) if rest.as_os_str().is_empty() => self.binding.path.clone(),
            Some(rest) => self.binding.path.join(rest),
            None => path,
        }
    }

    fn into_handler(self) -> Box<dyn FnMut(Result<notify::Event, notify::Error>) + Send + 'static> {
        Box::new(move |result| {
            let event = match result {
                Ok(event) => event,
                Err(error) => {
                    crate::watcher_health::record_backend_error();
                    let kind = if error_needs_restart(&error, |path| self.is_root(path)) {
                        log::warn!(
                            target: "commander::watcher",
                            "watcher for {} failed and restarts: {error}",
                            self.binding.path.display()
                        );
                        WatchMessageKind::BackendError
                    } else {
                        // Per-path noise (a scan racing a deletion): re-list.
                        log::debug!(
                            target: "commander::watcher",
                            "watcher error below {}, re-listing: {error}",
                            self.binding.path.display()
                        );
                        WatchMessageKind::Gap
                    };
                    self.send(kind);
                    return;
                }
            };

            crate::watcher_health::record_event();
            if event.need_rescan() {
                crate::watcher_health::record_rescan_signal();
                self.send(WatchMessageKind::Gap);
                return;
            }
            // Reads change nothing. inotify reports opens, so without this
            // the app's own listing, count and size walks would invalidate
            // sizes and re-list the folder they just read.
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }

            let paths: Vec<PathBuf> = event
                .paths
                .into_iter()
                .map(|path| self.to_binding_form(path))
                .collect();
            let root = self.binding.path.as_path();
            let direct = paths.is_empty()
                || paths
                    .iter()
                    .any(|path| path == root || path.parent() == Some(root));
            if direct {
                crate::watcher_health::record_direct_event();
            } else {
                crate::watcher_health::record_deep_event();
            }
            self.send(WatchMessageKind::Changed { paths, direct });
        })
    }
}

/// Owns one exact directory subscription and its typed event inbox.
///
/// Callbacks carry both the watched pathname and a monotonic binding epoch.
/// Dropped watcher callbacks may still race with navigation, but their messages
/// cannot affect the new directory because `poll` rejects a non-current binding.
pub(super) struct DirectoryWatcherState {
    watcher: Option<WatchLease>,
    pending_install: Option<PendingInstall>,
    binding: Option<WatchBinding>,
    retry: Option<(PathBuf, Instant)>,
    /// Consecutive failures to subscribe to this path with any backend.
    unavailable: Option<(PathBuf, u32)>,
    notify: Option<Notify>,
    sender: mpsc::SyncSender<WatchMessage>,
    receiver: mpsc::Receiver<WatchMessage>,
    wake_scheduled: Arc<AtomicBool>,
    overflowed: Arc<AtomicU8>,
    next_binding_epoch: u64,
    requested_generation: u64,
    applied_generation: u64,
    gap_generation: u64,
    applied_gap_generation: u64,
    retry_reconciliation_at: Option<Instant>,
}

/// UI-owned lease: dropping the last sender only signals the worker. Backend
/// destruction, like installation, remains on that worker even after navigation.
#[derive(Clone)]
struct WatchLease {
    _stop: mpsc::Sender<()>,
}

struct InstalledBackend {
    backend: crate::watcher_policy::WatcherBackend,
    depth: crate::watcher_policy::WatchDepth,
    fallback: bool,
}

struct PendingInstall {
    binding: WatchBinding,
    lease: WatchLease,
    recovering: bool,
    receiver: mpsc::Receiver<Result<InstalledBackend, String>>,
}

const MAX_WATCHER_INSTALLS: usize = 8;
static INSTALL_BUDGET: OnceLock<Arc<AtomicUsize>> = OnceLock::new();

struct InstallPermit(Arc<AtomicUsize>);
impl InstallPermit {
    fn acquire(budget: &Arc<AtomicUsize>) -> Option<Self> {
        budget
            .try_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_WATCHER_INSTALLS).then_some(active + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(budget)))
    }
}
impl Drop for InstallPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn install_backend(
    backend: crate::watcher_policy::WatcherBackend,
    depth: crate::watcher_policy::WatchDepth,
    interval: std::time::Duration,
    callback: WatcherCallback,
) -> Result<DirectoryWatcher, String> {
    use crate::watcher_policy::WatcherBackend;
    use notify::Watcher as _;
    let path = callback.binding.path.clone();
    // Watch the lexical tree: linked targets can escape it or form cycles even
    // in a shallow PollWatcher scan. Keep following disabled on supporting backends.
    let config = notify::Config::default()
        .with_poll_interval(interval)
        .with_follow_symlinks(false);
    let result = match backend {
        WatcherBackend::Native => notify::RecommendedWatcher::new(callback.into_handler(), config)
            .map(DirectoryWatcher::Native),
        WatcherBackend::Polling => {
            notify::PollWatcher::new(callback.into_handler(), config).map(DirectoryWatcher::Polling)
        }
    };
    let mut watcher = result.map_err(|error| {
        crate::watcher_health::record_start_failure();
        error.to_string()
    })?;
    watcher.watch(&path, depth).map_err(|error| {
        crate::watcher_health::record_watch_failure();
        error.to_string()
    })?;
    Ok(watcher)
}

/// This entire function runs off the UI thread, including mount probing and
/// the initial polling scan. Notifications carry metadata, never backend owners.
fn run_install(
    mut callback: WatcherCallback,
    sender: mpsc::SyncSender<Result<InstalledBackend, String>>,
    stop: mpsc::Receiver<()>,
    permit: InstallPermit,
) {
    use crate::watcher_policy::{WatchDepth, WatcherBackend};
    let path = callback.binding.path.clone();
    let policy = crate::watcher_policy::policy_for(crate::volume_profile::profile(&path).backend);
    callback.canonical = std::fs::canonicalize(&path)
        .ok()
        .filter(|canonical| canonical != &path);
    if matches!(
        stop.try_recv(),
        Err(mpsc::TryRecvError::Disconnected) | Ok(())
    ) {
        return;
    }
    let publish = |result| {
        let sent = sender.send(result).is_ok();
        if sent && let Some(wake) = &callback.wake {
            wake();
        }
        sent
    };
    let native = policy.backend == WatcherBackend::Native;
    let depth = if native {
        WatchDepth::DirectoryOnly
    } else {
        policy.depth
    };
    let mut watcher = match install_backend(
        WatcherBackend::Polling,
        depth,
        policy.poll_interval,
        callback.clone(),
    ) {
        Ok(watcher) => watcher,
        Err(error) => {
            publish(Err(error));
            return;
        }
    };
    if !publish(Ok(InstalledBackend {
        backend: WatcherBackend::Polling,
        depth,
        fallback: native,
    })) {
        return;
    }
    if native
        && !matches!(
            stop.try_recv(),
            Err(mpsc::TryRecvError::Disconnected) | Ok(())
        )
    {
        match install_backend(
            WatcherBackend::Native,
            policy.depth,
            policy.poll_interval,
            callback.clone(),
        ) {
            Ok(installed) => {
                watcher = installed;
                if !publish(Ok(InstalledBackend {
                    backend: WatcherBackend::Native,
                    depth: policy.depth,
                    fallback: false,
                })) {
                    return;
                }
            }
            Err(error) => {
                if !publish(Err(error)) {
                    return;
                }
            }
        }
    }
    drop(sender);
    // Healthy subscriptions do not consume admission slots while they wait.
    drop(permit);
    let _ = stop.recv();
    drop(watcher);
}

impl Default for DirectoryWatcherState {
    fn default() -> Self {
        let (sender, receiver) = mpsc::sync_channel(INBOX_CAPACITY);
        Self {
            watcher: None,
            pending_install: None,
            binding: None,
            retry: None,
            unavailable: None,
            notify: None,
            sender,
            receiver,
            wake_scheduled: Arc::new(AtomicBool::new(false)),
            overflowed: Arc::new(AtomicU8::new(0)),
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
        self.pending_install = None;
        self.binding = None;
        self.retry = None;
        self.unavailable = None;
        self.retry_reconciliation_at = None;
    }

    pub(super) fn has_notify(&self) -> bool {
        self.notify.is_some()
    }

    pub(super) const fn is_active(&self) -> bool {
        self.watcher.is_some()
    }

    pub(super) fn notify(&self) -> Option<Notify> {
        self.notify.clone()
    }

    pub(super) fn ensure_binding(&mut self, path: &Path) {
        self.poll_install();
        if self
            .binding
            .as_ref()
            .is_some_and(|binding| binding.path == path)
            && (self.watcher.is_some() || self.pending_install.is_some())
        {
            return;
        }
        if self
            .retry
            .as_ref()
            .is_some_and(|(retry_path, retry_at)| retry_path == path && Instant::now() < *retry_at)
            || self.notify.is_none()
        {
            return;
        }
        let recovering = self
            .retry
            .take()
            .is_some_and(|(retry_path, _)| retry_path == path);
        self.watcher = None;
        self.pending_install = None;
        self.wake_scheduled = Arc::new(AtomicBool::new(false));
        self.overflowed = Arc::new(AtomicU8::new(0));
        let binding = WatchBinding {
            path: path.to_path_buf(),
            epoch: self.next_binding_epoch,
        };
        self.next_binding_epoch = self.next_binding_epoch.wrapping_add(1).max(1);
        self.binding = Some(binding.clone());
        // A listing can start now; installation completion requests another
        // snapshot to cover all changes before the subscription became live.
        self.request_reconciliation(false);
        let callback = WatcherCallback {
            binding: binding.clone(),
            canonical: None,
            sender: self.sender.clone(),
            wake_scheduled: Arc::clone(&self.wake_scheduled),
            overflowed: Arc::clone(&self.overflowed),
            wake: self.notify.clone(),
        };
        let budget = INSTALL_BUDGET.get_or_init(|| Arc::new(AtomicUsize::new(0)));
        if !self.start_install(binding, callback, budget, run_install) {
            self.mark_unavailable(path);
        } else if let Some(pending) = self.pending_install.as_mut() {
            pending.recovering = recovering;
        }
    }

    fn start_install(
        &mut self,
        binding: WatchBinding,
        callback: WatcherCallback,
        budget: &Arc<AtomicUsize>,
        install: impl FnOnce(
            WatcherCallback,
            mpsc::SyncSender<Result<InstalledBackend, String>>,
            mpsc::Receiver<()>,
            InstallPermit,
        ) + Send
        + 'static,
    ) -> bool {
        let Some(permit) = InstallPermit::acquire(budget) else {
            return false;
        };
        let (sender, receiver) = mpsc::sync_channel(2);
        let (stop, stop_receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("commander-watcher-install".into())
            .spawn(move || {
                let failure_callback = callback.clone();
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    install(callback, sender, stop_receiver, permit);
                })).is_err() {
                    crate::watcher_health::record_backend_error();
                    failure_callback.send(WatchMessageKind::BackendError);
                    log::warn!(target: "commander::watcher", "watcher worker panicked; subscription will restart");
                }
            })
        {
            Ok(_) => {
                self.pending_install = Some(PendingInstall {
                    binding,
                    lease: WatchLease { _stop: stop },
                    recovering: false,
                    receiver,
                });
                true
            }
            Err(error) => {
                crate::watcher_health::record_start_failure();
                log::warn!(target: "commander::watcher", "cannot spawn watcher installer: {error}");
                false
            }
        }
    }

    fn poll_install(&mut self) {
        loop {
            let Some(pending) = self.pending_install.as_ref() else {
                return;
            };
            let result = match pending.receiver.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let path = pending.binding.path.clone();
                    self.pending_install = None;
                    if self.watcher.is_none() {
                        self.mark_unavailable(&path);
                    }
                    return;
                }
            };
            if self.binding.as_ref() != Some(&pending.binding) {
                self.pending_install = None;
                return;
            }
            match result {
                Ok(installed) => {
                    self.watcher = Some(pending.lease.clone());
                    if pending.recovering {
                        crate::watcher_health::record_reconnect();
                    }
                    self.pending_install.as_mut().unwrap().recovering = false;
                    self.retry = None;
                    self.unavailable = None;
                    self.request_reconciliation(true);
                    crate::watcher_health::record_watcher_start(
                        installed.backend,
                        installed.depth,
                        installed.fallback,
                    );
                }
                Err(error) => {
                    let path = pending.binding.path.clone();
                    self.pending_install = None;
                    if self.watcher.is_none() {
                        self.mark_unavailable(&path);
                    }
                    log::warn!(target: "commander::watcher", "cannot install watcher for {}: {error}", path.display());
                    return;
                }
            }
        }
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
            if let Some(wake) = self.notify.clone()
                && let Err(error) = std::thread::Builder::new()
                    .name("commander-listing-retry".to_string())
                    .spawn(move || {
                        std::thread::sleep(WATCHER_RETRY_BACKOFF);
                        wake();
                    })
            {
                log::warn!(
                    target: "commander::watcher",
                    "failed to spawn listing retry wake thread: {error}"
                );
            }
        }
    }

    pub(super) fn poll(&mut self, path: &Path) -> WatchPoll {
        self.ensure_binding(path);
        self.poll_install();
        let mut outcome = WatchPoll::default();
        let mut restart = false;
        let mut changed = Vec::new();
        let mut applied = 0usize;
        let mut backlog = false;

        loop {
            while applied < MAX_MESSAGES_PER_POLL
                && let Ok(message) = self.receiver.try_recv()
            {
                applied += 1;
                restart |= self.apply_message(message, &mut outcome, &mut changed);
            }
            if applied >= MAX_MESSAGES_PER_POLL {
                // Leave the coalescing gate set (callbacks stay quiet) and
                // wake the next frame to drain the rest.
                backlog = true;
                break;
            }

            // Clear the coalescing gate, then check once more. An event racing
            // before the clear is already in the inbox; one racing after it
            // schedules its own wake-up.
            self.wake_scheduled.store(false, Ordering::Release);
            match self.receiver.try_recv() {
                Ok(message) => {
                    applied += 1;
                    restart |= self.apply_message(message, &mut outcome, &mut changed);
                    continue;
                }
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
            }
        }
        let dropped = self.overflowed.swap(0, Ordering::AcqRel);
        if dropped != 0
            && let Some(binding) = &self.binding
        {
            // Events were dropped on a full inbox: re-list like a gap, and
            // resubscribe if one of them was a backend error.
            changed.push(binding.path.clone());
            self.request_reconciliation(true);
            restart |= dropped & DROPPED_RESTART != 0;
        }
        invalidate_size_cache_many(changed.iter().map(PathBuf::as_path));
        if backlog && let Some(wake) = &self.notify {
            wake();
        }

        if restart {
            self.watcher = None;
            self.pending_install = None;
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

    /// Apply one inbox message. Paths whose cached sizes went stale are
    /// collected into `changed` and invalidated once per poll.
    fn apply_message(
        &mut self,
        message: WatchMessage,
        outcome: &mut WatchPoll,
        changed: &mut Vec<PathBuf>,
    ) -> bool {
        if self.binding.as_ref() != Some(&message.binding) {
            return false;
        }
        match message.kind {
            WatchMessageKind::Changed { paths, direct } => {
                if paths.is_empty() {
                    changed.push(message.binding.path);
                } else {
                    changed.extend(paths);
                }
                if direct {
                    self.request_reconciliation(false);
                } else {
                    outcome.sizes_dirty = true;
                }
                false
            }
            WatchMessageKind::Gap => {
                changed.push(message.binding.path);
                self.request_reconciliation(true);
                false
            }
            WatchMessageKind::BackendError => {
                changed.push(message.binding.path);
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

    /// No backend could subscribe to `path`. The first failure re-lists and
    /// re-measures once; repeated failures only back off further (2s, 4s, ...
    /// up to 32s) instead of re-listing and force-rescanning sizes on every
    /// retry.
    fn mark_unavailable(&mut self, path: &Path) {
        let failures = match &self.unavailable {
            Some((failed, count)) if failed == path => count.saturating_add(1),
            _ => 0,
        };
        self.unavailable = Some((path.to_path_buf(), failures));
        self.watcher = None;
        self.pending_install = None;
        self.binding = None;
        let backoff = WATCHER_RETRY_BACKOFF
            .saturating_mul(1 << failures.min(WATCHER_RETRY_BACKOFF_MAX_SHIFT));
        self.retry = Some((path.to_path_buf(), Instant::now() + backoff));
        if failures == 0 {
            invalidate_size_cache(path);
            self.request_reconciliation(true);
        }
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
    pub(super) const fn has_pending_reconciliation_for_test(&self) -> bool {
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

    #[cfg(unix)]
    #[test]
    fn polling_watchers_ignore_link_cycles_and_external_targets_but_detect_real_changes() {
        use crate::watcher_policy::{WatchDepth, WatcherBackend};
        use std::time::{Duration, UNIX_EPOCH};
        for depth in [WatchDepth::Recursive, WatchDepth::DirectoryOnly] {
            let dir = crate::testutil::TempDir::new();
            let external = crate::testutil::TempDir::new();
            let direct = dir.file("direct", "before");
            let nested = dir.file("real/nested", "before");
            let outside = external.file("target", "outside");
            std::os::unix::fs::symlink(dir.path(), dir.path().join("loop")).unwrap();
            std::os::unix::fs::symlink(external.path(), dir.path().join("escape")).unwrap();
            std::os::unix::fs::symlink(&outside, dir.path().join("file-link")).unwrap();
            let mut state = DirectoryWatcherState::default();
            let binding = state.activate_test_binding(dir.path());
            let callback = callback_for(&state, binding);
            let mut backend = install_backend(
                WatcherBackend::Polling,
                depth,
                Duration::from_secs(3600),
                callback,
            )
            .unwrap();
            assert!(
                state.receiver.try_recv().is_err(),
                "initial scan must not report a symlink cycle as backend failure"
            );
            std::fs::write(&outside, "external payload changed").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&outside)
                .unwrap()
                .set_modified(UNIX_EPOCH + Duration::from_secs(2_000_000_000))
                .unwrap();
            let new_link = dir.path().join("new-link");
            std::os::unix::fs::symlink(&outside, &new_link).unwrap();
            let target = if depth == WatchDepth::Recursive {
                nested
            } else {
                direct
            };
            std::fs::write(&target, "real payload changed").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&target)
                .unwrap()
                .set_modified(UNIX_EPOCH + Duration::from_secs(2_000_000_001))
                .unwrap();
            let DirectoryWatcher::Polling(watcher) = &backend else {
                unreachable!()
            };
            watcher.poll().unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            let local_paths = |message: WatchMessage| match message.kind {
                WatchMessageKind::Changed { paths, .. } => {
                    assert!(
                        paths
                            .iter()
                            .all(|path| !path.starts_with(dir.path().join("escape"))
                                && !path.starts_with(dir.path().join("loop"))
                                && path != &dir.path().join("file-link")),
                        "linked targets must not be watched: {paths:?}"
                    );
                    paths
                }
                other => panic!("link cycle must not force reconciliation/restart: {other:?}"),
            };
            let mut observed = false;
            let mut observed_link = false;
            while !observed || !observed_link {
                let message = state
                    .receiver
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .expect("real file modification must still be observed");
                let paths = local_paths(message);
                observed |= paths.contains(&target);
                observed_link |= paths.contains(&new_link);
            }
            // Unwatch takes notify's scan locks, completing the in-flight scan
            // before we drain the rest or remove fixtures.
            let DirectoryWatcher::Polling(watcher) = &mut backend else {
                unreachable!()
            };
            notify::Watcher::unwatch(watcher, dir.path()).unwrap();
            for message in state.receiver.try_iter() {
                local_paths(message);
            }
            drop(backend);
        }
    }

    fn callback_for(state: &DirectoryWatcherState, binding: WatchBinding) -> WatcherCallback {
        WatcherCallback {
            binding,
            canonical: None,
            sender: state.sender.clone(),
            wake_scheduled: Arc::clone(&state.wake_scheduled),
            overflowed: Arc::clone(&state.overflowed),
            wake: state.notify.clone(),
        }
    }

    fn polling_notice() -> InstalledBackend {
        InstalledBackend {
            backend: crate::watcher_policy::WatcherBackend::Polling,
            depth: crate::watcher_policy::WatchDepth::DirectoryOnly,
            fallback: true,
        }
    }

    #[test]
    fn blocked_install_is_singleflight_off_ui_and_admission_stays_bounded_after_navigation() {
        let mut state = DirectoryWatcherState::default();
        state.set_notify(Arc::new(|| {}));
        let binding = state.activate_test_binding(Path::new("/watched"));
        let callback = callback_for(&state, binding.clone());
        let budget = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (exited_tx, exited_rx) = mpsc::channel();
        assert!(
            state.start_install(binding, callback, &budget, move |_, sender, _, permit| {
                entered_tx.send(std::thread::current().id()).unwrap();
                release_rx.recv().unwrap();
                assert!(
                    sender.send(Ok(polling_notice())).is_err(),
                    "navigation retired the receiver"
                );
                drop(permit);
                exited_tx.send(()).unwrap();
            })
        );
        let worker = entered_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_ne!(worker, std::thread::current().id());
        let epoch = state.next_binding_epoch;
        let generation = state.requested_generation;
        state.ensure_binding(Path::new("/watched"));
        assert_eq!(state.next_binding_epoch, epoch);
        assert_eq!(state.requested_generation, generation);
        assert_eq!(budget.load(Ordering::Acquire), 1);
        assert!(!state.is_active());
        assert!(state.snapshot_ticket_for(Path::new("/watched")).is_some());
        // Abandonment is nonblocking; an OS call may still occupy its permit.
        state.set_notify(Arc::new(|| {}));
        let held: Vec<_> = (1..MAX_WATCHER_INSTALLS)
            .map(|_| InstallPermit::acquire(&budget).unwrap())
            .collect();
        assert!(InstallPermit::acquire(&budget).is_none());
        release_tx.send(()).unwrap();
        exited_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(InstallPermit::acquire(&budget).is_some());
        drop(held);
        assert_eq!(budget.load(Ordering::Acquire), 0);
        assert!(!state.is_active() && state.binding.is_none());
    }

    #[test]
    fn async_subscription_completion_reconciles_the_initial_listing_gap() {
        let mut state = DirectoryWatcherState::default();
        let binding = state.activate_test_binding(Path::new("/watched"));
        let before = state.snapshot_ticket().unwrap();
        state.acknowledge_snapshot(Some(before.clone()), Path::new("/watched"));
        let (sender, receiver) = mpsc::sync_channel(2);
        let (stop, stop_rx) = mpsc::channel();
        state.pending_install = Some(PendingInstall {
            binding,
            lease: WatchLease { _stop: stop },
            recovering: false,
            receiver,
        });
        sender.send(Ok(polling_notice())).unwrap();
        drop(sender);
        state.poll_install();
        assert!(state.is_active());
        assert!(state.pending_install.is_none());
        assert!(state.snapshot_ticket().unwrap().generation > before.generation);
        assert!(state.has_pending_reconciliation_for_test());
        assert!(matches!(stop_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        state.set_notify(Arc::new(|| {}));
        assert!(matches!(
            stop_rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn stale_install_completion_cannot_activate_a_new_binding() {
        let mut state = DirectoryWatcherState::default();
        let old = state.activate_test_binding(Path::new("/old"));
        let (sender, receiver) = mpsc::sync_channel(2);
        let (stop, stop_rx) = mpsc::channel();
        state.pending_install = Some(PendingInstall {
            binding: old,
            lease: WatchLease { _stop: stop },
            recovering: false,
            receiver,
        });
        let current = state.activate_test_binding(Path::new("/new"));
        let generation = state.requested_generation;
        sender.send(Ok(polling_notice())).unwrap();
        state.poll_install();
        assert_eq!(state.binding.as_ref(), Some(&current));
        assert_eq!(state.requested_generation, generation);
        assert!(!state.is_active() && state.pending_install.is_none());
        assert!(matches!(
            stop_rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn failed_promotion_keeps_polling_but_failed_initial_install_backs_off() {
        for started in [false, true] {
            let mut state = DirectoryWatcherState::default();
            let binding = state.activate_test_binding(Path::new("/watched"));
            let (sender, receiver) = mpsc::sync_channel(2);
            let (stop, stop_rx) = mpsc::channel();
            state.pending_install = Some(PendingInstall {
                binding,
                lease: WatchLease { _stop: stop },
                recovering: false,
                receiver,
            });
            if started {
                sender.send(Ok(polling_notice())).unwrap();
            }
            sender.send(Err("installer failed".into())).unwrap();
            state.poll_install();
            assert_eq!(state.is_active(), started);
            assert_eq!(state.retry.is_some(), !started);
            assert_eq!(state.binding.is_some(), started);
            assert_eq!(
                matches!(stop_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
                started
            );
        }
    }

    #[test]
    fn worker_panic_after_publication_retires_the_dead_subscription() {
        let mut state = DirectoryWatcherState::default();
        let binding = state.activate_test_binding(Path::new("/watched"));
        let callback = callback_for(&state, binding.clone());
        let budget = Arc::new(AtomicUsize::new(0));
        assert!(
            state.start_install(binding, callback, &budget, |_, sender, _, _permit| {
                sender.send(Ok(polling_notice())).unwrap();
                panic!("injected watcher installation failure after publication");
            })
        );
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while state.retry.is_none() {
            state.poll(Path::new("/watched"));
            assert!(
                Instant::now() < deadline,
                "dead subscription remained active"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!state.is_active());
        assert!(state.binding.is_none() && state.pending_install.is_none());
        assert_eq!(budget.load(Ordering::Acquire), 0);
    }

    #[test]
    fn installed_background_backend_observes_real_child_creation() {
        let temp = crate::testutil::TempDir::new();
        let mut state = DirectoryWatcherState::default();
        state.set_notify(Arc::new(|| {}));
        state.ensure_binding(temp.path());
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while !state.is_active() || state.pending_install.is_some() {
            assert!(Instant::now() < deadline, "watcher did not install");
            state.ensure_binding(temp.path());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ticket = state.snapshot_ticket();
        state.acknowledge_snapshot(ticket, temp.path());
        std::fs::write(temp.path().join("created.txt"), "payload").unwrap();
        loop {
            let outcome = state.poll(temp.path());
            if outcome.ticket.is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "subscription lost the child event"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

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

    type TestHandler = Box<dyn FnMut(Result<notify::Event, notify::Error>) + Send>;

    fn test_callback(
        root: &str,
        canonical: Option<&str>,
    ) -> (TestHandler, mpsc::Receiver<WatchMessage>) {
        let (sender, receiver) = mpsc::sync_channel(INBOX_CAPACITY);
        let callback = WatcherCallback {
            binding: WatchBinding {
                path: PathBuf::from(root),
                epoch: 1,
            },
            canonical: canonical.map(PathBuf::from),
            sender,
            wake_scheduled: Arc::new(AtomicBool::new(false)),
            overflowed: Arc::new(AtomicU8::new(0)),
            wake: None,
        };
        (callback.into_handler(), receiver)
    }

    #[test]
    fn events_reported_under_the_canonical_path_are_direct() {
        let (mut handler, receiver) = test_callback("/tmp/work", Some("/private/tmp/work"));
        handler(Ok(notify::Event::new(notify::EventKind::Any)
            .add_path(PathBuf::from("/private/tmp/work/new.txt"))));
        let message = receiver.try_recv().unwrap();
        assert!(matches!(
            message.kind,
            WatchMessageKind::Changed { direct: true, .. }
        ));

        // Deep changes come back in the binding's form, so size-cache keys
        // built from `/tmp/work/...` are invalidated.
        handler(Ok(notify::Event::new(notify::EventKind::Any)
            .add_path(PathBuf::from("/private/tmp/work/sub/deep/file.txt"))));
        let message = receiver.try_recv().unwrap();
        match message.kind {
            WatchMessageKind::Changed { paths, direct } => {
                assert!(!direct);
                assert_eq!(paths, vec![PathBuf::from("/tmp/work/sub/deep/file.txt")]);
            }
            _ => panic!("expected a change"),
        }
    }

    #[test]
    fn per_path_backend_errors_reconcile_without_restarting() {
        let (mut handler, receiver) = test_callback("/watched", None);
        let child_error = notify::Error::io(std::io::Error::other("scan race"))
            .add_path(PathBuf::from("/watched/gone.txt"));
        handler(Err(child_error));
        assert!(matches!(
            receiver.try_recv().unwrap().kind,
            WatchMessageKind::Gap
        ));

        let root_error = notify::Error::path_not_found().add_path(PathBuf::from("/watched"));
        handler(Err(root_error));
        assert!(matches!(
            receiver.try_recv().unwrap().kind,
            WatchMessageKind::BackendError
        ));

        handler(Err(notify::Error::new(notify::ErrorKind::MaxFilesWatch)));
        assert!(matches!(
            receiver.try_recv().unwrap().kind,
            WatchMessageKind::BackendError
        ));
    }

    #[test]
    fn access_events_are_ignored() {
        let (mut handler, receiver) = test_callback("/watched", None);
        for kind in [
            notify::EventKind::Access(notify::event::AccessKind::Open(
                notify::event::AccessMode::Read,
            )),
            notify::EventKind::Access(notify::event::AccessKind::Close(
                notify::event::AccessMode::Read,
            )),
        ] {
            handler(Ok(
                notify::Event::new(kind).add_path(PathBuf::from("/watched/sub"))
            ));
        }
        assert!(receiver.try_recv().is_err(), "reads queue nothing");

        handler(Ok(notify::Event::new(notify::EventKind::Create(
            notify::event::CreateKind::File,
        ))
        .add_path(PathBuf::from("/watched/new.txt"))));
        assert!(matches!(
            receiver.try_recv().unwrap().kind,
            WatchMessageKind::Changed { direct: true, .. }
        ));
    }

    #[test]
    fn a_full_inbox_collapses_into_a_gap() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let overflowed = Arc::new(AtomicU8::new(0));
        let mut handler = WatcherCallback {
            binding: WatchBinding {
                path: PathBuf::from("/watched"),
                epoch: 1,
            },
            canonical: None,
            sender,
            wake_scheduled: Arc::new(AtomicBool::new(false)),
            overflowed: Arc::clone(&overflowed),
            wake: None,
        }
        .into_handler();
        for name in ["a", "b", "c"] {
            handler(Ok(notify::Event::new(notify::EventKind::Any)
                .add_path(PathBuf::from("/watched").join(name))));
        }
        assert_eq!(overflowed.load(Ordering::Acquire), DROPPED_EVENT);
        handler(Err(notify::Error::new(notify::ErrorKind::MaxFilesWatch)));
        assert_eq!(
            overflowed.load(Ordering::Acquire),
            DROPPED_EVENT | DROPPED_RESTART,
            "a dropped backend error is remembered as one"
        );
        assert_eq!(receiver.try_iter().count(), 1, "the inbox stays bounded");

        let mut watcher = DirectoryWatcherState::default();
        let binding = watcher.activate_test_binding(Path::new("/watched"));
        watcher.acknowledge_snapshot(watcher.snapshot_ticket(), Path::new("/watched"));
        assert!(!watcher.has_pending_reconciliation_for_test());
        watcher.overflowed.store(DROPPED_EVENT, Ordering::Release);
        let outcome = watcher.poll(&binding.path);
        assert!(outcome.ticket.is_some(), "dropped events force a re-list");
        assert!(outcome.recovered_gap);
        assert_eq!(watcher.overflowed.load(Ordering::Acquire), 0);
        assert!(watcher.retry.is_none(), "a dropped change does not restart");
    }

    #[test]
    fn a_backend_error_dropped_on_a_full_inbox_still_restarts() {
        let mut watcher = DirectoryWatcherState::default();
        let binding = watcher.activate_test_binding(Path::new("/watched"));
        watcher.acknowledge_snapshot(watcher.snapshot_ticket(), Path::new("/watched"));
        watcher
            .overflowed
            .store(DROPPED_EVENT | DROPPED_RESTART, Ordering::Release);
        let outcome = watcher.poll(&binding.path);
        assert!(outcome.recovered_gap);
        assert!(
            watcher
                .retry
                .as_ref()
                .is_some_and(|(path, _)| path == &binding.path),
            "the lost backend error tears the watcher down for a retry"
        );
    }

    #[test]
    fn one_poll_applies_a_bounded_number_of_messages() {
        let mut watcher = DirectoryWatcherState::default();
        let binding = watcher.activate_test_binding(Path::new("/watched"));
        for index in 0..MAX_MESSAGES_PER_POLL + 10 {
            watcher.inject_test_message(
                binding.clone(),
                WatchMessageKind::Changed {
                    paths: vec![PathBuf::from(format!("/watched/sub/{index}"))],
                    direct: false,
                },
            );
        }
        // The callbacks set the coalescing gate when they queued these.
        watcher.wake_scheduled.store(true, Ordering::Release);
        let outcome = watcher.poll(Path::new("/watched"));
        assert!(outcome.sizes_dirty);
        assert!(
            watcher.wake_scheduled.load(Ordering::Acquire),
            "callbacks stay coalesced while a backlog remains"
        );
        assert_eq!(watcher.receiver.try_iter().count(), 10);
    }

    #[test]
    fn repeated_subscription_failures_back_off_without_relisting() {
        let mut watcher = DirectoryWatcherState::default();
        let path = Path::new("/unwatchable");
        let start = watcher.requested_generation;

        watcher.mark_unavailable(path);
        assert_eq!(
            watcher.requested_generation,
            start + 1,
            "first failure re-lists"
        );
        let first_retry = watcher.retry.as_ref().unwrap().1;

        watcher.mark_unavailable(path);
        watcher.mark_unavailable(path);
        assert_eq!(
            watcher.requested_generation,
            start + 1,
            "retries do not re-list"
        );
        let later_retry = watcher.retry.as_ref().unwrap().1;
        assert!(later_retry >= first_retry + WATCHER_RETRY_BACKOFF * 2);

        for _ in 0..10 {
            watcher.mark_unavailable(path);
        }
        let capped = watcher.retry.as_ref().unwrap().1;
        assert!(capped <= Instant::now() + WATCHER_RETRY_BACKOFF * 16);

        // Another folder failing starts its own count.
        watcher.mark_unavailable(Path::new("/other"));
        assert_eq!(watcher.requested_generation, start + 2);
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

    #[test]
    fn deep_change_only_marks_sizes_dirty() {
        let mut watcher = DirectoryWatcherState::default();
        let binding = watcher.activate_test_binding(Path::new("/w"));
        let ticket = watcher.snapshot_ticket();
        watcher.acknowledge_snapshot(ticket, Path::new("/w"));
        let generation = watcher.requested_generation;

        watcher.inject_test_message(
            binding,
            WatchMessageKind::Changed {
                paths: vec![PathBuf::from("/w/a/b")],
                direct: false,
            },
        );
        let outcome = watcher.poll(Path::new("/w"));

        assert!(outcome.sizes_dirty);
        assert!(outcome.ticket.is_none(), "the listing itself is current");
        assert_eq!(watcher.requested_generation, generation);
    }

    #[test]
    fn a_deferred_snapshot_holds_back_its_ticket_until_the_backoff() {
        let mut watcher = DirectoryWatcherState::default();
        watcher.activate_test_binding(Path::new("/w"));
        assert!(watcher.poll(Path::new("/w")).ticket.is_some());

        let ticket = watcher.snapshot_ticket();
        watcher.defer_snapshot(ticket.as_ref());
        assert!(watcher.poll(Path::new("/w")).ticket.is_none());
        assert!(watcher.has_pending_reconciliation_for_test());

        // Once the backoff has passed, the retry is handed out again.
        watcher.retry_reconciliation_at = Some(Instant::now());
        assert!(watcher.poll(Path::new("/w")).ticket.is_some());
    }

    #[test]
    fn deferring_a_ticket_from_an_old_binding_is_ignored() {
        let mut watcher = DirectoryWatcherState::default();
        watcher.activate_test_binding(Path::new("/old"));
        let stale = watcher.snapshot_ticket();
        watcher.activate_test_binding(Path::new("/current"));

        watcher.defer_snapshot(stale.as_ref());

        assert_eq!(watcher.retry_reconciliation_at, None);
        assert!(watcher.poll(Path::new("/current")).ticket.is_some());
    }

    #[test]
    fn events_on_the_folder_or_its_children_are_direct_and_deeper_ones_are_not() {
        let (mut handler, receiver) = test_callback("/w", None);
        let mut direct_for = |paths: &[&str]| {
            let mut event = notify::Event::new(notify::EventKind::Any);
            for path in paths {
                event = event.add_path(PathBuf::from(path));
            }
            handler(Ok(event));
            match receiver.try_recv().unwrap().kind {
                WatchMessageKind::Changed { direct, .. } => direct,
                other => panic!("expected a change, got {other:?}"),
            }
        };

        assert!(direct_for(&["/w/child"]));
        assert!(direct_for(&["/w"]));
        assert!(direct_for(&[]), "an event without paths re-lists");
        assert!(!direct_for(&["/w/a/grandchild"]));
        assert!(!direct_for(&["/elsewhere/file"]));
        assert!(direct_for(&["/w/a/deep", "/w/renamed"]), "any direct path");
    }
}
