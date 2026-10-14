//! Per-format decoder circuit breakers (research **J010**).
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const FAILURE_THRESHOLD: u32 = 3;
const COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default)]
struct FormatState {
    consecutive_failures: u32,
    quarantined_until: Option<Instant>,
    /// The file the user asked to retry; only it may consume the probe, so a
    /// background preload of another file of the same format cannot steal it.
    probe_armed: Option<PathBuf>,
}

#[derive(Default)]
struct BreakerTable {
    formats: HashMap<String, FormatState>,
}

fn table() -> &'static Mutex<BreakerTable> {
    static TABLE: OnceLock<Mutex<BreakerTable>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(BreakerTable::default()))
}

fn format_key(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmitDecision {
    Allow,
    Quarantined,
    ProbeRetry,
}

pub fn admit(path: &Path) -> AdmitDecision {
    let key = format_key(path);
    let mut table = crate::lock_util::recover(table());
    let state = table.formats.entry(key).or_default();
    let now = Instant::now();
    if let Some(until) = state.quarantined_until {
        if now < until {
            if state.probe_armed.as_deref() == Some(path) {
                state.probe_armed = None;
                state.quarantined_until = None;
                // Half-open: a single failed probe re-quarantines the format.
                state.consecutive_failures = FAILURE_THRESHOLD - 1;
                return AdmitDecision::ProbeRetry;
            }
            return AdmitDecision::Quarantined;
        }
        state.quarantined_until = None;
        state.consecutive_failures = 0;
    }
    AdmitDecision::Allow
}

pub fn arm_probe_retry(path: &Path) {
    let key = format_key(path);
    let mut table = crate::lock_util::recover(table());
    let state = table.formats.entry(key).or_default();
    if state.quarantined_until.is_some() {
        state.probe_armed = Some(path.to_path_buf());
    }
}

pub fn record_success(path: &Path) {
    let key = format_key(path);
    crate::lock_util::recover(table())
        .formats
        .insert(key, FormatState::default());
}

pub fn record_failure(path: &Path) {
    let key = format_key(path);
    let mut table = crate::lock_util::recover(table());
    let state = table.formats.entry(key).or_default();
    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
    if state.consecutive_failures >= FAILURE_THRESHOLD {
        state.quarantined_until = Some(Instant::now() + COOLDOWN);
        state.probe_armed = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_failures_quarantine_until_probe_retry() {
        let path = PathBuf::from("/tmp/sample.j010fmt");
        for _ in 0..FAILURE_THRESHOLD {
            record_failure(&path);
        }
        assert_eq!(admit(&path), AdmitDecision::Quarantined);
        arm_probe_retry(&path);
        assert_eq!(admit(&path), AdmitDecision::ProbeRetry);
        record_success(&path);
        assert_eq!(admit(&path), AdmitDecision::Allow);
    }

    #[test]
    fn probe_is_reserved_for_the_retried_path() {
        let path = PathBuf::from("/tmp/user.j010probe");
        let other = PathBuf::from("/tmp/preload.j010probe");
        for _ in 0..FAILURE_THRESHOLD {
            record_failure(&path);
        }
        arm_probe_retry(&path);
        assert_eq!(admit(&other), AdmitDecision::Quarantined);
        assert_eq!(admit(&path), AdmitDecision::ProbeRetry);
    }

    #[test]
    fn failed_probe_requarantines_immediately() {
        let path = PathBuf::from("/tmp/sample.j010halfopen");
        for _ in 0..FAILURE_THRESHOLD {
            record_failure(&path);
        }
        arm_probe_retry(&path);
        assert_eq!(admit(&path), AdmitDecision::ProbeRetry);
        record_failure(&path);
        assert_eq!(admit(&path), AdmitDecision::Quarantined);
    }
}
