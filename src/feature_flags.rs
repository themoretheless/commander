//! Persisted rollout controls and immediate kill switches for risky subsystems.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const SCHEMA: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskyFeature {
    ContentIndex,
    ImagePreview,
    ExternalProviders,
}

impl RiskyFeature {
    pub const ALL: [Self; 3] = [
        Self::ContentIndex,
        Self::ImagePreview,
        Self::ExternalProviders,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::ContentIndex => "Content index",
            Self::ImagePreview => "Image preview",
            Self::ExternalProviders => "External providers",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::ContentIndex => "content_index",
            Self::ImagePreview => "image_preview",
            Self::ExternalProviders => "external_providers",
        }
    }

    fn kill_environment(self) -> &'static str {
        match self {
            Self::ContentIndex => "COMMANDER_DISABLE_CONTENT_INDEX",
            Self::ImagePreview => "COMMANDER_DISABLE_IMAGE_PREVIEW",
            Self::ExternalProviders => "COMMANDER_DISABLE_EXTERNAL_PROVIDERS",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FeaturePolicy {
    killed: bool,
    rollout_percent: u8,
}

impl Default for FeaturePolicy {
    fn default() -> Self {
        Self {
            killed: false,
            rollout_percent: 100,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FeatureStore {
    schema: u32,
    cohort: u64,
    policies: BTreeMap<RiskyFeature, FeaturePolicy>,
}

impl FeatureStore {
    fn new(cohort: u64) -> Self {
        let mut store = Self {
            schema: SCHEMA,
            cohort,
            policies: BTreeMap::new(),
        };
        store.normalize();
        store
    }

    fn normalize(&mut self) {
        self.schema = SCHEMA;
        for feature in RiskyFeature::ALL {
            let policy = self.policies.entry(feature).or_default();
            policy.rollout_percent = policy.rollout_percent.min(100);
        }
    }

    fn snapshot(&self, feature: RiskyFeature, environment_killed: bool) -> FeatureSnapshot {
        let policy = self.policies.get(&feature).copied().unwrap_or_default();
        let bucket = rollout_bucket(self.cohort, feature);
        FeatureSnapshot {
            feature,
            runtime_killed: policy.killed,
            environment_killed,
            rollout_percent: policy.rollout_percent,
            bucket,
            enabled: !policy.killed && !environment_killed && bucket < policy.rollout_percent,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureSnapshot {
    pub feature: RiskyFeature,
    pub runtime_killed: bool,
    pub environment_killed: bool,
    pub rollout_percent: u8,
    pub bucket: u8,
    pub enabled: bool,
}

#[cfg(not(test))]
fn feature_path() -> PathBuf {
    crate::fs_util::config_dir().join("runtime-features.json")
}

#[cfg(test)]
fn feature_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "commander-test-runtime-features-{}.json",
        std::process::id()
    ))
}

fn generated_cohort() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let mut hasher = blake3::Hasher::new();
    hasher.update(&nanos.to_le_bytes());
    hasher.update(&std::process::id().to_le_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&hasher.finalize().as_bytes()[..8]);
    u64::from_le_bytes(bytes)
}

fn load_at(path: &Path) -> FeatureStore {
    let mut store = std::fs::File::open(path)
        .ok()
        .and_then(|file| serde_json::from_reader::<_, FeatureStore>(file).ok())
        .filter(|store| store.schema == SCHEMA)
        .unwrap_or_else(|| FeatureStore::new(generated_cohort()));
    store.normalize();
    store
}

fn save_at(path: &Path, store: &FeatureStore) -> bool {
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return false;
    }
    serde_json::to_string_pretty(store)
        .ok()
        .is_some_and(|json| crate::fs_util::write_atomic(path, &json))
}

fn runtime_store() -> &'static Mutex<FeatureStore> {
    static STORE: OnceLock<Mutex<FeatureStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        let path = feature_path();
        let store = load_at(&path);
        let _ = save_at(&path, &store);
        Mutex::new(store)
    })
}

fn environment_killed(feature: RiskyFeature) -> bool {
    std::env::var_os(feature.kill_environment()).is_some_and(|value| {
        let value = value.to_string_lossy();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

fn rollout_bucket(cohort: u64, feature: RiskyFeature) -> u8 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&cohort.to_le_bytes());
    hasher.update(feature.key().as_bytes());
    hasher.finalize().as_bytes()[0] % 100
}

pub fn snapshot(feature: RiskyFeature) -> FeatureSnapshot {
    crate::lock_util::recover(runtime_store()).snapshot(feature, environment_killed(feature))
}

pub fn snapshots() -> Vec<FeatureSnapshot> {
    RiskyFeature::ALL.into_iter().map(snapshot).collect()
}

pub fn enabled(feature: RiskyFeature) -> bool {
    snapshot(feature).enabled
}

fn update(feature: RiskyFeature, change: impl FnOnce(&mut FeaturePolicy)) -> bool {
    let mut store = crate::lock_util::recover(runtime_store());
    let previous = store.clone();
    change(store.policies.entry(feature).or_default());
    store.normalize();
    if save_at(&feature_path(), &store) {
        true
    } else {
        *store = previous;
        false
    }
}

pub fn set_killed(feature: RiskyFeature, killed: bool) -> bool {
    update(feature, |policy| policy.killed = killed)
}

pub fn set_rollout_percent(feature: RiskyFeature, percent: u8) -> bool {
    update(feature, |policy| policy.rollout_percent = percent.min(100))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn cohort_buckets_are_stable_and_bounded() {
        let store = FeatureStore::new(42);
        for feature in RiskyFeature::ALL {
            let first = store.snapshot(feature, false);
            let second = store.snapshot(feature, false);
            assert_eq!(first.bucket, second.bucket);
            assert!(first.bucket < 100);
        }
    }

    #[test]
    fn kill_switch_and_rollout_boundary_both_disable_a_feature() {
        let mut store = FeatureStore::new(17);
        let feature = RiskyFeature::ContentIndex;
        let bucket = store.snapshot(feature, false).bucket;
        store.policies.get_mut(&feature).unwrap().rollout_percent = bucket;
        assert!(!store.snapshot(feature, false).enabled);

        store.policies.get_mut(&feature).unwrap().rollout_percent = 100;
        assert!(store.snapshot(feature, false).enabled);
        store.policies.get_mut(&feature).unwrap().killed = true;
        assert!(!store.snapshot(feature, false).enabled);
        assert!(!store.snapshot(feature, true).enabled);
    }

    #[test]
    fn persisted_controls_round_trip_and_clamp_rollouts() {
        let temp = TempDir::new();
        let path = temp.path().join("features.json");
        let mut store = FeatureStore::new(99);
        let policy = store
            .policies
            .get_mut(&RiskyFeature::ExternalProviders)
            .unwrap();
        policy.killed = true;
        policy.rollout_percent = 240;
        assert!(save_at(&path, &store));
        let loaded = load_at(&path);
        let policy = loaded
            .policies
            .get(&RiskyFeature::ExternalProviders)
            .unwrap();
        assert!(policy.killed);
        assert_eq!(policy.rollout_percent, 100);
        assert_eq!(loaded.cohort, 99);
    }
}
