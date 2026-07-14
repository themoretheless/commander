//! Durable per-step operation journal and recovery primitives.

use crate::operation::{
    ClassifiedFailure, DurabilityProfile, FailureClass, IdempotencyKey, OperationGroupId,
    OperationId,
};
use crate::panel::FileEntry;
use crate::path_identity::PathIdentity;
use crate::transfer::{
    CopyMethod, OverwritePolicy, PostTransferAction, ResumeCheckpoint, TransferExpectation,
    TransferKind, TransferSpec,
};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const JOURNAL_SCHEMA: u32 = 3;
const MAX_OPERATIONS: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationStatus {
    Planned,
    Running,
    Stopped,
    Failed,
    NeedsReview,
    Completed,
    RolledBack,
}

impl OperationStatus {
    pub fn recoverable(self) -> bool {
        matches!(
            self,
            Self::Planned | Self::Running | Self::Stopped | Self::Failed | Self::NeedsReview
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Planned => "Planned",
            Self::Running => "Interrupted",
            Self::Stopped => "Stopped",
            Self::Failed => "Failed",
            Self::NeedsReview => "Review required",
            Self::Completed => "Completed",
            Self::RolledBack => "Rolled back",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Planned,
    Running,
    Requeued,
    Completed,
    Skipped,
    Failed,
    RolledBack,
}

impl StepStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Planned => "Planned",
            Self::Running => "Interrupted",
            Self::Requeued => "Requeued",
            Self::Completed => "Completed",
            Self::Skipped => "Skipped",
            Self::Failed => "Failed",
            Self::RolledBack => "Rolled back",
        }
    }

    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Skipped | Self::RolledBack)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperationStep {
    pub key: IdempotencyKey,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub source_before: Option<PathIdentity>,
    pub destination_before: Option<PathIdentity>,
    #[serde(default)]
    pub landing: Option<PathBuf>,
    #[serde(default)]
    pub landing_before: Option<PathIdentity>,
    pub destination_after: Option<PathIdentity>,
    pub staging: Option<PathBuf>,
    #[serde(default)]
    pub checkpoint: Option<ResumeCheckpoint>,
    #[serde(default)]
    pub fast_path: Option<crate::transfer_tuning::FastPath>,
    pub status: StepStatus,
    pub attempts: u32,
    pub failure: Option<ClassifiedFailure>,
    pub preflight_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OperationRecord {
    pub id: OperationId,
    pub group_id: Option<OperationGroupId>,
    pub kind: TransferKind,
    pub target: PathBuf,
    pub policy: OverwritePolicy,
    pub method: CopyMethod,
    pub durability: DurabilityProfile,
    #[serde(default)]
    pub post_success: Option<PostTransferAction>,
    #[serde(default)]
    pub rollback_cleanup: Option<PathBuf>,
    pub status: OperationStatus,
    pub created_at_secs: u64,
    pub updated_at_secs: u64,
    pub steps: Vec<OperationStep>,
}

impl OperationRecord {
    pub fn completed_steps(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.status == StepStatus::Completed)
            .count()
    }

    pub fn failed_steps(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.status == StepStatus::Failed)
            .count()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Journal {
    schema: u32,
    pub operations: Vec<OperationRecord>,
}

impl Default for Journal {
    fn default() -> Self {
        Self {
            schema: JOURNAL_SCHEMA,
            operations: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepairItem {
    pub path: PathBuf,
    pub action: String,
    pub automatic: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepairPlan {
    pub operation_id: Option<OperationId>,
    pub completed: Vec<RepairItem>,
    pub remaining: Vec<RepairItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanStaging {
    pub path: PathBuf,
    pub identity: PathIdentity,
}

#[derive(Clone, Debug)]
pub struct RecoveryInventory {
    pub operations: Vec<OperationRecord>,
    pub orphans: Vec<OrphanStaging>,
}

static JOURNAL_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn journal_path() -> PathBuf {
    crate::fs_util::config_dir().join("operation-journal.json")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn load_at(path: &Path) -> Result<Journal, String> {
    match std::fs::File::open(path) {
        Ok(file) => {
            let journal: Journal = serde_json::from_reader(file)
                .map_err(|error| format!("Operation journal is corrupt: {error}"))?;
            if !(1..=JOURNAL_SCHEMA).contains(&journal.schema) {
                return Err(format!(
                    "Unsupported operation journal schema {}; expected {JOURNAL_SCHEMA}",
                    journal.schema
                ));
            }
            Ok(journal)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
        Err(error) => Err(format!("Could not open operation journal: {error}")),
    }
}

fn save_at(path: &Path, journal: &Journal) -> Result<(), String> {
    let json = serde_json::to_string_pretty(journal)
        .map_err(|error| format!("Could not serialize operation journal: {error}"))?;
    let parent = path
        .parent()
        .ok_or_else(|| "Operation journal has no parent directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create operation journal directory: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("Could not create operation journal staging file: {error}"))?;
    let write_result = (|| {
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("Could not durably save operation journal: {error}"));
    }
    Ok(())
}

fn mutate<T>(change: impl FnOnce(&mut Journal) -> Result<T, String>) -> Result<T, String> {
    let lock = JOURNAL_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = crate::lock_util::recover(lock);
    let path = journal_path();
    let mut journal = load_at(&path)?;
    journal.schema = JOURNAL_SCHEMA;
    let result = change(&mut journal)?;
    if journal.operations.len() > MAX_OPERATIONS {
        let drop_count = journal.operations.len() - MAX_OPERATIONS;
        journal.operations.drain(0..drop_count);
    }
    save_at(&path, &journal)?;
    Ok(result)
}

pub fn load() -> Result<Journal, String> {
    let lock = JOURNAL_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = crate::lock_util::recover(lock);
    load_at(&journal_path())
}

fn step_key(spec: &TransferSpec, index: usize, destination: &Path) -> IdempotencyKey {
    spec.expectations
        .get(index)
        .and_then(|expectation| expectation.key.clone())
        .unwrap_or_else(|| spec.operation_id.step_key(index, destination))
}

pub fn begin(spec: &TransferSpec) -> Result<(), String> {
    mutate(|journal| {
        let now = now_secs();
        if let Some(existing) = journal
            .operations
            .iter_mut()
            .find(|operation| operation.id == spec.operation_id)
        {
            if existing.status == OperationStatus::RolledBack {
                return Err(format!(
                    "Operation {} was already rolled back",
                    spec.operation_id.0
                ));
            }
            if existing.kind != spec.kind
                || existing.target != spec.target
                || existing.group_id != spec.group_id
                || existing.policy != spec.policy
                || existing.method != spec.method
                || existing.durability != spec.durability
                || existing.post_success != spec.post_success
                || existing.rollback_cleanup != spec.rollback_cleanup
            {
                return Err(format!(
                    "Operation {} was reused with a different transfer contract",
                    spec.operation_id.0
                ));
            }
            let finalization_only = spec.entries.is_empty()
                && existing
                    .steps
                    .iter()
                    .all(|step| matches!(step.status, StepStatus::Completed | StepStatus::Skipped));
            if !finalization_only && existing.steps.len() != spec.entries.len() {
                return Err(format!(
                    "Operation {} was reused with a different manifest length",
                    spec.operation_id.0
                ));
            }
            for (index, entry) in spec.entries.iter().enumerate() {
                let destination = spec.target.join(&entry.name);
                let key = step_key(spec, index, &destination);
                let matches_manifest = existing.steps.iter().any(|step| {
                    step.key == key && step.source == entry.path && step.destination == destination
                });
                if !matches_manifest {
                    return Err(format!(
                        "Operation {} was reused with a different manifest entry",
                        spec.operation_id.0
                    ));
                }
            }
            existing.status = OperationStatus::Running;
            existing.updated_at_secs = now;
            return Ok(());
        }
        let steps = spec
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let destination = spec.target.join(&entry.name);
                let expectation = spec.expectations.get(index);
                let source = expectation
                    .and_then(|value| value.source.as_ref().ok())
                    .cloned();
                let destination_before = expectation
                    .and_then(|value| value.destination.as_ref().ok())
                    .cloned();
                let preflight_error = expectation.and_then(|value| {
                    value
                        .source
                        .as_ref()
                        .err()
                        .or_else(|| value.destination.as_ref().err())
                        .cloned()
                });
                OperationStep {
                    key: step_key(spec, index, &destination),
                    source: entry.path.clone(),
                    destination,
                    source_before: source,
                    destination_before,
                    landing: None,
                    landing_before: None,
                    destination_after: None,
                    staging: None,
                    checkpoint: None,
                    fast_path: None,
                    status: StepStatus::Planned,
                    attempts: 0,
                    failure: None,
                    preflight_error,
                }
            })
            .collect();
        journal.operations.push(OperationRecord {
            id: spec.operation_id.clone(),
            group_id: spec.group_id.clone(),
            kind: spec.kind,
            target: spec.target.clone(),
            policy: spec.policy,
            method: spec.method,
            durability: spec.durability,
            post_success: spec.post_success.clone(),
            rollback_cleanup: spec.rollback_cleanup.clone(),
            status: OperationStatus::Running,
            created_at_secs: now,
            updated_at_secs: now,
            steps,
        });
        Ok(())
    })
}

fn update_step(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    update: impl FnOnce(&mut OperationStep),
) -> Result<(), String> {
    mutate(|journal| {
        let operation = journal
            .operations
            .iter_mut()
            .find(|operation| &operation.id == operation_id)
            .ok_or_else(|| format!("Unknown operation {}", operation_id.0))?;
        let step = operation
            .steps
            .iter_mut()
            .find(|step| &step.key == key)
            .ok_or_else(|| format!("Unknown operation step {}", key.0))?;
        update(step);
        operation.updated_at_secs = now_secs();
        Ok(())
    })
}

pub fn mark_running(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    staging: &Path,
    landing: &Path,
    landing_before: PathIdentity,
) -> Result<(), String> {
    update_step(operation_id, key, |step| {
        if !step.status.terminal() {
            step.status = StepStatus::Running;
            step.staging = Some(staging.to_path_buf());
            step.landing = Some(landing.to_path_buf());
            step.landing_before = Some(landing_before);
            step.attempts = step.attempts.saturating_add(1);
            step.failure = None;
            if step
                .checkpoint
                .as_ref()
                .is_some_and(|checkpoint| checkpoint.staging != staging)
            {
                step.checkpoint = None;
            }
        }
    })
}

pub fn mark_checkpoint(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    checkpoint: ResumeCheckpoint,
) -> Result<(), String> {
    update_step(operation_id, key, |step| {
        if !step.status.terminal() {
            step.status = StepStatus::Running;
            step.staging = Some(checkpoint.staging.clone());
            step.checkpoint = Some(checkpoint);
            step.failure = None;
        }
    })
}

pub fn mark_requeued(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    source_before: PathIdentity,
) -> Result<(), String> {
    update_step(operation_id, key, |step| {
        if !step.status.terminal() {
            step.status = StepStatus::Requeued;
            step.source_before = Some(source_before);
            step.staging = None;
            step.landing = None;
            step.landing_before = None;
            step.checkpoint = None;
            step.fast_path = None;
        }
    })
}

pub fn mark_completed(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    destination: &Path,
    fast_path: crate::transfer_tuning::FastPath,
) -> Result<(), String> {
    let destination_after = PathIdentity::observe_deep(destination)
        .map_err(|error| format!("Could not capture completed effect: {error}"))?;
    update_step(operation_id, key, |step| {
        if !step.status.terminal() {
            step.status = StepStatus::Completed;
            step.landing = Some(destination.to_path_buf());
            step.destination_after = Some(destination_after);
            step.staging = None;
            step.checkpoint = None;
            step.fast_path = Some(fast_path);
            step.failure = None;
        }
    })
}

pub fn mark_skipped(operation_id: &OperationId, key: &IdempotencyKey) -> Result<(), String> {
    update_step(operation_id, key, |step| {
        if !step.status.terminal() {
            step.status = StepStatus::Skipped;
            step.staging = None;
            step.checkpoint = None;
        }
    })
}

pub fn mark_failed(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    failure: ClassifiedFailure,
) -> Result<(), String> {
    update_step(operation_id, key, |step| {
        if !step.status.terminal() {
            step.status = StepStatus::Failed;
            if failure.class != FailureClass::IntegrityUncertain && step.checkpoint.is_none() {
                step.staging = None;
            }
            step.failure = Some(failure);
        }
    })
}

pub fn finish(operation_id: &OperationId, status: OperationStatus) -> Result<(), String> {
    mutate(|journal| {
        let operation = journal
            .operations
            .iter_mut()
            .find(|operation| &operation.id == operation_id)
            .ok_or_else(|| format!("Unknown operation {}", operation_id.0))?;
        if operation.status == OperationStatus::RolledBack && status != OperationStatus::RolledBack
        {
            return Err(format!(
                "Operation {} was already rolled back",
                operation_id.0
            ));
        }
        operation.status = status;
        operation.updated_at_secs = now_secs();
        Ok(())
    })
}

pub fn operation(operation_id: &OperationId) -> Result<OperationRecord, String> {
    load()?
        .operations
        .into_iter()
        .find(|operation| &operation.id == operation_id)
        .ok_or_else(|| format!("Unknown operation {}", operation_id.0))
}

pub fn completed_effect_is_current(
    operation_id: &OperationId,
    key: &IdempotencyKey,
) -> Result<bool, String> {
    let operation = operation(operation_id)?;
    let Some(step) = operation.steps.iter().find(|step| &step.key == key) else {
        return Ok(false);
    };
    if step.status != StepStatus::Completed {
        return Ok(false);
    }
    prove_completed_effect(step)
}

pub fn step_is_settled(operation_id: &OperationId, key: &IdempotencyKey) -> Result<bool, String> {
    let operation = operation(operation_id)?;
    let Some(step) = operation.steps.iter().find(|step| &step.key == key) else {
        return Ok(false);
    };
    settled_step(step)
}

fn settled_step(step: &OperationStep) -> Result<bool, String> {
    match step.status {
        StepStatus::Completed => prove_completed_effect(step),
        StepStatus::Skipped => Ok(true),
        StepStatus::RolledBack => Err(format!(
            "Operation step {} was already rolled back",
            step.key.0
        )),
        _ => Ok(false),
    }
}

fn prove_completed_effect(step: &OperationStep) -> Result<bool, String> {
    let expected = step
        .destination_after
        .as_ref()
        .ok_or_else(|| "Completed journal step has no effect identity".to_string())?;
    let effect = step.landing.as_ref().unwrap_or(&step.destination);
    let current = PathIdentity::observe_deep(effect)
        .map_err(|error| format!("Could not prove completed effect: {error}"))?;
    if !expected.same_version(&current) {
        return Err(format!(
            "Completed effect changed since operation: {}",
            effect.display()
        ));
    }
    Ok(true)
}

fn effect_path(step: &OperationStep) -> &Path {
    step.landing.as_deref().unwrap_or(&step.destination)
}

pub fn build_resume_spec(operation_id: &OperationId) -> Result<TransferSpec, String> {
    let record = operation(operation_id)?;
    build_resume_spec_from(record)
}

fn build_resume_spec_from(record: OperationRecord) -> Result<TransferSpec, String> {
    if record
        .steps
        .iter()
        .any(|step| step.status == StepStatus::RolledBack)
    {
        return Err(
            "Operation was partially rolled back; continue rollback or inspect it manually"
                .to_string(),
        );
    }
    for step in record
        .steps
        .iter()
        .filter(|step| step.status == StepStatus::Completed)
    {
        prove_completed_effect(step)?;
    }

    let mut entries = Vec::new();
    let mut expectations = Vec::new();
    for step in record.steps.iter().filter(|step| {
        matches!(
            step.status,
            StepStatus::Planned | StepStatus::Running | StepStatus::Requeued | StepStatus::Failed
        )
    }) {
        if step
            .failure
            .as_ref()
            .is_some_and(|failure| failure.class == FailureClass::IntegrityUncertain)
        {
            return Err(format!(
                "Recovery step requires manual review: {}",
                step.source.display()
            ));
        }
        let resume = validated_checkpoint(step)?;
        let metadata = std::fs::symlink_metadata(&step.source)
            .map_err(|error| format!("Could not inspect {}: {error}", step.source.display()))?;
        let entry = FileEntry::from_meta(step.source.clone(), &metadata)
            .ok_or_else(|| format!("Invalid recovery source: {}", step.source.display()))?;
        let source = PathIdentity::observe_deep(&step.source)
            .map_err(|error| format!("Could not re-stat recovery source: {error}"))?;
        let destination = PathIdentity::observe_deep(&step.destination)
            .map_err(|error| format!("Could not re-stat recovery destination: {error}"))?;
        let source_before = step.source_before.as_ref().ok_or_else(|| {
            format!(
                "Recovery source was never captured: {}",
                step.source.display()
            )
        })?;
        let destination_before = step.destination_before.as_ref().ok_or_else(|| {
            format!(
                "Recovery destination was never captured: {}",
                step.destination.display()
            )
        })?;
        if !source_before.same_version(&source) {
            return Err(format!(
                "Recovery source changed since the operation: {}",
                step.source.display()
            ));
        }
        if !destination_before.same_version(&destination) {
            return Err(format!(
                "Recovery destination changed since the operation: {}",
                step.destination.display()
            ));
        }
        if let (Some(landing), Some(landing_before)) = (&step.landing, &step.landing_before) {
            let current = PathIdentity::observe_deep(landing)
                .map_err(|error| format!("Could not re-stat recovery landing: {error}"))?;
            if !landing_before.same_version(&current) {
                return Err(format!(
                    "Recovery landing changed since the operation: {}",
                    landing.display()
                ));
            }
        }
        entries.push(entry);
        expectations.push(TransferExpectation {
            key: Some(step.key.clone()),
            source: Ok(source_before.clone()),
            destination: Ok(destination_before.clone()),
            landing: step.landing.clone(),
            landing_before: step.landing_before.clone(),
            resume,
        });
    }
    Ok(TransferSpec {
        operation_id: record.id,
        group_id: record.group_id,
        kind: record.kind,
        entries,
        expectations,
        target: record.target,
        policy: record.policy,
        method: record.method,
        durability: record.durability,
        post_success: record.post_success,
        rollback_cleanup: record.rollback_cleanup,
        #[cfg(test)]
        before_commit: None,
        #[cfg(test)]
        journal_enabled: false,
    })
}

fn validated_checkpoint(step: &OperationStep) -> Result<Option<ResumeCheckpoint>, String> {
    let Some(staging) = &step.staging else {
        return Ok(None);
    };
    if !crate::fs_util::path_is_taken(staging) {
        return Ok(None);
    }
    let checkpoint = step.checkpoint.as_ref().ok_or_else(|| {
        format!(
            "Recovery staging has no verified checkpoint: {}",
            staging.display()
        )
    })?;
    if checkpoint.staging != *staging {
        return Err(format!(
            "Recovery checkpoint points at different staging: {}",
            staging.display()
        ));
    }
    let source = PathIdentity::observe_deep(&step.source)
        .map_err(|error| format!("Could not verify checkpoint source: {error}"))?;
    if !checkpoint.source.same_version(&source) {
        return Err(format!(
            "Recovery source changed after checkpoint: {}",
            step.source.display()
        ));
    }
    let partial = PathIdentity::observe_deep(staging)
        .map_err(|error| format!("Could not verify checkpoint staging: {error}"))?;
    if !checkpoint.partial.same_version(&partial) || partial.size != checkpoint.offset {
        return Err(format!(
            "Recovery staging changed after checkpoint: {}",
            staging.display()
        ));
    }
    Ok(Some(checkpoint.clone()))
}

pub fn repair_plan(operation_id: &OperationId) -> Result<RepairPlan, String> {
    let record = operation(operation_id)?;
    let mut plan = RepairPlan {
        operation_id: Some(record.id.clone()),
        ..Default::default()
    };
    for step in record.steps.iter().rev() {
        match step.status {
            StepStatus::Completed => {
                let effect = effect_path(step);
                let current = PathIdentity::observe_deep(effect).ok();
                let unchanged = step
                    .destination_after
                    .as_ref()
                    .zip(current.as_ref())
                    .is_some_and(|(expected, current)| expected.same_version(current));
                if !unchanged {
                    plan.remaining.push(RepairItem {
                        path: effect.to_path_buf(),
                        action: "Inspect changed completed effect before rollback".to_string(),
                        automatic: false,
                    });
                } else if record.kind == TransferKind::Move {
                    plan.remaining.push(RepairItem {
                        path: effect.to_path_buf(),
                        action: format!("Move back to {}", step.source.display()),
                        automatic: !crate::fs_util::path_is_taken(&step.source),
                    });
                } else if step
                    .destination_before
                    .as_ref()
                    .is_some_and(|identity| !identity.exists)
                {
                    plan.remaining.push(RepairItem {
                        path: effect.to_path_buf(),
                        action: "Remove copy created by operation".to_string(),
                        automatic: true,
                    });
                } else {
                    let has_version = crate::version_store::record_for_key(&step.key).is_some();
                    plan.remaining.push(RepairItem {
                        path: effect.to_path_buf(),
                        action: "Restore replaced local version".to_string(),
                        automatic: has_version,
                    });
                }
            }
            StepStatus::Failed
            | StepStatus::Planned
            | StepStatus::Running
            | StepStatus::Requeued => plan.remaining.push(RepairItem {
                path: step.source.clone(),
                action: "Retry incomplete manifest entry".to_string(),
                automatic: step
                    .failure
                    .as_ref()
                    .is_none_or(|failure| failure.class != FailureClass::IntegrityUncertain),
            }),
            StepStatus::Skipped | StepStatus::RolledBack => {}
        }
    }
    if let Some(cleanup) = &record.rollback_cleanup
        && crate::fs_util::path_is_taken(cleanup)
    {
        plan.remaining.push(RepairItem {
            path: cleanup.clone(),
            action: "Remove operation-created folder after restoring entries".to_string(),
            automatic: cleanup_contains_only_operation_effects(&record, cleanup),
        });
    }
    Ok(plan)
}

fn cleanup_contains_only_operation_effects(record: &OperationRecord, cleanup: &Path) -> bool {
    let Ok(metadata) = std::fs::symlink_metadata(cleanup) else {
        return true;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    let expected = record
        .steps
        .iter()
        .filter(|step| step.status == StepStatus::Completed)
        .map(effect_path)
        .collect::<std::collections::HashSet<_>>();
    std::fs::read_dir(cleanup).is_ok_and(|entries| {
        entries
            .filter_map(Result::ok)
            .all(|entry| expected.contains(entry.path().as_path()))
    })
}

fn remove_path(path: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(|error| format!("Could not remove {}: {error}", path.display()))
}

fn rollback_step(record: &OperationRecord, step: &OperationStep) -> Result<(), String> {
    let effect = effect_path(step);
    if record.kind == TransferKind::Move {
        if crate::fs_util::path_is_taken(&step.source) {
            return Err(format!(
                "Source path is occupied: {}",
                step.source.display()
            ));
        }
        crate::native_copy::rename_noreplace(effect, &step.source)
            .map_err(|error| error.to_string())?;
        if step
            .destination_before
            .as_ref()
            .is_some_and(|identity| identity.exists)
        {
            let version = crate::version_store::record_for_key(&step.key)
                .ok_or_else(|| "Replaced destination has no local version".to_string())?;
            crate::version_store::restore(&version)?;
        }
        return Ok(());
    }

    if step
        .destination_before
        .as_ref()
        .is_some_and(|identity| identity.exists)
    {
        let version = crate::version_store::record_for_key(&step.key)
            .ok_or_else(|| "Replaced destination has no local version".to_string())?;
        remove_path(effect)?;
        crate::version_store::restore(&version)
    } else {
        remove_path(effect)
    }
}

fn rollback_created_container(record: &OperationRecord, plan: &mut RepairPlan) {
    let Some(cleanup) = &record.rollback_cleanup else {
        return;
    };
    match std::fs::remove_dir(cleanup) {
        Ok(()) => plan.completed.push(RepairItem {
            path: cleanup.clone(),
            action: "Removed operation-created folder".to_string(),
            automatic: true,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => plan.remaining.push(RepairItem {
            path: cleanup.clone(),
            action: format!("Could not remove operation-created folder: {error}"),
            automatic: false,
        }),
    }
}

pub fn rollback(operation_id: &OperationId) -> Result<RepairPlan, String> {
    let record = operation(operation_id)?;
    let mut plan = RepairPlan {
        operation_id: Some(record.id.clone()),
        ..Default::default()
    };
    for step in record
        .steps
        .iter()
        .rev()
        .filter(|step| step.status == StepStatus::Completed)
    {
        let effect = effect_path(step);
        match completed_effect_is_current(operation_id, &step.key) {
            Ok(true) => {}
            Ok(false) => {
                plan.remaining.push(RepairItem {
                    path: effect.to_path_buf(),
                    action: "Journal step changed before rollback; inspect manually".to_string(),
                    automatic: false,
                });
                continue;
            }
            Err(_) => {
                plan.remaining.push(RepairItem {
                    path: effect.to_path_buf(),
                    action: "Completed effect changed; inspect manually".to_string(),
                    automatic: false,
                });
                continue;
            }
        }
        let result = rollback_step(&record, step);
        match result {
            Ok(()) => {
                update_step(operation_id, &step.key, |journal_step| {
                    journal_step.status = StepStatus::RolledBack;
                })?;
                plan.completed.push(RepairItem {
                    path: effect.to_path_buf(),
                    action: "Rolled back".to_string(),
                    automatic: true,
                });
            }
            Err(error) => plan.remaining.push(RepairItem {
                path: effect.to_path_buf(),
                action: error,
                automatic: false,
            }),
        }
    }
    rollback_created_container(&record, &mut plan);
    finish(
        operation_id,
        if plan.remaining.is_empty() {
            OperationStatus::RolledBack
        } else {
            OperationStatus::NeedsReview
        },
    )?;
    Ok(plan)
}

pub fn recovery_inventory(extra: &[PathBuf]) -> Result<RecoveryInventory, String> {
    let journal = load()?;
    let mut roots = extra
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let mut referenced = std::collections::HashSet::new();
    for operation in &journal.operations {
        roots.insert(operation.target.clone());
        for step in &operation.steps {
            if let Some(staging) = &step.staging {
                referenced.insert(staging.clone());
            }
            for path in [
                Some(&step.source),
                Some(&step.destination),
                step.landing.as_ref(),
                step.staging.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                if let Some(parent) = path.parent() {
                    roots.insert(parent.to_path_buf());
                }
            }
        }
    }
    let mut roots = roots.into_iter().collect::<Vec<_>>();
    roots.sort();
    let mut operations = journal
        .operations
        .into_iter()
        .filter(|operation| operation.status.recoverable())
        .collect::<Vec<_>>();
    operations.sort_by_key(|operation| std::cmp::Reverse(operation.updated_at_secs));
    Ok(RecoveryInventory {
        operations,
        orphans: discover_orphan_staging_with(&roots, &referenced),
    })
}

fn discover_orphan_staging_with(
    roots: &[PathBuf],
    referenced: &std::collections::HashSet<PathBuf>,
) -> Vec<OrphanStaging> {
    let mut orphans = Vec::new();
    for root in roots {
        crate::io_budget::background_checkpoint(|| false);
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let temporary = name.contains(".cmdr-tmp.") || name.contains(".cmdr-quarantine.");
            if temporary
                && !referenced.contains(&path)
                && let Ok(identity) = PathIdentity::observe_deep(&path)
            {
                orphans.push(OrphanStaging { path, identity });
            }
        }
    }
    orphans.sort_by(|left, right| left.path.cmp(&right.path));
    orphans
}

pub fn clean_orphan(orphan: &OrphanStaging) -> Result<(), String> {
    let current = PathIdentity::observe_deep(&orphan.path)
        .map_err(|error| format!("Could not recheck orphan staging: {error}"))?;
    if !orphan.identity.same_version(&current) {
        return Err("Orphan staging changed after discovery".to_string());
    }
    remove_path(&orphan.path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn incomplete_record(source: &Path, destination: &Path, status: StepStatus) -> OperationRecord {
        let key = IdempotencyKey("step-1".to_string());
        OperationRecord {
            id: OperationId("operation-1".to_string()),
            group_id: None,
            kind: TransferKind::Copy,
            target: destination.parent().unwrap().to_path_buf(),
            policy: OverwritePolicy::OverwriteAll,
            method: CopyMethod::Native,
            durability: DurabilityProfile::Verified,
            post_success: None,
            rollback_cleanup: None,
            status: OperationStatus::Failed,
            created_at_secs: 1,
            updated_at_secs: 2,
            steps: vec![OperationStep {
                key,
                source: source.to_path_buf(),
                destination: destination.to_path_buf(),
                source_before: Some(PathIdentity::observe_deep(source).unwrap()),
                destination_before: Some(PathIdentity::observe_deep(destination).unwrap()),
                landing: None,
                landing_before: None,
                destination_after: None,
                staging: None,
                checkpoint: None,
                fast_path: None,
                status,
                attempts: 1,
                failure: None,
                preflight_error: None,
            }],
        }
    }

    #[test]
    fn journal_round_trips_atomically() {
        let temp = TempDir::new();
        let path = temp.path().join("journal.json");
        let journal = Journal::default();
        save_at(&path, &journal).unwrap();
        assert_eq!(load_at(&path).unwrap().schema, JOURNAL_SCHEMA);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn journal_persists_transfer_lifecycle_actions() {
        let temp = TempDir::new();
        let path = temp.path().join("journal.json");
        let source = temp.file("source.txt", "source");
        let folder = temp.path().join("gathered");
        std::fs::create_dir(&folder).unwrap();
        let mut record =
            incomplete_record(&source, &folder.join("source.txt"), StepStatus::Planned);
        record.post_success = Some(PostTransferAction::RemoveEmptyDir(folder.clone()));
        record.rollback_cleanup = Some(folder);
        let journal = Journal {
            operations: vec![record.clone()],
            ..Journal::default()
        };

        save_at(&path, &journal).unwrap();

        assert_eq!(load_at(&path).unwrap().operations, vec![record]);
    }

    #[test]
    fn skipped_steps_are_settled_but_rolled_back_steps_fail_closed() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source");
        let destination = temp.path().join("destination.txt");
        let mut step = incomplete_record(&source, &destination, StepStatus::Skipped)
            .steps
            .remove(0);

        assert!(settled_step(&step).unwrap());
        step.status = StepStatus::RolledBack;
        assert!(settled_step(&step).unwrap_err().contains("rolled back"));
    }

    #[test]
    fn unsupported_journal_schema_fails_closed() {
        let temp = TempDir::new();
        let path = temp.path().join("journal.json");
        std::fs::write(&path, r#"{"schema":99,"operations":[]}"#).unwrap();
        assert!(load_at(&path).unwrap_err().contains("Unsupported"));
    }

    #[test]
    fn resume_rejects_a_destination_changed_after_checkpoint() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source");
        let destination = temp.path().join("destination.txt");
        let record = incomplete_record(&source, &destination, StepStatus::Failed);
        std::fs::write(&destination, "foreign").unwrap();

        let error = build_resume_spec_from(record).err().unwrap();
        assert!(error.contains("destination changed"), "{error}");
    }

    #[test]
    fn resume_keeps_the_reviewed_landing_and_idempotency_key() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source");
        let destination = temp.file("destination.txt", "existing");
        let landing = temp.path().join("destination copy.txt");
        let mut record = incomplete_record(&source, &destination, StepStatus::Running);
        record.steps[0].landing = Some(landing.clone());
        record.steps[0].landing_before = Some(PathIdentity::missing(&landing));
        record.steps[0].staging = Some(temp.path().join(".destination.cmdr-tmp.0"));

        let spec = build_resume_spec_from(record).unwrap();
        assert_eq!(spec.expectations[0].key.as_ref().unwrap().0, "step-1");
        assert_eq!(spec.expectations[0].landing.as_ref(), Some(&landing));
        assert!(!spec.expectations[0].landing_before.as_ref().unwrap().exists);
    }

    #[test]
    fn resume_accepts_only_an_unchanged_verified_partial() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source contents");
        let destination = temp.path().join("destination.txt");
        let staging = temp.file(".destination.cmdr-tmp.0", "source");
        let mut record = incomplete_record(&source, &destination, StepStatus::Running);
        record.steps[0].staging = Some(staging.clone());
        record.steps[0].checkpoint = Some(ResumeCheckpoint {
            staging: staging.clone(),
            offset: staging.metadata().unwrap().len(),
            source: PathIdentity::observe_deep(&source).unwrap(),
            partial: PathIdentity::observe_deep(&staging).unwrap(),
        });

        let spec = build_resume_spec_from(record.clone()).unwrap();
        assert_eq!(
            spec.expectations[0]
                .resume
                .as_ref()
                .map(|checkpoint| checkpoint.offset),
            Some(6)
        );

        std::fs::write(&staging, "changed partial").unwrap();
        let error = build_resume_spec_from(record).err().unwrap();
        assert!(error.contains("staging changed"), "{error}");
    }

    #[test]
    fn operation_record_round_trips_the_selected_fast_path() {
        let temp = TempDir::new();
        let path = temp.path().join("journal.json");
        let source = temp.file("source.txt", "source");
        let destination = temp.path().join("destination.txt");
        let mut record = incomplete_record(&source, &destination, StepStatus::Completed);
        record.steps[0].fast_path = Some(crate::transfer_tuning::FastPath::Clone);
        let journal = Journal {
            operations: vec![record.clone()],
            ..Journal::default()
        };

        save_at(&path, &journal).unwrap();

        assert_eq!(load_at(&path).unwrap().operations, vec![record]);
    }

    #[test]
    fn resume_finalizes_post_success_after_every_manifest_step_settled() {
        let temp = TempDir::new();
        let folder = temp.dir("gathered");
        let source = temp.file("gathered/source.txt", "source");
        let destination = temp.path().join("source.txt");
        let mut record = incomplete_record(&source, &destination, StepStatus::Completed);
        record.kind = TransferKind::Move;
        std::fs::rename(&source, &destination).unwrap();
        record.steps[0].landing = Some(destination.clone());
        record.steps[0].destination_after = Some(PathIdentity::observe_deep(&destination).unwrap());
        record.post_success = Some(PostTransferAction::RemoveEmptyDir(folder.clone()));

        let spec = build_resume_spec_from(record).unwrap();

        assert!(spec.entries.is_empty());
        assert_eq!(
            spec.post_success,
            Some(PostTransferAction::RemoveEmptyDir(folder))
        );
    }

    #[test]
    fn resume_rejects_a_partially_rolled_back_operation() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source");
        let destination = temp.path().join("destination.txt");
        let record = incomplete_record(&source, &destination, StepStatus::RolledBack);

        let error = build_resume_spec_from(record).err().unwrap();

        assert!(error.contains("partially rolled back"), "{error}");
    }

    #[test]
    fn integrity_uncertain_step_requires_manual_review() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source");
        let destination = temp.path().join("destination.txt");
        let mut record = incomplete_record(&source, &destination, StepStatus::Failed);
        record.steps[0].failure = Some(ClassifiedFailure::message(
            FailureClass::IntegrityUncertain,
            Some(destination),
            "commit state unknown",
        ));
        assert!(
            build_resume_spec_from(record)
                .err()
                .unwrap()
                .contains("manual review")
        );
    }

    #[test]
    fn rollback_step_removes_an_unchanged_created_copy() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source");
        let destination = temp.path().join("destination.txt");
        let mut record = incomplete_record(&source, &destination, StepStatus::Completed);
        std::fs::write(&destination, "source").unwrap();
        record.steps[0].landing = Some(destination.clone());
        record.steps[0].destination_after = Some(PathIdentity::observe_deep(&destination).unwrap());

        rollback_step(&record, &record.steps[0]).unwrap();
        assert!(!destination.exists());
        assert!(source.exists());
    }

    #[test]
    fn rollback_removes_only_an_empty_operation_created_container() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source");
        let folder = temp.path().join("gathered");
        std::fs::create_dir(&folder).unwrap();
        let mut record =
            incomplete_record(&source, &folder.join("source.txt"), StepStatus::Planned);
        record.rollback_cleanup = Some(folder.clone());
        let mut plan = RepairPlan::default();

        rollback_created_container(&record, &mut plan);

        assert!(!folder.exists());
        assert_eq!(plan.completed.len(), 1);
        assert!(plan.remaining.is_empty());
    }

    #[test]
    fn rollback_preserves_a_created_container_with_foreign_content() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source");
        let folder = temp.path().join("gathered");
        std::fs::create_dir(&folder).unwrap();
        temp.file("gathered/foreign.txt", "foreign");
        let mut record =
            incomplete_record(&source, &folder.join("source.txt"), StepStatus::Planned);
        record.rollback_cleanup = Some(folder.clone());
        let mut plan = RepairPlan::default();

        assert!(!cleanup_contains_only_operation_effects(&record, &folder));
        rollback_created_container(&record, &mut plan);

        assert!(folder.join("foreign.txt").exists());
        assert!(plan.completed.is_empty());
        assert_eq!(plan.remaining.len(), 1);
    }

    #[test]
    fn orphan_cleanup_revalidates_identity() {
        let temp = TempDir::new();
        let path = temp.file(".file.cmdr-tmp.0", "staged");
        let orphan = OrphanStaging {
            identity: PathIdentity::observe_deep(&path).unwrap(),
            path: path.clone(),
        };
        std::fs::write(&path, "changed after scan").unwrap();
        assert!(clean_orphan(&orphan).is_err());
        assert!(path.exists());
    }

    #[test]
    fn orphan_discovery_includes_staging_and_quarantine_but_not_referenced_paths() {
        let temp = TempDir::new();
        let staging = temp.file(".copy.cmdr-tmp.0", "staged");
        let quarantine = temp.file(".copy.cmdr-quarantine.0", "quarantined");
        temp.file("ordinary.txt", "ordinary");
        let referenced = std::collections::HashSet::from([staging.clone()]);

        let found = discover_orphan_staging_with(&[temp.path().to_path_buf()], &referenced)
            .into_iter()
            .map(|orphan| orphan.path)
            .collect::<Vec<_>>();
        assert_eq!(found, vec![quarantine]);
    }
}
