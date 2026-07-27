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
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const JOURNAL_SCHEMA: u32 = 4;
const MAX_OPERATIONS: usize = 500;

pub const fn schema_version() -> u32 {
    JOURNAL_SCHEMA
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationEvent {
    Start,
    Stop,
    Fail,
    RequireReview,
    Complete,
    RollBack,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitionError {
    pub machine: &'static str,
    pub from: String,
    pub event: String,
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid {} transition from {} on {}",
            self.machine, self.from, self.event
        )
    }
}

impl OperationStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Planned | Self::Running)
    }

    pub fn transition(self, event: OperationEvent) -> Result<Self, TransitionError> {
        let target = match event {
            OperationEvent::Start => Self::Running,
            OperationEvent::Stop => Self::Stopped,
            OperationEvent::Fail => Self::Failed,
            OperationEvent::RequireReview => Self::NeedsReview,
            OperationEvent::Complete => Self::Completed,
            OperationEvent::RollBack => Self::RolledBack,
        };
        if self == target {
            return Ok(self);
        }
        let valid = matches!(
            (self, event),
            (
                Self::Planned | Self::Stopped | Self::Failed | Self::NeedsReview,
                OperationEvent::Start
            ) | (
                Self::Running,
                OperationEvent::Stop
                    | OperationEvent::Fail
                    | OperationEvent::RequireReview
                    | OperationEvent::Complete
            ) | (
                Self::Planned
                    | Self::Running
                    | Self::Stopped
                    | Self::Failed
                    | Self::NeedsReview
                    | Self::Completed,
                OperationEvent::RollBack
            ) | (
                Self::Stopped | Self::Failed | Self::Completed,
                OperationEvent::RequireReview
            )
        );
        valid.then_some(target).ok_or_else(|| TransitionError {
            machine: "operation",
            from: format!("{self:?}"),
            event: format!("{event:?}"),
        })
    }

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepEvent {
    Start,
    Checkpoint,
    Requeue,
    Complete,
    Skip,
    Fail,
    RollBack,
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

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Skipped | Self::RolledBack)
    }

    pub fn transition(self, event: StepEvent) -> Result<Self, TransitionError> {
        let target = match event {
            StepEvent::Start | StepEvent::Checkpoint => Self::Running,
            StepEvent::Requeue => Self::Requeued,
            StepEvent::Complete => Self::Completed,
            StepEvent::Skip => Self::Skipped,
            StepEvent::Fail => Self::Failed,
            StepEvent::RollBack => Self::RolledBack,
        };
        if self == target {
            return Ok(self);
        }
        let valid = matches!(
            (self, event),
            (
                Self::Planned | Self::Requeued | Self::Failed,
                StepEvent::Start
            ) | (Self::Running, StepEvent::Checkpoint | StepEvent::Requeue)
                | (
                    Self::Planned | Self::Running | Self::Requeued | Self::Failed,
                    StepEvent::Complete | StepEvent::Skip | StepEvent::Fail
                )
                | (Self::Completed, StepEvent::RollBack)
        );
        valid.then_some(target).ok_or_else(|| TransitionError {
            machine: "operation step",
            from: format!("{self:?}"),
            event: format!("{event:?}"),
        })
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
    #[serde(default)]
    pub replacement: Option<ReplacementBackup>,
    #[serde(default)]
    pub rollback_quarantine: Option<PathBuf>,
    pub status: StepStatus,
    pub attempts: u32,
    pub failure: Option<ClassifiedFailure>,
    pub preflight_error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplacementPhase {
    Prepared,
    OriginalBackedUp,
    ReplacementPlaced,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplacementBackup {
    pub path: PathBuf,
    pub original: PathIdentity,
    pub replacement: PathIdentity,
    pub phase: ReplacementPhase,
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
    pub version_retention: crate::operation::VersionRetentionPolicy,
    #[serde(default)]
    pub name_policy: crate::filesystem_policy::NamePolicy,
    #[serde(default)]
    pub symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    #[serde(default)]
    pub post_success: Option<PostTransferAction>,
    #[serde(default)]
    pub rollback_cleanup: Option<PathBuf>,
    #[serde(default)]
    pub rollback_cleanup_identity: Option<PathIdentity>,
    #[serde(default)]
    pub rollback_cleanup_quarantine: Option<PathBuf>,
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
#[cfg(test)]
static TEST_JOURNAL_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
#[cfg(test)]
static TEST_JOURNAL_SERIAL: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub(crate) struct TestJournalPathGuard {
    _serial: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for TestJournalPathGuard {
    fn drop(&mut self) {
        *crate::lock_util::recover(&TEST_JOURNAL_PATH) = None;
    }
}

#[cfg(test)]
pub(crate) fn use_test_journal(path: PathBuf) -> TestJournalPathGuard {
    let serial = crate::lock_util::recover(&TEST_JOURNAL_SERIAL);
    *crate::lock_util::recover(&TEST_JOURNAL_PATH) = Some(path);
    TestJournalPathGuard { _serial: serial }
}

struct StoreLock {
    _file: std::fs::File,
}

fn acquire_store_lock(path: &Path) -> Result<StoreLock, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Operation journal has no parent directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create operation journal directory: {error}"))?;
    let lock_path = parent.join(".operation-journal.lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let file = options
            .open(&lock_path)
            .map_err(|error| format!("Could not open operation journal lock: {error}"))?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(format!(
                "Could not acquire operation journal lock: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(StoreLock { _file: file })
    }
    #[cfg(not(unix))]
    {
        let file = options
            .open(&lock_path)
            .map_err(|error| format!("Could not open operation journal lock: {error}"))?;
        Ok(StoreLock { _file: file })
    }
}

fn journal_path() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = crate::lock_util::recover(&TEST_JOURNAL_PATH).clone() {
        return path;
    }
    crate::fs_util::config_dir().join("operation-journal.json")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn migrate_and_validate(mut journal: Journal) -> Result<Journal, String> {
    let source_schema = journal.schema;
    if source_schema < JOURNAL_SCHEMA {
        for operation in &mut journal.operations {
            let unproved_prefix = operation.steps.iter().any(|step| {
                step.checkpoint.as_ref().is_some_and(|checkpoint| {
                    checkpoint.layout == crate::transfer::CheckpointLayout::Prefix
                        && checkpoint.content_digest.is_none()
                })
            });
            let unproved_container = operation.rollback_cleanup.is_some()
                && operation.rollback_cleanup_identity.is_none();
            if operation.status.recoverable() && (unproved_prefix || unproved_container) {
                operation.status = OperationStatus::NeedsReview;
            }
        }
        journal.schema = JOURNAL_SCHEMA;
    }
    validate_journal(&journal)?;
    Ok(journal)
}

fn validate_journal(journal: &Journal) -> Result<(), String> {
    if journal.schema != JOURNAL_SCHEMA {
        return Err(format!(
            "Operation journal schema {} was not migrated to {JOURNAL_SCHEMA}",
            journal.schema
        ));
    }
    if journal.operations.len() > MAX_OPERATIONS {
        return Err(format!(
            "Operation journal contains {} records; the safe limit is {MAX_OPERATIONS}",
            journal.operations.len()
        ));
    }

    let mut operation_ids = HashSet::new();
    for operation in &journal.operations {
        if !operation_ids.insert(&operation.id) {
            return Err(format!(
                "Operation journal contains duplicate operation id {}",
                operation.id.0
            ));
        }
        if operation.rollback_cleanup.is_some() != operation.rollback_cleanup_identity.is_some()
            && operation.status != OperationStatus::NeedsReview
        {
            return Err(format!(
                "Operation {} has an unproved rollback container",
                operation.id.0
            ));
        }
        if let (Some(path), Some(identity)) = (
            operation.rollback_cleanup.as_ref(),
            operation.rollback_cleanup_identity.as_ref(),
        ) && identity.path != *path
        {
            return Err(format!(
                "Operation {} rollback container proof is bound to another path",
                operation.id.0
            ));
        }
        if operation.rollback_cleanup_quarantine.is_some() && operation.rollback_cleanup.is_none() {
            return Err(format!(
                "Operation {} has a container quarantine without a container",
                operation.id.0
            ));
        }

        let mut keys = HashSet::new();
        for step in &operation.steps {
            if !keys.insert(&step.key) {
                return Err(format!(
                    "Operation {} contains duplicate step key {}",
                    operation.id.0, step.key.0
                ));
            }
            validate_step(operation, step)?;
        }

        if operation.status == OperationStatus::Completed
            && (operation.rollback_cleanup_quarantine.is_some()
                || operation.steps.iter().any(|step| {
                    !matches!(step.status, StepStatus::Completed | StepStatus::Skipped)
                        || step.replacement.is_some()
                        || step.rollback_quarantine.is_some()
                }))
        {
            return Err(format!(
                "Completed operation {} has unsettled or unreconciled steps",
                operation.id.0
            ));
        }
        if operation.status == OperationStatus::RolledBack
            && (operation.rollback_cleanup_quarantine.is_some()
                || operation.steps.iter().any(|step| {
                    matches!(step.status, StepStatus::Running | StepStatus::Completed)
                        || step.rollback_quarantine.is_some()
                }))
        {
            return Err(format!(
                "Rolled-back operation {} still has an applied or in-flight effect",
                operation.id.0
            ));
        }
    }
    Ok(())
}

fn validate_step(operation: &OperationRecord, step: &OperationStep) -> Result<(), String> {
    let invalid_binding = |identity: &Option<PathIdentity>, path: &Path| {
        identity
            .as_ref()
            .is_some_and(|identity| identity.path != path)
    };
    if invalid_binding(&step.source_before, &step.source)
        || invalid_binding(&step.destination_before, &step.destination)
        || step.landing_before.as_ref().is_some_and(|identity| {
            step.landing
                .as_ref()
                .is_none_or(|landing| identity.path != *landing)
        })
        || step
            .destination_after
            .as_ref()
            .is_some_and(|identity| identity.path != effect_path(step))
    {
        return Err(format!(
            "Operation {} step {} contains a path identity bound to another path",
            operation.id.0, step.key.0
        ));
    }

    if matches!(step.status, StepStatus::Completed | StepStatus::RolledBack)
        && (step.landing.is_none() || step.destination_after.is_none() || step.fast_path.is_none())
    {
        return Err(format!(
            "Operation {} step {} is completed without a terminal proof",
            operation.id.0, step.key.0
        ));
    }
    if !matches!(step.status, StepStatus::Completed | StepStatus::RolledBack)
        && step.destination_after.is_some()
    {
        return Err(format!(
            "Operation {} step {} has a terminal proof in a non-completed state",
            operation.id.0, step.key.0
        ));
    }
    if let Some(checkpoint) = &step.checkpoint {
        if step.staging.as_ref() != Some(&checkpoint.staging)
            || checkpoint.partial.path != checkpoint.staging
            || checkpoint.source.path != step.source
        {
            return Err(format!(
                "Operation {} step {} has a checkpoint bound to another path",
                operation.id.0, step.key.0
            ));
        }
        if checkpoint.layout == crate::transfer::CheckpointLayout::Prefix
            && checkpoint.content_digest.is_none()
            && operation.status != OperationStatus::NeedsReview
        {
            return Err(format!(
                "Operation {} step {} has a legacy Prefix checkpoint without content proof",
                operation.id.0, step.key.0
            ));
        }
    }
    if let Some(replacement) = &step.replacement
        && (replacement.path == effect_path(step)
            || replacement.original.path != effect_path(step)
            || replacement.replacement.path
                != step.staging.as_deref().unwrap_or_else(|| effect_path(step)))
    {
        return Err(format!(
            "Operation {} step {} has an invalid overwrite proof",
            operation.id.0, step.key.0
        ));
    }
    Ok(())
}

fn prune_terminal_history(journal: &mut Journal) -> Result<(), String> {
    let remove_count = journal.operations.len().saturating_sub(MAX_OPERATIONS);
    if remove_count == 0 {
        return Ok(());
    }
    let removable = journal
        .operations
        .iter()
        .filter(|operation| !operation.status.recoverable())
        .count();
    if removable < remove_count {
        return Err(format!(
            "Operation journal is full with recoverable work; refusing to discard recovery records (limit {MAX_OPERATIONS})"
        ));
    }
    let mut remaining = remove_count;
    journal.operations.retain(|operation| {
        if remaining > 0 && !operation.status.recoverable() {
            remaining -= 1;
            false
        } else {
            true
        }
    });
    Ok(())
}

fn load_at(path: &Path) -> Result<Journal, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    match options.open(path) {
        Ok(file) => {
            let journal: Journal = serde_json::from_reader(file)
                .map_err(|error| format!("Operation journal is corrupt: {error}"))?;
            if !(1..=JOURNAL_SCHEMA).contains(&journal.schema) {
                return Err(format!(
                    "Unsupported operation journal schema {}; expected {JOURNAL_SCHEMA}",
                    journal.schema
                ));
            }
            migrate_and_validate(journal)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
        Err(error) => Err(format!("Could not open operation journal: {error}")),
    }
}

fn save_at(path: &Path, journal: &Journal) -> Result<(), String> {
    validate_journal(journal)?;
    match crate::persistence::save_json_atomic(path, journal) {
        Ok(crate::persistence::AtomicWriteOutcome::Durable) => Ok(()),
        Ok(crate::persistence::AtomicWriteOutcome::CommittedButNotDurable(error)) => Err(format!(
            "Operation journal commit is ambiguous across a crash: target was replaced but its directory could not be synced: {error}"
        )),
        Err(error) => Err(format!(
            "Operation journal was not committed at {:?}: {error:?}",
            error.stage()
        )),
    }
}

fn mutate<T>(change: impl FnOnce(&mut Journal) -> Result<T, String>) -> Result<T, String> {
    let lock = JOURNAL_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = crate::lock_util::recover(lock);
    let path = journal_path();
    let _store_lock = acquire_store_lock(&path)?;
    let mut journal = load_at(&path)?;
    journal.schema = JOURNAL_SCHEMA;
    let result = change(&mut journal)?;
    prune_terminal_history(&mut journal)?;
    save_at(&path, &journal)?;
    Ok(result)
}

pub fn load() -> Result<Journal, String> {
    let lock = JOURNAL_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = crate::lock_util::recover(lock);
    let path = journal_path();
    let _store_lock = acquire_store_lock(&path)?;
    load_at(&path)
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
        if spec.rollback_cleanup.is_some() != spec.rollback_cleanup_identity.is_some() {
            return Err(
                "Operation-created rollback containers require an ownership identity".to_string(),
            );
        }
        if let (Some(path), Some(identity)) = (
            spec.rollback_cleanup.as_ref(),
            spec.rollback_cleanup_identity.as_ref(),
        ) && identity.path != *path
        {
            return Err("Rollback container identity is bound to another path".to_string());
        }
        let manifest = spec
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let destination = spec.target.join(&entry.name);
                (
                    step_key(spec, index, &destination),
                    entry.path.clone(),
                    destination,
                )
            })
            .collect::<Vec<_>>();
        let mut manifest_keys = HashSet::new();
        if let Some((key, _, _)) = manifest
            .iter()
            .find(|(key, _, _)| !manifest_keys.insert(key.clone()))
        {
            return Err(format!(
                "Operation {} manifest contains duplicate idempotency key {}",
                spec.operation_id.0, key.0
            ));
        }
        if let Some(existing) = journal
            .operations
            .iter_mut()
            .find(|operation| operation.id == spec.operation_id)
        {
            if existing.status.is_terminal() && !existing.status.recoverable() {
                return Err(format!(
                    "Operation {} is already finalized as {}",
                    spec.operation_id.0,
                    existing.status.label()
                ));
            }
            if existing.kind != spec.kind
                || existing.target != spec.target
                || existing.group_id != spec.group_id
                || existing.policy != spec.policy
                || existing.method != spec.method
                || existing.durability != spec.durability
                || existing.version_retention != spec.version_retention
                || existing.name_policy != spec.name_policy
                || existing.symlink_policy != spec.symlink_policy
                || existing.post_success != spec.post_success
                || existing.rollback_cleanup != spec.rollback_cleanup
                || existing.rollback_cleanup_identity != spec.rollback_cleanup_identity
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
            for (step, (key, source, destination)) in existing.steps.iter().zip(manifest.iter()) {
                if step.key != *key || step.source != *source || step.destination != *destination {
                    return Err(format!(
                        "Operation {} was reused with a reordered or different manifest entry",
                        spec.operation_id.0
                    ));
                }
            }
            existing.status = existing
                .status
                .transition(OperationEvent::Start)
                .map_err(|error| error.to_string())?;
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
                    replacement: None,
                    rollback_quarantine: None,
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
            version_retention: spec.version_retention,
            name_policy: spec.name_policy,
            symlink_policy: spec.symlink_policy,
            post_success: spec.post_success.clone(),
            rollback_cleanup: spec.rollback_cleanup.clone(),
            rollback_cleanup_identity: spec.rollback_cleanup_identity.clone(),
            rollback_cleanup_quarantine: None,
            status: OperationStatus::Planned
                .transition(OperationEvent::Start)
                .map_err(|error| error.to_string())?,
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
    allowed_operation_statuses: &[OperationStatus],
    update: impl FnOnce(&mut OperationStep) -> Result<(), String>,
) -> Result<(), String> {
    mutate(|journal| {
        let operation = journal
            .operations
            .iter_mut()
            .find(|operation| &operation.id == operation_id)
            .ok_or_else(|| format!("Unknown operation {}", operation_id.0))?;
        if !allowed_operation_statuses.contains(&operation.status) {
            return Err(format!(
                "Operation {} cannot accept step callbacks while {}",
                operation_id.0,
                operation.status.label()
            ));
        }
        let step = operation
            .steps
            .iter_mut()
            .find(|step| &step.key == key)
            .ok_or_else(|| format!("Unknown operation step {}", key.0))?;
        update(step)?;
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
    update_step(operation_id, key, &[OperationStatus::Running], |step| {
        step.status = step
            .status
            .transition(StepEvent::Start)
            .map_err(|error| error.to_string())?;
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
        Ok(())
    })
}

pub fn mark_checkpoint(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    checkpoint: ResumeCheckpoint,
) -> Result<(), String> {
    update_step(operation_id, key, &[OperationStatus::Running], |step| {
        step.status = step
            .status
            .transition(StepEvent::Checkpoint)
            .map_err(|error| error.to_string())?;
        step.staging = Some(checkpoint.staging.clone());
        step.checkpoint = Some(checkpoint);
        step.failure = None;
        Ok(())
    })
}

pub fn mark_requeued(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    source_before: PathIdentity,
) -> Result<(), String> {
    update_step(operation_id, key, &[OperationStatus::Running], |step| {
        step.status = step
            .status
            .transition(StepEvent::Requeue)
            .map_err(|error| error.to_string())?;
        step.source_before = Some(source_before);
        step.staging = None;
        step.landing = None;
        step.landing_before = None;
        step.checkpoint = None;
        step.fast_path = None;
        Ok(())
    })
}

pub fn prepare_replacement(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    staged: &Path,
    destination: &Path,
    proposed_backup: &Path,
) -> Result<ReplacementBackup, String> {
    let original = PathIdentity::observe_deep(destination)
        .map_err(|error| format!("Could not prove overwrite destination: {error}"))?;
    let replacement = PathIdentity::observe_deep(staged)
        .map_err(|error| format!("Could not prove overwrite staging: {error}"))?;
    if !original.exists || !replacement.exists {
        return Err("Overwrite preparation requires both original and staged objects".to_string());
    }
    mutate(|journal| {
        let operation = journal
            .operations
            .iter_mut()
            .find(|operation| &operation.id == operation_id)
            .ok_or_else(|| format!("Unknown operation {}", operation_id.0))?;
        if operation.status != OperationStatus::Running {
            return Err(format!(
                "Operation {} cannot prepare an overwrite while {}",
                operation_id.0,
                operation.status.label()
            ));
        }
        let step = operation
            .steps
            .iter_mut()
            .find(|step| &step.key == key)
            .ok_or_else(|| format!("Unknown operation step {}", key.0))?;
        if step.status != StepStatus::Running
            || step.staging.as_deref() != Some(staged)
            || step.landing.as_deref() != Some(destination)
        {
            return Err(format!(
                "Operation step {} is not prepared for this overwrite",
                key.0
            ));
        }
        if let Some(existing) = &step.replacement {
            if existing.original.same_binding(&original)
                && existing.replacement.same_binding(&replacement)
            {
                return Ok(existing.clone());
            }
            return Err(format!(
                "Operation step {} already has a different overwrite proof",
                key.0
            ));
        }
        let prepared = ReplacementBackup {
            path: proposed_backup.to_path_buf(),
            original,
            replacement,
            phase: ReplacementPhase::Prepared,
        };
        step.replacement = Some(prepared.clone());
        operation.updated_at_secs = now_secs();
        Ok(prepared)
    })
}

pub fn mark_replacement_backed_up(
    operation_id: &OperationId,
    key: &IdempotencyKey,
) -> Result<(), String> {
    update_step(operation_id, key, &[OperationStatus::Running], |step| {
        let replacement = step
            .replacement
            .as_mut()
            .ok_or_else(|| "Overwrite backup was not prepared".to_string())?;
        if replacement.phase == ReplacementPhase::OriginalBackedUp {
            return Ok(());
        }
        if replacement.phase != ReplacementPhase::Prepared {
            return Err("Overwrite backup callback arrived in a stale phase".to_string());
        }
        let backup = PathIdentity::observe_deep(&replacement.path)
            .map_err(|error| format!("Could not prove overwrite backup: {error}"))?;
        if !replacement.original.same_object(&backup) {
            return Err("Overwrite backup does not contain the proven original object".to_string());
        }
        let destination = PathIdentity::observe(&replacement.original.path)
            .map_err(|error| format!("Could not prove vacated destination: {error}"))?;
        if destination.exists {
            return Err("Overwrite destination was repopulated before placement".to_string());
        }
        replacement.phase = ReplacementPhase::OriginalBackedUp;
        Ok(())
    })
}

pub fn mark_replacement_placed(
    operation_id: &OperationId,
    key: &IdempotencyKey,
) -> Result<(), String> {
    update_step(operation_id, key, &[OperationStatus::Running], |step| {
        let replacement = step
            .replacement
            .as_mut()
            .ok_or_else(|| "Overwrite placement was not prepared".to_string())?;
        if replacement.phase == ReplacementPhase::ReplacementPlaced {
            return Ok(());
        }
        if replacement.phase != ReplacementPhase::OriginalBackedUp {
            return Err("Overwrite placement callback arrived in a stale phase".to_string());
        }
        let destination = PathIdentity::observe_deep(&replacement.original.path)
            .map_err(|error| format!("Could not prove overwrite placement: {error}"))?;
        if !replacement.replacement.same_object(&destination) {
            return Err("Overwrite destination is not the proven staged object".to_string());
        }
        let backup = PathIdentity::observe_deep(&replacement.path)
            .map_err(|error| format!("Could not recheck overwrite backup: {error}"))?;
        if !replacement.original.same_object(&backup) {
            return Err("Overwrite original changed in its backup location".to_string());
        }
        replacement.phase = ReplacementPhase::ReplacementPlaced;
        Ok(())
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
    let replacement = mutate(|journal| {
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

        if step.status == StepStatus::Completed {
            let existing = step
                .destination_after
                .as_ref()
                .ok_or_else(|| "Completed step has no immutable effect proof".to_string())?;
            if step.landing.as_deref() != Some(destination)
                || step.fast_path != Some(fast_path)
                || !existing.same_binding(&destination_after)
            {
                return Err(format!(
                    "Duplicate completion for step {} conflicts with its immutable proof",
                    key.0
                ));
            }
            return Ok(step.replacement.clone());
        }
        if operation.status != OperationStatus::Running {
            return Err(format!(
                "Operation {} cannot accept completion while {}",
                operation_id.0,
                operation.status.label()
            ));
        }
        if let Some(replacement) = &step.replacement
            && replacement.phase != ReplacementPhase::ReplacementPlaced
        {
            return Err(format!(
                "Overwrite step {} reached completion before placement was proven",
                key.0
            ));
        }
        step.status = step
            .status
            .transition(StepEvent::Complete)
            .map_err(|error| error.to_string())?;
        step.landing = Some(destination.to_path_buf());
        step.destination_after = Some(destination_after);
        if step.replacement.is_none() {
            step.staging = None;
        }
        step.checkpoint = None;
        step.fast_path = Some(fast_path);
        step.failure = None;
        operation.updated_at_secs = now_secs();
        Ok(step.replacement.clone())
    })?;

    if let Some(replacement) = replacement {
        let backup = PathIdentity::observe_deep(&replacement.path)
            .map_err(|error| format!("Could not inspect overwrite backup cleanup: {error}"))?;
        if backup.exists {
            if !replacement.original.same_object(&backup) {
                return Err(
                    "Overwrite backup was replaced before cleanup; foreign data was preserved"
                        .to_string(),
                );
            }
            remove_expected_path(&replacement.path, &replacement.original)?;
        }
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
            if step.status != StepStatus::Completed
                || step.replacement.as_ref() != Some(&replacement)
            {
                return Err(format!(
                    "Overwrite proof for step {} changed before backup cleanup",
                    key.0
                ));
            }
            step.replacement = None;
            step.staging = None;
            operation.updated_at_secs = now_secs();
            Ok(())
        })?;
    }
    Ok(())
}

pub fn mark_skipped(operation_id: &OperationId, key: &IdempotencyKey) -> Result<(), String> {
    update_step(operation_id, key, &[OperationStatus::Running], |step| {
        step.status = step
            .status
            .transition(StepEvent::Skip)
            .map_err(|error| error.to_string())?;
        step.staging = None;
        step.checkpoint = None;
        Ok(())
    })
}

pub fn mark_failed(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    failure: ClassifiedFailure,
) -> Result<(), String> {
    update_step(operation_id, key, &[OperationStatus::Running], |step| {
        step.status = step
            .status
            .transition(StepEvent::Fail)
            .map_err(|error| error.to_string())?;
        if failure.class != FailureClass::IntegrityUncertain && step.checkpoint.is_none() {
            step.staging = None;
        }
        step.failure = Some(failure);
        Ok(())
    })
}

pub fn mark_rolled_back(operation_id: &OperationId, key: &IdempotencyKey) -> Result<(), String> {
    update_step(
        operation_id,
        key,
        &[
            OperationStatus::Running,
            OperationStatus::Stopped,
            OperationStatus::Failed,
            OperationStatus::NeedsReview,
            OperationStatus::Completed,
        ],
        |step| {
            step.status = step
                .status
                .transition(StepEvent::RollBack)
                .map_err(|error| error.to_string())?;
            step.checkpoint = None;
            step.rollback_quarantine = None;
            step.failure = None;
            Ok(())
        },
    )
}

pub fn finish(operation_id: &OperationId, status: OperationStatus) -> Result<(), String> {
    mutate(|journal| {
        let operation = journal
            .operations
            .iter_mut()
            .find(|operation| &operation.id == operation_id)
            .ok_or_else(|| format!("Unknown operation {}", operation_id.0))?;
        let event = match status {
            OperationStatus::Planned => {
                return Err("cannot finish an operation as planned".to_string());
            }
            OperationStatus::Running => {
                return Err("cannot use finish to start an operation".to_string());
            }
            OperationStatus::Stopped => OperationEvent::Stop,
            OperationStatus::Failed => OperationEvent::Fail,
            OperationStatus::NeedsReview => OperationEvent::RequireReview,
            OperationStatus::Completed => OperationEvent::Complete,
            OperationStatus::RolledBack => OperationEvent::RollBack,
        };
        if status == OperationStatus::Completed {
            for step in &operation.steps {
                match step.status {
                    StepStatus::Completed => {
                        if step.replacement.is_some() || step.rollback_quarantine.is_some() {
                            return Err(format!(
                                "Operation step {} still has unreconciled filesystem state",
                                step.key.0
                            ));
                        }
                        prove_completed_effect(step)?;
                    }
                    StepStatus::Skipped => {}
                    _ => {
                        return Err(format!(
                            "Operation step {} is not settled and cannot be completed",
                            step.key.0
                        ));
                    }
                }
            }
        }
        if status == OperationStatus::RolledBack
            && operation.steps.iter().any(|step| {
                matches!(step.status, StepStatus::Running | StepStatus::Completed)
                    || step.rollback_quarantine.is_some()
            })
        {
            return Err(
                "Operation still has applied or in-flight effects and cannot be rolled back"
                    .to_string(),
            );
        }
        operation.status = operation
            .status
            .transition(event)
            .map_err(|error| error.to_string())?;
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

pub fn step_checkpoint(
    operation_id: &OperationId,
    key: &IdempotencyKey,
) -> Result<Option<ResumeCheckpoint>, String> {
    let operation = operation(operation_id)?;
    Ok(operation
        .steps
        .iter()
        .find(|step| &step.key == key)
        .and_then(|step| step.checkpoint.clone()))
}

fn settled_step(step: &OperationStep) -> Result<bool, String> {
    if !step.status.is_terminal() {
        return Ok(false);
    }
    match step.status {
        StepStatus::Completed => prove_completed_effect(step),
        StepStatus::Skipped => Ok(true),
        StepStatus::RolledBack => Err(format!(
            "Operation step {} was already rolled back",
            step.key.0
        )),
        _ => unreachable!("all non-terminal step states returned above"),
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
    if !expected.same_binding(&current) {
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
    reconcile_prepared_replacements(operation_id)?;
    let record = operation(operation_id)?;
    build_resume_spec_from(record)
}

fn reconcile_prepared_replacements(operation_id: &OperationId) -> Result<(), String> {
    let record = operation(operation_id)?;
    for step in &record.steps {
        let Some(replacement) = &step.replacement else {
            continue;
        };
        let destination = PathIdentity::observe_deep(&replacement.original.path)
            .map_err(|error| format!("Could not inspect interrupted overwrite: {error}"))?;
        let backup = PathIdentity::observe_deep(&replacement.path)
            .map_err(|error| format!("Could not inspect interrupted overwrite backup: {error}"))?;
        if destination.exists && replacement.replacement.same_object(&destination) {
            return Err(format!(
                "Overwrite placement completed before its terminal proof; original is preserved at {} and requires review",
                replacement.path.display()
            ));
        }
        if backup.exists {
            if !replacement.original.same_object(&backup) {
                return Err(format!(
                    "Interrupted overwrite backup changed and requires review: {}",
                    replacement.path.display()
                ));
            }
            if destination.exists {
                return Err(format!(
                    "Interrupted overwrite destination was repopulated; original remains at {}",
                    replacement.path.display()
                ));
            }
            crate::native_copy::rename_noreplace(&replacement.path, &replacement.original.path)
                .map_err(|error| {
                    format!(
                        "Could not restore interrupted overwrite from {}: {error}",
                        replacement.path.display()
                    )
                })?;
            crate::fs_util::sync_parent_namespace(&replacement.original.path)
                .map_err(|error| format!("Could not sync restored overwrite: {error}"))?;
        } else if !replacement.original.same_binding(&destination) {
            return Err(format!(
                "Interrupted overwrite lost its proven original binding: {}",
                replacement.original.path.display()
            ));
        }

        mutate(|journal| {
            let operation = journal
                .operations
                .iter_mut()
                .find(|operation| &operation.id == operation_id)
                .ok_or_else(|| format!("Unknown operation {}", operation_id.0))?;
            let journal_step = operation
                .steps
                .iter_mut()
                .find(|journal_step| journal_step.key == step.key)
                .ok_or_else(|| format!("Unknown operation step {}", step.key.0))?;
            if journal_step.replacement.as_ref() != Some(replacement) {
                return Err(format!(
                    "Overwrite proof changed while reconciling step {}",
                    step.key.0
                ));
            }
            journal_step.replacement = None;
            operation.updated_at_secs = now_secs();
            Ok(())
        })?;
    }
    Ok(())
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
        if !source_before.same_binding(&source) {
            return Err(format!(
                "Recovery source changed since the operation: {}",
                step.source.display()
            ));
        }
        if !destination_before.same_binding(&destination) {
            return Err(format!(
                "Recovery destination changed since the operation: {}",
                step.destination.display()
            ));
        }
        if let (Some(landing), Some(landing_before)) = (&step.landing, &step.landing_before) {
            let current = PathIdentity::observe_deep(landing)
                .map_err(|error| format!("Could not re-stat recovery landing: {error}"))?;
            if !landing_before.same_binding(&current) {
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
        version_retention: record.version_retention,
        name_policy: record.name_policy,
        symlink_policy: record.symlink_policy,
        post_success: record.post_success,
        rollback_cleanup: record.rollback_cleanup,
        rollback_cleanup_identity: record.rollback_cleanup_identity,
        #[cfg(test)]
        mount_wait_override: None,
        #[cfg(test)]
        before_commit: None,
        #[cfg(test)]
        before_post_success: None,
        #[cfg(test)]
        before_terminal_publish: None,
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
    if !checkpoint.source.same_binding(&source) {
        return Err(format!(
            "Recovery source changed after checkpoint: {}",
            step.source.display()
        ));
    }
    let partial = PathIdentity::observe_deep(staging)
        .map_err(|error| format!("Could not verify checkpoint staging: {error}"))?;
    let same_object = match (
        checkpoint.partial.volume.zip(checkpoint.partial.file_id),
        partial.volume.zip(partial.file_id),
    ) {
        (Some(expected), Some(current)) => {
            checkpoint.partial.exists
                && partial.exists
                && checkpoint.partial.kind == partial.kind
                && expected == current
        }
        _ => checkpoint.partial.same_version(&partial),
    };
    let layout_matches = match checkpoint.layout {
        crate::transfer::CheckpointLayout::Prefix => partial.size >= checkpoint.offset,
        crate::transfer::CheckpointLayout::DeltaFixed
        | crate::transfer::CheckpointLayout::DeltaCdc => {
            partial.size == checkpoint.partial.size && partial.size >= checkpoint.offset
        }
    };
    if !same_object || partial.kind != Some(crate::path_identity::PathKind::File) || !layout_matches
    {
        return Err(format!(
            "Recovery staging changed after checkpoint: {}",
            staging.display()
        ));
    }
    if checkpoint.layout == crate::transfer::CheckpointLayout::Prefix {
        let expected = checkpoint.content_digest.ok_or_else(|| {
            format!(
                "Recovery Prefix checkpoint has no content proof; inspect manually: {}",
                staging.display()
            )
        })?;
        let partial_digest = crate::transfer::prefix_digest(staging, checkpoint.offset)
            .map_err(|error| format!("Could not hash recovery staging: {error}"))?;
        let source_digest = crate::transfer::prefix_digest(&step.source, checkpoint.offset)
            .map_err(|error| format!("Could not hash recovery source: {error}"))?;
        if partial_digest != expected || source_digest != expected {
            return Err(format!(
                "Recovery Prefix checkpoint bytes changed after proof: {}",
                staging.display()
            ));
        }
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
                    .is_some_and(|(expected, current)| expected.same_binding(current));
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

fn quarantine_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "item".to_string());
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    crate::fs_util::first_available(|index| parent.join(format!(".{name}.cmdr-quarantine.{index}")))
}

fn detach_expected_path(
    path: &Path,
    expected: &PathIdentity,
    quarantine: &Path,
) -> Result<PathBuf, String> {
    crate::native_copy::rename_noreplace(path, quarantine)
        .map_err(|error| format!("Could not quarantine {}: {error}", path.display()))?;
    crate::fs_util::sync_parent_namespace(path)
        .map_err(|error| format!("Could not sync quarantine rename: {error}"))?;
    let moved = PathIdentity::observe_deep(quarantine)
        .map_err(|error| format!("Could not prove quarantined object: {error}"))?;
    if expected.same_object(&moved) {
        return Ok(quarantine.to_path_buf());
    }

    let restored = crate::native_copy::rename_noreplace(quarantine, path);
    if restored.is_ok() {
        let _ = crate::fs_util::sync_parent_namespace(path);
    }
    Err(if restored.is_ok() {
        format!(
            "{} was replaced before quarantine; the foreign object was restored untouched",
            path.display()
        )
    } else {
        format!(
            "{} was replaced before quarantine; the foreign object is preserved at {}",
            path.display(),
            quarantine.display()
        )
    })
}

fn remove_expected_path(path: &Path, expected: &PathIdentity) -> Result<(), String> {
    let quarantine = quarantine_path(path);
    let quarantine = detach_expected_path(path, expected, &quarantine)?;
    remove_path(&quarantine)?;
    crate::fs_util::sync_parent_namespace(&quarantine)
        .map_err(|error| format!("Could not sync quarantined removal: {error}"))
}

fn prepare_rollback_quarantine(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    effect: &Path,
) -> Result<PathBuf, String> {
    let proposed = quarantine_path(effect);
    mutate(|journal| {
        let operation = journal
            .operations
            .iter_mut()
            .find(|operation| &operation.id == operation_id)
            .ok_or_else(|| format!("Unknown operation {}", operation_id.0))?;
        if !matches!(
            operation.status,
            OperationStatus::Running
                | OperationStatus::Stopped
                | OperationStatus::Failed
                | OperationStatus::NeedsReview
        ) {
            return Err(format!(
                "Operation {} cannot prepare rollback while {}",
                operation_id.0,
                operation.status.label()
            ));
        }
        let step = operation
            .steps
            .iter_mut()
            .find(|step| &step.key == key)
            .ok_or_else(|| format!("Unknown operation step {}", key.0))?;
        if step.status != StepStatus::Completed {
            return Err(format!(
                "Only a completed step can prepare rollback: {}",
                key.0
            ));
        }
        if let Some(existing) = &step.rollback_quarantine {
            return Ok(existing.clone());
        }
        step.rollback_quarantine = Some(proposed.clone());
        operation.updated_at_secs = now_secs();
        Ok(proposed)
    })
}

fn rollback_step(
    operation_id: &OperationId,
    record: &OperationRecord,
    step: &OperationStep,
) -> Result<(), String> {
    let effect = effect_path(step);
    let expected = step
        .destination_after
        .as_ref()
        .ok_or_else(|| "Completed rollback step has no effect proof".to_string())?;
    let quarantine = prepare_rollback_quarantine(operation_id, &step.key, effect)?;
    let quarantine_identity = match PathIdentity::observe_deep(&quarantine) {
        Ok(identity) if expected.same_object(&identity) => Some(identity),
        Ok(identity) if identity.exists => {
            return Err(format!(
                "Rollback quarantine contains a foreign object: {}",
                quarantine.display()
            ));
        }
        Ok(_) | Err(_) => None,
    };

    if quarantine_identity.is_none() {
        let current_effect = PathIdentity::observe_deep(effect)
            .map_err(|error| format!("Could not inspect rollback effect: {error}"))?;
        if current_effect.exists {
            if !expected.same_binding(&current_effect) {
                return Err(format!(
                    "Completed effect changed before rollback: {}",
                    effect.display()
                ));
            }
            detach_expected_path(effect, expected, &quarantine)?;
        } else if record.kind == TransferKind::Move {
            let source = PathIdentity::observe_deep(&step.source)
                .map_err(|error| format!("Could not inspect rollback source: {error}"))?;
            if expected.same_object(&source) {
                return Ok(());
            }
            return Err(format!(
                "Rollback effect is absent from both landing and source: {}",
                effect.display()
            ));
        } else if let Some(before) = &step.destination_before {
            let current = PathIdentity::observe_deep(effect)
                .map_err(|error| format!("Could not inspect rollback destination: {error}"))?;
            if before.exists && before.same_binding(&current) {
                return Ok(());
            }
            if !before.exists && !current.exists {
                return Ok(());
            }
            return Err(format!(
                "Rollback destination has an unproved state: {}",
                effect.display()
            ));
        }
    }

    if record.kind == TransferKind::Move {
        crate::native_copy::rename_noreplace(&quarantine, &step.source)
            .map_err(|error| error.to_string())?;
        crate::fs_util::sync_parent_namespace(&step.source)
            .map_err(|error| format!("Could not sync restored source: {error}"))?;
        if step
            .destination_before
            .as_ref()
            .is_some_and(|identity| identity.exists)
        {
            let version = crate::version_store::record_for_key(&step.key)
                .ok_or_else(|| "Replaced destination has no local version".to_string())?;
            crate::version_store::restore(&version)?;
            crate::fs_util::sync_parent_namespace(effect)
                .map_err(|error| format!("Could not sync restored destination: {error}"))?;
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
        remove_expected_path(&quarantine, expected)?;
        crate::version_store::restore(&version)?;
        crate::fs_util::sync_parent_namespace(effect)
            .map_err(|error| format!("Could not sync restored destination: {error}"))
    } else {
        remove_expected_path(&quarantine, expected)
    }
}

fn rollback_created_container(record: &OperationRecord, plan: &mut RepairPlan) {
    let Some(cleanup) = &record.rollback_cleanup else {
        return;
    };
    let Some(expected) = &record.rollback_cleanup_identity else {
        plan.remaining.push(RepairItem {
            path: cleanup.clone(),
            action: "Legacy operation-created folder has no ownership proof".to_string(),
            automatic: false,
        });
        return;
    };
    let quarantine = record
        .rollback_cleanup_quarantine
        .clone()
        .unwrap_or_else(|| quarantine_path(cleanup));
    if record.rollback_cleanup_quarantine.is_none()
        && let Err(error) = mutate(|journal| {
            let operation = journal
                .operations
                .iter_mut()
                .find(|operation| operation.id == record.id)
                .ok_or_else(|| format!("Unknown operation {}", record.id.0))?;
            operation.rollback_cleanup_quarantine = Some(quarantine.clone());
            operation.updated_at_secs = now_secs();
            Ok(())
        })
    {
        plan.remaining.push(RepairItem {
            path: cleanup.clone(),
            action: error,
            automatic: false,
        });
        return;
    }

    let quarantined = PathIdentity::observe_deep(&quarantine).ok();
    if quarantined
        .as_ref()
        .is_none_or(|identity| !expected.same_object(identity))
    {
        let current = match PathIdentity::observe_deep(cleanup) {
            Ok(identity) => identity,
            Err(error) => {
                plan.remaining.push(RepairItem {
                    path: cleanup.clone(),
                    action: format!("Could not prove operation-created folder: {error}"),
                    automatic: false,
                });
                return;
            }
        };
        if !current.exists {
            let _ = clear_container_quarantine(&record.id, &quarantine);
            return;
        }
        if !expected.same_binding(&current) {
            plan.remaining.push(RepairItem {
                path: cleanup.clone(),
                action: "Operation-created folder identity changed; inspect manually".to_string(),
                automatic: false,
            });
            return;
        }
        if let Err(error) = detach_expected_path(cleanup, expected, &quarantine) {
            plan.remaining.push(RepairItem {
                path: cleanup.clone(),
                action: error,
                automatic: false,
            });
            return;
        }
    }

    match std::fs::remove_dir(&quarantine) {
        Ok(()) => {
            if let Err(error) = crate::fs_util::sync_parent_namespace(&quarantine)
                .map_err(|error| error.to_string())
                .and_then(|()| clear_container_quarantine(&record.id, &quarantine))
            {
                plan.remaining.push(RepairItem {
                    path: quarantine,
                    action: error,
                    automatic: false,
                });
                return;
            }
            plan.completed.push(RepairItem {
                path: cleanup.clone(),
                action: "Removed operation-created folder".to_string(),
                automatic: true,
            });
        }
        Err(error) => {
            let restored = crate::native_copy::rename_noreplace(&quarantine, cleanup);
            if restored.is_ok() {
                let _ = crate::fs_util::sync_parent_namespace(cleanup);
                let _ = clear_container_quarantine(&record.id, &quarantine);
            }
            plan.remaining.push(RepairItem {
                path: cleanup.clone(),
                action: if restored.is_ok() {
                    format!("Operation-created folder was not empty and was preserved: {error}")
                } else {
                    format!(
                        "Operation-created folder was preserved at {}: {error}",
                        quarantine.display()
                    )
                },
                automatic: false,
            });
        }
    }
}

fn clear_container_quarantine(operation_id: &OperationId, quarantine: &Path) -> Result<(), String> {
    mutate(|journal| {
        let operation = journal
            .operations
            .iter_mut()
            .find(|operation| &operation.id == operation_id)
            .ok_or_else(|| format!("Unknown operation {}", operation_id.0))?;
        if operation.rollback_cleanup_quarantine.as_deref() != Some(quarantine) {
            return Err("Rollback container quarantine changed before cleanup".to_string());
        }
        operation.rollback_cleanup_quarantine = None;
        operation.updated_at_secs = now_secs();
        Ok(())
    })
}

pub fn rollback(operation_id: &OperationId) -> Result<RepairPlan, String> {
    let initial = operation(operation_id)?;
    if initial.status == OperationStatus::Completed {
        finish(operation_id, OperationStatus::NeedsReview)?;
    }
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
        if step.rollback_quarantine.is_none() {
            match completed_effect_is_current(operation_id, &step.key) {
                Ok(true) => {}
                Ok(false) => {
                    plan.remaining.push(RepairItem {
                        path: effect.to_path_buf(),
                        action: "Journal step changed before rollback; inspect manually"
                            .to_string(),
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
        }
        let result = rollback_step(operation_id, &record, step);
        match result {
            Ok(()) => {
                mark_rolled_back(operation_id, &step.key)?;
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
        if let Some(quarantine) = &operation.rollback_cleanup_quarantine {
            referenced.insert(quarantine.clone());
        }
        for step in &operation.steps {
            if let Some(staging) = &step.staging {
                referenced.insert(staging.clone());
            }
            if let Some(replacement) = &step.replacement {
                referenced.insert(replacement.path.clone());
            }
            if let Some(quarantine) = &step.rollback_quarantine {
                referenced.insert(quarantine.clone());
            }
            for path in [
                Some(&step.source),
                Some(&step.destination),
                step.landing.as_ref(),
                step.staging.as_ref(),
                step.replacement
                    .as_ref()
                    .map(|replacement| &replacement.path),
                step.rollback_quarantine.as_ref(),
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
        .filter(|operation| {
            operation.status.recoverable()
                || operation.rollback_cleanup_quarantine.is_some()
                || operation
                    .steps
                    .iter()
                    .any(|step| step.replacement.is_some() || step.rollback_quarantine.is_some())
        })
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
    if !orphan.identity.same_binding(&current) {
        return Err("Orphan staging changed after discovery".to_string());
    }
    remove_expected_path(&orphan.path, &orphan.identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn serializable_state_machines_exhaust_every_transition_and_terminal_state() {
        let operation_statuses = [
            OperationStatus::Planned,
            OperationStatus::Running,
            OperationStatus::Stopped,
            OperationStatus::Failed,
            OperationStatus::NeedsReview,
            OperationStatus::Completed,
            OperationStatus::RolledBack,
        ];
        let operation_events = [
            OperationEvent::Start,
            OperationEvent::Stop,
            OperationEvent::Fail,
            OperationEvent::RequireReview,
            OperationEvent::Complete,
            OperationEvent::RollBack,
        ];
        for status in operation_statuses {
            for event in operation_events {
                let target = match event {
                    OperationEvent::Start => OperationStatus::Running,
                    OperationEvent::Stop => OperationStatus::Stopped,
                    OperationEvent::Fail => OperationStatus::Failed,
                    OperationEvent::RequireReview => OperationStatus::NeedsReview,
                    OperationEvent::Complete => OperationStatus::Completed,
                    OperationEvent::RollBack => OperationStatus::RolledBack,
                };
                let expected = status == target
                    || matches!(
                        (status, event),
                        (
                            OperationStatus::Planned
                                | OperationStatus::Stopped
                                | OperationStatus::Failed
                                | OperationStatus::NeedsReview,
                            OperationEvent::Start
                        ) | (
                            OperationStatus::Running,
                            OperationEvent::Stop
                                | OperationEvent::Fail
                                | OperationEvent::RequireReview
                                | OperationEvent::Complete
                        ) | (
                            OperationStatus::Planned
                                | OperationStatus::Running
                                | OperationStatus::Stopped
                                | OperationStatus::Failed
                                | OperationStatus::NeedsReview
                                | OperationStatus::Completed,
                            OperationEvent::RollBack
                        ) | (
                            OperationStatus::Stopped
                                | OperationStatus::Failed
                                | OperationStatus::Completed,
                            OperationEvent::RequireReview
                        )
                    );
                assert_eq!(
                    status.transition(event).ok(),
                    expected.then_some(target),
                    "operation {status:?} + {event:?}"
                );
            }
        }
        assert!(!OperationStatus::Planned.is_terminal());
        assert!(!OperationStatus::Running.is_terminal());
        assert!(OperationStatus::Stopped.is_terminal());
        assert!(OperationStatus::RolledBack.is_terminal());

        let step_statuses = [
            StepStatus::Planned,
            StepStatus::Running,
            StepStatus::Requeued,
            StepStatus::Completed,
            StepStatus::Skipped,
            StepStatus::Failed,
            StepStatus::RolledBack,
        ];
        let step_events = [
            StepEvent::Start,
            StepEvent::Checkpoint,
            StepEvent::Requeue,
            StepEvent::Complete,
            StepEvent::Skip,
            StepEvent::Fail,
            StepEvent::RollBack,
        ];
        for status in step_statuses {
            for event in step_events {
                let encoded = serde_json::to_string(&(status, event)).unwrap();
                let decoded: (StepStatus, StepEvent) = serde_json::from_str(&encoded).unwrap();
                assert_eq!(decoded, (status, event));
                let target = match event {
                    StepEvent::Start | StepEvent::Checkpoint => StepStatus::Running,
                    StepEvent::Requeue => StepStatus::Requeued,
                    StepEvent::Complete => StepStatus::Completed,
                    StepEvent::Skip => StepStatus::Skipped,
                    StepEvent::Fail => StepStatus::Failed,
                    StepEvent::RollBack => StepStatus::RolledBack,
                };
                let expected = status == target
                    || matches!(
                        (status, event),
                        (
                            StepStatus::Planned | StepStatus::Requeued | StepStatus::Failed,
                            StepEvent::Start
                        ) | (
                            StepStatus::Running,
                            StepEvent::Checkpoint | StepEvent::Requeue
                        ) | (
                            StepStatus::Planned
                                | StepStatus::Running
                                | StepStatus::Requeued
                                | StepStatus::Failed,
                            StepEvent::Complete | StepEvent::Skip | StepEvent::Fail
                        ) | (StepStatus::Completed, StepEvent::RollBack)
                    );
                assert_eq!(
                    status.transition(event).ok(),
                    expected.then_some(target),
                    "step {status:?} + {event:?}"
                );
            }
        }
        assert!(StepStatus::Completed.is_terminal());
        assert!(StepStatus::Skipped.is_terminal());
        assert!(StepStatus::RolledBack.is_terminal());
        assert!(!StepStatus::Failed.is_terminal());
    }

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
            version_retention: crate::operation::VersionRetentionPolicy::default(),
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            post_success: None,
            rollback_cleanup: None,
            rollback_cleanup_identity: None,
            rollback_cleanup_quarantine: None,
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
                replacement: None,
                rollback_quarantine: None,
                status,
                attempts: 1,
                failure: None,
                preflight_error: None,
            }],
        }
    }

    fn transfer_spec(operation_id: &str, entries: Vec<FileEntry>, target: &Path) -> TransferSpec {
        let expectations = crate::transfer::capture_expectations(&entries, target);
        TransferSpec {
            operation_id: OperationId(operation_id.to_string()),
            group_id: None,
            kind: TransferKind::Copy,
            entries,
            expectations,
            target: target.to_path_buf(),
            policy: OverwritePolicy::OverwriteAll,
            method: CopyMethod::Buffered,
            durability: DurabilityProfile::Verified,
            version_retention: crate::operation::VersionRetentionPolicy::Recent,
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            post_success: None,
            rollback_cleanup: None,
            rollback_cleanup_identity: None,
            mount_wait_override: None,
            before_commit: None,
            before_post_success: None,
            before_terminal_publish: None,
            journal_enabled: true,
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
    fn legacy_operation_defaults_version_retention() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source");
        let destination = temp.path().join("destination.txt");
        let record = incomplete_record(&source, &destination, StepStatus::Planned);
        let mut value = serde_json::to_value(record).unwrap();
        value.as_object_mut().unwrap().remove("version_retention");

        let restored: OperationRecord = serde_json::from_value(value).unwrap();
        assert_eq!(
            restored.version_retention,
            crate::operation::VersionRetentionPolicy::Recent
        );
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
        record.rollback_cleanup = Some(folder.clone());
        record.rollback_cleanup_identity = Some(PathIdentity::observe_deep(&folder).unwrap());
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
    fn semantic_validation_rejects_duplicate_operation_and_step_identities() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source");
        let destination = temp.path().join("destination.txt");
        let record = incomplete_record(&source, &destination, StepStatus::Planned);
        let duplicate_operations = Journal {
            operations: vec![record.clone(), record.clone()],
            ..Journal::default()
        };
        let path = temp.path().join("duplicate-operations.json");
        std::fs::write(&path, serde_json::to_vec(&duplicate_operations).unwrap()).unwrap();
        assert!(load_at(&path).unwrap_err().contains("duplicate operation"));

        let mut duplicate_steps = record;
        duplicate_steps.steps.push(duplicate_steps.steps[0].clone());
        let journal = Journal {
            operations: vec![duplicate_steps],
            ..Journal::default()
        };
        let path = temp.path().join("duplicate-steps.json");
        std::fs::write(&path, serde_json::to_vec(&journal).unwrap()).unwrap();
        assert!(load_at(&path).unwrap_err().contains("duplicate step"));
    }

    #[test]
    fn retention_never_discards_recoverable_operations() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source");
        let destination = temp.path().join("destination.txt");
        let mut journal = Journal::default();
        for index in 0..=MAX_OPERATIONS {
            let mut record = incomplete_record(&source, &destination, StepStatus::Failed);
            record.id = OperationId(format!("recoverable-{index}"));
            journal.operations.push(record);
        }

        let error = prune_terminal_history(&mut journal).unwrap_err();
        assert!(error.contains("full with recoverable work"), "{error}");
        assert_eq!(journal.operations.len(), MAX_OPERATIONS + 1);

        journal.operations[0].status = OperationStatus::RolledBack;
        prune_terminal_history(&mut journal).unwrap();
        assert_eq!(journal.operations.len(), MAX_OPERATIONS);
        assert!(
            journal
                .operations
                .iter()
                .all(|operation| operation.status.recoverable())
        );
    }

    #[test]
    fn begin_rejects_duplicate_reordered_and_policy_changed_contracts() {
        let temp = TempDir::new();
        let journal_path = temp.path().join("journal.json");
        let _journal = use_test_journal(journal_path);
        let target = temp.dir("target");
        let first = temp.file("first.txt", "first");
        let second = temp.file("second.txt", "second");
        let entries = vec![
            FileEntry::from_meta(first.clone(), &first.symlink_metadata().unwrap()).unwrap(),
            FileEntry::from_meta(second.clone(), &second.symlink_metadata().unwrap()).unwrap(),
        ];
        let mut duplicate = transfer_spec("strict-contract", entries.clone(), &target);
        let duplicate_key = IdempotencyKey("duplicate-key".to_string());
        duplicate.expectations[0].key = Some(duplicate_key.clone());
        duplicate.expectations[1].key = Some(duplicate_key);
        assert!(begin(&duplicate).unwrap_err().contains("duplicate"));
        assert!(load().unwrap().operations.is_empty());

        let spec = transfer_spec("strict-contract", entries, &target);
        begin(&spec).unwrap();
        let mut reordered = spec.clone();
        reordered.entries.swap(0, 1);
        reordered.expectations.swap(0, 1);
        assert!(begin(&reordered).unwrap_err().contains("reordered"));
        let mut changed_policy = spec;
        changed_policy.version_retention = crate::operation::VersionRetentionPolicy::Forever;
        assert!(
            begin(&changed_policy)
                .unwrap_err()
                .contains("different transfer contract")
        );
    }

    #[test]
    fn terminal_completion_proof_is_structural_immutable_and_stale_safe() {
        let temp = TempDir::new();
        let journal_path = temp.path().join("journal.json");
        let _journal = use_test_journal(journal_path);
        let target = temp.dir("target");
        let source = temp.file("source.txt", "source");
        let entry =
            FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
        let spec = transfer_spec("terminal-proof", vec![entry], &target);
        begin(&spec).unwrap();
        assert!(
            finish(&spec.operation_id, OperationStatus::Completed)
                .unwrap_err()
                .contains("not settled")
        );

        let destination = target.join("source.txt");
        let staging = target.join(".source.txt.cmdr-tmp.0");
        let key = step_key(&spec, 0, &destination);
        mark_running(
            &spec.operation_id,
            &key,
            &staging,
            &destination,
            PathIdentity::missing(&destination),
        )
        .unwrap();
        std::fs::write(&destination, "source").unwrap();
        mark_completed(
            &spec.operation_id,
            &key,
            &destination,
            crate::transfer_tuning::FastPath::Buffered,
        )
        .unwrap();
        let proof = operation(&spec.operation_id).unwrap().steps[0]
            .destination_after
            .clone()
            .unwrap();
        mark_completed(
            &spec.operation_id,
            &key,
            &destination,
            crate::transfer_tuning::FastPath::Buffered,
        )
        .unwrap();
        assert_eq!(
            operation(&spec.operation_id).unwrap().steps[0].destination_after,
            Some(proof.clone())
        );
        finish(&spec.operation_id, OperationStatus::Completed).unwrap();
        assert!(
            mark_failed(
                &spec.operation_id,
                &key,
                ClassifiedFailure::message(FailureClass::Blocked, None, "late callback"),
            )
            .unwrap_err()
            .contains("cannot accept step callbacks")
        );

        std::fs::write(&destination, "changed").unwrap();
        assert!(
            mark_completed(
                &spec.operation_id,
                &key,
                &destination,
                crate::transfer_tuning::FastPath::Buffered,
            )
            .unwrap_err()
            .contains("immutable proof")
        );
        assert_eq!(
            operation(&spec.operation_id).unwrap().steps[0].destination_after,
            Some(proof)
        );
        assert!(!completed_effect_is_current(&spec.operation_id, &key).unwrap_or(false));
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
    fn resume_accepts_an_appended_tail_but_rejects_a_replaced_staging_inode() {
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
            layout: crate::transfer::CheckpointLayout::Prefix,
            content_digest: Some(
                crate::transfer::prefix_digest(&staging, staging.metadata().unwrap().len())
                    .unwrap(),
            ),
        });

        let spec = build_resume_spec_from(record.clone()).unwrap();
        assert_eq!(
            spec.expectations[0]
                .resume
                .as_ref()
                .map(|checkpoint| checkpoint.offset),
            Some(6)
        );

        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&staging)
            .unwrap()
            .write_all(b"uncheckpointed tail")
            .unwrap();
        assert!(build_resume_spec_from(record.clone()).is_ok());

        let displaced = temp.path().join("displaced-partial");
        std::fs::rename(&staging, displaced).unwrap();
        std::fs::write(&staging, "changed partial").unwrap();
        let error = build_resume_spec_from(record).err().unwrap();
        assert!(error.contains("staging changed"), "{error}");
    }

    #[test]
    fn prefix_checkpoint_rejects_same_inode_same_length_byte_tampering() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source contents");
        let destination = temp.path().join("destination.txt");
        let staging = temp.file(".destination.cmdr-tmp.0", "source");
        let offset = staging.metadata().unwrap().len();
        let mut record = incomplete_record(&source, &destination, StepStatus::Running);
        record.steps[0].staging = Some(staging.clone());
        record.steps[0].checkpoint = Some(ResumeCheckpoint {
            staging: staging.clone(),
            offset,
            source: PathIdentity::observe_deep(&source).unwrap(),
            partial: PathIdentity::observe_deep(&staging).unwrap(),
            layout: crate::transfer::CheckpointLayout::Prefix,
            content_digest: Some(crate::transfer::prefix_digest(&staging, offset).unwrap()),
        });

        let file_id = record.steps[0].checkpoint.as_ref().unwrap().partial.file_id;
        std::fs::write(&staging, "xxxxxx").unwrap();
        assert_eq!(
            PathIdentity::observe_deep(&staging).unwrap().file_id,
            file_id
        );
        let error = build_resume_spec_from(record).err().unwrap();
        assert!(error.contains("bytes changed"), "{error}");
    }

    #[test]
    fn restart_restores_a_journaled_overwrite_backup_before_resuming() {
        let temp = TempDir::new();
        let journal_path = temp.path().join("journal.json");
        let _journal = use_test_journal(journal_path);
        let target = temp.dir("target");
        let source = temp.file("source.txt", "new bytes");
        let destination = temp.file("target/source.txt", "old bytes");
        let staging = temp.file("target/.source.txt.cmdr-tmp.0", "new bytes");
        let entry =
            FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
        let spec = transfer_spec("overwrite-restart", vec![entry], &target);
        begin(&spec).unwrap();
        let key = step_key(&spec, 0, &destination);
        let destination_before = PathIdentity::observe_deep(&destination).unwrap();
        mark_running(
            &spec.operation_id,
            &key,
            &staging,
            &destination,
            destination_before.clone(),
        )
        .unwrap();
        let offset = staging.metadata().unwrap().len();
        mark_checkpoint(
            &spec.operation_id,
            &key,
            ResumeCheckpoint {
                staging: staging.clone(),
                offset,
                source: PathIdentity::observe_deep(&source).unwrap(),
                partial: PathIdentity::observe_deep(&staging).unwrap(),
                layout: crate::transfer::CheckpointLayout::Prefix,
                content_digest: Some(crate::transfer::prefix_digest(&staging, offset).unwrap()),
            },
        )
        .unwrap();
        let backup = target.join(".source.txt.cmdr-tmp.backup");
        prepare_replacement(&spec.operation_id, &key, &staging, &destination, &backup).unwrap();
        crate::native_copy::rename_noreplace(&destination, &backup).unwrap();
        crate::fs_util::sync_parent_namespace(&destination).unwrap();
        mark_replacement_backed_up(&spec.operation_id, &key).unwrap();
        finish(&spec.operation_id, OperationStatus::NeedsReview).unwrap();

        assert!(!destination.exists());
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "old bytes");
        assert_eq!(
            load().unwrap().operations[0].steps[0]
                .replacement
                .as_ref()
                .unwrap()
                .phase,
            ReplacementPhase::OriginalBackedUp
        );

        let resumed = build_resume_spec(&spec.operation_id).unwrap();
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "old bytes");
        assert!(!backup.exists());
        assert!(
            operation(&spec.operation_id).unwrap().steps[0]
                .replacement
                .is_none()
        );
        assert_eq!(
            resumed.expectations[0].resume.as_ref().unwrap().offset,
            offset
        );
    }

    #[test]
    fn restart_finishes_a_rollback_from_its_proven_quarantine() {
        let temp = TempDir::new();
        let journal_path = temp.path().join("journal.json");
        let _journal = use_test_journal(journal_path);
        let target = temp.dir("target");
        let source = temp.file("source.txt", "source");
        let destination = target.join("source.txt");
        let entry =
            FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
        let spec = transfer_spec("rollback-restart", vec![entry], &target);
        begin(&spec).unwrap();
        let key = step_key(&spec, 0, &destination);
        mark_running(
            &spec.operation_id,
            &key,
            &target.join(".source.txt.cmdr-tmp.0"),
            &destination,
            PathIdentity::missing(&destination),
        )
        .unwrap();
        std::fs::write(&destination, "source").unwrap();
        mark_completed(
            &spec.operation_id,
            &key,
            &destination,
            crate::transfer_tuning::FastPath::Buffered,
        )
        .unwrap();
        finish(&spec.operation_id, OperationStatus::Completed).unwrap();
        finish(&spec.operation_id, OperationStatus::NeedsReview).unwrap();

        let expected = operation(&spec.operation_id).unwrap().steps[0]
            .destination_after
            .clone()
            .unwrap();
        let quarantine =
            prepare_rollback_quarantine(&spec.operation_id, &key, &destination).unwrap();
        detach_expected_path(&destination, &expected, &quarantine).unwrap();
        assert!(!destination.exists());
        assert!(quarantine.exists());

        assert_eq!(
            load().unwrap().operations[0].status,
            OperationStatus::NeedsReview
        );
        let plan = rollback(&spec.operation_id).unwrap();
        assert!(plan.remaining.is_empty(), "{:?}", plan.remaining);
        assert!(!destination.exists());
        assert!(!quarantine.exists());
        let record = operation(&spec.operation_id).unwrap();
        assert_eq!(record.status, OperationStatus::RolledBack);
        assert_eq!(record.steps[0].status, StepStatus::RolledBack);
    }

    #[test]
    fn delta_checkpoint_accepts_a_seeded_file_larger_than_its_offset() {
        let temp = TempDir::new();
        let source = temp.file("source-delta.txt", "source contents");
        let destination = temp.path().join("destination-delta.txt");
        let staging = temp.file(".destination-delta.cmdr-tmp.0", "basis contents!");
        let mut record = incomplete_record(&source, &destination, StepStatus::Running);
        record.steps[0].staging = Some(staging.clone());
        record.steps[0].checkpoint = Some(ResumeCheckpoint {
            staging: staging.clone(),
            offset: 0,
            source: PathIdentity::observe_deep(&source).unwrap(),
            partial: PathIdentity::observe_deep(&staging).unwrap(),
            layout: crate::transfer::CheckpointLayout::DeltaFixed,
            content_digest: None,
        });

        let spec = build_resume_spec_from(record).unwrap();

        let checkpoint = spec.expectations[0].resume.as_ref().unwrap();
        assert_eq!(checkpoint.offset, 0);
        assert_eq!(
            checkpoint.layout,
            crate::transfer::CheckpointLayout::DeltaFixed
        );
    }

    #[test]
    fn operation_record_round_trips_the_selected_fast_path() {
        let temp = TempDir::new();
        let path = temp.path().join("journal.json");
        let source = temp.file("source.txt", "source");
        let destination = temp.path().join("destination.txt");
        let mut record = incomplete_record(&source, &destination, StepStatus::Completed);
        std::fs::write(&destination, "source").unwrap();
        record.steps[0].landing = Some(destination.clone());
        record.steps[0].destination_after = Some(PathIdentity::observe_deep(&destination).unwrap());
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
    fn quarantine_removes_an_unchanged_created_copy() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "source");
        let destination = temp.path().join("destination.txt");
        std::fs::write(&destination, "source").unwrap();
        let expected = PathIdentity::observe_deep(&destination).unwrap();

        remove_expected_path(&destination, &expected).unwrap();
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
        record.rollback_cleanup_identity = Some(PathIdentity::observe_deep(&folder).unwrap());
        let journal_path = temp.path().join("journal.json");
        let _journal = use_test_journal(journal_path.clone());
        save_at(
            &journal_path,
            &Journal {
                operations: vec![record.clone()],
                ..Journal::default()
            },
        )
        .unwrap();
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
        record.rollback_cleanup_identity = Some(PathIdentity::observe_deep(&folder).unwrap());
        let journal_path = temp.path().join("journal.json");
        let _journal = use_test_journal(journal_path.clone());
        save_at(
            &journal_path,
            &Journal {
                operations: vec![record.clone()],
                ..Journal::default()
            },
        )
        .unwrap();
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
