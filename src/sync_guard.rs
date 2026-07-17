//! Fail-closed synchronization preflight: source-generation fingerprints,
//! health markers, and excessive-change circuit breakers.

use crate::panel::FileEntry;
use crate::sync::{SyncAction, SyncDirection, SyncPolicy, SyncStatus};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GuardPolicy {
    pub max_change_fraction: f32,
    pub minimum_changed: usize,
    pub max_actions: usize,
    pub health_marker: Option<PathBuf>,
}

impl Default for GuardPolicy {
    fn default() -> Self {
        Self {
            max_change_fraction: 0.50,
            minimum_changed: 20,
            max_actions: 10_000,
            health_marker: None,
        }
    }
}

impl GuardPolicy {
    pub fn set_marker(&mut self, value: &str) -> Result<(), String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            self.health_marker = None;
            return Ok(());
        }
        let marker = PathBuf::from(trimmed);
        if marker.is_absolute()
            || marker.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err("Health marker must be a relative path below each sync root".to_string());
        }
        self.health_marker = Some(marker);
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeStamp {
    pub volume_id: u64,
    pub generation: u64,
    pub mount_point: PathBuf,
}

impl VolumeStamp {
    fn capture(path: &Path) -> Self {
        let profile = crate::volume_profile::profile(path);
        Self {
            volume_id: profile.volume_id,
            generation: profile.generation,
            mount_point: profile.mount_point,
        }
    }

    fn still_current(&self, path: &Path) -> bool {
        let profile = crate::volume_profile::refresh(path);
        self.volume_id == profile.volume_id
            && self.generation == profile.generation
            && self.mount_point == profile.mount_point
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStamp {
    pub left_root: PathBuf,
    pub right_root: PathBuf,
    pub left_show_hidden: bool,
    pub right_show_hidden: bool,
    pub source_fingerprint: u64,
    #[serde(default)]
    pub left_volume: Option<VolumeStamp>,
    #[serde(default)]
    pub right_volume: Option<VolumeStamp>,
}

impl PlanStamp {
    pub fn filter_key(&self) -> &'static str {
        match (self.left_show_hidden, self.right_show_hidden) {
            (false, false) => "hidden:0:0",
            (false, true) => "hidden:0:1",
            (true, false) => "hidden:1:0",
            (true, true) => "hidden:1:1",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Assessment {
    pub planned_actions: usize,
    pub additions: usize,
    pub changed_existing: usize,
    pub deleted: usize,
    pub existing_pairs: usize,
    pub change_fraction: f32,
    pub circuit_breaker: Option<String>,
}

pub struct GuardedPlan<'a> {
    pub actions: &'a [SyncAction],
    pub stamp: &'a PlanStamp,
    pub policy: SyncPolicy,
    pub guard: &'a GuardPolicy,
    pub expected_settings: u64,
    pub allow_large_plan: bool,
}

pub fn capture(
    left_root: &Path,
    right_root: &Path,
    left: &[FileEntry],
    right: &[FileEntry],
    left_show_hidden: bool,
    right_show_hidden: bool,
) -> PlanStamp {
    PlanStamp {
        left_root: left_root.to_path_buf(),
        right_root: right_root.to_path_buf(),
        left_show_hidden,
        right_show_hidden,
        source_fingerprint: source_fingerprint(left_root, right_root, left, right),
        left_volume: Some(VolumeStamp::capture(left_root)),
        right_volume: Some(VolumeStamp::capture(right_root)),
    }
}

/// Read both roots once and derive the plan and its baseline from those exact
/// entries. This prevents a plan from being paired with a newer fingerprint.
pub fn build_plan(
    left_root: &Path,
    right_root: &Path,
    left_show_hidden: bool,
    right_show_hidden: bool,
    policy: SyncPolicy,
) -> Result<(Vec<SyncAction>, PlanStamp), String> {
    let left = read_entries(left_root, left_show_hidden)?;
    let right = read_entries(right_root, right_show_hidden)?;
    let stamp = capture(
        left_root,
        right_root,
        &left,
        &right,
        left_show_hidden,
        right_show_hidden,
    );
    Ok((crate::sync::sync_diff(&left, &right, policy), stamp))
}

pub fn settings_fingerprint(policy: SyncPolicy, guard: &GuardPolicy, filter: &str) -> u64 {
    let mut hash = Fnv::new();
    hash.bytes(&1_u32.to_le_bytes());
    hash.bytes(&[match policy {
        SyncPolicy::MirrorLeftToRight => 1,
        SyncPolicy::MirrorRightToLeft => 2,
        SyncPolicy::TwoWay => 3,
    }]);
    hash.bytes(&guard.max_change_fraction.to_bits().to_le_bytes());
    hash.bytes(&guard.minimum_changed.to_le_bytes());
    hash.bytes(&guard.max_actions.to_le_bytes());
    if let Some(marker) = &guard.health_marker {
        hash.path(marker);
    }
    hash.bytes(filter.as_bytes());
    hash.finish()
}

pub fn assess(actions: &[SyncAction], guard: &GuardPolicy) -> Assessment {
    let mut assessment = Assessment::default();
    for action in actions {
        if !matches!(action.direction, SyncDirection::Skip) {
            assessment.planned_actions += 1;
            match action.status {
                SyncStatus::LeftOnly | SyncStatus::RightOnly => assessment.additions += 1,
                SyncStatus::Identical
                | SyncStatus::DirectoryPair
                | SyncStatus::TypeConflict
                | SyncStatus::CaseConflict => {}
                SyncStatus::LeftNewer | SyncStatus::RightNewer | SyncStatus::Differing => {
                    assessment.changed_existing += 1;
                }
            }
        }
        if matches!(
            action.status,
            SyncStatus::LeftNewer
                | SyncStatus::RightNewer
                | SyncStatus::Differing
                | SyncStatus::Identical
                | SyncStatus::DirectoryPair
                | SyncStatus::TypeConflict
        ) {
            assessment.existing_pairs += 1;
        }
    }
    assessment.change_fraction = if assessment.existing_pairs == 0 {
        0.0
    } else {
        assessment.changed_existing as f32 / assessment.existing_pairs as f32
    };
    assessment.circuit_breaker = if assessment.planned_actions > guard.max_actions {
        Some(format!(
            "Plan has {} actions; limit is {}",
            assessment.planned_actions, guard.max_actions
        ))
    } else if assessment.changed_existing >= guard.minimum_changed
        && assessment.change_fraction > guard.max_change_fraction
    {
        Some(format!(
            "Plan changes {:.0}% of existing pairs; limit is {:.0}%",
            assessment.change_fraction * 100.0,
            guard.max_change_fraction * 100.0
        ))
    } else {
        None
    };
    assessment
}

pub fn validate(plan: &GuardedPlan<'_>) -> Result<Assessment, String> {
    if settings_fingerprint(plan.policy, plan.guard, plan.stamp.filter_key())
        != plan.expected_settings
    {
        return Err("Synchronization settings changed after plan capture".to_string());
    }
    if plan
        .stamp
        .left_volume
        .as_ref()
        .is_some_and(|stamp| !stamp.still_current(&plan.stamp.left_root))
        || plan
            .stamp
            .right_volume
            .as_ref()
            .is_some_and(|stamp| !stamp.still_current(&plan.stamp.right_root))
    {
        return Err("Synchronization volume changed or remounted; rebuild the plan".to_string());
    }
    let left = read_entries(&plan.stamp.left_root, plan.stamp.left_show_hidden)?;
    let right = read_entries(&plan.stamp.right_root, plan.stamp.right_show_hidden)?;
    let current = source_fingerprint(&plan.stamp.left_root, &plan.stamp.right_root, &left, &right);
    if current != plan.stamp.source_fingerprint {
        return Err("Synchronization baseline is stale; rescan before applying".to_string());
    }
    verify_health_markers(
        plan.actions,
        &plan.stamp.left_root,
        &plan.stamp.right_root,
        plan.guard,
    )?;
    let assessment = assess(plan.actions, plan.guard);
    if assessment.circuit_breaker.is_some() && !plan.allow_large_plan {
        return Err(assessment
            .circuit_breaker
            .clone()
            .unwrap_or_else(|| "Synchronization circuit breaker opened".to_string()));
    }
    Ok(assessment)
}

fn verify_health_markers(
    actions: &[SyncAction],
    left_root: &Path,
    right_root: &Path,
    guard: &GuardPolicy,
) -> Result<(), String> {
    let Some(marker) = &guard.health_marker else {
        return Ok(());
    };
    let writes_left = actions
        .iter()
        .any(|action| action.direction == SyncDirection::ToLeft);
    let writes_right = actions
        .iter()
        .any(|action| action.direction == SyncDirection::ToRight);
    for root in [
        writes_left.then_some(left_root),
        writes_right.then_some(right_root),
    ]
    .into_iter()
    .flatten()
    {
        let path = root.join(marker);
        if !path.is_file() {
            return Err(format!("Health marker is missing: {}", path.display()));
        }
    }
    Ok(())
}

fn read_entries(root: &Path, show_hidden: bool) -> Result<Vec<FileEntry>, String> {
    let mut entries = Vec::new();
    let directory = std::fs::read_dir(root)
        .map_err(|error| format!("Could not rescan {}: {error}", root.display()))?;
    for item in directory {
        let item = item.map_err(|error| format!("Could not read sync entry: {error}"))?;
        let path = item.path();
        if !show_hidden
            && path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        let metadata = std::fs::metadata(&path)
            .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
        if let Some(entry) = FileEntry::from_meta(path, &metadata) {
            entries.push(entry);
        }
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

fn source_fingerprint(
    left_root: &Path,
    right_root: &Path,
    left: &[FileEntry],
    right: &[FileEntry],
) -> u64 {
    let mut hash = Fnv::new();
    hash.path(left_root);
    let mut left = left.iter().collect::<Vec<_>>();
    left.sort_by(|a, b| a.path.cmp(&b.path));
    for entry in left {
        hash.entry(entry);
    }
    hash.bytes(&[0xff]);
    hash.path(right_root);
    let mut right = right.iter().collect::<Vec<_>>();
    right.sort_by(|a, b| a.path.cmp(&b.path));
    for entry in right {
        hash.entry(entry);
    }
    hash.finish()
}

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn path(&mut self, path: &Path) {
        self.bytes(path.to_string_lossy().as_bytes());
        self.bytes(&[0]);
    }

    fn entry(&mut self, entry: &FileEntry) {
        self.path(&entry.path);
        self.bytes(&entry.size.to_le_bytes());
        self.bytes(&[u8::from(entry.is_dir)]);
        let modified = entry
            .modified
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos());
        self.bytes(&modified.to_le_bytes());
    }

    fn finish(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(status: SyncStatus, direction: SyncDirection) -> SyncAction {
        crate::sync::test_action("file", status, direction)
    }

    #[test]
    fn circuit_breaker_counts_changes_separately_from_additions() {
        let guard = GuardPolicy {
            minimum_changed: 2,
            max_change_fraction: 0.5,
            ..Default::default()
        };
        let actions = vec![
            action(SyncStatus::LeftNewer, SyncDirection::ToRight),
            action(SyncStatus::Differing, SyncDirection::ToLeft),
            action(SyncStatus::Identical, SyncDirection::Skip),
            action(SyncStatus::LeftOnly, SyncDirection::ToRight),
        ];
        let assessment = assess(&actions, &guard);
        assert_eq!(assessment.changed_existing, 2);
        assert_eq!(assessment.additions, 1);
        assert!(assessment.circuit_breaker.is_some());
    }

    #[test]
    fn health_marker_rejects_parent_escape() {
        let mut guard = GuardPolicy::default();
        assert!(guard.set_marker("../mounted").is_err());
        assert!(guard.set_marker(".commander-health").is_ok());
    }

    #[test]
    fn settings_fingerprint_changes_with_policy_and_marker() {
        let mut guard = GuardPolicy::default();
        let first = settings_fingerprint(SyncPolicy::TwoWay, &guard, "");
        guard.set_marker("healthy").unwrap();
        let second = settings_fingerprint(SyncPolicy::TwoWay, &guard, "");
        let third = settings_fingerprint(SyncPolicy::MirrorLeftToRight, &guard, "");
        assert_ne!(first, second);
        assert_ne!(second, third);
    }

    #[test]
    fn fresh_plan_and_validation_use_the_same_hidden_scope() {
        let left = crate::testutil::TempDir::new();
        let right = crate::testutil::TempDir::new();
        left.file("visible.txt", "visible");
        left.file(".private", "hidden");
        let guard = GuardPolicy::default();
        let (actions, stamp) = build_plan(
            left.path(),
            right.path(),
            false,
            false,
            SyncPolicy::MirrorLeftToRight,
        )
        .unwrap();
        assert_eq!(actions.len(), 1);
        let settings =
            settings_fingerprint(SyncPolicy::MirrorLeftToRight, &guard, stamp.filter_key());
        assert!(
            validate(&GuardedPlan {
                actions: &actions,
                stamp: &stamp,
                policy: SyncPolicy::MirrorLeftToRight,
                guard: &guard,
                expected_settings: settings,
                allow_large_plan: false,
            })
            .is_ok()
        );
    }
}
