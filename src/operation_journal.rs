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
/// Persist-envelope contract for this store. Domain schema stays on
/// [`Journal::schema`] / [`JOURNAL_SCHEMA`] and migrates independently.
const STORE: crate::persistence::StoreSpec =
    crate::persistence::StoreSpec::new("commander.operation_journal", 1, 32 * 1024 * 1024)
        .allow_legacy_schema_marker();

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
    pub const fn is_terminal(self) -> bool {
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

    pub const fn recoverable(self) -> bool {
        matches!(
            self,
            Self::Planned | Self::Running | Self::Stopped | Self::Failed | Self::NeedsReview
        )
    }

    pub const fn label(self) -> &'static str {
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
    pub const fn label(self) -> &'static str {
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

    pub const fn is_terminal(self) -> bool {
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
    #[serde(default)]
    pub source_followed: Vec<crate::path_identity::FollowedPathIdentity>,
    #[serde(default)]
    pub source_logical_bytes: Option<u64>,
    #[serde(default)]
    pub source_proof_complete: bool,
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
    /// Durable proof for non-overwrite placement (empty destination).
    /// Seals the staged identity before rename so resume can complete a
    /// crash between successful placement and `mark_completed`.
    #[serde(default)]
    pub placement: Option<PlacementProof>,
    #[serde(default)]
    pub rollback: Option<RollbackReceipt>,
    /// Schema-4 compatibility for journals written before rollback gained an
    /// explicit state machine. It is folded into `rollback` while loading and
    /// never populated by new writes.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlacementPhase {
    Prepared,
    Placed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementProof {
    /// Deep identity of the staged object sealed before rename-into-place.
    pub staged: PathIdentity,
    pub phase: PlacementPhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RollbackPhase {
    Prepared,
    EffectDetached,
    PrimaryReversed,
    DestinationRestored,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RollbackReceipt {
    pub quarantine: PathBuf,
    pub phase: RollbackPhase,
    #[serde(default)]
    pub version: Option<crate::version_store::VersionRecord>,
    #[serde(default)]
    pub version_identity: Option<PathIdentity>,
    #[serde(default)]
    pub restored_destination: Option<PathIdentity>,
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
pub struct TestJournalPathGuard {
    _serial: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for TestJournalPathGuard {
    fn drop(&mut self) {
        *crate::lock_util::recover(&TEST_JOURNAL_PATH) = None;
    }
}

#[cfg(test)]
pub fn use_test_journal(path: PathBuf) -> TestJournalPathGuard {
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
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(&lock_path).map_err(|error| {
        format!(
            "Could not open operation journal lock {}: {error}",
            lock_path.display()
        )
    })?;
    // `File::lock` is an exclusive advisory lock on every platform (flock on
    // unix, LockFileEx on Windows), so a second instance waits instead of
    // interleaving its load/modify/save. A signal only interrupts the wait.
    loop {
        match file.lock() {
            Ok(()) => return Ok(StoreLock { _file: file }),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                return Err(format!(
                    "Could not acquire operation journal lock {}: {error}",
                    lock_path.display()
                ));
            }
        }
    }
}

fn journal_path() -> PathBuf {
    #[cfg(test)]
    {
        if let Some(path) = crate::lock_util::recover(&TEST_JOURNAL_PATH).clone() {
            return path;
        }
        static DEFAULT_TEST_JOURNAL_PATH: OnceLock<PathBuf> = OnceLock::new();
        DEFAULT_TEST_JOURNAL_PATH
            .get_or_init(|| {
                std::env::temp_dir()
                    .join(format!("commander-test-journal-{}", std::process::id()))
                    .join("operation-journal.json")
            })
            .clone()
    }
    #[cfg(not(test))]
    crate::fs_util::config_dir().join("operation-journal.json")
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn migrate_and_validate(mut journal: Journal) -> Result<Journal, String> {
    let source_schema = journal.schema;
    for operation in &mut journal.operations {
        for step in &mut operation.steps {
            if step.rollback.is_none()
                && let Some(quarantine) = step.rollback_quarantine.take()
            {
                step.rollback = Some(RollbackReceipt {
                    quarantine,
                    phase: RollbackPhase::Prepared,
                    version: None,
                    version_identity: None,
                    restored_destination: None,
                });
                operation.status = OperationStatus::NeedsReview;
            }
        }
        if source_schema < JOURNAL_SCHEMA {
            let unproved_checkpoint = operation.steps.iter().any(|step| {
                step.checkpoint
                    .as_ref()
                    .is_some_and(|checkpoint| checkpoint.content_digest.is_none())
            });
            let unproved_container = operation.rollback_cleanup.is_some()
                && operation.rollback_cleanup_identity.is_none();
            if unproved_checkpoint || unproved_container {
                operation.status = OperationStatus::NeedsReview;
            }
        }
    }
    if source_schema < JOURNAL_SCHEMA {
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
                        || step.rollback.is_some()
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
                        || step
                            .rollback
                            .as_ref()
                            .is_some_and(|receipt| receipt.phase != RollbackPhase::Complete)
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
    let mut reachable_roots = vec![(
        step.source.clone(),
        step.source_before.as_ref().is_some_and(|identity| {
            identity.kind == Some(crate::path_identity::PathKind::Directory)
        }),
    )];
    for proof in &step.source_followed {
        if !reachable_roots.iter().any(|(root, descendants)| {
            proof.link == *root || (*descendants && proof.link.starts_with(root))
        }) {
            return Err(format!(
                "Operation {} step {} contains an unreachable followed proof",
                operation.id.0, step.key.0
            ));
        }
        if proof.target.kind == Some(crate::path_identity::PathKind::Directory) {
            reachable_roots.push((proof.target.path.clone(), true));
        }
    }
    if step.source_proof_complete
        && (step.source_before.is_none() || step.source_logical_bytes.is_none())
    {
        return Err(format!(
            "Operation {} step {} contains an incomplete source proof",
            operation.id.0, step.key.0
        ));
    }
    if !step.source_proof_complete
        && (!step.source_followed.is_empty() || step.source_logical_bytes.is_some())
    {
        return Err(format!(
            "Operation {} step {} contains unbound source proof fields",
            operation.id.0, step.key.0
        ));
    }
    if operation.symlink_policy != crate::filesystem_policy::SymlinkPolicy::Follow
        && !step.source_followed.is_empty()
    {
        return Err(format!(
            "Operation {} step {} contains followed proofs under another symlink policy",
            operation.id.0, step.key.0
        ));
    }

    if matches!(step.status, StepStatus::Completed | StepStatus::RolledBack)
        && (step.landing.is_none() || step.destination_after.is_none())
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
        if checkpoint.content_digest.is_none() && operation.status != OperationStatus::NeedsReview {
            return Err(format!(
                "Operation {} step {} has a legacy checkpoint without content proof",
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
    if let Some(placement) = &step.placement {
        if step.replacement.is_some() {
            return Err(format!(
                "Operation {} step {} has both overwrite and placement proofs",
                operation.id.0, step.key.0
            ));
        }
        let staging_ok = step
            .staging
            .as_ref()
            .is_some_and(|path| placement.staged.path == *path);
        if !staging_ok && !(placement.phase == PlacementPhase::Placed && step.landing.is_some()) {
            return Err(format!(
                "Operation {} step {} has an invalid placement proof",
                operation.id.0, step.key.0
            ));
        }
        if matches!(step.status, StepStatus::Completed | StepStatus::RolledBack) {
            return Err(format!(
                "Operation {} step {} still has an unreconciled placement proof",
                operation.id.0, step.key.0
            ));
        }
    }
    if step.rollback_quarantine.is_some() {
        return Err(format!(
            "Operation {} step {} contains an unmigrated rollback quarantine",
            operation.id.0, step.key.0
        ));
    }
    if let Some(receipt) = &step.rollback {
        if step.status != StepStatus::Completed && step.status != StepStatus::RolledBack {
            return Err(format!(
                "Operation {} step {} has rollback state outside an applied effect",
                operation.id.0, step.key.0
            ));
        }
        if receipt.quarantine == effect_path(step)
            || receipt.quarantine == step.source
            || receipt.version.as_ref().is_some_and(|version| {
                version.key != step.key || version.original != effect_path(step)
            })
            || receipt.version_identity.as_ref().is_some_and(|identity| {
                receipt
                    .version
                    .as_ref()
                    .is_none_or(|version| identity.path != version.stored)
            })
            || receipt
                .restored_destination
                .as_ref()
                .is_some_and(|identity| identity.path != effect_path(step))
        {
            return Err(format!(
                "Operation {} step {} has an invalid rollback receipt",
                operation.id.0, step.key.0
            ));
        }
        let replaces_existing = step
            .destination_before
            .as_ref()
            .is_some_and(|identity| identity.exists);
        if replaces_existing
            && receipt.phase >= RollbackPhase::EffectDetached
            && (receipt.version.is_none() || receipt.version_identity.is_none())
        {
            return Err(format!(
                "Operation {} step {} detached an overwrite without a durable version receipt",
                operation.id.0, step.key.0
            ));
        }
        if (replaces_existing
            && receipt.phase >= RollbackPhase::DestinationRestored
            && receipt.restored_destination.is_none())
            || (!replaces_existing && receipt.restored_destination.is_some())
        {
            return Err(format!(
                "Operation {} step {} claims an invalid destination restore",
                operation.id.0, step.key.0
            ));
        }
        if receipt.phase == RollbackPhase::Complete && step.status != StepStatus::RolledBack {
            return Err(format!(
                "Operation {} step {} has a terminal rollback receipt before rollback completion",
                operation.id.0, step.key.0
            ));
        }
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

struct LoadedJournal {
    journal: Journal,
    gate: crate::persistence::StoreGate,
}

fn load_journal_at(path: &Path) -> Result<LoadedJournal, String> {
    let persist = crate::persistence::FsPersist::default();
    let (journal, gate) =
        crate::persistence::load_strict::<Journal>(&persist, path, STORE, JOURNAL_NOUN)?;
    let Some(journal) = journal else {
        return Ok(LoadedJournal {
            journal: Journal::default(),
            gate,
        });
    };
    if journal.schema > JOURNAL_SCHEMA {
        return Err(format!(
            "Unsupported operation journal schema {} at {}: it was written by a newer Commander version; upgrade Commander to recover its operations",
            journal.schema,
            path.display()
        ));
    }
    if journal.schema == 0 {
        return Err(format!(
            "Unsupported operation journal schema 0 at {}; expected {JOURNAL_SCHEMA}",
            path.display()
        ));
    }
    Ok(LoadedJournal {
        journal: migrate_and_validate(journal)?,
        gate,
    })
}

/// Sentence-case name used in this store's error messages.
const JOURNAL_NOUN: &str = "Operation journal";

fn load_at(path: &Path) -> Result<Journal, String> {
    Ok(load_journal_at(path)?.journal)
}

fn map_journal_save_error(path: &Path, error: crate::persistence::JsonSaveError) -> String {
    use crate::persistence::PreCommitError;
    match error {
        crate::persistence::JsonSaveError::PreCommit(error) => {
            let stage = error.stage();
            match error {
                PreCommitError::Conflict => {
                    "Operation journal revision mismatch: another writer changed the store"
                        .to_string()
                }
                PreCommitError::Serialize(error) => {
                    format!("Operation journal could not be serialized: {error}")
                }
                PreCommitError::Io {
                    source, cleanup, ..
                } => {
                    let mut message =
                        format!("Operation journal {} was not committed", path.display());
                    if let Some(stage) = stage {
                        message.push_str(&format!(": could not {stage}"));
                    }
                    message.push_str(&format!(": {source}"));
                    if let Some(cleanup) = cleanup {
                        message.push_str(&format!("; cleanup also failed: {cleanup}"));
                    }
                    message
                }
            }
        }
        crate::persistence::JsonSaveError::Blocked(status) => {
            format!("Operation journal save blocked ({status:?})")
        }
        crate::persistence::JsonSaveError::GenerationExhausted => {
            "Operation journal generation counter exhausted".to_string()
        }
        crate::persistence::JsonSaveError::RecoveryPreservation => {
            "Operation journal could not preserve recovered source before upgrade".to_string()
        }
    }
}

fn save_journal_at(
    path: &Path,
    journal: &Journal,
    gate: &mut crate::persistence::StoreGate,
) -> Result<(), String> {
    validate_journal(journal)?;
    let persist = crate::persistence::FsPersist::default();
    match crate::persistence::save_enveloped(
        &persist,
        path,
        STORE,
        journal,
        gate,
        crate::persistence::SaveIntent::Automatic,
    ) {
        Ok(outcome) => crate::persistence::require_durable(JOURNAL_NOUN, outcome),
        Err(error) => Err(map_journal_save_error(path, error)),
    }
}

#[cfg(test)]
fn save_at(path: &Path, journal: &Journal) -> Result<(), String> {
    let persist = crate::persistence::FsPersist::default();
    let mut gate = crate::persistence::load_enveloped::<Journal>(&persist, path, STORE).gate;
    save_journal_at(path, journal, &mut gate)
}

fn mutate<T>(change: impl FnOnce(&mut Journal) -> Result<T, String>) -> Result<T, String> {
    let lock = JOURNAL_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = crate::lock_util::recover(lock);
    let path = journal_path();
    let _store_lock = acquire_store_lock(&path)?;
    let mut loaded = load_journal_at(&path)?;
    loaded.journal.schema = JOURNAL_SCHEMA;
    let result = change(&mut loaded.journal)?;
    prune_terminal_history(&mut loaded.journal)?;
    save_journal_at(&path, &loaded.journal, &mut loaded.gate)?;
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
            // A resume manifest lists only the steps still to run, in journal
            // order; every step it omits must already be settled.
            let settled = |step: &OperationStep| {
                matches!(step.status, StepStatus::Completed | StepStatus::Skipped)
            };
            let mut cursor = 0;
            for (key, source, destination) in &manifest {
                let matched = existing.steps[cursor..]
                    .iter()
                    .position(|step| step.key == *key)
                    .map(|offset| cursor + offset)
                    .filter(|&index| existing.steps[cursor..index].iter().all(settled));
                let Some(index) = matched.filter(|&index| {
                    let step = &existing.steps[index];
                    step.source == *source && step.destination == *destination
                }) else {
                    return Err(format!(
                        "Operation {} was reused with a reordered or different manifest entry",
                        spec.operation_id.0
                    ));
                };
                cursor = index + 1;
            }
            if !existing.steps[cursor..].iter().all(settled) {
                return Err(format!(
                    "Operation {} was reused with a different manifest length",
                    spec.operation_id.0
                ));
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
                let source_proof = expectation
                    .and_then(|value| value.source.as_ref().ok())
                    .cloned();
                let source = source_proof.as_ref().map(|proof| proof.lexical.clone());
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
                    source_followed: source_proof
                        .as_ref()
                        .map(|proof| proof.followed.clone())
                        .unwrap_or_default(),
                    source_logical_bytes: source_proof.as_ref().map(|proof| proof.logical_bytes),
                    source_proof_complete: source_proof.is_some(),
                    destination_before,
                    landing: None,
                    landing_before: None,
                    destination_after: None,
                    staging: None,
                    checkpoint: None,
                    fast_path: None,
                    replacement: None,
                    placement: None,
                    rollback: None,
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

fn operation_mut<'a>(
    operations: &'a mut [OperationRecord],
    operation_id: &OperationId,
) -> Result<&'a mut OperationRecord, String> {
    operations
        .iter_mut()
        .find(|operation| &operation.id == operation_id)
        .ok_or_else(|| format!("Unknown operation {}", operation_id.0))
}

fn step_mut<'a>(
    steps: &'a mut [OperationStep],
    key: &IdempotencyKey,
) -> Result<&'a mut OperationStep, String> {
    steps
        .iter_mut()
        .find(|step| &step.key == key)
        .ok_or_else(|| format!("Unknown operation step {}", key.0))
}

fn update_step(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    allowed_operation_statuses: &[OperationStatus],
    update: impl FnOnce(&mut OperationStep) -> Result<(), String>,
) -> Result<(), String> {
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
        if !allowed_operation_statuses.contains(&operation.status) {
            return Err(format!(
                "Operation {} cannot accept step callbacks while {}",
                operation_id.0,
                operation.status.label()
            ));
        }
        let step = step_mut(&mut operation.steps, key)?;
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

pub fn prepare_replacement(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    staged: &Path,
    destination: &Path,
    proposed_backup: &Path,
    expected_original: &PathIdentity,
) -> Result<ReplacementBackup, String> {
    let original = PathIdentity::observe_deep(destination)
        .map_err(|error| format!("Could not prove overwrite destination: {error}"))?;
    let replacement = PathIdentity::observe_deep(staged)
        .map_err(|error| format!("Could not prove overwrite staging: {error}"))?;
    if !original.exists || !replacement.exists {
        return Err("Overwrite preparation requires both original and staged objects".to_string());
    }
    let expected_matches = if expected_original.tree_fingerprint.is_some() {
        expected_original.same_binding(&original)
    } else {
        expected_original.same_shallow_binding(&original)
    };
    if !expected_matches {
        return Err("Overwrite destination changed after conflict review".to_string());
    }
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
        if operation.status != OperationStatus::Running {
            return Err(format!(
                "Operation {} cannot prepare an overwrite while {}",
                operation_id.0,
                operation.status.label()
            ));
        }
        let step = step_mut(&mut operation.steps, key)?;
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
        if !replacement.original.same_version(&backup) {
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

/// The journal's current record of one step, read under the journal lock and
/// released again so filesystem proofs can run without holding it.
fn step_snapshot(
    operation_id: &OperationId,
    key: &IdempotencyKey,
) -> Result<Option<OperationStep>, String> {
    let record = operation(operation_id)?;
    Ok((record.status == OperationStatus::Running)
        .then(|| record.steps.into_iter().find(|step| &step.key == key))
        .flatten())
}

pub fn mark_replacement_placed(
    operation_id: &OperationId,
    key: &IdempotencyKey,
) -> Result<(), String> {
    // Walk both trees before taking the journal lock; the update below only
    // compares these observations with the proof it finds there.
    let observed = step_snapshot(operation_id, key)?
        .and_then(|step| step.replacement)
        .filter(|replacement| replacement.phase == ReplacementPhase::OriginalBackedUp)
        .map(|replacement| {
            let destination = PathIdentity::observe_deep(&replacement.original.path)
                .map_err(|error| format!("Could not prove overwrite placement: {error}"));
            let backup = PathIdentity::observe_deep(&replacement.path)
                .map_err(|error| format!("Could not recheck overwrite backup: {error}"));
            (replacement, destination, backup)
        });
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
        let Some((_, destination, backup)) = observed
            .as_ref()
            .filter(|(proved, _, _)| proved == replacement)
        else {
            return Err("Overwrite proof changed while its placement was being proven".to_string());
        };
        if !replacement.replacement.same_version(destination.as_ref()?) {
            return Err("Overwrite destination is not the proven staged object".to_string());
        }
        if !replacement.original.same_version(backup.as_ref()?) {
            return Err("Overwrite original changed in its backup location".to_string());
        }
        replacement.phase = ReplacementPhase::ReplacementPlaced;
        Ok(())
    })
}

pub fn prepare_placement(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    staged: &Path,
    destination: &Path,
) -> Result<PlacementProof, String> {
    let staged_identity = PathIdentity::observe_deep(staged)
        .map_err(|error| format!("Could not prove non-overwrite staging: {error}"))?;
    if !staged_identity.exists {
        return Err("Non-overwrite placement requires a staged object".to_string());
    }
    let destination_identity = PathIdentity::observe(destination)
        .map_err(|error| format!("Could not prove empty destination: {error}"))?;
    if destination_identity.exists {
        return Err("Non-overwrite destination is already occupied".to_string());
    }
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
        if operation.status != OperationStatus::Running {
            return Err(format!(
                "Operation {} cannot prepare placement while {}",
                operation_id.0,
                operation.status.label()
            ));
        }
        let step = step_mut(&mut operation.steps, key)?;
        if step.status != StepStatus::Running
            || step.staging.as_deref() != Some(staged)
            || step.landing.as_deref() != Some(destination)
        {
            return Err(format!(
                "Operation step {} is not prepared for this placement",
                key.0
            ));
        }
        if step.replacement.is_some() {
            return Err(format!(
                "Operation step {} already has an overwrite proof",
                key.0
            ));
        }
        if let Some(existing) = &step.placement {
            if existing.staged.same_binding(&staged_identity) {
                return Ok(existing.clone());
            }
            return Err(format!(
                "Operation step {} already has a different placement proof",
                key.0
            ));
        }
        let prepared = PlacementProof {
            staged: staged_identity,
            phase: PlacementPhase::Prepared,
        };
        step.placement = Some(prepared.clone());
        operation.updated_at_secs = now_secs();
        Ok(prepared)
    })
}

pub fn mark_placement_placed(
    operation_id: &OperationId,
    key: &IdempotencyKey,
) -> Result<(), String> {
    // Walk the landing before taking the journal lock (see above).
    let observed = step_snapshot(operation_id, key)?
        .filter(|step| {
            step.placement
                .as_ref()
                .is_some_and(|placement| placement.phase == PlacementPhase::Prepared)
        })
        .and_then(|step| step.landing.map(|landing| (step.placement, landing)))
        .map(|(placement, landing)| {
            let destination = PathIdentity::observe_deep(&landing)
                .map_err(|error| format!("Could not prove non-overwrite placement: {error}"));
            (placement, landing, destination)
        });
    update_step(operation_id, key, &[OperationStatus::Running], |step| {
        let placement = step
            .placement
            .as_mut()
            .ok_or_else(|| "Non-overwrite placement was not prepared".to_string())?;
        if placement.phase == PlacementPhase::Placed {
            return Ok(());
        }
        if placement.phase != PlacementPhase::Prepared {
            return Err("Non-overwrite placement callback arrived in a stale phase".to_string());
        }
        let landing = step
            .landing
            .as_deref()
            .ok_or_else(|| "Non-overwrite placement has no landing".to_string())?;
        let Some((_, _, destination)) = observed.as_ref().filter(|(proved, proved_landing, _)| {
            proved.as_ref() == Some(&*placement) && proved_landing == landing
        }) else {
            return Err(
                "Non-overwrite proof changed while its placement was being proven".to_string(),
            );
        };
        if !placement.staged.same_version(destination.as_ref()?) {
            return Err("Non-overwrite destination is not the proven staged object".to_string());
        }
        placement.phase = PlacementPhase::Placed;
        Ok(())
    })
}

/// Durably record a step's completed effect.
///
/// `Err` means the completion was not recorded. Once the step is durably
/// `Completed`, a failure to release the overwrite backup does not undo
/// that: it comes back as `Ok(Some(warning))` so the caller can report the
/// kept backup instead of a failed journal update.
pub fn mark_completed(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    destination: &Path,
    fast_path: crate::transfer_tuning::FastPath,
) -> Result<Option<String>, String> {
    let destination_after = PathIdentity::observe_deep(destination)
        .map_err(|error| format!("Could not capture completed effect: {error}"))?;
    let replacement = mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
        let step = step_mut(&mut operation.steps, key)?;

        if step.status == StepStatus::Completed {
            check_duplicate_completion(step, destination, &destination_after, Some(fast_path))?;
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
        if let Some(placement) = &step.placement
            && placement.phase != PlacementPhase::Placed
        {
            return Err(format!(
                "Placement step {} reached completion before landing was proven",
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
        step.placement = None;
        step.checkpoint = None;
        step.fast_path = Some(fast_path);
        step.failure = None;
        operation.updated_at_secs = now_secs();
        Ok(step.replacement.clone())
    })?;

    let Some(replacement) = replacement else {
        return Ok(None);
    };
    let cleanup = PathIdentity::observe_deep(&replacement.path)
        .map_err(|error| format!("Could not inspect overwrite backup cleanup: {error}"))
        .and_then(|backup| release_overwrite_backup(operation_id, key, &replacement, &backup));
    Ok(cleanup.err().map(|error| {
        format!(
            "copy completed, but the overwritten original at {} was not cleaned up: {error}",
            replacement.path.display()
        )
    }))
}

/// A repeated completion of an already completed step must match the
/// immutable proof the first completion recorded. `fast_path` is compared
/// only when the caller knows it (recovery promotions do not).
fn check_duplicate_completion(
    step: &OperationStep,
    landing: &Path,
    destination_after: &PathIdentity,
    fast_path: Option<crate::transfer_tuning::FastPath>,
) -> Result<(), String> {
    let existing = step
        .destination_after
        .as_ref()
        .ok_or_else(|| "Completed step has no immutable effect proof".to_string())?;
    if step.landing.as_deref() != Some(landing)
        || fast_path.is_some_and(|fast_path| step.fast_path != Some(fast_path))
        || !existing.same_binding(destination_after)
    {
        return Err(format!(
            "Duplicate completion for step {} conflicts with its immutable proof",
            step.key.0
        ));
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
        step.failure = None;
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
        // An overwrite or placement proof is bound to the staging path, so
        // the path stays recorded until resume reconciles that proof.
        let proof_holds_staging = step.replacement.is_some() || step.placement.is_some();
        if failure.class != FailureClass::IntegrityUncertain
            && step.checkpoint.is_none()
            && !proof_holds_staging
        {
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
            let required_phase = if step
                .destination_before
                .as_ref()
                .is_some_and(|identity| identity.exists)
            {
                RollbackPhase::DestinationRestored
            } else {
                RollbackPhase::PrimaryReversed
            };
            let receipt = step
                .rollback
                .as_mut()
                .ok_or_else(|| "Rollback completion has no durable receipt".to_string())?;
            if receipt.phase < required_phase {
                return Err(format!(
                    "Rollback completion arrived before its filesystem proof for step {}",
                    step.key.0
                ));
            }
            receipt.phase = RollbackPhase::Complete;
            step.status = step
                .status
                .transition(StepEvent::RollBack)
                .map_err(|error| error.to_string())?;
            step.checkpoint = None;
            step.failure = None;
            Ok(())
        },
    )
}

pub fn finish(operation_id: &OperationId, status: OperationStatus) -> Result<(), String> {
    // Re-prove completed effects (deep tree walks) before taking the journal
    // lock; inside it each proof is only matched to the unchanged step.
    let proofs = if status == OperationStatus::Completed {
        operation(operation_id)?
            .steps
            .into_iter()
            .filter(|step| step.status == StepStatus::Completed)
            .map(|step| {
                let proof = prove_completed_effect(&step);
                (step, proof)
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
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
                        if step.replacement.is_some()
                            || step.rollback.is_some()
                            || step.rollback_quarantine.is_some()
                        {
                            return Err(format!(
                                "Operation step {} still has unreconciled filesystem state",
                                step.key.0
                            ));
                        }
                        let (_, proof) = proofs
                            .iter()
                            .find(|(proved, _)| proved == step)
                            .ok_or_else(|| {
                                format!(
                                    "Operation step {} changed while its effect was being proven",
                                    step.key.0
                                )
                            })?;
                        proof.clone()?;
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
                    || step
                        .rollback
                        .as_ref()
                        .is_some_and(|receipt| receipt.phase != RollbackPhase::Complete)
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

/// The step states an operation held when a run started, loaded once so the
/// run does not lock and parse the whole journal for every item.
///
/// Only a previous run can have settled a step: within a run each step is
/// checked before it is worked on and never again after it completes. The
/// completed-effect proof is still checked against the live filesystem.
pub struct SettledSteps {
    steps: std::collections::HashMap<IdempotencyKey, OperationStep>,
}

impl SettledSteps {
    pub fn load(operation_id: &OperationId) -> Result<Self, String> {
        let steps = operation(operation_id)?
            .steps
            .into_iter()
            .map(|step| (step.key.clone(), step))
            .collect();
        Ok(Self { steps })
    }

    pub fn is_settled(&self, key: &IdempotencyKey) -> Result<bool, String> {
        self.steps.get(key).map_or(Ok(false), settled_step)
    }
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
    reconcile_interrupted_placements(operation_id)?;
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
        if destination.exists && replacement.replacement.same_version(&destination) {
            // Placement already landed; close the crash window before
            // `mark_completed` rather than failing closed for review.
            complete_interrupted_overwrite_placement(
                operation_id,
                &step.key,
                replacement,
                &destination,
                &backup,
            )?;
            continue;
        }
        if backup.exists {
            if !replacement.original.same_version(&backup) {
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
            crate::fs_at::rename_noreplace(&replacement.path, &replacement.original.path).map_err(
                |error| {
                    format!(
                        "Could not restore interrupted overwrite from {}: {error}",
                        replacement.path.display()
                    )
                },
            )?;
            crate::fs_util::sync_parent_namespace(&replacement.original.path)
                .map_err(|error| format!("Could not sync restored overwrite: {error}"))?;
        } else if !replacement.original.same_binding(&destination) {
            return Err(format!(
                "Interrupted overwrite lost its proven original binding: {}",
                replacement.original.path.display()
            ));
        }

        mutate(|journal| {
            let operation = operation_mut(&mut journal.operations, operation_id)?;
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

/// Promote a proven overwrite placement that crashed before `mark_completed`.
///
/// Durable `ReplacementPlaced` (or live destination matching the staged
/// replacement after `OriginalBackedUp`) is enough to write the terminal
/// effect proof, clean the backup, and leave the step completed.
fn complete_interrupted_overwrite_placement(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    replacement: &ReplacementBackup,
    destination: &PathIdentity,
    backup: &PathIdentity,
) -> Result<(), String> {
    match replacement.phase {
        ReplacementPhase::Prepared => {
            return Err(format!(
                "Overwrite placement completed before its terminal proof; original is preserved at {} and requires review",
                replacement.path.display()
            ));
        }
        ReplacementPhase::OriginalBackedUp => {
            if !backup.exists {
                return Err(format!(
                    "Overwrite placement landed without its proven backup; inspect {}",
                    replacement.original.path.display()
                ));
            }
            if !replacement.original.same_version(backup) {
                return Err(format!(
                    "Interrupted overwrite backup changed and requires review: {}",
                    replacement.path.display()
                ));
            }
        }
        ReplacementPhase::ReplacementPlaced => {
            if backup.exists && !replacement.original.same_version(backup) {
                return Err(format!(
                    "Interrupted overwrite backup changed and requires review: {}",
                    replacement.path.display()
                ));
            }
        }
    }

    let destination_after = destination.clone();
    let landing = replacement.original.path.clone();
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
        let step = step_mut(&mut operation.steps, key)?;
        if step.replacement.as_ref() != Some(replacement) {
            return Err(format!(
                "Overwrite proof changed while completing step {}",
                key.0
            ));
        }
        if step.status == StepStatus::Completed {
            return check_duplicate_completion(step, &landing, &destination_after, None);
        }
        step.status = step
            .status
            .transition(StepEvent::Complete)
            .map_err(|error| error.to_string())?;
        step.landing = Some(landing);
        step.destination_after = Some(destination_after);
        step.checkpoint = None;
        if step.fast_path.is_none() {
            step.fast_path = Some(crate::transfer_tuning::FastPath::Resumed);
        }
        step.failure = None;
        operation.updated_at_secs = now_secs();
        Ok(())
    })?;

    release_overwrite_backup(operation_id, key, replacement, backup)
}

/// Remove a completed overwrite's backup of the original (only if it is still
/// that original), then drop the step's replacement record. Foreign data at
/// the backup path is preserved and reported.
fn release_overwrite_backup(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    replacement: &ReplacementBackup,
    backup: &PathIdentity,
) -> Result<(), String> {
    if backup.exists {
        if !replacement.original.same_version(backup) {
            return Err(
                "Overwrite backup was replaced before cleanup; foreign data was preserved"
                    .to_string(),
            );
        }
        remove_expected_path(&replacement.path, &replacement.original)?;
    }
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
        let step = step_mut(&mut operation.steps, key)?;
        if step.status != StepStatus::Completed || step.replacement.as_ref() != Some(replacement) {
            return Err(format!(
                "Overwrite proof for step {} changed before backup cleanup",
                key.0
            ));
        }
        step.replacement = None;
        step.staging = None;
        operation.updated_at_secs = now_secs();
        Ok(())
    })
}

/// Promote a proven non-overwrite placement that crashed before `mark_completed`.
///
/// Durable `Placed` (or live destination matching the sealed staged identity
/// after `Prepared`) is enough to write the terminal effect proof and leave
/// the step completed without re-copying.
fn reconcile_interrupted_placements(operation_id: &OperationId) -> Result<(), String> {
    let record = operation(operation_id)?;
    for step in &record.steps {
        let Some(placement) = &step.placement else {
            continue;
        };
        let landing = step
            .landing
            .as_deref()
            .ok_or_else(|| format!("Placement step {} has no landing", step.key.0))?;
        let destination = PathIdentity::observe_deep(landing)
            .map_err(|error| format!("Could not inspect interrupted placement: {error}"))?;
        let staging_gone = step
            .staging
            .as_ref()
            .map(|path| {
                PathIdentity::observe(path)
                    .map(|identity| !identity.exists)
                    .map_err(|error| format!("Could not inspect interrupted staging: {error}"))
            })
            .transpose()?
            .unwrap_or(true);
        if destination.exists && placement.staged.same_version(&destination) {
            complete_interrupted_placement(
                operation_id,
                &step.key,
                placement,
                landing,
                &destination,
            )?;
            continue;
        }
        let retryable_failure = step.status == StepStatus::Failed
            && step
                .failure
                .as_ref()
                .is_none_or(|failure| failure.class != FailureClass::IntegrityUncertain);
        if retryable_failure && placement.phase == PlacementPhase::Prepared && staging_gone {
            // An ordinary placement failure already undid its staging (deleted
            // the copy or moved a renamed source back); nothing was placed, so
            // the step is retried from its source.
            clear_failed_placement(operation_id, &step.key, placement)?;
            continue;
        }
        match placement.phase {
            PlacementPhase::Prepared if !destination.exists && !staging_gone => {
                // Staging still holds the sealed object; resume can retry rename.
            }
            PlacementPhase::Prepared if !staging_gone => {
                return Err(format!(
                    "Interrupted placement for {} cannot finish: the destination is now occupied by another object; the staged copy is kept at {}",
                    landing.display(),
                    step.staging
                        .as_deref()
                        .unwrap_or_else(|| Path::new("?"))
                        .display()
                ));
            }
            PlacementPhase::Prepared | PlacementPhase::Placed => {
                return Err(format!(
                    "Interrupted placement for {} lost its proven staged object and requires review",
                    landing.display()
                ));
            }
        }
    }
    Ok(())
}

fn clear_failed_placement(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    placement: &PlacementProof,
) -> Result<(), String> {
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
        let step = step_mut(&mut operation.steps, key)?;
        if step.status != StepStatus::Failed || step.placement.as_ref() != Some(placement) {
            return Err(format!(
                "Placement proof changed while retrying failed step {}",
                key.0
            ));
        }
        step.placement = None;
        step.staging = None;
        step.checkpoint = None;
        operation.updated_at_secs = now_secs();
        Ok(())
    })
}

fn complete_interrupted_placement(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    placement: &PlacementProof,
    landing: &Path,
    destination: &PathIdentity,
) -> Result<(), String> {
    match placement.phase {
        PlacementPhase::Prepared | PlacementPhase::Placed => {}
    }

    let destination_after = destination.clone();
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
        let step = step_mut(&mut operation.steps, key)?;
        if step.placement.as_ref() != Some(placement) {
            return Err(format!(
                "Placement proof changed while completing step {}",
                key.0
            ));
        }
        if step.status == StepStatus::Completed {
            return check_duplicate_completion(step, landing, &destination_after, None);
        }
        step.status = step
            .status
            .transition(StepEvent::Complete)
            .map_err(|error| error.to_string())?;
        step.landing = Some(landing.to_path_buf());
        step.destination_after = Some(destination_after);
        step.checkpoint = None;
        if step.fast_path.is_none() {
            step.fast_path = Some(crate::transfer_tuning::FastPath::Resumed);
        }
        step.failure = None;
        step.placement = None;
        step.staging = None;
        operation.updated_at_secs = now_secs();
        Ok(())
    })
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
    let mut preflight_bytes = 0_u64;
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
        let source = crate::scan::capture_transfer_source(&step.source, record.symlink_policy)
            .map_err(|failure| {
                format!(
                    "Could not re-stat recovery source {}: {}",
                    step.source.display(),
                    failure.message
                )
            })?;
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
        let destination = observe_with_expected_depth(&step.destination, destination_before)
            .map_err(|error| format!("Could not re-stat recovery destination: {error}"))?;
        if !source_before.same_binding(&source.lexical) {
            return Err(format!(
                "Recovery source changed since the operation: {}",
                step.source.display()
            ));
        }
        let expected_source = if step.source_proof_complete {
            let expected = crate::path_identity::TransferSourceIdentity {
                lexical: source_before.clone(),
                followed: step.source_followed.clone(),
                logical_bytes: step.source_logical_bytes.ok_or_else(|| {
                    format!(
                        "Recovery source proof has no logical size: {}",
                        step.source.display()
                    )
                })?,
            };
            if !expected.same_binding(&source) {
                return Err(format!(
                    "Recovery source bytes changed since the operation: {}",
                    step.source.display()
                ));
            }
            expected
        } else {
            if record.symlink_policy == crate::filesystem_policy::SymlinkPolicy::Follow
                && !source.followed.is_empty()
            {
                return Err(format!(
                    "Legacy recovery lacks followed-target proofs for {}",
                    step.source.display()
                ));
            }
            source
        };
        preflight_bytes = preflight_bytes
            .checked_add(expected_source.logical_bytes)
            .ok_or_else(|| "Recovery source size exceeds the supported range".to_string())?;
        if !destination_before.same_binding(&destination) {
            return Err(format!(
                "Recovery destination changed since the operation: {}",
                step.destination.display()
            ));
        }
        if let (Some(landing), Some(landing_before)) = (&step.landing, &step.landing_before) {
            let current = observe_with_expected_depth(landing, landing_before)
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
            source: Ok(expected_source),
            destination: Ok(destination_before.clone()),
            landing: step.landing.clone(),
            landing_before: step.landing_before.clone(),
            resume,
        });
    }
    Ok(TransferSpec {
        operation_id: record.id,
        group_id: record.group_id,
        expectations,
        policy: record.policy,
        method: record.method,
        durability: record.durability,
        version_retention: record.version_retention,
        name_policy: record.name_policy,
        symlink_policy: record.symlink_policy,
        preflight_bytes: Some(preflight_bytes),
        post_success: record.post_success,
        rollback_cleanup: record.rollback_cleanup,
        rollback_cleanup_identity: record.rollback_cleanup_identity,
        ..TransferSpec::new(record.kind, entries, record.target)
    })
}

fn observe_with_expected_depth(
    path: &Path,
    expected: &PathIdentity,
) -> std::io::Result<PathIdentity> {
    if expected.tree_fingerprint.is_some() {
        PathIdentity::observe_deep(path)
    } else {
        PathIdentity::observe(path)
    }
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
    let ids_known = checkpoint
        .partial
        .volume
        .zip(checkpoint.partial.file_id)
        .is_some()
        && partial.volume.zip(partial.file_id).is_some();
    // Without file ids (Windows) a prefix staging cannot be followed by
    // identity, and the tail written after the last checkpoint changes its
    // size and mtime. The prefix digests below prove the resumed bytes.
    let same_object =
        if !ids_known && checkpoint.layout == crate::transfer::CheckpointLayout::Prefix {
            checkpoint.partial.exists && partial.exists && checkpoint.partial.kind == partial.kind
        } else {
            checkpoint.partial.same_object(&partial)
        };
    if !same_object
        || partial.kind != Some(crate::path_identity::PathKind::File)
        || !checkpoint.size_fits(&partial)
    {
        return Err(format!(
            "Recovery staging changed after checkpoint: {}",
            staging.display()
        ));
    }
    let expected = checkpoint.content_digest.ok_or_else(|| {
        format!(
            "Recovery checkpoint has no content proof; inspect manually: {}",
            staging.display()
        )
    })?;
    let partial_digest = crate::transfer::prefix_digest(staging, checkpoint.offset)
        .map_err(|error| format!("Could not hash recovery staging: {error}"))?;
    let source_digest = crate::transfer::prefix_digest(&step.source, checkpoint.offset)
        .map_err(|error| format!("Could not hash recovery source: {error}"))?;
    if partial_digest != expected || source_digest != expected {
        return Err(format!(
            "Recovery checkpoint bytes changed after proof: {}",
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
                let current = match PathIdentity::observe_deep(effect) {
                    Ok(current) => current,
                    Err(error) => {
                        plan.remaining.push(RepairItem {
                            path: effect.to_path_buf(),
                            action: format!("Could not inspect completed effect: {error}"),
                            automatic: false,
                        });
                        continue;
                    }
                };
                let unchanged = step
                    .destination_after
                    .as_ref()
                    .is_some_and(|expected| expected.same_binding(&current));
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
                    // Rolling back an overwriting move also needs the
                    // replaced destination's version.
                    if step
                        .destination_before
                        .as_ref()
                        .is_some_and(|identity| identity.exists)
                    {
                        plan.remaining.push(restore_version_item(step, effect));
                    }
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
                    plan.remaining.push(restore_version_item(step, effect));
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

fn restore_version_item(step: &OperationStep, effect: &Path) -> RepairItem {
    let receipt_has_version = step
        .rollback
        .as_ref()
        .is_some_and(|receipt| receipt.version.is_some());
    let stored = if receipt_has_version {
        Ok(true)
    } else {
        crate::version_store::record_for_key(&step.key).map(|record| record.is_some())
    };
    let (action, automatic) = match stored {
        Ok(true) => ("Restore replaced local version".to_string(), true),
        Ok(false) => (
            "No preserved version of the replaced original was found; inspect manually".to_string(),
            false,
        ),
        Err(error) => (
            format!("Could not read the version store ({error}); inspect manually"),
            false,
        ),
    };
    RepairItem {
        path: effect.to_path_buf(),
        action,
        automatic,
    }
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

/// Longest stem kept in `.{stem}.cmdr-quarantine.{index}`, so the name fits
/// the common 255-byte file-name limit with room for any index.
const QUARANTINE_STEM_MAX: usize = 255 - ".".len() - ".cmdr-quarantine.".len() - 20;

fn quarantine_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "item".to_string());
    let mut end = name.len().min(QUARANTINE_STEM_MAX);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name.truncate(end);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    crate::fs_util::first_available(|index| parent.join(format!(".{name}.cmdr-quarantine.{index}")))
}

fn detach_expected_path(
    path: &Path,
    expected: &PathIdentity,
    quarantine: &Path,
) -> Result<PathBuf, String> {
    crate::fs_at::rename_noreplace(path, quarantine)
        .map_err(|error| format!("Could not quarantine {}: {error}", path.display()))?;
    // From here on the object sits at `quarantine`; every error names it.
    crate::fs_util::sync_parent_namespace(path).map_err(|error| {
        format!(
            "Could not sync quarantine rename of {} to {}: {error}",
            path.display(),
            quarantine.display()
        )
    })?;
    let moved = observe_with_expected_depth(quarantine, expected).map_err(|error| {
        format!(
            "Could not prove quarantined object at {}: {error}",
            quarantine.display()
        )
    })?;
    if expected.same_version(&moved) {
        return Ok(quarantine.to_path_buf());
    }

    let restored = crate::fs_at::rename_noreplace(quarantine, path);
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

fn rollback_version_receipt(
    step: &OperationStep,
    effect: &Path,
) -> Result<Option<(crate::version_store::VersionRecord, PathIdentity)>, String> {
    if !step
        .destination_before
        .as_ref()
        .is_some_and(|identity| identity.exists)
    {
        return Ok(None);
    }
    let version = crate::version_store::record_for_key(&step.key)
        .map_err(|error| format!("Could not read the version store: {error}"))?
        .ok_or_else(|| "Replaced destination has no local version".to_string())?;
    if version.key != step.key || version.original != effect {
        return Err("Replaced destination version is bound to another journal step".to_string());
    }
    let identity = PathIdentity::observe_deep(&version.stored)
        .map_err(|error| format!("Could not prove rollback version: {error}"))?;
    if !identity.exists {
        return Err("Replaced destination version is missing".to_string());
    }
    Ok(Some((version, identity)))
}

fn prepare_rollback_receipt(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    effect: &Path,
) -> Result<RollbackReceipt, String> {
    let proposed = quarantine_path(effect);
    let record = operation(operation_id)?;
    let step = record
        .steps
        .iter()
        .find(|step| &step.key == key)
        .ok_or_else(|| format!("Unknown operation step {}", key.0))?;
    // A receipt that already holds its version is reused below; the stored
    // version is re-proved by identity before restore, so a version-store
    // record pruned since the first attempt must not block the retry.
    let version = if step
        .rollback
        .as_ref()
        .is_some_and(|receipt| receipt.version.is_some())
    {
        None
    } else {
        rollback_version_receipt(step, effect)?
    };
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
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
        let step = step_mut(&mut operation.steps, key)?;
        if step.status != StepStatus::Completed {
            return Err(format!(
                "Only a completed step can prepare rollback: {}",
                key.0
            ));
        }
        if let Some(existing) = &mut step.rollback {
            if existing.version.is_none()
                && let Some((version, identity)) = &version
            {
                existing.version = Some(version.clone());
                existing.version_identity = Some(identity.clone());
            }
            return Ok(existing.clone());
        }
        let (version, version_identity) =
            version.clone().map_or((None, None), |(version, identity)| {
                (Some(version), Some(identity))
            });
        let receipt = RollbackReceipt {
            quarantine: proposed,
            phase: RollbackPhase::Prepared,
            version,
            version_identity,
            restored_destination: None,
        };
        step.rollback = Some(receipt.clone());
        operation.updated_at_secs = now_secs();
        Ok(receipt)
    })
}

fn record_rollback_phase(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    phase: RollbackPhase,
    restored_destination: Option<PathIdentity>,
) -> Result<RollbackReceipt, String> {
    update_step(
        operation_id,
        key,
        &[
            OperationStatus::Running,
            OperationStatus::Stopped,
            OperationStatus::Failed,
            OperationStatus::NeedsReview,
        ],
        |step| {
            let receipt = step
                .rollback
                .as_mut()
                .ok_or_else(|| "Rollback phase has no durable receipt".to_string())?;
            if phase < receipt.phase {
                return Ok(());
            }
            if let Some(restored) = restored_destination {
                if let Some(existing) = &receipt.restored_destination
                    && !existing.same_binding(&restored)
                {
                    return Err(
                        "Rollback destination restore conflicts with its immutable proof"
                            .to_string(),
                    );
                }
                receipt.restored_destination = Some(restored);
            }
            receipt.phase = phase;
            Ok(())
        },
    )?;
    operation(operation_id)?
        .steps
        .into_iter()
        .find(|step| step.key == *key)
        .and_then(|step| step.rollback)
        .ok_or_else(|| "Rollback receipt disappeared after phase update".to_string())
}

fn receipt_version_is_current(receipt: &RollbackReceipt) -> Result<(), String> {
    let Some(version) = &receipt.version else {
        return Ok(());
    };
    let expected = receipt
        .version_identity
        .as_ref()
        .ok_or_else(|| "Rollback version has no immutable identity".to_string())?;
    let current = PathIdentity::observe_deep(&version.stored)
        .map_err(|error| format!("Could not recheck rollback version: {error}"))?;
    if !expected.same_binding(&current) {
        return Err(format!(
            "Rollback version changed before restore: {}",
            version.stored.display()
        ));
    }
    Ok(())
}

fn observe_detached_effect(
    path: &Path,
    expected: &PathIdentity,
) -> Result<Option<PathIdentity>, String> {
    let current = PathIdentity::observe_deep(path)
        .map_err(|error| format!("Could not inspect detached rollback effect: {error}"))?;
    if !current.exists {
        return Ok(None);
    }
    if !expected.same_object(&current) || !expected.same_version(&current) {
        return Err(format!(
            "Rollback quarantine contains a foreign or changed object: {}",
            path.display()
        ));
    }
    Ok(Some(current))
}

fn remove_detached_effect(path: &Path, expected: &PathIdentity) -> Result<(), String> {
    if observe_detached_effect(path, expected)?.is_none() {
        return Ok(());
    }
    remove_path(path)?;
    crate::fs_util::sync_parent_namespace(path)
        .map_err(|error| format!("Could not sync rollback effect removal: {error}"))
}

fn restore_previous_destination(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    effect: &Path,
    mut receipt: RollbackReceipt,
) -> Result<RollbackReceipt, String> {
    receipt_version_is_current(&receipt)?;
    let version = receipt
        .version
        .as_ref()
        .ok_or_else(|| "Rollback overwrite has no durable version receipt".to_string())?;
    let current = PathIdentity::observe_deep(effect)
        .map_err(|error| format!("Could not inspect rollback destination: {error}"))?;
    if current.exists {
        if let Some(expected) = &receipt.restored_destination {
            if !expected.same_binding(&current) {
                return Err(format!(
                    "Restored rollback destination changed: {}",
                    effect.display()
                ));
            }
        } else if !crate::version_store::paths_equal(&version.stored, effect) {
            return Err(format!(
                "Rollback destination was repopulated with foreign data: {}",
                effect.display()
            ));
        }
    } else {
        crate::version_store::restore(version)?;
        crate::fs_util::sync_parent_namespace(effect)
            .map_err(|error| format!("Could not sync restored destination: {error}"))?;
    }
    if !crate::version_store::paths_equal(&version.stored, effect) {
        return Err(format!(
            "Rollback destination does not match its preserved version: {}",
            effect.display()
        ));
    }
    let restored = PathIdentity::observe_deep(effect)
        .map_err(|error| format!("Could not prove restored destination: {error}"))?;
    receipt = record_rollback_phase(
        operation_id,
        key,
        RollbackPhase::DestinationRestored,
        Some(restored),
    )?;
    Ok(receipt)
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
    let replaces_existing = step
        .destination_before
        .as_ref()
        .is_some_and(|identity| identity.exists);
    let mut receipt = prepare_rollback_receipt(operation_id, &step.key, effect)?;
    receipt_version_is_current(&receipt)?;

    let quarantined = observe_detached_effect(&receipt.quarantine, expected)?;
    let current_effect = PathIdentity::observe_deep(effect)
        .map_err(|error| format!("Could not inspect rollback effect: {error}"))?;
    let source = (record.kind == TransferKind::Move)
        .then(|| PathIdentity::observe_deep(&step.source))
        .transpose()
        .map_err(|error| format!("Could not inspect rollback source: {error}"))?;
    let replacement_at_effect = expected.same_binding(&current_effect);
    let replacement_at_source = source
        .as_ref()
        .is_some_and(|source| expected.same_object(source) && expected.same_version(source));

    if quarantined.is_none() && replacement_at_effect {
        detach_expected_path(effect, expected, &receipt.quarantine)?;
        receipt =
            record_rollback_phase(operation_id, &step.key, RollbackPhase::EffectDetached, None)?;
    } else if quarantined.is_some() && receipt.phase < RollbackPhase::EffectDetached {
        receipt =
            record_rollback_phase(operation_id, &step.key, RollbackPhase::EffectDetached, None)?;
    } else if current_effect.exists
        && !replacement_at_effect
        && receipt
            .restored_destination
            .as_ref()
            .is_none_or(|restored| !restored.same_binding(&current_effect))
        && receipt
            .version
            .as_ref()
            .is_none_or(|version| !crate::version_store::paths_equal(&version.stored, effect))
    {
        return Err(format!(
            "Completed effect changed before rollback: {}",
            effect.display()
        ));
    }

    if record.kind == TransferKind::Move {
        if !replacement_at_source {
            let quarantine =
                observe_detached_effect(&receipt.quarantine, expected)?.ok_or_else(|| {
                    format!(
                        "Rollback effect is absent from landing, quarantine, and source: {}",
                        effect.display()
                    )
                })?;
            let current_source = PathIdentity::observe_deep(&step.source)
                .map_err(|error| format!("Could not recheck rollback source: {error}"))?;
            if current_source.exists {
                return Err(format!(
                    "Rollback source was repopulated before restore: {}",
                    step.source.display()
                ));
            }
            if !expected.same_object(&quarantine) || !expected.same_version(&quarantine) {
                return Err(
                    "Rollback quarantine no longer contains the completed effect".to_string(),
                );
            }
            crate::fs_at::rename_noreplace(&receipt.quarantine, &step.source).map_err(|error| {
                format!(
                    "Could not move {} back to {}: {error}",
                    receipt.quarantine.display(),
                    step.source.display()
                )
            })?;
            crate::fs_util::sync_parent_namespace(&step.source)
                .map_err(|error| format!("Could not sync restored source: {error}"))?;
            crate::fs_util::sync_parent_namespace(&receipt.quarantine)
                .map_err(|error| format!("Could not sync detached rollback namespace: {error}"))?;
        }
        receipt = record_rollback_phase(
            operation_id,
            &step.key,
            RollbackPhase::PrimaryReversed,
            None,
        )?;
    } else if !replaces_existing {
        remove_detached_effect(&receipt.quarantine, expected)?;
        if PathIdentity::observe_deep(effect)
            .map_err(|error| format!("Could not verify removed copy: {error}"))?
            .exists
        {
            return Err(format!(
                "Rollback copy destination was repopulated: {}",
                effect.display()
            ));
        }
        receipt = record_rollback_phase(
            operation_id,
            &step.key,
            RollbackPhase::PrimaryReversed,
            None,
        )?;
    }

    if replaces_existing {
        receipt = restore_previous_destination(operation_id, &step.key, effect, receipt)?;
        if record.kind == TransferKind::Copy {
            remove_detached_effect(&receipt.quarantine, expected)?;
        }
    }

    if receipt.phase
        < if replaces_existing {
            RollbackPhase::DestinationRestored
        } else {
            RollbackPhase::PrimaryReversed
        }
    {
        return Err("Rollback filesystem effects were not durably proven".to_string());
    }
    Ok(())
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
    let mut quarantine = record
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

    // An unreadable quarantine may still hold the folder; never treat it as
    // absent, or the record that points at it would be cleared below.
    let quarantined = match PathIdentity::observe_deep(&quarantine) {
        Ok(identity) => identity,
        Err(error) => {
            plan.remaining.push(RepairItem {
                path: quarantine,
                action: format!("Could not inspect operation-created folder quarantine: {error}"),
                automatic: false,
            });
            return;
        }
    };
    if !expected.same_object(&quarantined) {
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
            if let Err(error) = clear_container_quarantine(&record.id, &quarantine) {
                plan.remaining.push(RepairItem {
                    path: cleanup.clone(),
                    action: error,
                    automatic: false,
                });
            }
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
        if quarantined.exists {
            // A foreign object took the persisted quarantine name; leave it
            // untouched and detach the folder under a fresh name instead.
            let fresh = quarantine_path(cleanup);
            if let Err(error) = replace_container_quarantine(&record.id, &quarantine, &fresh) {
                plan.remaining.push(RepairItem {
                    path: cleanup.clone(),
                    action: error,
                    automatic: false,
                });
                return;
            }
            quarantine = fresh;
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
            let restored = crate::fs_at::rename_noreplace(&quarantine, cleanup);
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

fn replace_container_quarantine(
    operation_id: &OperationId,
    quarantine: &Path,
    fresh: &Path,
) -> Result<(), String> {
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
        if operation.rollback_cleanup_quarantine.as_deref() != Some(quarantine) {
            return Err("Rollback container quarantine changed before cleanup".to_string());
        }
        operation.rollback_cleanup_quarantine = Some(fresh.to_path_buf());
        operation.updated_at_secs = now_secs();
        Ok(())
    })
}

fn clear_container_quarantine(operation_id: &OperationId, quarantine: &Path) -> Result<(), String> {
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
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
        if step.rollback.is_none() {
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
                Err(error) => {
                    plan.remaining.push(RepairItem {
                        path: effect.to_path_buf(),
                        action: format!("Could not verify completed effect: {error}"),
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
    // Detaching a folder that still holds unresolved entries would only
    // rename it to a hidden name and back. A persisted quarantine still runs:
    // the folder may be sitting under it from an interrupted attempt.
    if plan.remaining.is_empty() || record.rollback_cleanup_quarantine.is_some() {
        rollback_created_container(&record, &mut plan);
    } else if let Some(cleanup) = &record.rollback_cleanup
        && crate::fs_util::path_is_taken(cleanup)
    {
        plan.remaining.push(RepairItem {
            path: cleanup.clone(),
            action: "Operation-created folder is kept until the remaining entries are resolved"
                .to_string(),
            automatic: false,
        });
    }
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
            if let Some(receipt) = &step.rollback
                && receipt.phase != RollbackPhase::Complete
            {
                referenced.insert(receipt.quarantine.clone());
            }
            for path in [
                Some(&step.source),
                Some(&step.destination),
                step.landing.as_ref(),
                step.staging.as_ref(),
                step.replacement
                    .as_ref()
                    .map(|replacement| &replacement.path),
                step.rollback
                    .as_ref()
                    .filter(|receipt| receipt.phase != RollbackPhase::Complete)
                    .map(|receipt| &receipt.quarantine),
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
                || operation.steps.iter().any(|step| {
                    step.replacement.is_some()
                        || step
                            .rollback
                            .as_ref()
                            .is_some_and(|receipt| receipt.phase != RollbackPhase::Complete)
                })
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
            if is_commander_temporary_name(&name)
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

/// Match only names Commander generates: `.{name}.cmdr-tmp.{n}` and
/// `.{name}.cmdr-quarantine.{n}`. A user file that merely contains a marker
/// is never offered for deletion.
fn is_commander_temporary_name(name: &str) -> bool {
    name.starts_with('.')
        && [".cmdr-tmp.", ".cmdr-quarantine."].iter().any(|marker| {
            name.rfind(marker).is_some_and(|at| {
                let index = &name[at + marker.len()..];
                at > 0 && !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
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
mod tests;
