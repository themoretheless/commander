//! Per-volume transfer telemetry, adaptive limits, and persisted network rules.

use crate::volume_profile::{BackendKind, VolumeProfile};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_SAMPLES: usize = 128;
const RULE_SCHEMA: u32 = 1;
pub const BANDWIDTH_CHOICES: [Option<u64>; 5] = [
    None,
    Some(1024 * 1024),
    Some(5 * 1024 * 1024),
    Some(20 * 1024 * 1024),
    Some(100 * 1024 * 1024),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FastPath {
    Rename,
    Clone,
    Sparse,
    DeltaFixed,
    DeltaCdc,
    Resumed,
    Native,
    Buffered,
}

impl FastPath {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rename => "rename",
            Self::Clone => "clone",
            Self::Sparse => "sparse",
            Self::DeltaFixed => "delta",
            Self::DeltaCdc => "delta CDC",
            Self::Resumed => "resumed",
            Self::Native => "native copy",
            Self::Buffered => "buffered copy",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TuningSnapshot {
    pub p50_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub error_rate: f64,
    pub samples: usize,
    pub concurrency: usize,
}

#[derive(Clone, Copy)]
struct Sample {
    latency_ms: f64,
    success: bool,
}

static SAMPLES: OnceLock<Mutex<HashMap<u64, VecDeque<Sample>>>> = OnceLock::new();

pub fn record(profile: &VolumeProfile, elapsed: Duration, success: bool) {
    let samples = SAMPLES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut samples = crate::lock_util::recover(samples);
    let volume = samples.entry(profile.volume_id).or_default();
    volume.push_back(Sample {
        latency_ms: elapsed.as_secs_f64() * 1_000.0,
        success,
    });
    while volume.len() > MAX_SAMPLES {
        volume.pop_front();
    }
}

pub fn snapshot(profile: &VolumeProfile) -> TuningSnapshot {
    let samples = SAMPLES.get_or_init(|| Mutex::new(HashMap::new()));
    let samples = crate::lock_util::recover(samples);
    snapshot_from(
        profile,
        samples
            .get(&profile.volume_id)
            .map(VecDeque::as_slices)
            .map(|(left, right)| left.iter().chain(right.iter()).copied().collect())
            .unwrap_or_default(),
    )
}

fn snapshot_from(profile: &VolumeProfile, samples: Vec<Sample>) -> TuningSnapshot {
    let latencies = samples
        .iter()
        .map(|sample| sample.latency_ms)
        .collect::<Vec<_>>();
    let latency = if latencies.is_empty() {
        let assumed = assumed_latency(profile.backend);
        crate::measurement::LatencyPercentiles {
            p50_ms: assumed,
            p95_ms: assumed,
            p99_ms: assumed,
            samples: 0,
        }
    } else {
        crate::measurement::latency_percentiles(&latencies)
    };
    let error_rate = if samples.is_empty() {
        0.0
    } else {
        samples.iter().filter(|sample| !sample.success).count() as f64 / samples.len() as f64
    };
    let desired = if error_rate >= 0.10 || latency.p95_ms >= 80.0 {
        1
    } else if latency.p95_ms >= 15.0 {
        2
    } else {
        4
    };
    TuningSnapshot {
        p50_latency_ms: latency.p50_ms,
        p95_latency_ms: latency.p95_ms,
        p99_latency_ms: latency.p99_ms,
        error_rate,
        samples: samples.len(),
        concurrency: desired.min(profile.capabilities.max_concurrency).max(1),
    }
}

const fn assumed_latency(backend: BackendKind) -> f64 {
    match backend {
        BackendKind::LocalFast => 1.0,
        BackendKind::LocalSlow => 8.0,
        BackendKind::Removable => 15.0,
        BackendKind::Remote => 35.0,
        BackendKind::Unknown => 25.0,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietHours {
    pub start_hour: u8,
    pub end_hour: u8,
}

impl QuietHours {
    pub const fn valid(self) -> bool {
        self.start_hour < 24 && self.end_hour < 24 && self.start_hour != self.end_hour
    }

    pub fn contains(self, hour: u8) -> bool {
        if !self.valid() || hour >= 24 {
            return false;
        }
        if self.start_hour < self.end_hour {
            (self.start_hour..self.end_hour).contains(&hour)
        } else {
            hour >= self.start_hour || hour < self.end_hour
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeRule {
    pub max_bytes_per_second: Option<u64>,
    pub quiet_hours: Option<QuietHours>,
}

impl VolumeRule {
    pub fn normalized(mut self) -> Self {
        self.max_bytes_per_second = self
            .max_bytes_per_second
            .filter(|limit| *limit >= 64 * 1024);
        self.quiet_hours = self.quiet_hours.filter(|hours| hours.valid());
        self
    }

    pub fn is_quiet_at(self, hour: u8) -> bool {
        self.quiet_hours.is_some_and(|hours| hours.contains(hour))
    }

    pub fn is_quiet_now(self) -> bool {
        use chrono::Timelike;
        self.is_quiet_at(chrono::Local::now().hour() as u8)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RuleStore {
    #[serde(default = "rule_schema")]
    schema: u32,
    #[serde(default)]
    volumes: HashMap<String, VolumeRule>,
}

impl Default for RuleStore {
    fn default() -> Self {
        Self {
            schema: RULE_SCHEMA,
            volumes: HashMap::new(),
        }
    }
}

const fn rule_schema() -> u32 {
    RULE_SCHEMA
}

static RULES: OnceLock<Mutex<RuleStore>> = OnceLock::new();

fn rules_path() -> PathBuf {
    crate::fs_util::config_dir().join("transfer-rules.json")
}

/// Load the persisted rules. A file that does not parse, or that a newer
/// version wrote, is moved aside to `<name>.bak` so the next [`set_rule`]
/// cannot silently replace the user's rules with an otherwise empty store.
fn load_store_at(path: &Path) -> RuleStore {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return RuleStore::default(),
        Err(error) => {
            log::warn!(
                target: "commander::transfer_tuning",
                "Could not read transfer rules {}: {error}",
                path.display()
            );
            return RuleStore::default();
        }
    };
    let problem = match serde_json::from_slice::<RuleStore>(&bytes) {
        Ok(store) if store.schema == RULE_SCHEMA => return store,
        Ok(store) => format!("unsupported schema {}", store.schema),
        Err(error) => error.to_string(),
    };
    let mut backup = path.as_os_str().to_owned();
    backup.push(".bak");
    let backup = PathBuf::from(backup);
    match std::fs::rename(path, &backup) {
        Ok(()) => log::warn!(
            target: "commander::transfer_tuning",
            "Ignoring transfer rules {} ({problem}); kept as {}",
            path.display(),
            backup.display()
        ),
        Err(error) => log::warn!(
            target: "commander::transfer_tuning",
            "Ignoring transfer rules {} ({problem}); could not keep a backup at {}: {error}",
            path.display(),
            backup.display()
        ),
    }
    RuleStore::default()
}

fn save_store_at(path: &Path, store: &RuleStore) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create transfer-rule directory: {error}"))?;
    }
    let json = serde_json::to_string_pretty(store)
        .map_err(|error| format!("Could not serialize transfer rules: {error}"))?;
    if crate::fs_util::write_atomic(path, &json) {
        Ok(())
    } else {
        Err("Could not save transfer rules".to_string())
    }
}

fn rule_key(volume_id: u64) -> String {
    format!("{volume_id:016x}")
}

fn rules() -> &'static Mutex<RuleStore> {
    RULES.get_or_init(|| Mutex::new(load_store_at(&rules_path())))
}

pub fn rule_for(profile: &VolumeProfile) -> VolumeRule {
    crate::lock_util::recover(rules())
        .volumes
        .get(&rule_key(profile.volume_id))
        .copied()
        .unwrap_or_default()
        .normalized()
}

pub fn set_rule(profile: &VolumeProfile, rule: VolumeRule) -> Result<(), String> {
    let mut guard = crate::lock_util::recover(rules());
    let mut updated = guard.clone();
    let rule = rule.normalized();
    if rule == VolumeRule::default() {
        updated.volumes.remove(&rule_key(profile.volume_id));
    } else {
        updated.volumes.insert(rule_key(profile.volume_id), rule);
    }
    save_store_at(&rules_path(), &updated)?;
    *guard = updated;
    Ok(())
}

pub struct BandwidthLimiter {
    limit: Option<u64>,
    started: Instant,
    bytes: u64,
}

impl BandwidthLimiter {
    pub fn new(rule: VolumeRule) -> Self {
        Self {
            limit: rule.normalized().max_bytes_per_second,
            started: Instant::now(),
            bytes: 0,
        }
    }

    pub fn consume(&mut self, bytes: usize, cancelled: impl Fn() -> bool) -> std::io::Result<()> {
        let Some(limit) = self.limit else {
            return Ok(());
        };
        self.bytes = self.bytes.saturating_add(bytes as u64);
        let expected = Duration::from_secs_f64(self.bytes as f64 / limit as f64);
        while self.started.elapsed() < expected {
            if cancelled() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
            let remaining = expected.saturating_sub(self.started.elapsed());
            std::thread::sleep(remaining.min(Duration::from_millis(20)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use crate::volume_profile::{BackendKind, VolumeCapabilities};

    fn profile(backend: BackendKind, max_concurrency: usize) -> VolumeProfile {
        VolumeProfile {
            volume_id: 7,
            generation: 1,
            backend,
            filesystem: "test".to_string(),
            mount_point: PathBuf::from("/test"),
            read_only: false,
            case_sensitive: Some(true),
            capabilities: VolumeCapabilities {
                atomic_rename: true,
                clone: false,
                sparse: true,
                resumable: true,
                delta: true,
                max_concurrency,
            },
            reason: "test".to_string(),
        }
    }

    #[test]
    fn p95_and_errors_reduce_concurrency() {
        let profile = profile(BackendKind::Remote, 4);
        let fast = snapshot_from(
            &profile,
            (0..20)
                .map(|_| Sample {
                    latency_ms: 2.0,
                    success: true,
                })
                .collect(),
        );
        assert_eq!(fast.concurrency, 4);
        assert_eq!(fast.p50_latency_ms, 2.0);
        assert_eq!(fast.p95_latency_ms, 2.0);
        assert_eq!(fast.p99_latency_ms, 2.0);

        let unstable = snapshot_from(
            &profile,
            vec![
                Sample {
                    latency_ms: 10.0,
                    success: true,
                },
                Sample {
                    latency_ms: 10.0,
                    success: false,
                },
            ],
        );
        assert_eq!(unstable.concurrency, 1);
        assert_eq!(unstable.error_rate, 0.5);
    }

    #[test]
    fn quiet_hours_handle_same_day_and_midnight_ranges() {
        let daytime = QuietHours {
            start_hour: 9,
            end_hour: 17,
        };
        assert!(daytime.contains(9));
        assert!(!daytime.contains(17));

        let overnight = QuietHours {
            start_hour: 22,
            end_hour: 7,
        };
        assert!(overnight.contains(23));
        assert!(overnight.contains(3));
        assert!(!overnight.contains(12));
    }

    #[test]
    fn rules_round_trip_and_normalize_invalid_values() {
        let temp = TempDir::new();
        let path = temp.path().join("rules.json");
        let mut store = RuleStore::default();
        store.volumes.insert(
            "one".to_string(),
            VolumeRule {
                max_bytes_per_second: Some(5 * 1024 * 1024),
                quiet_hours: Some(QuietHours {
                    start_hour: 22,
                    end_hour: 7,
                }),
            },
        );
        save_store_at(&path, &store).unwrap();
        assert_eq!(load_store_at(&path).volumes, store.volumes);

        let invalid = VolumeRule {
            max_bytes_per_second: Some(1),
            quiet_hours: Some(QuietHours {
                start_hour: 24,
                end_hour: 7,
            }),
        };
        assert_eq!(invalid.normalized(), VolumeRule::default());
    }

    fn uniform(latency_ms: f64, failures: usize, total: usize) -> Vec<Sample> {
        (0..total)
            .map(|index| Sample {
                latency_ms,
                success: index >= failures,
            })
            .collect()
    }

    #[test]
    fn no_samples_assume_the_backend_latency() {
        for (backend, latency, concurrency) in [
            (BackendKind::LocalFast, 1.0, 4),
            (BackendKind::LocalSlow, 8.0, 4),
            (BackendKind::Removable, 15.0, 2),
            (BackendKind::Remote, 35.0, 2),
            (BackendKind::Unknown, 25.0, 2),
        ] {
            let snapshot = snapshot_from(&profile(backend, 4), Vec::new());
            assert_eq!(snapshot.p95_latency_ms, latency, "{backend:?}");
            assert_eq!(snapshot.concurrency, concurrency, "{backend:?}");
            assert_eq!(snapshot.error_rate, 0.0);
            assert_eq!(snapshot.samples, 0);
        }
    }

    #[test]
    fn concurrency_tiers_switch_at_15ms_80ms_and_ten_percent_errors() {
        let profile = profile(BackendKind::Remote, 4);
        let concurrency =
            |latency, failures, total| snapshot_from(&profile, uniform(latency, failures, total));
        assert_eq!(concurrency(14.9, 0, 20).concurrency, 4);
        assert_eq!(concurrency(15.0, 0, 20).concurrency, 2);
        assert_eq!(concurrency(79.9, 0, 20).concurrency, 2);
        assert_eq!(concurrency(80.0, 0, 20).concurrency, 1);
        let at_limit = concurrency(2.0, 1, 10);
        assert_eq!(at_limit.error_rate, 0.10);
        assert_eq!(at_limit.concurrency, 1);
        assert_eq!(concurrency(2.0, 1, 11).concurrency, 4);
    }

    #[test]
    fn concurrency_respects_the_volume_capability_and_never_reaches_zero() {
        let fast = uniform(2.0, 0, 20);
        assert_eq!(
            snapshot_from(&profile(BackendKind::LocalFast, 0), fast.clone()).concurrency,
            1
        );
        assert_eq!(
            snapshot_from(&profile(BackendKind::LocalFast, 2), fast).concurrency,
            2
        );
    }

    #[test]
    fn quiet_hours_edges_and_invalid_ranges() {
        let overnight = QuietHours {
            start_hour: 22,
            end_hour: 7,
        };
        assert!(overnight.contains(22));
        assert!(overnight.contains(0));
        assert!(overnight.contains(6));
        assert!(!overnight.contains(7));
        assert!(!overnight.contains(21));
        assert!(!overnight.contains(24));

        let empty = QuietHours {
            start_hour: 5,
            end_hour: 5,
        };
        assert!(!empty.valid());
        assert!((0..24).all(|hour| !empty.contains(hour)));
    }

    #[test]
    fn limited_limiter_throttles_and_honours_cancel() {
        let rule = VolumeRule {
            max_bytes_per_second: Some(64 * 1024),
            quiet_hours: None,
        };
        let mut limiter = BandwidthLimiter::new(rule);
        let error = limiter.consume(1024 * 1024, || true).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);

        // 6.4 KiB at 64 KiB/s is ~100 ms of budget.
        let mut limiter = BandwidthLimiter::new(rule);
        let started = Instant::now();
        limiter.consume(64 * 1024 / 10, || false).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(50));
    }

    #[test]
    fn unreadable_or_newer_rules_are_kept_aside_instead_of_overwritten() {
        let temp = TempDir::new();
        let path = temp.path().join("rules.json");
        let backup = temp.path().join("rules.json.bak");
        assert!(load_store_at(&path).volumes.is_empty());
        assert!(!backup.exists(), "a missing file needs no backup");

        for contents in [
            "{ not json".to_string(),
            format!(
                r#"{{"schema": {}, "volumes": {{"one": {{"max_bytes_per_second": 1048576}}}}}}"#,
                RULE_SCHEMA + 1
            ),
        ] {
            std::fs::write(&path, &contents).unwrap();
            assert!(load_store_at(&path).volumes.is_empty());
            assert!(!path.exists());
            assert_eq!(std::fs::read_to_string(&backup).unwrap(), contents);
        }
    }

    #[test]
    fn unlimited_limiter_never_sleeps_or_cancels() {
        let mut limiter = BandwidthLimiter::new(VolumeRule::default());
        limiter.consume(1024, || true).unwrap();
    }
}
