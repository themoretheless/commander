//! Persisted rollout controls and immediate kill switches for risky subsystems.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

const SCHEMA: u32 = 1;
const STORE: crate::persistence::StoreSpec =
    crate::persistence::StoreSpec::new("commander.feature_flags", 1, 1024 * 1024)
        .allow_legacy_schema_marker();

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

    fn bit(self) -> u8 {
        match self {
            Self::ContentIndex => 1 << 0,
            Self::ImagePreview => 1 << 1,
            Self::ExternalProviders => 1 << 2,
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

    fn fail_closed(cohort: u64) -> Self {
        let mut store = Self::new(cohort);
        for policy in store.policies.values_mut() {
            policy.killed = true;
            policy.rollout_percent = 0;
        }
        store
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

struct LoadedFeatureStore {
    store: FeatureStore,
    gate: crate::persistence::StoreGate,
    blocked: bool,
}

fn load_at_with(persist: &dyn crate::persistence::Persist, path: &Path) -> LoadedFeatureStore {
    let loaded = crate::persistence::load_enveloped::<FeatureStore>(persist, path, STORE);
    let status = loaded.gate.status();
    let mut gate = loaded.gate;
    let blocked_status = matches!(
        status,
        crate::persistence::LoadStatus::Corrupt
            | crate::persistence::LoadStatus::FutureVersion
            | crate::persistence::LoadStatus::Unreadable
    );
    if let Some(mut store) = loaded.value {
        if store.schema == SCHEMA && !blocked_status {
            store.normalize();
            return LoadedFeatureStore {
                store,
                gate,
                blocked: false,
            };
        }
        if !blocked_status {
            gate.block(if store.schema > SCHEMA {
                crate::persistence::LoadStatus::FutureVersion
            } else {
                crate::persistence::LoadStatus::Corrupt
            });
        }
    }
    if blocked_status || status != crate::persistence::LoadStatus::Missing {
        crate::persistence::record_unreadable("Feature flags");
    }
    let blocked = status != crate::persistence::LoadStatus::Missing;
    LoadedFeatureStore {
        store: if status == crate::persistence::LoadStatus::Missing {
            FeatureStore::new(generated_cohort())
        } else {
            FeatureStore::fail_closed(generated_cohort())
        },
        gate,
        blocked,
    }
}

fn save_at_with(
    persist: &dyn crate::persistence::Persist,
    path: &Path,
    store: &FeatureStore,
    gate: &mut crate::persistence::StoreGate,
) -> Result<crate::persistence::AtomicWriteOutcome, crate::persistence::JsonSaveError> {
    crate::persistence::save_enveloped(
        persist,
        path,
        STORE,
        store,
        gate,
        crate::persistence::SaveIntent::Explicit,
    )
}

#[cfg(test)]
fn load_at(path: &Path) -> FeatureStore {
    load_at_with(&crate::persistence::FsPersist::default(), path).store
}

#[cfg(test)]
fn save_at(path: &Path, store: &FeatureStore) -> bool {
    let persist = crate::persistence::FsPersist::default();
    let mut gate = crate::persistence::StoreGate::missing();
    match save_at_with(&persist, path, store, &mut gate) {
        Ok(crate::persistence::AtomicWriteOutcome::Durable) => true,
        Ok(crate::persistence::AtomicWriteOutcome::CommittedButNotDurable(error)) => {
            crate::persistence::record_durability_warning("Feature flags", &error);
            true
        }
        Err(error) => {
            crate::persistence::record_json_save_failure("Feature flags", &error);
            false
        }
    }
}

struct RuntimeControls {
    state: Mutex<LoadedFeatureStore>,
    environment_killed_mask: u8,
    enabled_mask: AtomicU8,
    persistence: std::sync::Arc<dyn crate::persistence::Persist>,
}

fn runtime_controls() -> &'static RuntimeControls {
    static CONTROLS: OnceLock<RuntimeControls> = OnceLock::new();
    CONTROLS.get_or_init(|| {
        let path = feature_path();
        let persistence = crate::persistence::fs_persist();
        let state = load_at_with(persistence.as_ref(), &path);
        let environment_killed_mask = RiskyFeature::ALL
            .into_iter()
            .filter(|feature| environment_killed(*feature))
            .fold(0_u8, |mask, feature| mask | feature.bit());
        RuntimeControls {
            enabled_mask: AtomicU8::new(if state.blocked {
                0
            } else {
                effective_mask(&state.store, environment_killed_mask)
            }),
            state: Mutex::new(state),
            environment_killed_mask,
            persistence,
        }
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
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest.as_bytes()[..8]);
    (u64::from_le_bytes(bytes) % 100) as u8
}

fn effective_mask(store: &FeatureStore, environment_killed_mask: u8) -> u8 {
    RiskyFeature::ALL.into_iter().fold(0_u8, |mask, feature| {
        let environment_killed = environment_killed_mask & feature.bit() != 0;
        if store.snapshot(feature, environment_killed).enabled {
            mask | feature.bit()
        } else {
            mask
        }
    })
}

fn apply_update(
    state: &mut LoadedFeatureStore,
    persist: &dyn crate::persistence::Persist,
    path: &Path,
    change: impl FnOnce(&mut FeaturePolicy),
    feature: RiskyFeature,
) -> Result<crate::persistence::AtomicWriteOutcome, crate::persistence::JsonSaveError> {
    if state.blocked {
        return Err(crate::persistence::JsonSaveError::Blocked(
            state.gate.status(),
        ));
    }
    let previous = state.store.clone();
    change(state.store.policies.entry(feature).or_default());
    state.store.normalize();
    let store = state.store.clone();
    match save_at_with(persist, path, &store, &mut state.gate) {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            state.store = previous;
            Err(error)
        }
    }
}

pub fn snapshot(feature: RiskyFeature) -> FeatureSnapshot {
    let controls = runtime_controls();
    crate::lock_util::recover(&controls.state).store.snapshot(
        feature,
        controls.environment_killed_mask & feature.bit() != 0,
    )
}

pub fn snapshots() -> Vec<FeatureSnapshot> {
    let controls = runtime_controls();
    let state = crate::lock_util::recover(&controls.state);
    RiskyFeature::ALL
        .into_iter()
        .map(|feature| {
            state.store.snapshot(
                feature,
                controls.environment_killed_mask & feature.bit() != 0,
            )
        })
        .collect()
}

pub fn enabled(feature: RiskyFeature) -> bool {
    runtime_controls().enabled_mask.load(Ordering::Acquire) & feature.bit() != 0
}

fn update(feature: RiskyFeature, change: impl FnOnce(&mut FeaturePolicy)) -> bool {
    let controls = runtime_controls();
    let mut state = crate::lock_util::recover(&controls.state);
    match apply_update(
        &mut state,
        controls.persistence.as_ref(),
        &feature_path(),
        change,
        feature,
    ) {
        Ok(crate::persistence::AtomicWriteOutcome::Durable) => {
            controls.enabled_mask.store(
                effective_mask(&state.store, controls.environment_killed_mask),
                Ordering::Release,
            );
            true
        }
        Ok(crate::persistence::AtomicWriteOutcome::CommittedButNotDurable(error)) => {
            crate::persistence::record_durability_warning("Feature flags", &error);
            controls.enabled_mask.store(
                effective_mask(&state.store, controls.environment_killed_mask),
                Ordering::Release,
            );
            true
        }
        Err(error) => {
            crate::persistence::record_json_save_failure("Feature flags", &error);
            false
        }
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone, Copy)]
    enum CommitMode {
        Reject,
        Weak,
    }

    struct ScriptedPersist {
        commits: AtomicUsize,
        mode: CommitMode,
    }

    impl crate::persistence::Persist for ScriptedPersist {
        fn read(
            &self,
            _path: &Path,
            _max_bytes: usize,
        ) -> Result<crate::persistence::ReadOutcome, crate::persistence::ReadFailure> {
            Ok(crate::persistence::ReadOutcome::Missing)
        }

        fn commit(
            &self,
            _path: &Path,
            _bytes: &[u8],
            _expected: crate::persistence::ExpectedRevision,
        ) -> Result<crate::persistence::AtomicWriteOutcome, crate::persistence::PreCommitError>
        {
            self.commits.fetch_add(1, Ordering::Relaxed);
            match self.mode {
                CommitMode::Reject => Err(crate::persistence::PreCommitError::Conflict),
                CommitMode::Weak => Ok(
                    crate::persistence::AtomicWriteOutcome::CommittedButNotDurable(
                        std::io::Error::other("injected sync failure"),
                    ),
                ),
            }
        }
    }

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

    #[test]
    fn effective_mask_matches_snapshots_for_every_feature() {
        let mut store = FeatureStore::new(31);
        store
            .policies
            .get_mut(&RiskyFeature::ImagePreview)
            .unwrap()
            .killed = true;
        let environment = RiskyFeature::ExternalProviders.bit();
        let mask = effective_mask(&store, environment);
        for feature in RiskyFeature::ALL {
            assert_eq!(
                mask & feature.bit() != 0,
                store
                    .snapshot(feature, environment & feature.bit() != 0)
                    .enabled
            );
        }
    }

    #[test]
    fn missing_store_initializes_in_memory_without_write_on_load() {
        let temp = TempDir::new();
        let path = temp.path().join("features.json");
        let persist = crate::persistence::FsPersist::default();

        let loaded = load_at_with(&persist, &path);

        assert!(!loaded.blocked);
        assert_eq!(
            loaded.gate.status(),
            crate::persistence::LoadStatus::Missing
        );
        assert!(!path.exists());
    }

    #[test]
    fn legacy_inner_schema_loads_without_rewriting_the_store() {
        let temp = TempDir::new();
        let path = temp.file(
            "features.json",
            r#"{"schema":1,"cohort":41,"policies":{"content_index":{"killed":true,"rollout_percent":0}}}"#,
        );
        let original = std::fs::read(&path).unwrap();

        let loaded = load_at_with(&crate::persistence::FsPersist::default(), &path);

        assert!(!loaded.blocked);
        assert_eq!(loaded.gate.status(), crate::persistence::LoadStatus::Legacy);
        assert!(
            !loaded
                .store
                .snapshot(RiskyFeature::ContentIndex, false)
                .enabled
        );
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn corrupt_store_fails_closed_and_cannot_overwrite_the_source() {
        let temp = TempDir::new();
        let path = temp.file("features.json", "{not-json");
        let original = std::fs::read(&path).unwrap();
        let persist = crate::persistence::FsPersist::default();

        let mut loaded = load_at_with(&persist, &path);

        assert!(loaded.blocked);
        assert_eq!(
            loaded.gate.status(),
            crate::persistence::LoadStatus::Corrupt
        );
        assert!(
            RiskyFeature::ALL
                .into_iter()
                .all(|feature| !loaded.store.snapshot(feature, false).enabled)
        );
        let store = loaded.store.clone();
        assert!(matches!(
            save_at_with(&persist, &path, &store, &mut loaded.gate),
            Err(crate::persistence::JsonSaveError::Blocked(
                crate::persistence::LoadStatus::Corrupt
            ))
        ));
        assert_eq!(std::fs::read(path).unwrap(), original);
    }

    #[test]
    fn future_envelope_fails_closed_without_downgrade() {
        let temp = TempDir::new();
        let path = temp.path().join("features.json");
        let bytes = br#"{
          "format":"commander.persist",
          "store":"commander.feature_flags",
          "schema":99,
          "generation":1,
          "payload":{}
        }"#;
        std::fs::write(&path, bytes).unwrap();
        let persist = crate::persistence::FsPersist::default();

        let loaded = load_at_with(&persist, &path);

        assert!(loaded.blocked);
        assert_eq!(
            loaded.gate.status(),
            crate::persistence::LoadStatus::FutureVersion
        );
        assert!(
            RiskyFeature::ALL
                .into_iter()
                .all(|feature| !loaded.store.snapshot(feature, false).enabled)
        );
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn wrong_store_and_future_inner_schema_both_fail_closed() {
        let temp = TempDir::new();
        let path = temp.path().join("features.json");
        let cases = [
            br#"{
              "format":"commander.persist",
              "store":"commander.session",
              "schema":1,
              "generation":1,
              "payload":{}
            }"#
            .as_slice(),
            br#"{
              "format":"commander.persist",
              "store":"commander.feature_flags",
              "schema":1,
              "generation":1,
              "payload":{"schema":99,"cohort":1,"policies":{}}
            }"#
            .as_slice(),
        ];
        for bytes in cases {
            std::fs::write(&path, bytes).unwrap();
            let loaded = load_at_with(&crate::persistence::FsPersist::default(), &path);
            assert!(loaded.blocked);
            assert!(
                RiskyFeature::ALL
                    .into_iter()
                    .all(|feature| !loaded.store.snapshot(feature, false).enabled)
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn failed_update_rolls_back_memory_and_weak_commit_applies_once() {
        let original = FeatureStore::new(7);
        let mut rejected = LoadedFeatureStore {
            store: original.clone(),
            gate: crate::persistence::StoreGate::missing(),
            blocked: false,
        };
        let reject = ScriptedPersist {
            commits: AtomicUsize::new(0),
            mode: CommitMode::Reject,
        };
        let result = apply_update(
            &mut rejected,
            &reject,
            Path::new("features.json"),
            |policy| policy.killed = true,
            RiskyFeature::ContentIndex,
        );
        assert!(result.is_err());
        assert_eq!(rejected.store, original);
        assert_eq!(reject.commits.load(Ordering::Relaxed), 1);

        let weak = ScriptedPersist {
            commits: AtomicUsize::new(0),
            mode: CommitMode::Weak,
        };
        let mut committed = LoadedFeatureStore {
            store: FeatureStore::new(7),
            gate: crate::persistence::StoreGate::missing(),
            blocked: false,
        };
        let result = apply_update(
            &mut committed,
            &weak,
            Path::new("features.json"),
            |policy| policy.killed = true,
            RiskyFeature::ContentIndex,
        );
        assert!(matches!(
            result,
            Ok(crate::persistence::AtomicWriteOutcome::CommittedButNotDurable(_))
        ));
        assert!(
            committed
                .store
                .policies
                .get(&RiskyFeature::ContentIndex)
                .unwrap()
                .killed
        );
        assert_eq!(weak.commits.load(Ordering::Relaxed), 1);
        assert_eq!(
            committed.gate.status(),
            crate::persistence::LoadStatus::Current
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_symlink_store_fails_closed_without_following_it() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new();
        let target = temp.file("target.json", r#"{"schema":1,"cohort":1,"policies":{}}"#);
        let path = temp.path().join("features.json");
        symlink(&target, &path).unwrap();

        let loaded = load_at_with(&crate::persistence::FsPersist::default(), &path);

        assert!(loaded.blocked);
        assert_eq!(
            loaded.gate.status(),
            crate::persistence::LoadStatus::Unreadable
        );
        assert!(
            RiskyFeature::ALL
                .into_iter()
                .all(|feature| !loaded.store.snapshot(feature, false).enabled)
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            br#"{"schema":1,"cohort":1,"policies":{}}"#
        );
    }
}
