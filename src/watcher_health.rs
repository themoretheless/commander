//! Aggregate, path-free health counters for filesystem observation.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatcherHealth {
    pub events: u64,
    pub direct_events: u64,
    pub deep_events: u64,
    pub rescan_signals: u64,
    pub backend_errors: u64,
    pub start_failures: u64,
    pub watch_failures: u64,
    pub reconnects: u64,
    pub listing_reconciliations: u64,
    pub gap_reconciliations: u64,
    pub size_reconciliations: u64,
    #[serde(default)]
    pub event_batches: u64,
    #[serde(default)]
    pub coalesced_events: u64,
    #[serde(default)]
    pub native_starts: u64,
    #[serde(default)]
    pub polling_starts: u64,
    #[serde(default)]
    pub shallow_starts: u64,
    #[serde(default)]
    pub backend_fallbacks: u64,
}

#[derive(Default)]
struct Counters {
    events: AtomicU64,
    direct_events: AtomicU64,
    deep_events: AtomicU64,
    rescan_signals: AtomicU64,
    backend_errors: AtomicU64,
    start_failures: AtomicU64,
    watch_failures: AtomicU64,
    reconnects: AtomicU64,
    listing_reconciliations: AtomicU64,
    gap_reconciliations: AtomicU64,
    size_reconciliations: AtomicU64,
    event_batches: AtomicU64,
    coalesced_events: AtomicU64,
    native_starts: AtomicU64,
    polling_starts: AtomicU64,
    shallow_starts: AtomicU64,
    backend_fallbacks: AtomicU64,
}

fn counters() -> &'static Counters {
    static COUNTERS: OnceLock<Counters> = OnceLock::new();
    COUNTERS.get_or_init(Counters::default)
}

fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_event() {
    bump(&counters().events);
}

pub(crate) fn record_direct_event() {
    bump(&counters().direct_events);
}

pub(crate) fn record_deep_event() {
    bump(&counters().deep_events);
}

pub(crate) fn record_rescan_signal() {
    bump(&counters().rescan_signals);
}

pub(crate) fn record_backend_error() {
    bump(&counters().backend_errors);
}

pub(crate) fn record_start_failure() {
    bump(&counters().start_failures);
}

pub(crate) fn record_watch_failure() {
    bump(&counters().watch_failures);
}

pub(crate) fn record_reconnect() {
    bump(&counters().reconnects);
}

pub(crate) fn record_listing_reconciliation(recovered_gap: bool) {
    let counters = counters();
    bump(&counters.listing_reconciliations);
    if recovered_gap {
        bump(&counters.gap_reconciliations);
    }
}

pub(crate) fn record_size_reconciliation() {
    bump(&counters().size_reconciliations);
}

pub(crate) fn record_event_batch(coalesced: bool) {
    if coalesced {
        bump(&counters().coalesced_events);
    } else {
        bump(&counters().event_batches);
    }
}

pub(crate) fn record_watcher_start(
    backend: crate::watcher_policy::WatcherBackend,
    depth: crate::watcher_policy::WatchDepth,
    fallback: bool,
) {
    let counters = counters();
    match backend {
        crate::watcher_policy::WatcherBackend::Native => bump(&counters.native_starts),
        crate::watcher_policy::WatcherBackend::Polling => bump(&counters.polling_starts),
    }
    if depth == crate::watcher_policy::WatchDepth::DirectoryOnly {
        bump(&counters.shallow_starts);
    }
    if fallback {
        bump(&counters.backend_fallbacks);
    }
}

pub fn snapshot() -> WatcherHealth {
    let counters = counters();
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    WatcherHealth {
        events: load(&counters.events),
        direct_events: load(&counters.direct_events),
        deep_events: load(&counters.deep_events),
        rescan_signals: load(&counters.rescan_signals),
        backend_errors: load(&counters.backend_errors),
        start_failures: load(&counters.start_failures),
        watch_failures: load(&counters.watch_failures),
        reconnects: load(&counters.reconnects),
        listing_reconciliations: load(&counters.listing_reconciliations),
        gap_reconciliations: load(&counters.gap_reconciliations),
        size_reconciliations: load(&counters.size_reconciliations),
        event_batches: load(&counters.event_batches),
        coalesced_events: load(&counters.coalesced_events),
        native_starts: load(&counters.native_starts),
        polling_starts: load(&counters.polling_starts),
        shallow_starts: load(&counters.shallow_starts),
        backend_fallbacks: load(&counters.backend_fallbacks),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_captures_gap_recovery_without_paths() {
        let before = snapshot();
        record_event();
        record_rescan_signal();
        record_listing_reconciliation(true);
        let after = snapshot();

        assert!(after.events > before.events);
        assert!(after.rescan_signals > before.rescan_signals);
        assert!(after.listing_reconciliations > before.listing_reconciliations);
        assert!(after.gap_reconciliations > before.gap_reconciliations);
    }

    #[test]
    fn snapshot_counts_batches_and_backend_policy_without_paths() {
        let before = snapshot();
        record_event_batch(false);
        record_event_batch(true);
        record_watcher_start(
            crate::watcher_policy::WatcherBackend::Polling,
            crate::watcher_policy::WatchDepth::DirectoryOnly,
            true,
        );
        let after = snapshot();

        assert!(after.event_batches > before.event_batches);
        assert!(after.coalesced_events > before.coalesced_events);
        assert!(after.polling_starts > before.polling_starts);
        assert!(after.shallow_starts > before.shallow_starts);
        assert!(after.backend_fallbacks > before.backend_fallbacks);
    }
}
