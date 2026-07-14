//! Background transfer engine: copy/move with progress reporting.
//!
//! This module is UI-agnostic. Progress is shared through [`TransferState`]
//! and the caller supplies a `notify` callback (e.g. a repaint request), so
//! the engine never depends on egui.

use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::fs_util;
use crate::operation::{
    ClassifiedFailure, DurabilityProfile, FailureClass, IdempotencyKey, OperationGroupId,
    OperationId,
};
use crate::panel::FileEntry;
use crate::path_identity::PathIdentity;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

const COPY_BUF_SIZE: usize = 1024 * 1024; // 1 MB buffer
const CHECKPOINT_INTERVAL: u64 = 16 * 1024 * 1024;
const MAX_SOURCE_REQUEUES: usize = 2;
const MAX_MOUNT_RETRIES: usize = 2;

/// What to do when destination file already exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverwritePolicy {
    Ask,
    OverwriteAll,
    SkipAll,
    /// Write the incoming entry under a fresh "name copy" name, keeping the
    /// existing destination intact (Finder's "Keep Both").
    KeepBoth,
}

/// Copy strategy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CopyMethod {
    /// Byte-by-byte with 1MB buffer, full progress tracking.
    Buffered,
    /// Native macOS copyfile() with APFS clone support, xattr/ACL preservation.
    Native,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferKind {
    Copy,
    Move,
}

/// Live transfer progress shared between background thread and UI.
#[derive(Clone)]
pub struct TransferProgress {
    pub operation_id: Option<OperationId>,
    pub group_id: Option<OperationGroupId>,
    pub total_bytes: u64,
    pub copied_bytes: u64,
    pub current_file: String,
    pub current_file_size: u64,
    pub current_file_copied: u64,
    pub files_done: usize,
    pub files_total: usize,
    pub requeued_files: usize,
    pub backend_label: String,
    pub backend_reason: String,
    pub p95_latency_ms: f64,
    pub adaptive_concurrency: usize,
    pub bandwidth_limit: Option<u64>,
    pub waiting_reason: Option<String>,
    pub fast_paths: Vec<crate::transfer_tuning::FastPath>,
    pub delta_reused_bytes: u64,
    pub delta_source_bytes: u64,
    pub active_workers: usize,
    pub peak_workers: usize,
    pub speed_samples: Vec<(f64, f64)>, // (timestamp_secs, bytes_at_that_time)
    pub started_at: std::time::Instant,
    pub finished: bool,
    pub cancelled: bool,
    pub stop_requested: bool,
    pub stopped: bool,
    /// Per-file failures collected during the transfer.
    pub errors: Vec<String>,
    pub failures: Vec<ClassifiedFailure>,
    /// `(source, final landing)` for every entry a Move actually placed, so
    /// undo can target where files really landed: a KeepBoth conflict lands at
    /// "name copy.ext", not "name". Empty for copies.
    pub placements: Vec<(PathBuf, PathBuf)>,
}

pub type TransferState = Arc<Mutex<TransferProgress>>;

impl TransferProgress {
    pub fn new(total_bytes: u64, files_total: usize) -> Self {
        Self {
            operation_id: None,
            group_id: None,
            total_bytes,
            copied_bytes: 0,
            current_file: String::new(),
            current_file_size: 0,
            current_file_copied: 0,
            files_done: 0,
            files_total,
            requeued_files: 0,
            backend_label: String::new(),
            backend_reason: String::new(),
            p95_latency_ms: 0.0,
            adaptive_concurrency: 1,
            bandwidth_limit: None,
            waiting_reason: None,
            fast_paths: Vec::new(),
            delta_reused_bytes: 0,
            delta_source_bytes: 0,
            active_workers: 1,
            peak_workers: 1,
            speed_samples: vec![(0.0, 0.0)],
            started_at: std::time::Instant::now(),
            finished: false,
            cancelled: false,
            stop_requested: false,
            stopped: false,
            errors: Vec::new(),
            failures: Vec::new(),
            placements: Vec::new(),
        }
    }

    /// Current speed in bytes/sec (averaged over last 2 seconds).
    pub fn speed_bps(&self) -> f64 {
        self.speed_bps_at(self.started_at.elapsed().as_secs_f64())
    }

    /// Same as [`speed_bps`](Self::speed_bps) with an explicit "now"
    /// (seconds since transfer start) so the math is testable.
    fn speed_bps_at(&self, now: f64) -> f64 {
        if self.speed_samples.len() < 2 {
            return 0.0;
        }
        // Find sample ~2 seconds ago
        let window = 2.0;
        let cutoff = now - window;
        let old = self
            .speed_samples
            .iter()
            .rev()
            .find(|(t, _)| *t <= cutoff)
            .unwrap_or(&self.speed_samples[0]);
        let dt = now - old.0;
        if dt < 0.01 {
            return 0.0;
        }
        (self.copied_bytes as f64 - old.1) / dt
    }

    /// Estimated time remaining in seconds.
    pub fn eta_secs(&self) -> f64 {
        let speed = self.speed_bps();
        if speed < 1.0 {
            return 0.0;
        }
        let remaining = self.total_bytes.saturating_sub(self.copied_bytes) as f64;
        remaining / speed
    }

    /// Record a sample if at least 500ms passed since the last one.
    pub fn maybe_sample(&mut self) {
        let now = self.started_at.elapsed().as_secs_f64();
        if self
            .speed_samples
            .last()
            .is_none_or(|&(t, _)| now - t >= 0.5)
        {
            self.record_sample();
        }
    }

    /// Record a speed sample (call periodically from copy thread).
    pub fn record_sample(&mut self) {
        let t = self.started_at.elapsed().as_secs_f64();
        self.speed_samples.push((t, self.copied_bytes as f64));
        // Keep last 120 samples (~60 seconds at 2Hz)
        if self.speed_samples.len() > 120 {
            self.speed_samples.remove(0);
        }
    }
}

impl CopyMethod {
    /// Strategy entry point: copy one top-level entry with this method.
    /// Returns bytes copied (used as the progress base for the next entry).
    fn copy_entry(
        self,
        src: &Path,
        is_dir: bool,
        dest: &Path,
        context: CopyContext<'_>,
    ) -> std::io::Result<CopyOutcome> {
        if std::fs::symlink_metadata(src)?.file_type().is_symlink() {
            return match context.symlink_policy {
                crate::filesystem_policy::SymlinkPolicy::Preserve => {
                    copy_symlink(src, dest).map(|()| CopyOutcome {
                        bytes: 0,
                        fast_path: crate::transfer_tuning::FastPath::Buffered,
                    })
                }
                crate::filesystem_policy::SymlinkPolicy::Follow => {
                    let followed = std::fs::canonicalize(src)?;
                    let metadata = std::fs::symlink_metadata(&followed)?;
                    self.copy_entry(&followed, metadata.is_dir(), dest, context)
                }
                crate::filesystem_policy::SymlinkPolicy::Skip => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "skipped symlink reached the copy backend",
                )),
            };
        }
        let bandwidth_limited = context.rule.max_bytes_per_second.is_some();
        let source_size = (!is_dir)
            .then(|| src.metadata().map(|metadata| metadata.len()))
            .transpose()?;
        let resume_delta = context
            .resume
            .and_then(|checkpoint| checkpoint.layout.delta_mode());
        if resume_delta.is_some() && context.basis.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "delta checkpoint lost its basis",
            ));
        }
        if !is_dir
            && !bandwidth_limited
            && let Some(basis) = context.basis
            && let Ok(basis_metadata) = basis.metadata()
            && basis_metadata.is_file()
        {
            let mode = resume_delta.or_else(|| {
                if context.resume.is_none() {
                    crate::delta_copy::select(
                        context.profile.capabilities.delta,
                        source_size.unwrap_or_default(),
                        basis_metadata.len(),
                        context.tuning,
                    )
                } else {
                    None
                }
            });
            if let Some(mode) = mode {
                return copy_file_delta(src, basis, dest, mode, context).map(|stats| CopyOutcome {
                    bytes: stats.logical_bytes,
                    fast_path: mode.fast_path(),
                });
            }
        }
        let force_resumable =
            source_size.is_some_and(|size| requires_resumable_buffer(context.profile, size));
        let force_buffered = bandwidth_limited
            || force_resumable
            || context.symlink_policy != crate::filesystem_policy::SymlinkPolicy::Preserve;
        if !is_dir
            && context.resume.is_none()
            && context.profile.capabilities.sparse
            && is_sparse_file(src)
        {
            let mut limiter = crate::transfer_tuning::BandwidthLimiter::new(context.rule);
            return copy_file_sparse(src, dest, context.progress, &mut limiter).map(|bytes| {
                CopyOutcome {
                    bytes,
                    fast_path: crate::transfer_tuning::FastPath::Sparse,
                }
            });
        }
        match self {
            CopyMethod::Native if !force_buffered && context.resume.is_none() => {
                if is_dir {
                    crate::native_copy::copy_dir_native(
                        src,
                        dest,
                        context.progress,
                        context.base_bytes,
                    )
                    .map(|bytes| CopyOutcome {
                        bytes,
                        fast_path: crate::transfer_tuning::FastPath::Native,
                    })
                } else {
                    crate::native_copy::copy_file_native(
                        src,
                        dest,
                        context.progress,
                        context.base_bytes,
                        context.profile.capabilities.clone,
                    )
                    .map(|outcome| CopyOutcome {
                        bytes: outcome.bytes,
                        fast_path: if outcome.cloned {
                            crate::transfer_tuning::FastPath::Clone
                        } else {
                            crate::transfer_tuning::FastPath::Native
                        },
                    })
                }
            }
            CopyMethod::Native | CopyMethod::Buffered => {
                let mut limiter = crate::transfer_tuning::BandwidthLimiter::new(context.rule);
                if is_dir {
                    let workers = crate::transfer_tuning::snapshot(context.profile).concurrency;
                    if workers > 1
                        && context.rule.max_bytes_per_second.is_none()
                        && context.symlink_policy
                            == crate::filesystem_policy::SymlinkPolicy::Preserve
                    {
                        copy_dir_buffered_parallel(
                            src,
                            dest,
                            context.progress,
                            workers,
                            context.profile.capabilities.sparse,
                        )
                    } else {
                        copy_dir_buffered_with_limiter(
                            src,
                            dest,
                            context.progress,
                            &mut limiter,
                            context.profile.capabilities.sparse,
                            context.symlink_policy,
                        )
                    }
                    .map(|bytes| CopyOutcome {
                        bytes,
                        fast_path: crate::transfer_tuning::FastPath::Buffered,
                    })
                } else {
                    copy_file_buffered_with_limiter(
                        src,
                        dest,
                        context.progress,
                        &mut limiter,
                        context.resume,
                        Some(context.journal),
                        context.profile.capabilities.sparse,
                    )
                    .map(|bytes| CopyOutcome {
                        bytes,
                        fast_path: if context.resume.is_some() {
                            crate::transfer_tuning::FastPath::Resumed
                        } else {
                            crate::transfer_tuning::FastPath::Buffered
                        },
                    })
                }
            }
        }
    }
}

fn requires_resumable_buffer(
    profile: &crate::volume_profile::VolumeProfile,
    source_size: u64,
) -> bool {
    profile.capabilities.resumable
        && profile.backend.is_slow_link()
        && source_size >= crate::delta_copy::DELTA_MIN_BYTES
}

#[derive(Clone, Copy, Debug)]
struct CopyOutcome {
    bytes: u64,
    fast_path: crate::transfer_tuning::FastPath,
}

#[derive(Clone, Copy)]
struct JournalStep<'a> {
    operation_id: &'a OperationId,
    key: &'a IdempotencyKey,
    enabled: bool,
}

struct CopyContext<'a> {
    progress: &'a TransferState,
    base_bytes: u64,
    profile: &'a crate::volume_profile::VolumeProfile,
    rule: crate::transfer_tuning::VolumeRule,
    tuning: crate::transfer_tuning::TuningSnapshot,
    basis: Option<&'a Path>,
    resume: Option<&'a ResumeCheckpoint>,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    journal: JournalStep<'a>,
}

fn copy_file_delta(
    source: &Path,
    basis: &Path,
    destination: &Path,
    mode: crate::delta_copy::DeltaMode,
    context: CopyContext<'_>,
) -> std::io::Result<crate::delta_copy::DeltaStats> {
    let layout = match mode {
        crate::delta_copy::DeltaMode::Fixed => CheckpointLayout::DeltaFixed,
        crate::delta_copy::DeltaMode::ContentDefined => CheckpointLayout::DeltaCdc,
    };
    let resume_offset = context.resume.map(|checkpoint| checkpoint.offset);
    let journal = context.journal;
    let mut checkpoint =
        |offset| persist_delta_checkpoint(source, destination, offset, layout, journal);
    crate::delta_copy::copy_file(
        crate::delta_copy::DeltaRequest {
            source,
            basis,
            destination,
            mode,
            state: context.progress,
            base_bytes: context.base_bytes,
            allow_clone_seed: context.profile.capabilities.clone,
            resume_offset,
        },
        &mut checkpoint,
    )
}

fn persist_delta_checkpoint(
    source: &Path,
    destination: &Path,
    offset: u64,
    layout: CheckpointLayout,
    journal: JournalStep<'_>,
) -> std::io::Result<()> {
    if !journal.enabled {
        return Ok(());
    }
    let checkpoint = ResumeCheckpoint {
        staging: destination.to_path_buf(),
        offset,
        source: PathIdentity::observe_deep(source)?,
        partial: PathIdentity::observe_deep(destination)?,
        layout,
    };
    crate::operation_journal::mark_checkpoint(journal.operation_id, journal.key, checkpoint)
        .map_err(std::io::Error::other)
}

/// A copy/move request, fully described and detached from any UI state.
///
/// Note there is no conflict list here: the engine checks the destination
/// live at copy time (the confirmation dialog may sit open while the
/// filesystem changes), driven only by [`OverwritePolicy`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PostTransferAction {
    /// Remove a directory only if it is empty after a successful transfer.
    /// Used by Undo for "New Folder with Selection"; `remove_dir` deliberately
    /// preserves the folder if another process added anything to it.
    RemoveEmptyDir(PathBuf),
}

#[derive(Clone)]
pub struct TransferSpec {
    pub operation_id: OperationId,
    pub group_id: Option<OperationGroupId>,
    pub kind: TransferKind,
    pub entries: Vec<FileEntry>,
    pub expectations: Vec<TransferExpectation>,
    pub target: PathBuf,
    pub policy: OverwritePolicy,
    pub method: CopyMethod,
    pub durability: DurabilityProfile,
    pub name_policy: crate::filesystem_policy::NamePolicy,
    pub symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    pub post_success: Option<PostTransferAction>,
    /// A container created specifically for this operation and removable only
    /// after every completed effect has been rolled back out of it.
    pub rollback_cleanup: Option<PathBuf>,
    #[cfg(test)]
    pub before_commit: Option<BeforeCommitHook>,
    #[cfg(test)]
    pub journal_enabled: bool,
}

#[cfg(test)]
pub type BeforeCommitHook = Arc<dyn Fn(&Path, &Path) + Send + Sync>;

#[derive(Clone, Debug)]
pub struct TransferExpectation {
    pub key: Option<crate::operation::IdempotencyKey>,
    pub source: Result<PathIdentity, String>,
    pub destination: Result<PathIdentity, String>,
    pub landing: Option<PathBuf>,
    pub landing_before: Option<PathIdentity>,
    pub resume: Option<ResumeCheckpoint>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeCheckpoint {
    pub staging: PathBuf,
    pub offset: u64,
    pub source: PathIdentity,
    pub partial: PathIdentity,
    #[serde(default)]
    pub layout: CheckpointLayout,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckpointLayout {
    #[default]
    Prefix,
    DeltaFixed,
    DeltaCdc,
}

impl CheckpointLayout {
    fn delta_mode(self) -> Option<crate::delta_copy::DeltaMode> {
        match self {
            Self::Prefix => None,
            Self::DeltaFixed => Some(crate::delta_copy::DeltaMode::Fixed),
            Self::DeltaCdc => Some(crate::delta_copy::DeltaMode::ContentDefined),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Prefix => "buffered",
            Self::DeltaFixed => "delta",
            Self::DeltaCdc => "delta CDC",
        }
    }
}

pub fn capture_expectations(entries: &[FileEntry], target: &Path) -> Vec<TransferExpectation> {
    entries
        .iter()
        .map(|entry| TransferExpectation {
            key: None,
            source: capture_identity(&entry.path),
            destination: capture_identity(&target.join(&entry.name)),
            landing: None,
            landing_before: None,
            resume: None,
        })
        .collect()
}

fn capture_identity(path: &Path) -> Result<PathIdentity, String> {
    PathIdentity::observe_deep(path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))
}

struct TransferWorkItem {
    key: IdempotencyKey,
    entry: FileEntry,
    expectation: TransferExpectation,
    requeues: usize,
    mount_retries: usize,
}

fn prepare_source_retry(
    item: &mut TransferWorkItem,
    identity: PathIdentity,
) -> Result<(u64, u64), String> {
    if item.requeues >= MAX_SOURCE_REQUEUES {
        return Err(format!(
            "source changed repeatedly during transfer: {}",
            item.entry.path.display()
        ));
    }
    let old_size = entry_size(&item.entry);
    let metadata = std::fs::symlink_metadata(&item.entry.path)
        .map_err(|error| format!("Could not requeue {}: {error}", item.entry.path.display()))?;
    let refreshed = FileEntry::from_meta(item.entry.path.clone(), &metadata)
        .ok_or_else(|| format!("Could not requeue {}", item.entry.path.display()))?;
    let new_size = entry_size(&refreshed);
    item.entry = refreshed;
    item.expectation.source = Ok(identity);
    item.expectation.resume = None;
    item.requeues += 1;
    Ok((old_size, new_size))
}

fn reset_for_retry(progress: &TransferState, completed_bytes: u64, old_size: u64, new_size: u64) {
    let mut state = crate::lock_util::recover(progress);
    state.total_bytes = state
        .total_bytes
        .saturating_sub(old_size)
        .saturating_add(new_size);
    state.copied_bytes = completed_bytes;
    state.current_file_copied = 0;
    state.current_file_size = new_size;
    state.requeued_files += 1;
}

fn wait_for_mount(
    guard: &crate::mount_guard::MountGuard,
    label: &str,
    progress: &TransferState,
    notify: &impl Fn(),
) -> std::io::Result<()> {
    if guard.check() == crate::mount_guard::MountAvailability::Available {
        return Ok(());
    }
    {
        let mut state = crate::lock_util::recover(progress);
        state.waiting_reason = Some(format!(
            "{label} disconnected; waiting up to {} seconds",
            guard.policy.timeout_ms / 1_000
        ));
    }
    notify();
    let result = guard.wait_until_available(|| {
        notify();
        let state = crate::lock_util::recover(progress);
        !state.cancelled && !state.stop_requested
    });
    crate::lock_util::recover(progress).waiting_reason = None;
    notify();
    result
}

fn is_disconnect_error(error: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        error.kind(),
        ErrorKind::NotFound
            | ErrorKind::TimedOut
            | ErrorKind::ConnectionAborted
            | ErrorKind::ConnectionReset
            | ErrorKind::BrokenPipe
            | ErrorKind::UnexpectedEof
            | ErrorKind::NetworkDown
            | ErrorKind::NotConnected
            | ErrorKind::HostUnreachable
            | ErrorKind::StaleNetworkFileHandle
    )
}

fn record_failure(progress: &TransferState, entry_name: &str, failure: ClassifiedFailure) {
    let mut state = crate::lock_util::recover(progress);
    state
        .errors
        .push(format!("{entry_name}: {}", failure.message));
    state.failures.push(failure);
}

fn record_step_failure(
    progress: &TransferState,
    operation_id: &OperationId,
    key: &IdempotencyKey,
    entry_name: &str,
    failure: ClassifiedFailure,
    journal_enabled: bool,
) {
    if journal_enabled
        && let Err(error) =
            crate::operation_journal::mark_failed(operation_id, key, failure.clone())
    {
        record_failure(
            progress,
            entry_name,
            ClassifiedFailure::message(
                FailureClass::IntegrityUncertain,
                failure.path.clone(),
                format!("operation journal update failed: {error}"),
            ),
        );
    }
    record_failure(progress, entry_name, failure);
}

fn record_journal_error(
    progress: &TransferState,
    entry_name: &str,
    path: Option<PathBuf>,
    error: impl std::fmt::Display,
) {
    record_failure(
        progress,
        entry_name,
        ClassifiedFailure::message(
            FailureClass::IntegrityUncertain,
            path,
            format!("operation journal update failed: {error}"),
        ),
    );
}

fn complete_without_copy(
    progress: &TransferState,
    completed_bytes: &mut u64,
    size: u64,
    entry_name: &str,
    failure: Option<ClassifiedFailure>,
    journal: Option<(&OperationId, &IdempotencyKey, bool)>,
    notify: &impl Fn(),
) {
    *completed_bytes += size;
    let mut state = crate::lock_util::recover(progress);
    state.copied_bytes = *completed_bytes;
    state.files_done += 1;
    drop(state);
    if let Some(failure) = failure {
        if let Some((operation_id, key, enabled)) = journal {
            record_step_failure(progress, operation_id, key, entry_name, failure, enabled);
        } else {
            record_failure(progress, entry_name, failure);
        }
    } else if let Some((operation_id, key, true)) = journal
        && let Err(error) = crate::operation_journal::mark_skipped(operation_id, key)
    {
        record_journal_error(progress, entry_name, None, error);
    }
    notify();
}

#[cfg(test)]
fn journal_enabled(spec: &TransferSpec) -> bool {
    spec.journal_enabled
}

#[cfg(not(test))]
fn journal_enabled(_spec: &TransferSpec) -> bool {
    true
}

/// Size of one entry: its byte length, or the recursive size of a directory.
fn entry_size(entry: &FileEntry) -> u64 {
    if entry.is_dir {
        fs_util::dir_size_recursive(&entry.path)
    } else {
        entry.size
    }
}

/// Total bytes for all entries (recursively for dirs).
pub fn total_bytes(entries: &[FileEntry]) -> u64 {
    entries.iter().map(entry_size).sum()
}

/// Run the transfer on a background thread.
///
/// `notify` is invoked whenever visible progress changed; the UI passes a
/// repaint request here, keeping this module free of egui types.
pub fn spawn_transfer(
    spec: TransferSpec,
    progress: TransferState,
    notify: impl Fn() + Send + 'static,
) {
    std::thread::spawn(move || {
        let journal_enabled = journal_enabled(&spec);
        let target_profile = crate::volume_profile::profile(&spec.target);
        let target_mount = crate::mount_guard::MountGuard::capture(
            &spec.target,
            crate::mount_guard::ReconnectPolicy::default(),
        );
        let resource_rule = crate::transfer_tuning::rule_for(&target_profile);
        let tuning = crate::transfer_tuning::snapshot(&target_profile);
        {
            let mut state = crate::lock_util::recover(&progress);
            state.operation_id = Some(spec.operation_id.clone());
            state.group_id = spec.group_id.clone();
            state.backend_label = target_profile.backend.label().to_string();
            state.backend_reason = target_profile.reason.clone();
            state.p95_latency_ms = tuning.p95_latency_ms;
            state.adaptive_concurrency = tuning.concurrency;
            state.bandwidth_limit = resource_rule.max_bytes_per_second;
        }
        if journal_enabled && let Err(error) = crate::operation_journal::begin(&spec) {
            record_journal_error(&progress, "Operation", Some(spec.target.clone()), error);
            finish_progress(&progress);
            notify();
            return;
        }
        if let Err(error) = wait_for_mount(&target_mount, "Destination volume", &progress, &notify)
        {
            record_failure(
                &progress,
                "Operation",
                ClassifiedFailure::io(Some(spec.target.clone()), "mount unavailable", &error),
            );
            finish_progress(&progress);
            if journal_enabled {
                let _ = crate::operation_journal::finish(
                    &spec.operation_id,
                    crate::operation_journal::OperationStatus::Failed,
                );
            }
            notify();
            return;
        }
        while resource_rule.is_quiet_now() {
            let should_stop = {
                let mut state = crate::lock_util::recover(&progress);
                state.waiting_reason = Some("Quiet hours are active for this volume".to_string());
                state.cancelled || state.stop_requested
            };
            notify();
            if should_stop {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        crate::lock_util::recover(&progress).waiting_reason = None;
        let is_move = spec.kind == TransferKind::Move;
        let mut base_bytes: u64 = 0;
        let total_bytes = spec.entries.iter().map(entry_size).sum();
        crate::lock_util::recover(&progress).total_bytes = total_bytes;
        let expectations = if spec.expectations.len() == spec.entries.len() {
            spec.expectations
        } else {
            capture_expectations(&spec.entries, &spec.target)
        };
        let mut work = spec
            .entries
            .into_iter()
            .zip(expectations)
            .enumerate()
            .map(|(index, (entry, expectation))| {
                let destination = spec.target.join(&entry.name);
                let key = expectation
                    .key
                    .clone()
                    .unwrap_or_else(|| spec.operation_id.step_key(index, &destination));
                TransferWorkItem {
                    key,
                    entry,
                    expectation,
                    requeues: 0,
                    mount_retries: 0,
                }
            })
            .collect::<VecDeque<_>>();

        'work: while let Some(mut work_item) = work.pop_front() {
            let this_size = entry_size(&work_item.entry);
            let entry = work_item.entry.clone();
            let dest = spec.target.join(&entry.name);

            {
                let mut s = crate::lock_util::recover(&progress);
                s.current_file = entry.name.clone();
                if s.cancelled {
                    break; // fall through to the finished-setter below
                }
                if s.stop_requested {
                    s.stopped = true;
                    break;
                }
            }

            if let Err(error) =
                wait_for_mount(&target_mount, "Destination volume", &progress, &notify)
            {
                complete_without_copy(
                    &progress,
                    &mut base_bytes,
                    this_size,
                    &entry.name,
                    Some(ClassifiedFailure::io(
                        Some(spec.target.clone()),
                        "mount unavailable",
                        &error,
                    )),
                    Some((&spec.operation_id, &work_item.key, journal_enabled)),
                    &notify,
                );
                continue;
            }

            if journal_enabled {
                match crate::operation_journal::step_is_settled(&spec.operation_id, &work_item.key)
                {
                    Ok(true) => {
                        complete_without_copy(
                            &progress,
                            &mut base_bytes,
                            this_size,
                            &entry.name,
                            None,
                            None,
                            &notify,
                        );
                        continue;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        complete_without_copy(
                            &progress,
                            &mut base_bytes,
                            this_size,
                            &entry.name,
                            Some(ClassifiedFailure::message(
                                FailureClass::IntegrityUncertain,
                                Some(dest.clone()),
                                error,
                            )),
                            None,
                            &notify,
                        );
                        continue;
                    }
                }
            }

            let expected_source = match work_item.expectation.source.clone() {
                Ok(identity) => identity,
                Err(message) => {
                    complete_without_copy(
                        &progress,
                        &mut base_bytes,
                        this_size,
                        &entry.name,
                        Some(ClassifiedFailure::message(
                            FailureClass::Blocked,
                            Some(entry.path.clone()),
                            message,
                        )),
                        Some((&spec.operation_id, &work_item.key, journal_enabled)),
                        &notify,
                    );
                    continue;
                }
            };
            let source_before = match PathIdentity::observe_deep(&entry.path) {
                Ok(identity) => identity,
                Err(error) => {
                    complete_without_copy(
                        &progress,
                        &mut base_bytes,
                        this_size,
                        &entry.name,
                        Some(ClassifiedFailure::io(
                            Some(entry.path.clone()),
                            "source re-stat failed",
                            &error,
                        )),
                        Some((&spec.operation_id, &work_item.key, journal_enabled)),
                        &notify,
                    );
                    continue;
                }
            };
            if !expected_source.same_version(&source_before) {
                let entry_name = entry.name.clone();
                let entry_path = entry.path.clone();
                match prepare_source_retry(&mut work_item, source_before.clone()) {
                    Ok((old_size, new_size)) => {
                        if journal_enabled
                            && let Err(error) = crate::operation_journal::mark_requeued(
                                &spec.operation_id,
                                &work_item.key,
                                source_before,
                            )
                        {
                            complete_without_copy(
                                &progress,
                                &mut base_bytes,
                                this_size,
                                &entry_name,
                                Some(ClassifiedFailure::message(
                                    FailureClass::IntegrityUncertain,
                                    Some(entry_path),
                                    format!("operation journal update failed: {error}"),
                                )),
                                None,
                                &notify,
                            );
                            continue;
                        }
                        reset_for_retry(&progress, base_bytes, old_size, new_size);
                        work.push_back(work_item);
                        notify();
                    }
                    Err(message) => complete_without_copy(
                        &progress,
                        &mut base_bytes,
                        this_size,
                        &entry_name,
                        Some(ClassifiedFailure::message(
                            FailureClass::Retryable,
                            Some(entry_path),
                            message,
                        )),
                        Some((&spec.operation_id, &work_item.key, journal_enabled)),
                        &notify,
                    ),
                }
                continue;
            }

            if spec.symlink_policy == crate::filesystem_policy::SymlinkPolicy::Skip
                && source_before.kind == Some(crate::path_identity::PathKind::Symlink)
            {
                complete_without_copy(
                    &progress,
                    &mut base_bytes,
                    this_size,
                    &entry.name,
                    None,
                    Some((&spec.operation_id, &work_item.key, journal_enabled)),
                    &notify,
                );
                continue;
            }

            let expected_destination = match work_item.expectation.destination.clone() {
                Ok(identity) => identity,
                Err(message) => {
                    complete_without_copy(
                        &progress,
                        &mut base_bytes,
                        this_size,
                        &entry.name,
                        Some(ClassifiedFailure::message(
                            FailureClass::Blocked,
                            Some(dest.clone()),
                            message,
                        )),
                        Some((&spec.operation_id, &work_item.key, journal_enabled)),
                        &notify,
                    );
                    continue;
                }
            };

            // Reject destructive self-referential transfers (a directory into
            // itself or its own subtree, a file onto itself) before touching
            // anything, so neither source nor destination is harmed.
            if fs_util::is_within_or_equal(&dest, &entry.path) {
                complete_without_copy(
                    &progress,
                    &mut base_bytes,
                    this_size,
                    &entry.name,
                    Some(ClassifiedFailure::message(
                        FailureClass::UserDecision,
                        Some(entry.path.clone()),
                        "cannot copy a path into itself",
                    )),
                    Some((&spec.operation_id, &work_item.key, journal_enabled)),
                    &notify,
                );
                continue;
            }

            // Check the destination LIVE, not the scan-time conflict list: the
            // confirmation dialog can sit open while the filesystem changes.
            // `path_is_taken` (not `exists`) so a broken symlink occupying the
            // name is honoured as a conflict, matching `find_conflicts`.
            let dest_present = fs_util::path_is_taken(&dest);
            if dest_present {
                match spec.policy {
                    OverwritePolicy::SkipAll => {
                        complete_without_copy(
                            &progress,
                            &mut base_bytes,
                            this_size,
                            &entry.name,
                            None,
                            Some((&spec.operation_id, &work_item.key, journal_enabled)),
                            &notify,
                        );
                        continue;
                    }
                    OverwritePolicy::Ask => {
                        // No overwrite was confirmed (no conflict was shown, or
                        // the destination appeared after the scan): refuse
                        // rather than silently clobber it.
                        complete_without_copy(
                            &progress,
                            &mut base_bytes,
                            this_size,
                            &entry.name,
                            Some(ClassifiedFailure::message(
                                FailureClass::UserDecision,
                                Some(dest.clone()),
                                "destination already exists",
                            )),
                            Some((&spec.operation_id, &work_item.key, journal_enabled)),
                            &notify,
                        );
                        continue;
                    }
                    // Fall through; KeepBoth/OverwriteAll handled below.
                    OverwritePolicy::OverwriteAll | OverwritePolicy::KeepBoth => {}
                }
            }

            let (errors_before, failures_before) = {
                let state = crate::lock_util::recover(&progress);
                (state.errors.len(), state.failures.len())
            };

            // Every entry is staged beside its final landing. This gives new
            // directories the same no-merge guarantee as files and leaves one
            // commit point where destination identity can be revalidated.
            let landing = work_item.expectation.landing.clone().unwrap_or_else(|| {
                if dest_present && spec.policy == OverwritePolicy::KeepBoth {
                    fs_util::available_copy_name(&dest)
                } else {
                    dest.clone()
                }
            });
            let expected_landing =
                work_item
                    .expectation
                    .landing_before
                    .clone()
                    .unwrap_or_else(|| {
                        if landing == dest {
                            expected_destination
                        } else {
                            PathIdentity::missing(&landing)
                        }
                    });
            let replace_existing = expected_landing.exists;
            let copy_target = work_item.expectation.resume.as_ref().map_or_else(
                || staging_path(&landing),
                |checkpoint| checkpoint.staging.clone(),
            );
            if journal_enabled
                && let Err(error) = crate::operation_journal::mark_running(
                    &spec.operation_id,
                    &work_item.key,
                    &copy_target,
                    &landing,
                    expected_landing.clone(),
                )
            {
                complete_without_copy(
                    &progress,
                    &mut base_bytes,
                    this_size,
                    &entry.name,
                    Some(ClassifiedFailure::message(
                        FailureClass::IntegrityUncertain,
                        Some(landing.clone()),
                        format!("operation journal update failed: {error}"),
                    )),
                    None,
                    &notify,
                );
                continue;
            }

            // Same-volume moves are an instant, atomic rename instead of a
            // copy-then-delete: no transient duplication, no walk-and-copy of
            // every byte, and no second pass to remove the source. The copy
            // path is reserved for cross-volume moves and all copies.
            let renamed = is_move
                && !(source_before.kind == Some(crate::path_identity::PathKind::Symlink)
                    && spec.symlink_policy == crate::filesystem_policy::SymlinkPolicy::Follow)
                && entry
                    .path
                    .parent()
                    .is_some_and(|p| fs_util::same_volume(p, &spec.target));

            let attempt_base = base_bytes;
            let attempt_started = std::time::Instant::now();
            let attempt_tuning = crate::transfer_tuning::snapshot(&target_profile);
            let result = if renamed {
                rename_entry(
                    &entry.path,
                    &copy_target,
                    &entry,
                    this_size,
                    &progress,
                    base_bytes,
                )
                .map(|bytes| CopyOutcome {
                    bytes,
                    fast_path: crate::transfer_tuning::FastPath::Rename,
                })
            } else {
                spec.method.copy_entry(
                    &entry.path,
                    entry.is_dir,
                    &copy_target,
                    CopyContext {
                        progress: &progress,
                        base_bytes,
                        profile: &target_profile,
                        rule: resource_rule,
                        tuning: attempt_tuning,
                        basis: dest_present.then_some(dest.as_path()),
                        resume: work_item.expectation.resume.as_ref(),
                        symlink_policy: spec.symlink_policy,
                        journal: JournalStep {
                            operation_id: &spec.operation_id,
                            key: &work_item.key,
                            enabled: journal_enabled,
                        },
                    },
                )
            };
            crate::transfer_tuning::record(
                &target_profile,
                attempt_started.elapsed(),
                result.is_ok(),
            );
            let tuning = crate::transfer_tuning::snapshot(&target_profile);
            {
                let mut state = crate::lock_util::recover(&progress);
                state.p95_latency_ms = tuning.p95_latency_ms;
                state.adaptive_concurrency = tuning.concurrency;
            }

            match &result {
                Ok(outcome) => base_bytes += outcome.bytes,
                Err(e) => {
                    let resumable_partial = journal_enabled
                        && !renamed
                        && !entry.is_dir
                        && crate::fs_util::path_is_taken(&copy_target);
                    if crate::lock_util::recover(&progress).cancelled {
                        // For a rename this is a no-op (a failed rename never
                        // created `copy_target`); for a copy it drops the
                        // partial. Either way the source is left intact.
                        if !resumable_partial {
                            let _ = undo_placement(&copy_target, &entry.path, renamed);
                        }
                        break; // fall through to the finished-setter below
                    }
                    if is_disconnect_error(e) && work_item.mount_retries < MAX_MOUNT_RETRIES {
                        match wait_for_mount(
                            &target_mount,
                            "Destination volume",
                            &progress,
                            &notify,
                        ) {
                            Ok(()) => {
                                let checkpoint = if journal_enabled {
                                    crate::operation_journal::step_checkpoint(
                                        &spec.operation_id,
                                        &work_item.key,
                                    )
                                    .ok()
                                    .flatten()
                                } else {
                                    None
                                };
                                if checkpoint.is_none() {
                                    let _ = undo_placement(&copy_target, &entry.path, renamed);
                                }
                                work_item.expectation.resume = checkpoint;
                                work_item.mount_retries += 1;
                                base_bytes = attempt_base;
                                {
                                    let mut state = crate::lock_util::recover(&progress);
                                    state.copied_bytes = attempt_base;
                                    state.current_file_copied = 0;
                                }
                                work.push_front(work_item);
                                notify();
                                continue 'work;
                            }
                            Err(wait_error) => record_failure(
                                &progress,
                                &entry.name,
                                ClassifiedFailure::io(
                                    Some(entry.path.clone()),
                                    "mount reconnect failed",
                                    &wait_error,
                                ),
                            ),
                        }
                    } else {
                        record_failure(
                            &progress,
                            &entry.name,
                            ClassifiedFailure::io(Some(entry.path.clone()), "copy failed", e),
                        );
                    }
                    crate::volume_profile::invalidate(target_profile.volume_id);
                }
            }

            #[cfg(test)]
            if result.is_ok()
                && let Some(hook) = &spec.before_commit
            {
                hook(&entry.path, &landing);
            }

            if result.is_ok() && crate::lock_util::recover(&progress).errors.len() == errors_before
            {
                let observed_source = if renamed {
                    copy_target.as_path()
                } else {
                    entry.path.as_path()
                };
                match PathIdentity::observe_deep(observed_source) {
                    Ok(source_after) if source_before.same_version(&source_after) => {}
                    Ok(_) => {
                        let restore_error = undo_placement(&copy_target, &entry.path, renamed);
                        if let Some(message) = restore_error {
                            record_failure(
                                &progress,
                                &entry.name,
                                ClassifiedFailure::message(
                                    FailureClass::IntegrityUncertain,
                                    Some(copy_target.clone()),
                                    message,
                                ),
                            );
                        } else {
                            match PathIdentity::observe_deep(&entry.path) {
                                Ok(fresh_identity) => {
                                    let entry_name = entry.name.clone();
                                    let entry_path = entry.path.clone();
                                    match prepare_source_retry(&mut work_item, fresh_identity) {
                                        Ok((old_size, new_size)) => {
                                            let refreshed_identity =
                                                work_item.expectation.source.as_ref().ok().cloned();
                                            if journal_enabled
                                                && let Some(identity) = refreshed_identity
                                                && let Err(error) =
                                                    crate::operation_journal::mark_requeued(
                                                        &spec.operation_id,
                                                        &work_item.key,
                                                        identity,
                                                    )
                                            {
                                                record_journal_error(
                                                    &progress,
                                                    &entry_name,
                                                    Some(entry_path),
                                                    error,
                                                );
                                                continue 'work;
                                            }
                                            base_bytes = attempt_base;
                                            reset_for_retry(
                                                &progress, base_bytes, old_size, new_size,
                                            );
                                            work.push_back(work_item);
                                            notify();
                                            continue 'work;
                                        }
                                        Err(message) => record_failure(
                                            &progress,
                                            &entry_name,
                                            ClassifiedFailure::message(
                                                FailureClass::Retryable,
                                                Some(entry_path),
                                                message,
                                            ),
                                        ),
                                    }
                                }
                                Err(error) => record_failure(
                                    &progress,
                                    &entry.name,
                                    ClassifiedFailure::io(
                                        Some(entry.path.clone()),
                                        "source re-stat failed after copy",
                                        &error,
                                    ),
                                ),
                            }
                        }
                    }
                    Err(error) => {
                        let restore_error = undo_placement(&copy_target, &entry.path, renamed);
                        record_failure(
                            &progress,
                            &entry.name,
                            ClassifiedFailure::io(
                                Some(entry.path.clone()),
                                "source re-stat failed after copy",
                                &error,
                            ),
                        );
                        if let Some(message) = restore_error {
                            record_failure(
                                &progress,
                                &entry.name,
                                ClassifiedFailure::message(
                                    FailureClass::IntegrityUncertain,
                                    Some(copy_target.clone()),
                                    message,
                                ),
                            );
                        }
                    }
                }
            }

            // "Clean" = Ok return AND no per-file errors recorded by the
            // native callback during this entry.
            let mut clean = result.is_ok()
                && crate::lock_util::recover(&progress).errors.len() == errors_before;
            let verification_source = if spec.symlink_policy
                == crate::filesystem_policy::SymlinkPolicy::Follow
                && source_before.kind == Some(crate::path_identity::PathKind::Symlink)
            {
                std::fs::canonicalize(&entry.path).unwrap_or_else(|_| entry.path.clone())
            } else {
                entry.path.clone()
            };
            if clean
                && spec.durability.verifies()
                && !renamed
                && !crate::version_store::paths_equal(&verification_source, &copy_target)
            {
                record_failure(
                    &progress,
                    &entry.name,
                    ClassifiedFailure::message(
                        FailureClass::IntegrityUncertain,
                        Some(copy_target.clone()),
                        "copy verification failed",
                    ),
                );
                clean = false;
            }
            if clean {
                match PathIdentity::observe_deep(&landing) {
                    Ok(current) if expected_landing.same_version(&current) => {}
                    Ok(_) => {
                        record_failure(
                            &progress,
                            &entry.name,
                            ClassifiedFailure::message(
                                FailureClass::UserDecision,
                                Some(landing.clone()),
                                "destination changed after conflict review",
                            ),
                        );
                        clean = false;
                    }
                    Err(error) => {
                        record_failure(
                            &progress,
                            &entry.name,
                            ClassifiedFailure::io(
                                Some(landing.clone()),
                                "destination identity check failed",
                                &error,
                            ),
                        );
                        clean = false;
                    }
                }
            }

            let resumable_partial = result.is_err()
                && journal_enabled
                && !renamed
                && !entry.is_dir
                && crate::fs_util::path_is_taken(&copy_target);
            let placed = if !clean {
                // Undo our placement; a pre-existing dest is untouched. For a
                // rename this restores the source rather than deleting its only
                // copy.
                if !resumable_partial
                    && let Some(msg) = undo_placement(&copy_target, &entry.path, renamed)
                {
                    record_failure(
                        &progress,
                        &entry.name,
                        ClassifiedFailure::message(
                            FailureClass::IntegrityUncertain,
                            Some(copy_target.clone()),
                            msg,
                        ),
                    );
                }
                false
            } else {
                let version_result = if replace_existing && spec.durability.keeps_versions() {
                    crate::version_store::preserve(
                        &landing,
                        &spec.operation_id,
                        work_item.key.clone(),
                    )
                    .map(|_| ())
                    .map_err(std::io::Error::other)
                } else {
                    Ok(())
                };
                let placement = version_result.and_then(|()| {
                    if replace_existing {
                        swap_into_place(&copy_target, &landing)
                    } else {
                        crate::native_copy::rename_noreplace(&copy_target, &landing)
                    }
                });
                match placement {
                    Ok(()) => true,
                    Err(e) => {
                        // The swap left the staged data at `copy_target`. For a
                        // rename that is the source's ONLY copy, so move it back
                        // to the source instead of deleting it (the previous
                        // unconditional cleanup here lost the source on a failed
                        // same-volume overwrite move).
                        let extra = undo_placement(&copy_target, &entry.path, renamed);
                        record_failure(
                            &progress,
                            &entry.name,
                            ClassifiedFailure::io(
                                Some(landing.clone()),
                                "final placement failed",
                                &e,
                            ),
                        );
                        if let Some(msg) = extra {
                            record_failure(
                                &progress,
                                &entry.name,
                                ClassifiedFailure::message(
                                    FailureClass::IntegrityUncertain,
                                    Some(copy_target.clone()),
                                    msg,
                                ),
                            );
                        }
                        false
                    }
                }
            };

            // Delete the source only once the destination is fully in place.
            // A same-volume rename already moved the source, so there is
            // nothing left to remove.
            if is_move
                && placed
                && !renamed
                && let Err(error) = cleanup_path(&entry.path)
            {
                record_failure(
                    &progress,
                    &entry.name,
                    ClassifiedFailure::message(
                        FailureClass::IntegrityUncertain,
                        Some(entry.path.clone()),
                        format!("destination is complete but source cleanup failed: {error}"),
                    ),
                );
            }

            // Record where a successfully moved entry actually landed, so undo
            // reverses the real placement (a KeepBoth conflict lands at a "copy"
            // name, not the original).
            if is_move && placed {
                crate::lock_util::recover(&progress)
                    .placements
                    .push((entry.path.clone(), landing.clone()));
            }
            if placed {
                crate::lock_util::recover(&progress).fast_paths.push(
                    result
                        .as_ref()
                        .expect("placed transfer has an outcome")
                        .fast_path,
                );
            }

            if journal_enabled {
                let step_failure = {
                    let state = crate::lock_util::recover(&progress);
                    state
                        .failures
                        .iter()
                        .skip(failures_before)
                        .max_by_key(|failure| {
                            if failure.class == FailureClass::IntegrityUncertain {
                                1
                            } else {
                                0
                            }
                        })
                        .cloned()
                };
                if let Some(failure) = step_failure {
                    if let Err(error) = crate::operation_journal::mark_failed(
                        &spec.operation_id,
                        &work_item.key,
                        failure,
                    ) {
                        record_journal_error(&progress, &entry.name, Some(landing.clone()), error);
                    }
                } else if placed
                    && let Err(error) = crate::operation_journal::mark_completed(
                        &spec.operation_id,
                        &work_item.key,
                        &landing,
                        result
                            .as_ref()
                            .expect("placed transfer has an outcome")
                            .fast_path,
                    )
                {
                    record_journal_error(&progress, &entry.name, Some(landing.clone()), error);
                }
            }

            {
                let mut s = crate::lock_util::recover(&progress);
                s.files_done += 1;
                s.record_sample();
            }
            notify();
        }

        run_post_success(spec.post_success.as_ref(), &progress);
        finish_progress(&progress);
        if journal_enabled {
            let status = {
                let state = crate::lock_util::recover(&progress);
                if state
                    .failures
                    .iter()
                    .any(|failure| failure.class == FailureClass::IntegrityUncertain)
                {
                    crate::operation_journal::OperationStatus::NeedsReview
                } else if state.cancelled || state.stopped {
                    crate::operation_journal::OperationStatus::Stopped
                } else if !state.errors.is_empty() {
                    crate::operation_journal::OperationStatus::Failed
                } else {
                    crate::operation_journal::OperationStatus::Completed
                }
            };
            if let Err(error) = crate::operation_journal::finish(&spec.operation_id, status) {
                record_journal_error(&progress, "Operation", Some(spec.target.clone()), error);
            }
        }
        notify();
    });
}

/// Run a transfer-owned follow-up only after every entry completed without an
/// error or cancellation. Failure is appended to the normal transfer error
/// surface, so the progress dialog remains open instead of hiding cleanup loss.
fn run_post_success(action: Option<&PostTransferAction>, progress: &TransferState) {
    let Some(action) = action else {
        return;
    };
    let ready = {
        let state = crate::lock_util::recover(progress);
        state.files_done == state.files_total
            && state.errors.is_empty()
            && !state.cancelled
            && !state.stop_requested
            && !state.stopped
    };
    if !ready {
        return;
    }

    let PostTransferAction::RemoveEmptyDir(path) = action;
    let result = std::fs::remove_dir(path).or_else(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(e)
        }
    });
    if let Err(error) = result {
        record_failure(
            progress,
            &path.display().to_string(),
            ClassifiedFailure::io(Some(path.clone()), "Post-transfer cleanup failed", &error),
        );
    }
}

/// Publish the terminal state. A cancel request that arrived after the final
/// entry was committed is too late to cancel anything; treating that as a
/// cancelled Move would discard its valid undo action.
fn finish_progress(progress: &TransferState) {
    let mut s = crate::lock_util::recover(progress);
    if s.files_done == s.files_total {
        s.cancelled = false;
        s.stop_requested = false;
        s.stopped = false;
    } else if s.stop_requested {
        s.stopped = true;
    }
    s.finished = true;
    s.record_sample();
}

/// Move one entry to `dst` with a single atomic, no-clobber rename (the
/// same-volume move fast path). `dst` is always a path that should not exist
/// yet (an absent destination, a fresh "copy" name, or a staging sibling), so
/// the rename mirrors the copy path's `EXCL` no-clobber guarantee. The entry's
/// full size is reported as copied, since a rename transfers it whole; the size
/// is read before the move while the source still exists.
fn rename_entry(
    src: &Path,
    dst: &Path,
    entry: &FileEntry,
    size: u64,
    progress: &TransferState,
    base_bytes: u64,
) -> std::io::Result<u64> {
    {
        let mut s = crate::lock_util::recover(progress);
        s.current_file = entry.name.clone();
        s.current_file_size = size;
        s.current_file_copied = 0;
    }
    crate::native_copy::rename_noreplace(src, dst)?;
    let mut s = crate::lock_util::recover(progress);
    s.current_file_copied = size;
    s.copied_bytes = base_bytes + size;
    s.maybe_sample();
    Ok(size)
}

/// A hidden sibling of `dest` that does not exist yet, used to stage an
/// overwrite copy before swapping it into place (same directory = same
/// volume, so the final rename is atomic and clone-friendly).
fn staging_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "item".to_string());
    let parent = dest.parent().unwrap_or(Path::new("."));
    fs_util::first_available(|i| parent.join(format!(".{}.cmdr-tmp.{}", name, i)))
}

/// Remove a file or directory tree, treating "not found" as success.
fn cleanup_path(path: &Path) -> std::io::Result<()> {
    let result = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match result {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Undo our own placement at `staged` after a failed transfer of one entry.
///
/// For a COPY, `staged` is a disposable duplicate (the original still sits at
/// `source`), so it is simply removed. For the same-volume rename fast path,
/// `staged` IS the source's only copy (the source was already moved into it),
/// so it must be moved back to `source` rather than deleted, or the user's data
/// would be lost. If the restore cannot complete, the data is left in place
/// (never deleted) and its location is returned so it can be recovered.
fn undo_placement(staged: &Path, source: &Path, was_renamed: bool) -> Option<String> {
    if !was_renamed {
        let _ = cleanup_path(staged);
        return None;
    }
    if std::fs::rename(staged, source).is_ok() {
        return None;
    }
    if staged.symlink_metadata().is_ok() {
        Some(format!("data preserved at {}", staged.display()))
    } else {
        None
    }
}

/// Replace `dest` with the freshly-staged `staged`: move the existing `dest`
/// to a backup, rename `staged` into place, then drop the backup. Restores
/// the original on failure, so an interrupted overwrite never loses data.
fn swap_into_place(staged: &Path, dest: &Path) -> std::io::Result<()> {
    if !fs_util::path_is_taken(dest) {
        return crate::native_copy::rename_noreplace(staged, dest);
    }
    let backup = staging_path(dest);
    std::fs::rename(dest, &backup)?;
    match std::fs::rename(staged, dest) {
        Ok(()) => {
            let _ = cleanup_path(&backup);
            Ok(())
        }
        Err(e) => {
            // Put the original back. If even that fails, the original now
            // lives only at the hidden backup path; name it in the error so
            // it can be recovered rather than vanishing silently.
            if std::fs::rename(&backup, dest).is_err() {
                return Err(std::io::Error::other(format!(
                    "{e}; original preserved at {}",
                    backup.display()
                )));
            }
            Err(e)
        }
    }
}

/// Copy a single file with progress reporting (buffered strategy).
/// Removes the partial destination file on any failure.
/// Returns the file size on success.
fn copy_file_buffered_with_limiter(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
    resume: Option<&ResumeCheckpoint>,
    journal: Option<JournalStep<'_>>,
    preserve_sparse: bool,
) -> std::io::Result<u64> {
    if preserve_sparse && resume.is_none() && is_sparse_file(src) {
        return copy_file_sparse(src, dst, state, limiter);
    }
    let result = copy_file_buffered_inner(src, dst, state, limiter, resume, journal);
    if let Err(ref e) = result {
        // Clean up our own partial write, but never delete a destination that
        // was already there (AlreadyExists means create_new refused to clobber).
        if journal.is_none() && e.kind() != std::io::ErrorKind::AlreadyExists {
            let _ = std::fs::remove_file(dst);
        }
    }
    result
}

#[cfg(unix)]
fn is_sparse_file(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    path.metadata().is_ok_and(|metadata| {
        metadata.is_file()
            && metadata.len() > 0
            && metadata.blocks().saturating_mul(512) < metadata.len()
    })
}

#[cfg(not(unix))]
fn is_sparse_file(_path: &Path) -> bool {
    false
}

#[cfg(unix)]
fn copy_file_sparse(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
) -> std::io::Result<u64> {
    use std::os::fd::AsRawFd;

    let mut reader = std::fs::File::open(src)?;
    let file_size = reader.metadata()?.len();
    let mut writer = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)?;
    writer.set_len(file_size)?;
    let base = {
        let mut progress = crate::lock_util::recover(state);
        progress.current_file = src
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        progress.current_file_size = file_size;
        progress.current_file_copied = 0;
        progress.copied_bytes
    };
    let mut cursor = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUF_SIZE];
    while cursor < file_size {
        if crate::lock_util::recover(state).cancelled {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }
        let data = unsafe {
            libc::lseek(
                reader.as_raw_fd(),
                cursor.try_into().map_err(|_| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "file offset overflow")
                })?,
                libc::SEEK_DATA,
            )
        };
        if data < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENXIO) {
                break;
            }
            return Err(error);
        }
        let hole = unsafe { libc::lseek(reader.as_raw_fd(), data, libc::SEEK_HOLE) };
        if hole < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let data = data as u64;
        let hole = (hole as u64).min(file_size);
        reader.seek(SeekFrom::Start(data))?;
        writer.seek(SeekFrom::Start(data))?;
        let mut extent_offset = data;
        while extent_offset < hole {
            let wanted = (hole - extent_offset).min(buffer.len() as u64) as usize;
            let read = reader.read(&mut buffer[..wanted])?;
            if read == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "sparse extent ended early",
                ));
            }
            writer.write_all(&buffer[..read])?;
            limiter.consume(read, || crate::lock_util::recover(state).cancelled)?;
            extent_offset = extent_offset.saturating_add(read as u64);
            let mut progress = crate::lock_util::recover(state);
            progress.current_file_copied = extent_offset;
            progress.copied_bytes = base.saturating_add(extent_offset);
            progress.maybe_sample();
        }
        cursor = hole;
    }
    writer.sync_data()?;
    if let Ok(metadata) = src.metadata() {
        std::fs::set_permissions(dst, metadata.permissions())?;
    }
    let mut progress = crate::lock_util::recover(state);
    progress.current_file_copied = file_size;
    progress.copied_bytes = base.saturating_add(file_size);
    progress
        .fast_paths
        .push(crate::transfer_tuning::FastPath::Sparse);
    Ok(file_size)
}

#[cfg(not(unix))]
fn copy_file_sparse(
    _src: &Path,
    _dst: &Path,
    _state: &TransferState,
    _limiter: &mut crate::transfer_tuning::BandwidthLimiter,
) -> std::io::Result<u64> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "sparse copy is unavailable",
    ))
}

fn copy_file_buffered_inner(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
    resume: Option<&ResumeCheckpoint>,
    journal: Option<JournalStep<'_>>,
) -> std::io::Result<u64> {
    let file_size = src.metadata().map(|m| m.len()).unwrap_or(0);
    let offset = resume.map_or(0, |checkpoint| checkpoint.offset);
    if offset > file_size
        || resume.is_some_and(|checkpoint| {
            checkpoint.staging != dst || checkpoint.layout != CheckpointLayout::Prefix
        })
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid transfer checkpoint",
        ));
    }

    // Init per-file progress
    {
        let mut s = crate::lock_util::recover(state);
        s.current_file = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        s.current_file_size = file_size;
        s.current_file_copied = offset;
        s.copied_bytes = s.copied_bytes.saturating_add(offset);
    }

    let mut source = std::fs::File::open(src)?;
    source.seek(SeekFrom::Start(offset))?;
    let mut reader = std::io::BufReader::with_capacity(COPY_BUF_SIZE, source);
    // create_new (O_EXCL): the caller always targets a path that should not
    // exist yet, so refuse to truncate a file that races into being.
    let mut options = std::fs::OpenOptions::new();
    options.write(true);
    let mut dst_file = if resume.is_some() {
        options.open(dst)?
    } else {
        options.create_new(true).open(dst)?
    };
    let staging_len = dst_file.metadata()?.len();
    if staging_len < offset {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "checkpoint staging is shorter than its verified offset",
        ));
    }
    if resume.is_some() && staging_len > offset {
        dst_file.set_len(offset)?;
    }
    dst_file.seek(SeekFrom::Start(offset))?;
    let mut writer = std::io::BufWriter::with_capacity(COPY_BUF_SIZE, dst_file);

    let mut buf = vec![0u8; COPY_BUF_SIZE];
    let mut copied = offset;
    let mut next_checkpoint = offset.saturating_add(CHECKPOINT_INTERVAL);

    loop {
        {
            let s = crate::lock_util::recover(state);
            if s.cancelled {
                drop(s);
                persist_checkpoint(src, dst, copied, &mut writer, journal)?;
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
        }

        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        copied = copied.saturating_add(n as u64);
        if let Err(error) = limiter.consume(n, || crate::lock_util::recover(state).cancelled) {
            persist_checkpoint(src, dst, copied, &mut writer, journal)?;
            return Err(error);
        }

        {
            let mut s = crate::lock_util::recover(state);
            s.copied_bytes += n as u64;
            s.current_file_copied += n as u64;
            s.maybe_sample();
        }
        if copied >= next_checkpoint {
            persist_checkpoint(src, dst, copied, &mut writer, journal)?;
            next_checkpoint = copied.saturating_add(CHECKPOINT_INTERVAL);
        }
    }
    persist_checkpoint(src, dst, copied, &mut writer, journal)?;
    // Apply the source's permissions only after the contents are complete, so
    // a concurrent reader never sees a partial file already wearing its final
    // (possibly executable) mode.
    if let Ok(meta) = src.metadata() {
        let _ = std::fs::set_permissions(dst, meta.permissions());
    }
    Ok(file_size)
}

fn persist_checkpoint(
    src: &Path,
    dst: &Path,
    offset: u64,
    writer: &mut std::io::BufWriter<std::fs::File>,
    journal: Option<JournalStep<'_>>,
) -> std::io::Result<()> {
    writer.flush()?;
    writer.get_ref().sync_data()?;
    let Some(journal) = journal.filter(|journal| journal.enabled) else {
        return Ok(());
    };
    let source = PathIdentity::observe_deep(src)?;
    let partial = PathIdentity::observe_deep(dst)?;
    let checkpoint = ResumeCheckpoint {
        staging: dst.to_path_buf(),
        offset,
        source,
        partial,
        layout: CheckpointLayout::Prefix,
    };
    crate::operation_journal::mark_checkpoint(journal.operation_id, journal.key, checkpoint)
        .map_err(std::io::Error::other)
}

fn copy_dir_buffered_parallel(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    workers: usize,
    preserve_sparse: bool,
) -> std::io::Result<u64> {
    let mut files = Vec::new();
    let mut permissions = Vec::new();
    prepare_buffered_tree(src, dst, &mut files, &mut permissions)?;
    let workers = workers.max(1).min(files.len().max(1));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|index| format!("commander-copy-{index}"))
        .build()
        .map_err(std::io::Error::other)?;
    {
        let mut progress = crate::lock_util::recover(state);
        progress.active_workers = workers;
        progress.peak_workers = progress.peak_workers.max(workers);
    }
    let results = pool.install(|| {
        files
            .par_iter()
            .map(|(source, destination)| {
                let mut limiter = crate::transfer_tuning::BandwidthLimiter::new(Default::default());
                copy_file_buffered_with_limiter(
                    source,
                    destination,
                    state,
                    &mut limiter,
                    None,
                    None,
                    preserve_sparse,
                )
            })
            .collect::<Vec<_>>()
    });
    crate::lock_util::recover(state).active_workers = 1;
    let copied = results
        .into_iter()
        .try_fold(0_u64, |total, result| result.map(|bytes| total + bytes))?;
    for (path, mode) in permissions.into_iter().rev() {
        std::fs::set_permissions(path, mode)?;
    }
    Ok(copied)
}

fn prepare_buffered_tree(
    src: &Path,
    dst: &Path,
    files: &mut Vec<(PathBuf, PathBuf)>,
    permissions: &mut Vec<(PathBuf, std::fs::Permissions)>,
) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(src)?;
    std::fs::create_dir_all(dst)?;
    permissions.push((dst.to_path_buf(), metadata.permissions()));
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let source = entry.path();
        let destination = dst.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            copy_symlink(&source, &destination)?;
        } else if file_type.is_dir() {
            prepare_buffered_tree(&source, &destination, files, permissions)?;
        } else {
            files.push((source, destination));
        }
    }
    Ok(())
}

/// Recursively copy a directory with progress (buffered strategy).
/// Returns total bytes copied. Symlinks are recreated as links rather than
/// followed, so a link pointing back into the tree cannot cause infinite
/// recursion.
fn copy_dir_buffered_with_limiter(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
    preserve_sparse: bool,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
) -> std::io::Result<u64> {
    let mut ancestors = std::collections::HashSet::new();
    copy_dir_buffered_inner(
        src,
        dst,
        state,
        limiter,
        preserve_sparse,
        symlink_policy,
        &mut ancestors,
    )
}

fn copy_dir_buffered_inner(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
    preserve_sparse: bool,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    ancestors: &mut std::collections::HashSet<PathBuf>,
) -> std::io::Result<u64> {
    let canonical = std::fs::canonicalize(src)?;
    if !ancestors.insert(canonical.clone()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("symlink cycle detected at {}", src.display()),
        ));
    }
    std::fs::create_dir_all(dst)?;
    let mut copied = 0u64;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        // file_type() does NOT follow symlinks (unlike Path::is_dir).
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            match symlink_policy {
                crate::filesystem_policy::SymlinkPolicy::Preserve => {
                    copy_symlink(&src_path, &dst_path)?;
                }
                crate::filesystem_policy::SymlinkPolicy::Skip => {}
                crate::filesystem_policy::SymlinkPolicy::Follow => {
                    let followed = std::fs::canonicalize(&src_path)?;
                    let followed_metadata = std::fs::symlink_metadata(&followed)?;
                    if followed_metadata.is_dir() {
                        copied += copy_dir_buffered_inner(
                            &followed,
                            &dst_path,
                            state,
                            limiter,
                            preserve_sparse,
                            symlink_policy,
                            ancestors,
                        )?;
                    } else {
                        copied += copy_file_buffered_with_limiter(
                            &followed,
                            &dst_path,
                            state,
                            limiter,
                            None,
                            None,
                            preserve_sparse,
                        )?;
                    }
                }
            }
        } else if ft.is_dir() {
            copied += copy_dir_buffered_inner(
                &src_path,
                &dst_path,
                state,
                limiter,
                preserve_sparse,
                symlink_policy,
                ancestors,
            )?;
        } else {
            copied += copy_file_buffered_with_limiter(
                &src_path,
                &dst_path,
                state,
                limiter,
                None,
                None,
                preserve_sparse,
            )?;
        }
    }
    ancestors.remove(&canonical);
    Ok(copied)
}

/// Recreate a symlink at `dst` pointing at the same target as `src`.
#[cfg(unix)]
fn copy_symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    let target = std::fs::read_link(src)?;
    let _ = std::fs::remove_file(dst);
    std::os::unix::fs::symlink(target, dst)
}

#[cfg(not(unix))]
fn copy_symlink(_src: &Path, _dst: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn entry_for(path: &Path) -> FileEntry {
        let meta = std::fs::metadata(path).unwrap();
        FileEntry::from_meta(path.to_path_buf(), &meta).unwrap()
    }

    /// Run a transfer to completion and return the final progress state.
    fn run(spec: TransferSpec) -> TransferProgress {
        let total = total_bytes(&spec.entries);
        let progress: TransferState =
            Arc::new(Mutex::new(TransferProgress::new(total, spec.entries.len())));
        spawn_transfer(spec, progress.clone(), || {});

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            {
                let s = progress.lock().unwrap();
                if s.finished {
                    return s.clone();
                }
            }
            assert!(std::time::Instant::now() < deadline, "transfer timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn finish_distinguishes_a_late_cancel_from_a_partial_cancel() {
        let completed = Arc::new(Mutex::new(TransferProgress::new(1, 1)));
        {
            let mut state = completed.lock().unwrap();
            state.files_done = 1;
            state.cancelled = true;
        }
        finish_progress(&completed);
        let state = completed.lock().unwrap();
        assert!(state.finished);
        assert!(!state.cancelled);
        drop(state);

        let partial = Arc::new(Mutex::new(TransferProgress::new(2, 2)));
        {
            let mut state = partial.lock().unwrap();
            state.files_done = 1;
            state.cancelled = true;
        }
        finish_progress(&partial);
        let state = partial.lock().unwrap();
        assert!(state.finished);
        assert!(state.cancelled);
    }

    #[test]
    fn successful_post_action_removes_the_empty_source_folder() {
        let root = TempDir::new();
        let folder = root.dir("Gathered");
        let file = root.file("Gathered/a.txt", "a");
        let state = run(TransferSpec {
            operation_id: OperationId::new(),
            group_id: None,
            kind: TransferKind::Move,
            entries: vec![entry_for(&file)],
            expectations: capture_expectations(&[entry_for(&file)], root.path()),
            target: root.path().to_path_buf(),
            policy: OverwritePolicy::Ask,
            method: CopyMethod::Native,
            durability: DurabilityProfile::Fast,
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            post_success: Some(PostTransferAction::RemoveEmptyDir(folder.clone())),
            rollback_cleanup: None,
            before_commit: None,
            journal_enabled: false,
        });

        assert!(state.errors.is_empty(), "{:?}", state.errors);
        assert!(root.path().join("a.txt").is_file());
        assert!(!folder.exists());
    }

    #[test]
    fn recovery_finalization_runs_post_action_without_manifest_entries() {
        let root = TempDir::new();
        let folder = root.dir("Gathered");
        let state = run(TransferSpec {
            operation_id: OperationId::new(),
            group_id: None,
            kind: TransferKind::Move,
            entries: Vec::new(),
            expectations: Vec::new(),
            target: root.path().to_path_buf(),
            policy: OverwritePolicy::Ask,
            method: CopyMethod::Native,
            durability: DurabilityProfile::Fast,
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            post_success: Some(PostTransferAction::RemoveEmptyDir(folder.clone())),
            rollback_cleanup: None,
            before_commit: None,
            journal_enabled: false,
        });

        assert!(state.errors.is_empty(), "{:?}", state.errors);
        assert!(!folder.exists());
    }

    #[test]
    fn post_action_preserves_a_folder_that_gained_another_file() {
        let root = TempDir::new();
        let folder = root.dir("Gathered");
        let file = root.file("Gathered/a.txt", "a");
        root.file("Gathered/foreign.txt", "foreign");
        let state = run(TransferSpec {
            operation_id: OperationId::new(),
            group_id: None,
            kind: TransferKind::Move,
            entries: vec![entry_for(&file)],
            expectations: capture_expectations(&[entry_for(&file)], root.path()),
            target: root.path().to_path_buf(),
            policy: OverwritePolicy::Ask,
            method: CopyMethod::Native,
            durability: DurabilityProfile::Fast,
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            post_success: Some(PostTransferAction::RemoveEmptyDir(folder.clone())),
            rollback_cleanup: None,
            before_commit: None,
            journal_enabled: false,
        });

        assert!(root.path().join("a.txt").is_file());
        assert!(folder.join("foreign.txt").is_file());
        assert_eq!(state.errors.len(), 1);
        assert!(state.errors[0].contains("Post-transfer cleanup failed"));
    }

    // The engine no longer consults a conflict list (it checks the
    // destination live), but the tests keep passing one to document intent;
    // it is ignored here.
    fn spec(
        kind: TransferKind,
        method: CopyMethod,
        entries: Vec<FileEntry>,
        target: &Path,
        _conflicts: Vec<String>,
        policy: OverwritePolicy,
    ) -> TransferSpec {
        TransferSpec {
            operation_id: OperationId::new(),
            group_id: None,
            kind,
            expectations: capture_expectations(&entries, target),
            entries,
            target: target.to_path_buf(),
            policy,
            method,
            durability: DurabilityProfile::Fast,
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            post_success: None,
            rollback_cleanup: None,
            before_commit: None,
            journal_enabled: false,
        }
    }

    #[test]
    fn buffered_copy_file_copies_bytes_and_reports_progress() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "hello world");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "hello world"
        );
        assert!(file.exists(), "copy keeps the source");
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert_eq!(s.files_done, 1);
        assert_eq!(s.copied_bytes, 11);
    }

    #[test]
    fn large_slow_link_files_force_the_checkpointed_buffered_path() {
        let temp = TempDir::new();
        let mut profile = crate::volume_profile::profile(temp.path());
        profile.backend = crate::volume_profile::BackendKind::Remote;
        profile.capabilities.resumable = true;

        assert!(requires_resumable_buffer(
            &profile,
            crate::delta_copy::DELTA_MIN_BYTES
        ));
        assert!(!requires_resumable_buffer(
            &profile,
            crate::delta_copy::DELTA_MIN_BYTES - 1
        ));
        profile.capabilities.resumable = false;
        assert!(!requires_resumable_buffer(
            &profile,
            crate::delta_copy::DELTA_MIN_BYTES
        ));
    }

    #[test]
    fn buffered_copy_resumes_from_a_verified_offset() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let source = src.path().join("large.bin");
        let staging = dst.path().join(".large.bin.cmdr-tmp.0");
        let bytes = (0..(3 * 1024 * 1024 + 17))
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        std::fs::write(&source, &bytes).unwrap();
        let offset = 1024 * 1024 + 7;
        std::fs::write(&staging, &bytes[..offset]).unwrap();
        let checkpoint = ResumeCheckpoint {
            staging: staging.clone(),
            offset: offset as u64,
            source: PathIdentity::observe_deep(&source).unwrap(),
            partial: PathIdentity::observe_deep(&staging).unwrap(),
            layout: CheckpointLayout::Prefix,
        };
        std::fs::OpenOptions::new()
            .append(true)
            .open(&staging)
            .unwrap()
            .write_all(b"uncheckpointed crash tail")
            .unwrap();
        let state = Arc::new(Mutex::new(TransferProgress::new(bytes.len() as u64, 1)));
        let mut limiter = crate::transfer_tuning::BandwidthLimiter::new(Default::default());

        let copied = copy_file_buffered_with_limiter(
            &source,
            &staging,
            &state,
            &mut limiter,
            Some(&checkpoint),
            None,
            false,
        )
        .unwrap();

        assert_eq!(copied, bytes.len() as u64);
        assert_eq!(std::fs::read(staging).unwrap(), bytes);
        assert_eq!(state.lock().unwrap().copied_bytes, bytes.len() as u64);
    }

    #[cfg(unix)]
    #[test]
    fn sparse_copy_preserves_holes_and_content() {
        use std::os::unix::fs::MetadataExt;

        let (src, dst) = (TempDir::new(), TempDir::new());
        let source = src.path().join("sparse.bin");
        let destination = dst.path().join("sparse.bin");
        let mut file = std::fs::File::create(&source).unwrap();
        file.set_len(32 * 1024 * 1024).unwrap();
        file.seek(SeekFrom::Start(1024 * 1024)).unwrap();
        file.write_all(b"first extent").unwrap();
        file.seek(SeekFrom::End(-16)).unwrap();
        file.write_all(b"last extent").unwrap();
        file.sync_all().unwrap();
        assert!(is_sparse_file(&source));
        let state = Arc::new(Mutex::new(TransferProgress::new(32 * 1024 * 1024, 1)));
        let mut limiter = crate::transfer_tuning::BandwidthLimiter::new(Default::default());

        copy_file_sparse(&source, &destination, &state, &mut limiter).unwrap();

        assert!(crate::version_store::paths_equal(&source, &destination));
        let source_blocks = source.metadata().unwrap().blocks();
        let destination_blocks = destination.metadata().unwrap().blocks();
        assert!(destination_blocks <= source_blocks.saturating_add(16));
        assert_eq!(destination.metadata().unwrap().len(), 32 * 1024 * 1024);
    }

    #[test]
    fn native_copy_file_works() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "native");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Native,
            vec![entry_for(&file)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "native"
        );
        assert!(s.errors.is_empty());
    }

    #[test]
    fn buffered_copy_dir_recurses() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("folder");
        src.file("folder/one.txt", "1");
        src.file("folder/nested/two.txt", "22");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&dir)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        assert_eq!(
            std::fs::read_to_string(dst.path().join("folder/one.txt")).unwrap(),
            "1"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("folder/nested/two.txt")).unwrap(),
            "22"
        );
        assert!(s.errors.is_empty());
    }

    #[test]
    fn adaptive_directory_copy_uses_the_requested_worker_cap() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let source = src.dir("folder");
        for index in 0..8 {
            src.file(&format!("folder/{index}.txt"), &format!("value {index}"));
        }
        let destination = dst.path().join("folder");
        let progress = Arc::new(Mutex::new(TransferProgress::new(0, 1)));

        copy_dir_buffered_parallel(&source, &destination, &progress, 2, true).unwrap();

        assert_eq!(crate::lock_util::recover(&progress).peak_workers, 2);
        assert_eq!(
            std::fs::read_to_string(destination.join("7.txt")).unwrap(),
            "value 7"
        );
    }

    #[test]
    fn move_deletes_source_only_on_clean_copy() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "move me");

        run(spec(
            TransferKind::Move,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        assert!(dst.path().join("a.txt").exists());
        assert!(!file.exists(), "clean move removes the source");
    }

    #[test]
    fn move_keeps_source_when_placement_fails() {
        use std::os::unix::fs::PermissionsExt;

        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("folder");
        let inner = src.file("folder/data.txt", "payload");
        // Make the destination unwritable so neither the rename fast path nor
        // the copy path can land the entry. The source must survive untouched.
        std::fs::set_permissions(dst.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Buffered,
            vec![entry_for(&dir)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        // Restore permissions so TempDir cleanup works everywhere.
        let _ = std::fs::set_permissions(dst.path(), std::fs::Permissions::from_mode(0o755));

        assert!(!s.errors.is_empty(), "the failure must be reported");
        assert!(dir.exists(), "source must survive a failed move");
        assert!(inner.exists());
    }

    #[test]
    fn same_volume_move_relocates_whole_tree_via_rename() {
        // src and dst temp dirs share a volume, so a Move takes the rename
        // fast path: the source is relinked, not copied byte-by-byte.
        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("folder");
        src.file("folder/a.txt", "one");
        src.file("folder/sub/b.txt", "two");

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Native,
            vec![entry_for(&dir)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert!(!dir.exists(), "source is gone after a move");
        assert_eq!(
            std::fs::read_to_string(dst.path().join("folder/a.txt")).unwrap(),
            "one"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("folder/sub/b.txt")).unwrap(),
            "two"
        );
        assert_eq!(s.files_done, 1);
        assert_eq!(s.copied_bytes, s.total_bytes);
    }

    #[test]
    fn same_volume_move_needs_no_read_access_to_contents() {
        use std::os::unix::fs::PermissionsExt;
        // A rename relocates an unreadable file whole; the old copy-then-delete
        // path would have failed trying to read it. This documents that a
        // same-volume move is a true rename, not a copy.
        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("folder");
        let secret = src.file("folder/secret.txt", "unreadable");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Native,
            vec![entry_for(&dir)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        let moved = dst.path().join("folder/secret.txt");
        let _ = std::fs::set_permissions(&moved, std::fs::Permissions::from_mode(0o644));

        assert!(
            s.errors.is_empty(),
            "rename needs no read access to contents"
        );
        assert!(!dir.exists());
        assert!(moved.exists());
    }

    #[test]
    fn same_volume_move_overwrite_replaces_destination() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "new contents");
        dst.file("a.txt", "old contents");

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Native,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(s.errors.is_empty());
        assert!(!file.exists(), "source is gone after a move");
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "new contents"
        );
    }

    #[test]
    fn undo_placement_restores_a_renamed_source_instead_of_deleting_it() {
        // The same-volume move fast path moves the source INTO `staged`; on a
        // later failure `undo_placement` must put it back, not delete the only
        // copy (the data-loss bug the swap-failure path used to have).
        let tmp = TempDir::new();
        let staged = tmp.file("staged.tmp", "the only copy");
        let source = tmp.path().join("source.txt"); // emptied by the rename

        let msg = undo_placement(&staged, &source, true);
        assert!(msg.is_none(), "restore should succeed");
        assert!(!staged.exists(), "staged moved back");
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "the only copy");
    }

    #[test]
    fn undo_placement_deletes_a_copied_duplicate() {
        let tmp = TempDir::new();
        let staged = tmp.file("dup.tmp", "disposable");
        let source = tmp.file("source.txt", "original stays");

        let msg = undo_placement(&staged, &source, false);
        assert!(msg.is_none());
        assert!(!staged.exists(), "disposable copy removed");
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            "original stays",
            "the real source is never touched on a copy"
        );
    }

    #[test]
    fn undo_placement_preserves_data_when_a_rename_restore_cannot_complete() {
        // If the source location is unreachable (its parent is gone), the data
        // must be left at `staged`, never deleted, and its path surfaced.
        let tmp = TempDir::new();
        let staged = tmp.file("staged.tmp", "irreplaceable");
        let source = tmp.path().join("missing_dir").join("source.txt");

        let msg = undo_placement(&staged, &source, true);
        assert!(msg.is_some(), "must report where the data was kept");
        assert!(msg.unwrap().contains("staged.tmp"));
        assert!(staged.exists(), "data left in place, not destroyed");
    }

    #[test]
    fn skip_all_leaves_existing_destination_untouched() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "new contents");
        dst.file("a.txt", "old contents");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::SkipAll,
        ));

        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "old contents"
        );
        assert_eq!(s.files_done, 1, "skipped entries still count as processed");
    }

    #[test]
    fn overwrite_all_replaces_existing_file() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "new contents");
        dst.file("a.txt", "old contents");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "new contents"
        );
        assert!(s.errors.is_empty());
    }

    #[test]
    fn verified_overwrite_checks_the_staged_copy_before_replacement() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "new verified contents");
        dst.file("a.txt", "old contents");
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::OverwriteAll,
        );
        request.durability = DurabilityProfile::Verified;

        let state = run(request);

        assert!(state.failures.is_empty(), "{:?}", state.failures);
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "new verified contents"
        );
    }

    #[test]
    fn source_modified_between_copy_and_commit_is_requeued() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "first version");
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        let fired = Arc::new(AtomicBool::new(false));
        let fired_for_hook = Arc::clone(&fired);
        request.before_commit = Some(Arc::new(move |source, _| {
            if !fired_for_hook.swap(true, Ordering::SeqCst) {
                std::fs::write(source, "second version after copy").unwrap();
            }
        }));

        let state = run(request);

        assert!(state.failures.is_empty(), "{:?}", state.failures);
        assert_eq!(state.requeued_files, 1);
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "second version after copy"
        );
    }

    #[test]
    fn destination_modified_between_review_and_commit_is_preserved() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "incoming");
        dst.file("a.txt", "reviewed destination");
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::OverwriteAll,
        );
        let fired = Arc::new(AtomicBool::new(false));
        let fired_for_hook = Arc::clone(&fired);
        request.before_commit = Some(Arc::new(move |_, landing| {
            if !fired_for_hook.swap(true, Ordering::SeqCst) {
                std::fs::write(landing, "changed by another process").unwrap();
            }
        }));

        let state = run(request);

        assert_eq!(state.failures.len(), 1, "{:?}", state.failures);
        assert_eq!(state.failures[0].class, FailureClass::UserDecision);
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "changed by another process"
        );
        assert!(
            std::fs::read_dir(dst.path()).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("cmdr-tmp")),
            "staged copy was cleaned"
        );
    }

    #[test]
    fn graceful_stop_finishes_current_entry_and_accepts_no_next_entry() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let first = src.file("a.txt", "first");
        let second = src.file("b.txt", "second");
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&first), entry_for(&second)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        let progress: TransferState = Arc::new(Mutex::new(TransferProgress::new(0, 2)));
        let progress_for_hook = Arc::clone(&progress);
        request.before_commit = Some(Arc::new(move |_, _| {
            crate::lock_util::recover(&progress_for_hook).stop_requested = true;
        }));

        spawn_transfer(request, Arc::clone(&progress), || {});
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if crate::lock_util::recover(&progress).finished {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "transfer timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let state = crate::lock_util::recover(&progress);

        assert!(state.stopped);
        assert!(!state.cancelled);
        assert_eq!(state.files_done, 1);
        assert!(dst.path().join("a.txt").is_file());
        assert!(!dst.path().join("b.txt").exists());
    }

    #[test]
    fn failed_buffered_copy_removes_partial_destination() {
        use std::os::unix::fs::PermissionsExt;

        let (src, dst) = (TempDir::new(), TempDir::new());
        let secret = src.file("secret.txt", "data");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&secret)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        let _ = std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644));

        assert!(!s.errors.is_empty());
        assert!(
            !dst.path().join("secret.txt").exists(),
            "partial destination must be cleaned up"
        );
    }

    #[test]
    fn total_bytes_sums_files_and_dirs() {
        let tmp = TempDir::new();
        let f = tmp.file("a.bin", "12345");
        let d = tmp.dir("folder");
        tmp.file("folder/b.bin", "123");

        let entries = vec![entry_for(&f), entry_for(&d)];
        assert_eq!(total_bytes(&entries), 8);
    }

    #[test]
    fn speed_is_averaged_over_recent_samples() {
        let mut p = TransferProgress::new(1000, 1);
        // 100 bytes/sec: sample at t=0 (0 bytes) and t=4 (400 bytes).
        p.speed_samples = vec![(0.0, 0.0), (4.0, 400.0)];
        p.copied_bytes = 600;
        // At t=6 the 2-second window looks back to the t=4 sample:
        // (600 - 400) / (6 - 4) = 100 B/s.
        assert!((p.speed_bps_at(6.0) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn speed_needs_at_least_two_samples() {
        let p = TransferProgress::new(1000, 1);
        assert_eq!(p.speed_bps_at(5.0), 0.0);
    }

    // ── Data-loss regressions (from the safety audit) ──────────────────

    #[test]
    fn moving_dir_into_its_own_parent_is_rejected() {
        // Both panels on the same dir: target == source's parent, so
        // dest == source. The source must survive untouched.
        let work = TempDir::new();
        let data = work.dir("data");
        work.file("data/important.txt", "keep me");

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Native,
            vec![entry_for(&data)],
            work.path(),
            vec!["data".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(!s.errors.is_empty(), "self-referential move must error");
        assert!(data.exists(), "source directory must survive");
        assert_eq!(
            std::fs::read_to_string(data.join("important.txt")).unwrap(),
            "keep me"
        );
    }

    #[test]
    fn moving_file_onto_itself_keeps_it_buffered() {
        // The buffered path used to truncate-on-create then delete the source.
        let work = TempDir::new();
        let f = work.file("a.txt", "content");

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Buffered,
            vec![entry_for(&f)],
            work.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(f.exists(), "file must not be deleted");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "content");
        assert!(!s.errors.is_empty());
    }

    #[test]
    fn failed_overwrite_preserves_existing_destination() {
        use std::os::unix::fs::PermissionsExt;

        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("box");
        src.file("box/ok.txt", "new");
        let secret = src.file("box/secret.txt", "x");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();

        dst.dir("box");
        dst.file("box/existing.txt", "OLD");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&dir)],
            dst.path(),
            vec!["box".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        let _ = std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644));

        assert!(!s.errors.is_empty());
        // The pre-existing destination must NOT have been destroyed before
        // the (failing) copy.
        assert_eq!(
            std::fs::read_to_string(dst.path().join("box/existing.txt")).unwrap(),
            "OLD"
        );
    }

    #[test]
    fn successful_overwrite_replaces_directory() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("box");
        src.file("box/new.txt", "NEW");
        dst.dir("box");
        dst.file("box/old.txt", "OLD");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&dir)],
            dst.path(),
            vec!["box".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(s.errors.is_empty());
        assert_eq!(
            std::fs::read_to_string(dst.path().join("box/new.txt")).unwrap(),
            "NEW"
        );
        assert!(
            !dst.path().join("box/old.txt").exists(),
            "overwrite replaces the directory"
        );
    }

    #[test]
    fn keep_both_writes_a_copy_and_preserves_the_original() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "NEW");
        dst.file("a.txt", "OLD");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::KeepBoth,
        ));

        assert!(s.errors.is_empty());
        // Original untouched, incoming written under a fresh "copy" name.
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "OLD"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a copy.txt")).unwrap(),
            "NEW"
        );
    }

    #[test]
    fn keep_both_move_keeps_original_and_removes_source() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "NEW");
        dst.file("a.txt", "OLD");

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::KeepBoth,
        ));

        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "OLD"
        );
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a copy.txt")).unwrap(),
            "NEW"
        );
        assert!(!file.exists(), "move removes the source after keep-both");
        // The placement records the REAL landing path ("a copy.txt"), so undo
        // can tell it apart from a faithfully-reversible move to "a.txt".
        assert_eq!(
            s.placements,
            vec![(file.clone(), dst.path().join("a copy.txt"))]
        );
    }

    #[test]
    fn native_overwrite_replaces_existing_file() {
        // Native copyfile uses CLONE|EXCL and used to silently fail on an
        // existing destination; staging + swap fixes that.
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "NEW");
        dst.file("a.txt", "OLD");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Native,
            vec![entry_for(&file)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(s.errors.is_empty(), "native overwrite must not error");
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "NEW"
        );
    }

    #[test]
    fn copying_dir_into_its_own_subdir_is_rejected() {
        // dest = a/sub/a lives inside the source a: must be rejected and
        // must not recurse forever.
        let work = TempDir::new();
        let a = work.dir("a");
        let sub = work.dir("a/sub");
        work.file("a/f.txt", "x");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&a)],
            &sub,
            vec![],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(!s.errors.is_empty(), "copy into own subtree must error");
    }

    #[test]
    fn buffered_copy_recreates_symlink_without_following() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let tree = src.dir("tree");
        src.file("tree/real.txt", "hi");
        // A link pointing back to its own ancestor: following it would loop.
        std::os::unix::fs::symlink(&tree, tree.join("loop")).unwrap();

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&tree)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        assert!(s.errors.is_empty());
        assert_eq!(
            std::fs::read_to_string(dst.path().join("tree/real.txt")).unwrap(),
            "hi"
        );
        let link_meta = std::fs::symlink_metadata(dst.path().join("tree/loop")).unwrap();
        assert!(
            link_meta.file_type().is_symlink(),
            "the link must be recreated, not followed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn explicit_follow_copies_a_symlink_target_as_regular_data() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let target = src.file("target.txt", "followed contents");
        let link = src.path().join("alias.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Native,
            vec![entry_for(&link)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        request.symlink_policy = crate::filesystem_policy::SymlinkPolicy::Follow;
        request.durability = DurabilityProfile::Verified;

        let state = run(request);

        assert!(state.errors.is_empty(), "{:?}", state.errors);
        let copied = dst.path().join("alias.txt");
        assert_eq!(
            std::fs::read_to_string(&copied).unwrap(),
            "followed contents"
        );
        assert!(
            !std::fs::symlink_metadata(copied)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn explicit_skip_omits_a_top_level_symlink_without_an_error() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let target = src.file("target.txt", "contents");
        let link = src.path().join("alias.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&link)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        request.symlink_policy = crate::filesystem_policy::SymlinkPolicy::Skip;

        let state = run(request);

        assert!(state.errors.is_empty(), "{:?}", state.errors);
        assert_eq!(state.files_done, 1);
        assert!(!dst.path().join("alias.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn followed_directory_symlink_cycles_fail_without_publishing_a_tree() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let tree = src.dir("tree");
        src.file("tree/file.txt", "contents");
        std::os::unix::fs::symlink(&tree, tree.join("loop")).unwrap();
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Native,
            vec![entry_for(&tree)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        request.symlink_policy = crate::filesystem_policy::SymlinkPolicy::Follow;

        let state = run(request);

        assert!(
            state
                .errors
                .iter()
                .any(|error| error.contains("symlink cycle"))
        );
        assert!(!dst.path().join("tree").exists());
        assert!(src.path().join("tree/file.txt").is_file());
    }

    #[test]
    fn skip_all_progress_reaches_total() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let a = src.file("a.txt", "12345"); // 5 bytes, will be skipped
        let b = src.file("b.txt", "123"); // 3 bytes, will be copied
        dst.file("a.txt", "old");

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&a), entry_for(&b)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::SkipAll,
        ));

        assert_eq!(s.total_bytes, 8);
        assert_eq!(
            s.copied_bytes, s.total_bytes,
            "skipped bytes must still advance the bar to 100%"
        );
    }

    #[test]
    fn ask_policy_refuses_surprise_existing_destination() {
        // A destination that appeared after the scan (so it is NOT in the
        // conflict list and the user never confirmed an overwrite) must not be
        // clobbered, and a Move must keep its source.
        let (src, dst) = (TempDir::new(), TempDir::new());
        let file = src.file("a.txt", "NEW");
        dst.file("a.txt", "OLD-IMPORTANT");

        let s = run(spec(
            TransferKind::Move,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            dst.path(),
            vec![], // empty: the engine must rely on a live check, not this
            OverwritePolicy::Ask,
        ));

        assert!(!s.errors.is_empty(), "a surprise existing dest must error");
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "OLD-IMPORTANT",
            "destination must not be clobbered"
        );
        assert!(file.exists(), "move must keep the source when refused");
    }

    #[test]
    fn self_referential_entry_still_advances_progress() {
        let work = TempDir::new();
        let data = work.dir("data");
        work.file("data/x.txt", "12345"); // 5 bytes

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&data)],
            work.path(),
            vec!["data".to_string()],
            OverwritePolicy::OverwriteAll,
        ));

        assert!(!s.errors.is_empty());
        assert_eq!(
            s.copied_bytes, s.total_bytes,
            "a rejected entry must still advance the bar to 100%"
        );
    }

    #[test]
    fn failed_new_dir_copy_cleans_partial_destination() {
        use std::os::unix::fs::PermissionsExt;

        let (src, dst) = (TempDir::new(), TempDir::new());
        let dir = src.dir("box");
        let secret = src.file("box/secret.txt", "x");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();

        let s = run(spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&dir)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ));

        let _ = std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644));

        assert!(!s.errors.is_empty());
        assert!(
            !dst.path().join("box").exists(),
            "partial directory must be cleaned up"
        );
    }
}
