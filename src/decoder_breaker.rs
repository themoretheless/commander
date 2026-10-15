//! Per-format decoder circuit breakers (research **J010**).
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const FAILURE_THRESHOLD: u32 = 3;
const COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default)]
struct FormatState {
    /// Distinct files of this format that failed since the last success. One
    /// corrupt file retried again and again counts once, so it cannot
    /// quarantine every other file of its format.
    failing_paths: HashSet<PathBuf>,
    /// A probe retry is in flight: its failure re-quarantines at once.
    half_open: bool,
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
    admit_at(table.formats.entry(key).or_default(), path, Instant::now())
}

/// [`admit`] against one format's state at `now`; an expired quarantine
/// resets the format so its files are tried again.
fn admit_at(state: &mut FormatState, path: &Path, now: Instant) -> AdmitDecision {
    if let Some(until) = state.quarantined_until {
        if now < until {
            if state.probe_armed.as_deref() == Some(path) {
                state.probe_armed = None;
                state.quarantined_until = None;
                // Half-open: a single failed probe re-quarantines the format.
                state.half_open = true;
                return AdmitDecision::ProbeRetry;
            }
            return AdmitDecision::Quarantined;
        }
        state.quarantined_until = None;
        state.failing_paths.clear();
        state.half_open = false;
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
    state.failing_paths.insert(path.to_path_buf());
    if state.half_open || state.failing_paths.len() >= FAILURE_THRESHOLD as usize {
        state.quarantined_until = Some(Instant::now() + COOLDOWN);
        state.probe_armed = None;
        state.half_open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fail `FAILURE_THRESHOLD` distinct files sharing `path`'s format,
    /// ending with `path` itself.
    fn quarantine_format_of(path: &Path) {
        let ext = path.extension().unwrap().to_string_lossy().into_owned();
        for n in 1..FAILURE_THRESHOLD {
            record_failure(&PathBuf::from(format!("/tmp/other-{n}.{ext}")));
        }
        record_failure(path);
    }

    #[test]
    fn repeated_failures_quarantine_until_probe_retry() {
        let path = PathBuf::from("/tmp/sample.j010fmt");
        quarantine_format_of(&path);
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
        quarantine_format_of(&path);
        arm_probe_retry(&path);
        assert_eq!(admit(&other), AdmitDecision::Quarantined);
        assert_eq!(admit(&path), AdmitDecision::ProbeRetry);
    }

    #[test]
    fn failed_probe_requarantines_immediately() {
        let path = PathBuf::from("/tmp/sample.j010halfopen");
        quarantine_format_of(&path);
        arm_probe_retry(&path);
        assert_eq!(admit(&path), AdmitDecision::ProbeRetry);
        record_failure(&path);
        assert_eq!(admit(&path), AdmitDecision::Quarantined);
    }

    #[test]
    fn one_file_failing_repeatedly_does_not_quarantine_its_format() {
        let corrupt = PathBuf::from("/tmp/corrupt.j010repeat");
        for _ in 0..FAILURE_THRESHOLD * 2 {
            record_failure(&corrupt);
        }
        assert_eq!(admit(&corrupt), AdmitDecision::Allow);
        assert_eq!(
            admit(Path::new("/tmp/fine.j010repeat")),
            AdmitDecision::Allow
        );
    }

    #[test]
    fn expired_cooldown_readmits_the_format_with_a_fresh_count() {
        let path = Path::new("/tmp/expired.j010cool");
        let start = Instant::now();
        let mut state = FormatState {
            failing_paths: (0..FAILURE_THRESHOLD)
                .map(|n| PathBuf::from(format!("/tmp/{n}.j010cool")))
                .collect(),
            half_open: true,
            quarantined_until: Some(start + COOLDOWN),
            probe_armed: None,
        };
        // Still cooling down one instant before the deadline.
        let before = start + COOLDOWN - Duration::from_millis(1);
        assert_eq!(
            admit_at(&mut state, path, before),
            AdmitDecision::Quarantined
        );
        assert!(state.quarantined_until.is_some());
        // At the deadline the format is allowed again and its failure count
        // and half-open probe are reset, so it starts counting afresh.
        assert_eq!(
            admit_at(&mut state, path, start + COOLDOWN),
            AdmitDecision::Allow
        );
        assert_eq!(state.quarantined_until, None);
        assert!(state.failing_paths.is_empty());
        assert!(!state.half_open);
        assert_eq!(
            admit_at(&mut state, path, start + COOLDOWN * 2),
            AdmitDecision::Allow
        );
    }
}
