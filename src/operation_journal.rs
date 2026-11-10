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

mod validate;
use validate::*;
mod reconcile;
use reconcile::*;
mod resume;
pub use resume::*;
mod repair;
pub use repair::*;
mod inventory;
pub use inventory::*;

const JOURNAL_SCHEMA: u32 = 4;
const MAX_OPERATIONS: usize = 500;
/// Persist-envelope contract for this store. Domain schema stays on
/// [`Journal::schema`] / [`JOURNAL_SCHEMA`] and migrates independently.
const STORE: crate::persistence::StoreSpec =
    crate::persistence::StoreSpec::new("commander.operation_journal", 1, 32 * 1024 * 1024)
        .allow_legacy_schema_marker()
        .compact();

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
            // A persisted Running record may belong either to another live
            // Commander instance or to a process that stopped unexpectedly.
            // The journal intentionally does not claim ownership liveness, so
            // keep the user-facing label honest about both possibilities.
            Self::Running => "Running / interrupted",
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedStaging {
    pub path: PathBuf,
    pub identity: PathIdentity,
    pub operation_id: OperationId,
}

#[derive(Clone, Debug)]
pub struct RecoveryInventory {
    pub operations: Vec<OperationRecord>,
    pub orphans: Vec<OrphanStaging>,
    pub retained: Vec<RetainedStaging>,
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

fn recoverable_operation_is_effect_free(operation: &OperationRecord) -> bool {
    matches!(
        operation.status,
        OperationStatus::Failed | OperationStatus::Stopped
    ) && operation.rollback_cleanup.is_none()
        && operation.rollback_cleanup_quarantine.is_none()
        && operation.steps.iter().all(|step| {
            !matches!(step.status, StepStatus::Running | StepStatus::Completed)
                && step.staging.is_none()
                && step.checkpoint.is_none()
                && step.placement.is_none()
                && step.replacement.is_none()
                && step.rollback.is_none()
                && step.rollback_quarantine.is_none()
                && step.destination_after.is_none()
        })
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
        let now = crate::clock::now_secs();
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
        operation.updated_at_secs = crate::clock::now_secs();
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
        operation.updated_at_secs = crate::clock::now_secs();
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

/// Record that a failed replacement placement put the proven original back
/// at its destination. The staged replacement remains available for a later
/// retry, so the overwrite returns to its prepared phase.
pub fn mark_replacement_restored(
    operation_id: &OperationId,
    key: &IdempotencyKey,
) -> Result<(), String> {
    let observed = step_snapshot(operation_id, key)?
        .and_then(|step| step.replacement)
        .filter(|replacement| replacement.phase == ReplacementPhase::OriginalBackedUp)
        .map(|replacement| {
            let destination =
                PathIdentity::observe_deep(&replacement.original.path).map_err(|error| {
                    format!("Could not prove restored overwrite destination: {error}")
                });
            let staging = PathIdentity::observe_deep(&replacement.replacement.path)
                .map_err(|error| format!("Could not recheck staged replacement: {error}"));
            let backup = PathIdentity::observe(&replacement.path)
                .map_err(|error| format!("Could not prove vacated overwrite backup: {error}"));
            (replacement, destination, staging, backup)
        });
    update_step(operation_id, key, &[OperationStatus::Running], |step| {
        let replacement = step
            .replacement
            .as_mut()
            .ok_or_else(|| "Overwrite restoration was not prepared".to_string())?;
        if replacement.phase == ReplacementPhase::Prepared {
            return Ok(());
        }
        if replacement.phase != ReplacementPhase::OriginalBackedUp {
            return Err("Overwrite restoration callback arrived in a stale phase".to_string());
        }
        let Some((_, destination, staging, backup)) = observed
            .as_ref()
            .filter(|(proved, _, _, _)| proved == replacement)
        else {
            return Err(
                "Overwrite proof changed while its restoration was being proven".to_string(),
            );
        };
        if !replacement.original.same_version(destination.as_ref()?) {
            return Err("Overwrite destination is not the proven restored original".to_string());
        }
        if !replacement.replacement.same_version(staging.as_ref()?) {
            return Err("Staged overwrite changed while restoring the original".to_string());
        }
        if backup.as_ref()?.exists {
            return Err("Overwrite backup still exists after restoration".to_string());
        }
        replacement.phase = ReplacementPhase::Prepared;
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
        operation.updated_at_secs = crate::clock::now_secs();
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
        operation.updated_at_secs = crate::clock::now_secs();
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
        if step.placement.is_some() || step.replacement.is_some() {
            return Err(format!(
                "Operation step {} has an unresolved placement proof; resume or roll it back before skipping",
                step.key.0
            ));
        }
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
        operation.updated_at_secs = crate::clock::now_secs();
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

#[cfg(test)]
mod tests;
