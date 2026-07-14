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
    pub fn label(self) -> &'static str {
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

fn assumed_latency(backend: BackendKind) -> f64 {
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
    pub fn valid(self) -> bool {
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

fn rule_schema() -> u32 {
    RULE_SCHEMA
}

static RULES: OnceLock<Mutex<RuleStore>> = OnceLock::new();

fn rules_path() -> PathBuf {
    crate::fs_util::config_dir().join("transfer-rules.json")
}

fn load_store_at(path: &Path) -> RuleStore {
    std::fs::File::open(path)
        .ok()
        .and_then(|file| serde_json::from_reader::<_, RuleStore>(file).ok())
        .filter(|store| store.schema == RULE_SCHEMA)
        .unwrap_or_default()
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

    #[test]
    fn unlimited_limiter_never_sleeps_or_cancels() {
        let mut limiter = BandwidthLimiter::new(VolumeRule::default());
        limiter.consume(1024, || true).unwrap();
    }
}
