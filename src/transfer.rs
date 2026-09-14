//! Background transfer engine: copy/move with progress reporting.
//!
//! This module is UI-agnostic. Progress is shared through [`TransferState`]
//! and the caller supplies a `notify` callback (e.g. a repaint request), so
//! the engine never depends on egui.

mod backend;
mod executor;

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
const MAX_MOUNT_RETRIES: usize = 2;

pub(crate) use crate::verified_hash::prefix as prefix_digest;

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
    pub submitted: Option<crate::operation_view::SubmittedSummary>,
    pub phase: crate::operation_view::OperationPhase,
    pub total_known: bool,
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
    pub pause_reason: Option<crate::operation_view::PauseReason>,
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
    finalization: Option<FinalizationOutcome>,
    scheduler_task: Option<crate::workload::TaskHandle>,
}

pub type TransferState = Arc<Mutex<TransferProgress>>;

impl TransferProgress {
    pub fn new(total_bytes: u64, files_total: usize) -> Self {
        Self {
            operation_id: None,
            group_id: None,
            submitted: None,
            phase: crate::operation_view::OperationPhase::Scan,
            total_known: true,
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
            pause_reason: None,
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
            finalization: None,
            scheduler_task: None,
        }
    }

    pub fn request_cancel(&mut self) -> bool {
        if self.finished || self.finalization.is_some() {
            return false;
        }
        self.cancelled = true;
        true
    }

    pub fn request_stop(&mut self) {
        if !self.finished && self.finalization.is_none() {
            self.stop_requested = true;
        }
    }

    fn finalization_committed_success(&self) -> bool {
        self.files_done == self.files_total
            && self
                .finalization
                .is_some_and(FinalizationOutcome::permits_late_interruption_success)
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

    pub fn set_phase(&mut self, phase: crate::operation_view::OperationPhase) {
        self.phase = phase;
    }

    pub fn unknown(files_total: usize) -> Self {
        let mut progress = Self::new(0, files_total);
        progress.total_known = false;
        progress
    }

    /// Overall byte progress is unknown while the scan/plan has not produced a
    /// denominator. Returning `None` lets every UI keep the same bar geometry
    /// while changing only its contents.
    pub fn progress_fraction(&self) -> Option<f32> {
        if !self.total_known {
            return None;
        }
        if self.total_bytes > 0 {
            return Some((self.copied_bytes as f32 / self.total_bytes as f32).clamp(0.0, 1.0));
        }
        Some(if self.files_total == 0 {
            1.0
        } else {
            (self.files_done as f32 / self.files_total as f32).clamp(0.0, 1.0)
        })
    }

    /// Estimate the remainder of the current phase. Transfer uses its rolling
    /// byte-rate; verification/finalization use completed-file throughput.
    pub fn phase_eta_secs(&self) -> Option<f64> {
        use crate::operation_view::OperationPhase;
        match self.phase {
            OperationPhase::Transfer => (self.eta_secs() > 0.0).then(|| self.eta_secs()),
            OperationPhase::Scan
            | OperationPhase::Plan
            | OperationPhase::Verify
            | OperationPhase::Finalize => None,
        }
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

pub(crate) fn request_cancel(progress: &TransferState) {
    let task = {
        let mut state = crate::lock_util::recover(progress);
        if state.request_cancel() {
            state.scheduler_task.clone()
        } else {
            None
        }
    };
    if let Some(task) = task {
        task.cancel();
    }
}

#[derive(Clone, Copy)]
struct JournalStep<'a> {
    operation_id: &'a OperationId,
    key: &'a IdempotencyKey,
    enabled: bool,
}

struct JournalCheckpointSink<'a> {
    step: JournalStep<'a>,
}

impl backend::CheckpointSink for JournalCheckpointSink<'_> {
    fn publish(&mut self, checkpoint: ResumeCheckpoint) -> std::io::Result<()> {
        if !self.step.enabled {
            return Ok(());
        }
        crate::operation_journal::mark_checkpoint(self.step.operation_id, self.step.key, checkpoint)
            .map_err(std::io::Error::other)
    }
}

#[cfg(test)]
fn requires_resumable_buffer(
    profile: &crate::volume_profile::VolumeProfile,
    source_size: u64,
) -> bool {
    profile.capabilities.resumable
        && profile.backend.is_slow_link()
        && source_size >= crate::delta_copy::DELTA_MIN_BYTES
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
    pub version_retention: crate::operation::VersionRetentionPolicy,
    pub name_policy: crate::filesystem_policy::NamePolicy,
    pub symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    /// Exact logical size published by a completed confirmation preflight.
    /// Queue/recovery callers without that snapshot leave this unset and the
    /// worker computes it off the UI thread.
    pub preflight_bytes: Option<u64>,
    pub post_success: Option<PostTransferAction>,
    /// A container created specifically for this operation and removable only
    /// after every completed effect has been rolled back out of it.
    pub rollback_cleanup: Option<PathBuf>,
    /// Stable ownership proof captured immediately after the operation created
    /// `rollback_cleanup`. Legacy path-only cleanup must fail closed.
    pub rollback_cleanup_identity: Option<PathIdentity>,
    #[cfg(test)]
    pub mount_wait_override: Option<Arc<dyn Fn() -> std::io::Result<()> + Send + Sync>>,
    #[cfg(test)]
    pub before_commit: Option<BeforeCommitHook>,
    #[cfg(test)]
    pub before_post_success: Option<BeforePostSuccessHook>,
    #[cfg(test)]
    pub before_terminal_publish: Option<BeforeTerminalPublishHook>,
    #[cfg(test)]
    pub journal_enabled: bool,
}

#[cfg(test)]
pub type BeforeCommitHook = Arc<dyn Fn(&Path, &Path) + Send + Sync>;
#[cfg(test)]
pub type BeforePostSuccessHook = Arc<dyn Fn() + Send + Sync>;
#[cfg(test)]
pub type BeforeTerminalPublishHook = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferExpectation {
    pub key: Option<crate::operation::IdempotencyKey>,
    pub source: Result<crate::path_identity::TransferSourceIdentity, String>,
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
    /// BLAKE3 of the first `offset` bytes for every resumable layout.
    #[serde(default)]
    pub content_digest: Option<[u8; 32]>,
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

#[cfg(test)]
pub fn capture_expectations(entries: &[FileEntry], target: &Path) -> Vec<TransferExpectation> {
    let scan =
        crate::scan::transfer_preflight(entries, crate::filesystem_policy::SymlinkPolicy::Preserve);
    match expectations_from_source_identities(entries, target, scan.source_identities) {
        Ok(expectations) => expectations,
        Err(failure) => entries
            .iter()
            .map(|entry| TransferExpectation {
                key: None,
                source: Err(failure.message.clone()),
                destination: capture_destination_identity(&target.join(&entry.name)),
                landing: None,
                landing_before: None,
                resume: None,
            })
            .collect(),
    }
}

pub(crate) fn expectations_from_source_identities(
    entries: &[FileEntry],
    target: &Path,
    sources: Vec<Result<crate::path_identity::TransferSourceIdentity, crate::ports::NativeFailure>>,
) -> Result<Vec<TransferExpectation>, crate::ports::NativeFailure> {
    if sources.len() != entries.len() {
        return Err(crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Unknown,
            message: "Resource preflight returned the wrong number of source proofs".to_string(),
        });
    }
    entries
        .iter()
        .zip(sources)
        .map(|(entry, source)| {
            let source = source?;
            let destination_path = target.join(&entry.name);
            let destination = observe_destination(&destination_path).map_err(|error| {
                let mut failure = crate::ports::NativeFailure::from_io(&error);
                failure.message = format!(
                    "Could not inspect destination {}: {}",
                    destination_path.display(),
                    failure.message
                );
                failure
            })?;
            Ok(TransferExpectation {
                key: None,
                source: Ok(source),
                destination: Ok(destination),
                landing: None,
                landing_before: None,
                resume: None,
            })
        })
        .collect()
}

fn observe_destination(path: &Path) -> std::io::Result<PathIdentity> {
    let shallow = PathIdentity::observe(path)?;
    if shallow.kind == Some(crate::path_identity::PathKind::Directory) {
        PathIdentity::observe_deep(path)
    } else {
        Ok(shallow)
    }
}

fn rebind_expectations_to_worker_scan(
    expectations: &mut [TransferExpectation],
    sources: Vec<Result<crate::path_identity::TransferSourceIdentity, crate::ports::NativeFailure>>,
) -> Result<(), crate::ports::NativeFailure> {
    if expectations.len() != sources.len() {
        return Err(crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Unknown,
            message: "Worker preflight returned the wrong number of source proofs".to_string(),
        });
    }
    for (expectation, current) in expectations.iter_mut().zip(sources) {
        let current = current?;
        let expected =
            expectation
                .source
                .as_ref()
                .map_err(|message| crate::ports::NativeFailure {
                    kind: crate::ports::NativeFailureKind::Stale,
                    message: message.clone(),
                })?;
        if !expected.same_binding(&current) {
            return Err(crate::ports::NativeFailure {
                kind: crate::ports::NativeFailureKind::Stale,
                message: format!(
                    "Source changed before worker preflight: {}",
                    current.lexical.path.display()
                ),
            });
        }
        expectation.source = Ok(current);
    }
    Ok(())
}

#[cfg(test)]
fn capture_destination_identity(path: &Path) -> Result<PathIdentity, String> {
    observe_destination(path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))
}

struct TransferWorkItem {
    key: IdempotencyKey,
    entry: FileEntry,
    expectation: TransferExpectation,
    mount_retries: usize,
}

enum MountWaitError {
    Interrupted,
    Unavailable(std::io::Error),
}

fn interruption_requested(
    scheduler_cancel: &crate::workload::CancellationToken,
    progress: &TransferState,
) -> bool {
    let mut state = crate::lock_util::recover(progress);
    if scheduler_cancel.is_cancelled() {
        state.cancelled = true;
    }
    if state.stop_requested {
        state.stopped = true;
    }
    state.cancelled || state.stop_requested
}

fn wait_for_mount(
    guard: &crate::mount_guard::MountGuard,
    label: &str,
    progress: &TransferState,
    scheduler_cancel: &crate::workload::CancellationToken,
    wait_override: Option<&(dyn Fn() -> std::io::Result<()> + Send + Sync)>,
    notify: &impl Fn(),
) -> Result<(), MountWaitError> {
    let result = if let Some(wait) = wait_override {
        wait()
    } else if guard.check() == crate::mount_guard::MountAvailability::Available {
        Ok(())
    } else {
        {
            let mut state = crate::lock_util::recover(progress);
            let reason = crate::operation_view::PauseReason::MountDisconnected {
                label: label.to_string(),
                timeout_secs: guard.policy.timeout_ms / 1_000,
            };
            state.waiting_reason = Some(reason.label());
            state.pause_reason = Some(reason);
        }
        notify();
        let result = guard.wait_until_available(|| {
            notify();
            !interruption_requested(scheduler_cancel, progress)
        });
        {
            let mut state = crate::lock_util::recover(progress);
            state.waiting_reason = None;
            state.pause_reason = None;
        }
        notify();
        result
    };

    match result {
        Ok(()) if interruption_requested(scheduler_cancel, progress) => {
            Err(MountWaitError::Interrupted)
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::Interrupted
                && interruption_requested(scheduler_cancel, progress) =>
        {
            Err(MountWaitError::Interrupted)
        }
        Ok(()) => Ok(()),
        Err(error) => Err(MountWaitError::Unavailable(error)),
    }
}

fn check_mount_fence(
    guard: &crate::mount_guard::MountGuard,
    check_override: Option<&(dyn Fn() -> std::io::Result<()> + Send + Sync)>,
) -> std::io::Result<()> {
    if let Some(check) = check_override {
        return check();
    }
    match guard.check() {
        crate::mount_guard::MountAvailability::Available => Ok(()),
        crate::mount_guard::MountAvailability::Disconnected => Err(std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "destination volume disconnected before commit",
        )),
        crate::mount_guard::MountAvailability::Replaced => Err(std::io::Error::new(
            std::io::ErrorKind::StaleNetworkFileHandle,
            "destination volume changed before commit",
        )),
    }
}

fn valid_runtime_checkpoint(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    source: &Path,
    staging: &Path,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
    journal_enabled: bool,
) -> Option<ResumeCheckpoint> {
    if !journal_enabled {
        return None;
    }
    let checkpoint = crate::operation_journal::step_checkpoint(operation_id, key)
        .ok()
        .flatten()?;
    if checkpoint.staging != staging || checkpoint.content_digest.is_none() {
        return None;
    }
    let checkpoint_source = if symlink_policy == crate::filesystem_policy::SymlinkPolicy::Follow
        && std::fs::symlink_metadata(source)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        std::fs::canonicalize(source).ok()?
    } else {
        source.to_path_buf()
    };
    let source_now = PathIdentity::observe_deep(&checkpoint_source).ok()?;
    if !checkpoint.source.same_binding(&source_now) {
        return None;
    }
    let partial_now = PathIdentity::observe_deep(staging).ok()?;
    if !checkpoint.partial.same_object(&partial_now)
        || partial_now.kind != Some(crate::path_identity::PathKind::File)
    {
        return None;
    }
    let layout_matches = match checkpoint.layout {
        CheckpointLayout::Prefix => partial_now.size >= checkpoint.offset,
        CheckpointLayout::DeltaFixed | CheckpointLayout::DeltaCdc => {
            partial_now.size == checkpoint.partial.size && partial_now.size >= checkpoint.offset
        }
    };
    if !layout_matches {
        return None;
    }
    let expected = checkpoint.content_digest?;
    if prefix_digest(&checkpoint_source, checkpoint.offset).ok()? != expected
        || prefix_digest(staging, checkpoint.offset).ok()? != expected
    {
        return None;
    }
    Some(checkpoint)
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
    if let crate::panel::ListingIdentity::Captured(identity) = &entry.identity {
        return match identity.kind {
            Some(crate::path_identity::PathKind::Directory) => {
                fs_util::dir_size_recursive(&entry.path)
            }
            Some(crate::path_identity::PathKind::File)
            | Some(crate::path_identity::PathKind::Other)
            | None => identity.size,
            Some(crate::path_identity::PathKind::Symlink) => 0,
        };
    }
    if entry.is_dir {
        fs_util::dir_size_recursive(&entry.path)
    } else {
        entry.size
    }
}

fn transfer_paths_equal(
    source: &Path,
    destination: &Path,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
) -> bool {
    enum Task {
        Compare(PathBuf, PathBuf),
        ExitDirectory(PathBuf),
    }

    let mut tasks = vec![Task::Compare(
        source.to_path_buf(),
        destination.to_path_buf(),
    )];
    let mut ancestors = std::collections::HashSet::new();
    while let Some(task) = tasks.pop() {
        match task {
            Task::ExitDirectory(canonical) => {
                ancestors.remove(&canonical);
            }
            Task::Compare(source, destination) => {
                let Ok(source_metadata) = std::fs::symlink_metadata(&source) else {
                    return false;
                };
                let Ok(destination_metadata) = std::fs::symlink_metadata(&destination) else {
                    return false;
                };
                if source_metadata.file_type().is_symlink() {
                    match symlink_policy {
                        crate::filesystem_policy::SymlinkPolicy::Preserve => {
                            if !destination_metadata.file_type().is_symlink()
                                || std::fs::read_link(&source).ok()
                                    != std::fs::read_link(&destination).ok()
                            {
                                return false;
                            }
                        }
                        crate::filesystem_policy::SymlinkPolicy::Skip => return false,
                        crate::filesystem_policy::SymlinkPolicy::Follow => {
                            if destination_metadata.file_type().is_symlink() {
                                return false;
                            }
                            let Ok(followed) = std::fs::canonicalize(&source) else {
                                return false;
                            };
                            tasks.push(Task::Compare(followed, destination));
                        }
                    }
                    continue;
                }
                if source_metadata.is_dir() {
                    if !destination_metadata.is_dir()
                        || destination_metadata.file_type().is_symlink()
                    {
                        return false;
                    }
                    let Ok(canonical) = std::fs::canonicalize(&source) else {
                        return false;
                    };
                    if !ancestors.insert(canonical.clone()) {
                        return false;
                    }
                    let Some(source_names) = transfer_child_names(&source, Some(symlink_policy))
                    else {
                        return false;
                    };
                    let Some(destination_names) = transfer_child_names(&destination, None) else {
                        return false;
                    };
                    if source_names != destination_names {
                        return false;
                    }
                    tasks.push(Task::ExitDirectory(canonical));
                    for name in source_names.into_iter().rev() {
                        tasks.push(Task::Compare(source.join(&name), destination.join(name)));
                    }
                    continue;
                }
                if destination_metadata.is_dir()
                    || destination_metadata.file_type().is_symlink()
                    || !crate::fs_util::files_equal(&source, &destination)
                {
                    return false;
                }
            }
        }
    }
    true
}

fn transfer_child_names(
    path: &Path,
    source_policy: Option<crate::filesystem_policy::SymlinkPolicy>,
) -> Option<Vec<std::ffi::OsString>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(path).ok()? {
        let entry = entry.ok()?;
        if source_policy == Some(crate::filesystem_policy::SymlinkPolicy::Skip)
            && entry.file_type().ok()?.is_symlink()
        {
            continue;
        }
        names.push(entry.file_name());
    }
    names.sort();
    Some(names)
}

/// Total bytes for all entries (recursively for dirs).
#[cfg(test)]
pub fn total_bytes(entries: &[FileEntry]) -> u64 {
    entries
        .iter()
        .map(entry_size)
        .fold(0_u64, u64::saturating_add)
}

#[cfg(test)]
fn planned_total_bytes(spec: &TransferSpec) -> u64 {
    spec.preflight_bytes.unwrap_or_else(|| {
        crate::scan::transfer_preflight(&spec.entries, spec.symlink_policy)
            .need_bytes
            .unwrap_or(0)
    })
}

/// Run the transfer on a background thread.
///
/// `notify` is invoked whenever visible progress changed; the UI passes a
/// repaint request here, keeping this module free of egui types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PostSuccessOutcome {
    NotRequired,
    Succeeded,
    Skipped,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FinalizationOutcome {
    NotReached,
    Reached(PostSuccessOutcome),
}

impl FinalizationOutcome {
    fn permits_late_interruption_success(self) -> bool {
        matches!(
            self,
            Self::Reached(PostSuccessOutcome::NotRequired | PostSuccessOutcome::Succeeded)
        )
    }
}

pub fn spawn_transfer(
    spec: TransferSpec,
    progress: TransferState,
    notify: impl Fn() + Send + 'static,
) {
    executor::spawn_transfer(spec, progress, notify);
}

#[cfg(test)]
pub(crate) fn spawn_transfer_with_workload(
    workload: crate::workload::WorkloadHandle,
    spec: TransferSpec,
    progress: TransferState,
    notify: impl Fn() + Send + 'static,
) {
    executor::spawn_transfer_with_workload(workload, spec, progress, notify);
}

#[cfg(test)]
use executor::{FailureRollback, TransferExecutor, finish_progress, run_failure_rollback};

#[cfg(test)]
use executor::{cleanup_moved_source, quarantine_expected_path, undo_placement};

/// Copy a single file with progress reporting (buffered strategy).
/// Removes the partial destination file on any failure.
/// Returns the file size on success.
fn copy_file_buffered_with_limiter(
    src: &Path,
    dst: &Path,
    state: &TransferState,
    limiter: &mut crate::transfer_tuning::BandwidthLimiter,
    resume: Option<&ResumeCheckpoint>,
    checkpoints: Option<&mut dyn backend::CheckpointSink>,
    preserve_sparse: bool,
) -> std::io::Result<u64> {
    if preserve_sparse && resume.is_none() && is_sparse_file(src) {
        return copy_file_sparse(src, dst, state, limiter);
    }
    let retain_partial = checkpoints.is_some();
    let result = copy_file_buffered_inner(src, dst, state, limiter, resume, checkpoints);
    if let Err(ref e) = result {
        // Clean up our own partial write, but never delete a destination that
        // was already there (AlreadyExists means create_new refused to clobber).
        if !retain_partial && e.kind() != std::io::ErrorKind::AlreadyExists {
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
    {
        let mut progress = crate::lock_util::recover(state);
        progress.current_file = src
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        progress.current_file_size = file_size;
        progress.current_file_copied = 0;
    }
    let mut cursor = 0_u64;
    let mut reported = 0_u64;
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
            let advanced = extent_offset.saturating_sub(reported);
            reported = extent_offset;
            let mut progress = crate::lock_util::recover(state);
            progress.current_file_copied = extent_offset;
            progress.copied_bytes = progress.copied_bytes.saturating_add(advanced);
            progress.maybe_sample();
        }
        cursor = hole;
    }
    writer.sync_data()?;
    if let Ok(metadata) = src.metadata() {
        std::fs::set_permissions(dst, metadata.permissions())?;
    }
    let mut progress = crate::lock_util::recover(state);
    progress.copied_bytes = progress
        .copied_bytes
        .saturating_add(file_size.saturating_sub(reported));
    progress.current_file_copied = file_size;
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
    mut checkpoints: Option<&mut dyn backend::CheckpointSink>,
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
    let mut content_hasher = blake3::Hasher::new();
    if let Some(checkpoint) = resume {
        let expected = checkpoint.content_digest.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Prefix checkpoint has no content proof",
            )
        })?;
        crate::verified_hash::prefix_into(src, offset, &mut content_hasher)?;
        if *content_hasher.clone().finalize().as_bytes() != expected
            || prefix_digest(dst, offset)? != expected
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Prefix checkpoint content proof does not match source and staging",
            ));
        }
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
                persist_checkpoint(
                    src,
                    dst,
                    copied,
                    &mut writer,
                    &content_hasher,
                    &mut checkpoints,
                )?;
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
        content_hasher.update(&buf[..n]);
        copied = copied.saturating_add(n as u64);
        if let Err(error) = limiter.consume(n, || crate::lock_util::recover(state).cancelled) {
            persist_checkpoint(
                src,
                dst,
                copied,
                &mut writer,
                &content_hasher,
                &mut checkpoints,
            )?;
            return Err(error);
        }

        {
            let mut s = crate::lock_util::recover(state);
            s.copied_bytes += n as u64;
            s.current_file_copied += n as u64;
            s.maybe_sample();
        }
        if copied >= next_checkpoint {
            persist_checkpoint(
                src,
                dst,
                copied,
                &mut writer,
                &content_hasher,
                &mut checkpoints,
            )?;
            next_checkpoint = copied.saturating_add(CHECKPOINT_INTERVAL);
        }
    }
    persist_checkpoint(
        src,
        dst,
        copied,
        &mut writer,
        &content_hasher,
        &mut checkpoints,
    )?;
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
    content_hasher: &blake3::Hasher,
    checkpoints: &mut Option<&mut dyn backend::CheckpointSink>,
) -> std::io::Result<()> {
    writer.flush()?;
    writer.get_ref().sync_data()?;
    let Some(checkpoints) = checkpoints.as_deref_mut() else {
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
        content_digest: Some(*content_hasher.clone().finalize().as_bytes()),
    };
    checkpoints.publish(checkpoint)
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
    prepare_buffered_tree(src, dst, state, &mut files, &mut permissions)?;
    let workers = workers.max(1).min(files.len().max(1));
    let pool = directory_copy_pool(workers)?;
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
    state: &TransferState,
    files: &mut Vec<(PathBuf, PathBuf)>,
    permissions: &mut Vec<(PathBuf, std::fs::Permissions)>,
) -> std::io::Result<()> {
    let mut tasks = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((source, destination)) = tasks.pop() {
        if crate::lock_util::recover(state).cancelled {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled while scanning the directory copy",
            ));
        }
        let metadata = std::fs::symlink_metadata(&source)?;
        if metadata.file_type().is_symlink() {
            copy_symlink(&source, &destination)?;
            continue;
        }
        if !metadata.is_dir() {
            files.push((source, destination));
            continue;
        }
        std::fs::create_dir(&destination)?;
        permissions.push((destination.clone(), metadata.permissions()));
        let mut children = std::fs::read_dir(&source)?.collect::<Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for entry in children.into_iter().rev() {
            tasks.push((entry.path(), destination.join(entry.file_name())));
        }
    }
    Ok(())
}

fn directory_copy_pool(workers: usize) -> std::io::Result<Arc<rayon::ThreadPool>> {
    static POOLS: std::sync::OnceLock<
        Mutex<std::collections::HashMap<usize, Arc<rayon::ThreadPool>>>,
    > = std::sync::OnceLock::new();
    let pools = POOLS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some(pool) = crate::lock_util::recover(pools).get(&workers).cloned() {
        return Ok(pool);
    }
    let candidate = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .thread_name(|index| format!("commander-copy-{index}"))
            .build()
            .map_err(std::io::Error::other)?,
    );
    let mut pools = crate::lock_util::recover(pools);
    Ok(Arc::clone(
        pools.entry(workers).or_insert_with(|| candidate),
    ))
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
    enum Task {
        Visit(PathBuf, PathBuf),
        ExitDirectory(PathBuf),
    }

    let mut tasks = vec![Task::Visit(src.to_path_buf(), dst.to_path_buf())];
    let mut ancestors = std::collections::HashSet::new();
    let mut copied = 0u64;
    while let Some(task) = tasks.pop() {
        if crate::lock_util::recover(state).cancelled {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }
        match task {
            Task::ExitDirectory(canonical) => {
                ancestors.remove(&canonical);
            }
            Task::Visit(source, destination) => {
                let metadata = std::fs::symlink_metadata(&source)?;
                if metadata.file_type().is_symlink() {
                    match symlink_policy {
                        crate::filesystem_policy::SymlinkPolicy::Preserve => {
                            copy_symlink(&source, &destination)?;
                        }
                        crate::filesystem_policy::SymlinkPolicy::Skip => {}
                        crate::filesystem_policy::SymlinkPolicy::Follow => {
                            tasks.push(Task::Visit(std::fs::canonicalize(&source)?, destination));
                        }
                    }
                    continue;
                }
                if !metadata.is_dir() {
                    copied += copy_file_buffered_with_limiter(
                        &source,
                        &destination,
                        state,
                        limiter,
                        None,
                        None,
                        preserve_sparse,
                    )?;
                    continue;
                }
                let canonical = std::fs::canonicalize(&source)?;
                if !ancestors.insert(canonical.clone()) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("symlink cycle detected at {}", source.display()),
                    ));
                }
                std::fs::create_dir(&destination)?;
                let mut children = std::fs::read_dir(&source)?.collect::<Result<Vec<_>, _>>()?;
                children.sort_by_key(|entry| entry.file_name());
                tasks.push(Task::ExitDirectory(canonical));
                for entry in children.into_iter().rev() {
                    tasks.push(Task::Visit(
                        entry.path(),
                        destination.join(entry.file_name()),
                    ));
                }
            }
        }
    }
    Ok(copied)
}

/// Recreate a symlink at `dst` pointing at the same target as `src`.
#[cfg(unix)]
fn copy_symlink(src: &Path, dst: &Path) -> std::io::Result<()> {
    let target = std::fs::read_link(src)?;
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
    use std::sync::atomic::Ordering;

    fn entry_for(path: &Path) -> FileEntry {
        let lexical = std::fs::symlink_metadata(path).unwrap();
        let display = if lexical.file_type().is_symlink() {
            std::fs::metadata(path).unwrap()
        } else {
            lexical.clone()
        };
        let mut entry = FileEntry::from_meta(path.to_path_buf(), &display).unwrap();
        entry.identity = crate::panel::ListingIdentity::Captured(
            crate::path_identity::PathIdentity::from_metadata(path.to_path_buf(), &lexical),
        );
        entry
    }

    fn bind_preflight(spec: &mut TransferSpec) {
        let scan = crate::scan::transfer_preflight(&spec.entries, spec.symlink_policy);
        let bytes = scan.need_bytes.unwrap();
        spec.expectations = expectations_from_source_identities(
            &spec.entries,
            &spec.target,
            scan.source_identities,
        )
        .unwrap();
        spec.preflight_bytes = Some(bytes);
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

    fn run_with_backends(spec: TransferSpec, backends: backend::BackendPorts) -> TransferProgress {
        let total = total_bytes(&spec.entries);
        let progress: TransferState =
            Arc::new(Mutex::new(TransferProgress::new(total, spec.entries.len())));
        TransferExecutor::with_backends(
            crate::workload::global_handle(),
            spec,
            Arc::clone(&progress),
            || {},
            backends,
        )
        .submit();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            {
                let state = progress.lock().unwrap();
                if state.finished {
                    return state.clone();
                }
            }
            assert!(std::time::Instant::now() < deadline, "transfer timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    struct FakeStageBackend {
        calls: Arc<Mutex<Vec<&'static str>>>,
        cancel_after_stage: bool,
        durable_receipt: bool,
    }

    impl FakeStageBackend {
        fn stage(
            &self,
            label: &'static str,
            request: backend::StageRequest<'_>,
        ) -> std::io::Result<backend::StageReceipt> {
            assert!(!request.is_dir, "test backend only stages files");
            assert!(
                request
                    .staging
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().contains(".cmdr-tmp.")),
                "backend must receive an isolated staging pathname"
            );
            crate::lock_util::recover(&self.calls).push(label);
            let bytes = std::fs::copy(request.source, request.staging)?;
            std::fs::File::open(request.staging)?.sync_data()?;
            request.progress.complete_file(request.base_bytes, bytes);
            if self.cancel_after_stage {
                request.progress.request_cancel();
            }
            Ok(backend::StageReceipt {
                bytes,
                fast_path: crate::transfer_tuning::FastPath::Buffered,
                artifact: PathIdentity::observe(request.staging)?,
                durable: self.durable_receipt,
            })
        }
    }

    impl backend::NativeCloneBackend for FakeStageBackend {
        fn stage(
            &self,
            request: backend::StageRequest<'_>,
        ) -> std::io::Result<backend::StageReceipt> {
            self.stage("native", request)
        }
    }

    impl backend::DeltaBackend for FakeStageBackend {
        fn stage(
            &self,
            _mode: crate::delta_copy::DeltaMode,
            request: backend::StageRequest<'_>,
            _checkpoints: &mut dyn backend::CheckpointSink,
        ) -> std::io::Result<backend::StageReceipt> {
            self.stage("delta", request)
        }
    }

    impl backend::SparseBackend for FakeStageBackend {
        fn stage(
            &self,
            request: backend::StageRequest<'_>,
        ) -> std::io::Result<backend::StageReceipt> {
            self.stage("sparse", request)
        }
    }

    impl backend::BufferedBackend for FakeStageBackend {
        fn stage(
            &self,
            request: backend::StageRequest<'_>,
            _checkpoints: &mut dyn backend::CheckpointSink,
        ) -> std::io::Result<backend::StageReceipt> {
            self.stage("buffered", request)
        }
    }

    fn fake_ports(
        calls: Arc<Mutex<Vec<&'static str>>>,
        cancel_after_stage: bool,
    ) -> backend::BackendPorts {
        let fake = Arc::new(FakeStageBackend {
            calls,
            cancel_after_stage,
            durable_receipt: true,
        });
        backend::BackendPorts::new(fake.clone(), fake.clone(), fake.clone(), fake)
    }

    fn fake_ports_without_durability(
        calls: Arc<Mutex<Vec<&'static str>>>,
    ) -> backend::BackendPorts {
        let fake = Arc::new(FakeStageBackend {
            calls,
            cancel_after_stage: false,
            durable_receipt: false,
        });
        backend::BackendPorts::new(fake.clone(), fake.clone(), fake.clone(), fake)
    }

    #[test]
    fn finish_normalizes_only_after_complete_or_unneeded_post_success() {
        let completed = Arc::new(Mutex::new(TransferProgress::new(1, 1)));
        {
            let mut state = completed.lock().unwrap();
            state.files_done = 1;
            state.cancelled = true;
        }
        finish_progress(
            &completed,
            FinalizationOutcome::Reached(PostSuccessOutcome::NotRequired),
        );
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
        finish_progress(
            &partial,
            FinalizationOutcome::Reached(PostSuccessOutcome::NotRequired),
        );
        let state = partial.lock().unwrap();
        assert!(state.finished);
        assert!(state.cancelled);
        drop(state);

        let skipped_post_success = Arc::new(Mutex::new(TransferProgress::new(1, 1)));
        {
            let mut state = skipped_post_success.lock().unwrap();
            state.files_done = 1;
            state.cancelled = true;
        }
        finish_progress(
            &skipped_post_success,
            FinalizationOutcome::Reached(PostSuccessOutcome::Skipped),
        );
        let state = skipped_post_success.lock().unwrap();
        assert!(state.finished);
        assert!(state.cancelled);

        let completed_post_success = Arc::new(Mutex::new(TransferProgress::new(1, 1)));
        {
            let mut state = completed_post_success.lock().unwrap();
            state.files_done = 1;
            state.cancelled = true;
        }
        finish_progress(
            &completed_post_success,
            FinalizationOutcome::Reached(PostSuccessOutcome::Succeeded),
        );
        let state = completed_post_success.lock().unwrap();
        assert!(state.finished);
        assert!(!state.cancelled);
    }

    #[test]
    fn failed_gather_rolls_completed_placements_back_and_removes_its_folder() {
        let root = TempDir::new();
        let container = root.dir("Gathered");
        let landing = root.file("Gathered/a.txt", "a");
        let source = root.path().join("a.txt");
        let progress = Arc::new(Mutex::new(TransferProgress::new(2, 2)));
        {
            let mut state = crate::lock_util::recover(&progress);
            state.files_done = 1;
            state.errors.push("second entry failed".to_string());
            state.placements.push((source.clone(), landing.clone()));
        }

        let outcome = run_failure_rollback(Some(&container), &OperationId::new(), false, &progress);

        assert_eq!(outcome, FailureRollback::Complete);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "a");
        assert!(!landing.exists());
        assert!(!container.exists());
        let state = crate::lock_util::recover(&progress);
        assert!(state.placements.is_empty());
        assert_eq!(state.errors, ["second entry failed"]);
    }

    #[test]
    fn failed_gather_never_clobbers_a_recreated_source_during_rollback() {
        let root = TempDir::new();
        let container = root.dir("Gathered");
        let landing = root.file("Gathered/a.txt", "moved");
        let source = root.file("a.txt", "external");
        let progress = Arc::new(Mutex::new(TransferProgress::new(2, 2)));
        {
            let mut state = crate::lock_util::recover(&progress);
            state.files_done = 1;
            state.errors.push("second entry failed".to_string());
            state.placements.push((source.clone(), landing.clone()));
        }

        let outcome = run_failure_rollback(Some(&container), &OperationId::new(), false, &progress);

        assert_eq!(outcome, FailureRollback::Incomplete);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "external");
        assert_eq!(std::fs::read_to_string(&landing).unwrap(), "moved");
        assert!(container.exists());
        let state = crate::lock_util::recover(&progress);
        assert_eq!(state.placements, [(source, landing)]);
        assert!(
            state
                .errors
                .iter()
                .any(|error| error.contains("data preserved"))
        );
    }

    #[test]
    fn failed_gather_transfer_restores_sources_and_removes_orphan_folder() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let root = TempDir::new();
        let first = root.file("a.txt", "aaa");
        let second = root.file("b.txt", "bbb");
        let folder = root.dir("Gathered");
        let rollback_cleanup_identity = PathIdentity::observe_deep(&folder).unwrap();

        let progress: TransferState = Arc::new(Mutex::new(TransferProgress::new(
            total_bytes(&[entry_for(&first), entry_for(&second)]),
            2,
        )));
        let progress_for_hook = Arc::clone(&progress);
        let commits = Arc::new(AtomicUsize::new(0));
        let commits_for_hook = Arc::clone(&commits);

        let mut request = spec(
            TransferKind::Move,
            CopyMethod::Native,
            vec![entry_for(&first), entry_for(&second)],
            &folder,
            vec![],
            OverwritePolicy::Ask,
        );
        request.rollback_cleanup = Some(folder.clone());
        request.rollback_cleanup_identity = Some(rollback_cleanup_identity);
        request.before_commit = Some(Arc::new(move |_, _| {
            // Let the first Gather placement commit, then abort before the
            // second lands so failure rollback must restore sources and
            // remove the orphan container.
            if commits_for_hook.fetch_add(1, Ordering::SeqCst) >= 1 {
                crate::lock_util::recover(&progress_for_hook).cancelled = true;
            }
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

        assert_eq!(std::fs::read_to_string(&first).unwrap(), "aaa");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "bbb");
        assert!(!folder.exists(), "orphan gather folder must be removed");
        assert!(!folder.join("a.txt").exists());
        assert!(!folder.join("b.txt").exists());
        assert!(state.placements.is_empty(), "{:?}", state.placements);
        assert!(commits.load(Ordering::SeqCst) >= 2);
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
            version_retention: crate::operation::VersionRetentionPolicy::default(),
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            preflight_bytes: None,
            post_success: Some(PostTransferAction::RemoveEmptyDir(folder.clone())),
            rollback_cleanup: None,
            rollback_cleanup_identity: None,
            mount_wait_override: None,
            before_commit: None,
            before_post_success: None,
            before_terminal_publish: None,
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
            version_retention: crate::operation::VersionRetentionPolicy::default(),
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            preflight_bytes: None,
            post_success: Some(PostTransferAction::RemoveEmptyDir(folder.clone())),
            rollback_cleanup: None,
            rollback_cleanup_identity: None,
            mount_wait_override: None,
            before_commit: None,
            before_post_success: None,
            before_terminal_publish: None,
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
            version_retention: crate::operation::VersionRetentionPolicy::default(),
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            preflight_bytes: None,
            post_success: Some(PostTransferAction::RemoveEmptyDir(folder.clone())),
            rollback_cleanup: None,
            rollback_cleanup_identity: None,
            mount_wait_override: None,
            before_commit: None,
            before_post_success: None,
            before_terminal_publish: None,
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
            version_retention: crate::operation::VersionRetentionPolicy::default(),
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            preflight_bytes: None,
            post_success: None,
            rollback_cleanup: None,
            rollback_cleanup_identity: None,
            mount_wait_override: None,
            before_commit: None,
            before_post_success: None,
            before_terminal_publish: None,
            journal_enabled: false,
        }
    }

    #[test]
    fn executor_places_only_the_artifact_returned_by_the_selected_backend_port() {
        let (source, target) = (TempDir::new(), TempDir::new());
        let file = source.file("port.txt", "through the port");
        let calls = Arc::new(Mutex::new(Vec::new()));

        let state = run_with_backends(
            spec(
                TransferKind::Copy,
                CopyMethod::Buffered,
                vec![entry_for(&file)],
                target.path(),
                vec![],
                OverwritePolicy::Ask,
            ),
            fake_ports(Arc::clone(&calls), false),
        );

        assert!(state.errors.is_empty(), "{:?}", state.errors);
        assert_eq!(*crate::lock_util::recover(&calls), ["buffered"]);
        assert_eq!(
            std::fs::read_to_string(target.path().join("port.txt")).unwrap(),
            "through the port"
        );
        assert!(std::fs::read_dir(target.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".cmdr-tmp.")
        }));
    }

    #[test]
    fn executor_honors_cancel_after_stage_and_before_namespace_commit() {
        let (source, target, journal_root) = (TempDir::new(), TempDir::new(), TempDir::new());
        let file = source.file("late-cancel.txt", "source survives");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let _journal = crate::operation_journal::use_test_journal(
            journal_root.path().join("operation-journal.json"),
        );
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            target.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        request.journal_enabled = true;
        let operation_id = request.operation_id.clone();

        let state = run_with_backends(request, fake_ports(Arc::clone(&calls), true));

        assert!(state.cancelled);
        assert!(file.exists());
        assert!(!target.path().join("late-cancel.txt").exists());
        assert_eq!(*crate::lock_util::recover(&calls), ["buffered"]);
        assert!(std::fs::read_dir(target.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".cmdr-tmp.")
        }));
        let record = crate::operation_journal::operation(&operation_id).unwrap();
        assert!(record.steps[0].checkpoint.is_none());
        assert!(
            record.steps[0]
                .staging
                .as_ref()
                .is_none_or(|staging| !staging.exists())
        );
    }

    #[test]
    fn verified_executor_rejects_a_backend_without_a_durability_receipt() {
        let (source, target) = (TempDir::new(), TempDir::new());
        let file = source.file("receipt.txt", "must be durable");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            target.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        request.durability = DurabilityProfile::Verified;

        let state = run_with_backends(request, fake_ports_without_durability(Arc::clone(&calls)));

        assert!(!target.path().join("receipt.txt").exists());
        assert_eq!(*crate::lock_util::recover(&calls), ["buffered"]);
        assert!(
            state
                .errors
                .iter()
                .any(|error| error.contains("no durability receipt"))
        );
    }

    #[test]
    fn executor_rechecks_destination_mount_before_placement() {
        let (source, target) = (TempDir::new(), TempDir::new());
        let file = source.file("mount-fence.txt", "source survives");
        let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&file)],
            target.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        let worker_checks = Arc::clone(&checks);
        request.mount_wait_override = Some(Arc::new(move || {
            let call = worker_checks.fetch_add(1, Ordering::SeqCst);
            if call < 2 {
                Ok(())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::NotConnected,
                    "fixture mount replaced before commit",
                ))
            }
        }));

        let state = run(request);

        assert!(file.exists());
        assert!(!target.path().join("mount-fence.txt").exists());
        assert!(checks.load(Ordering::SeqCst) >= 3);
        assert!(
            state
                .errors
                .iter()
                .any(|error| error.contains("mount reconnect failed"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn move_directory_with_skip_preserves_omitted_symlinks_in_source() {
        let (source, target) = (TempDir::new(), TempDir::new());
        let folder = source.dir("folder");
        let regular = source.file("folder/data.txt", "moved");
        let omitted = folder.join("omitted-link");
        std::os::unix::fs::symlink(&regular, &omitted).unwrap();
        let mut request = spec(
            TransferKind::Move,
            CopyMethod::Buffered,
            vec![entry_for(&folder)],
            target.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        request.symlink_policy = crate::filesystem_policy::SymlinkPolicy::Skip;
        bind_preflight(&mut request);

        let state = run(request);

        assert!(state.errors.is_empty(), "{:?}", state.errors);
        assert_eq!(
            std::fs::read_to_string(target.path().join("folder/data.txt")).unwrap(),
            "moved"
        );
        assert!(!target.path().join("folder/omitted-link").exists());
        assert!(!regular.exists());
        assert!(omitted.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(folder.is_dir());
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
            content_digest: Some(prefix_digest(&staging, offset as u64).unwrap()),
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
        assert!(s.errors.is_empty(), "{:?}", s.errors);
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

    #[cfg(unix)]
    #[test]
    fn parallel_sparse_copy_reports_the_sum_without_progress_regression() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let source = src.dir("folder");
        let file_size = 8 * 1024 * 1024_u64;
        for name in ["one.bin", "two.bin"] {
            let path = source.join(name);
            let mut file = std::fs::File::create(path).unwrap();
            file.set_len(file_size).unwrap();
            file.seek(SeekFrom::Start(1024 * 1024)).unwrap();
            file.write_all(name.as_bytes()).unwrap();
            file.sync_all().unwrap();
        }
        let destination = dst.path().join("folder");
        let progress = Arc::new(Mutex::new(TransferProgress::new(file_size * 2, 1)));

        let copied = copy_dir_buffered_parallel(&source, &destination, &progress, 2, true).unwrap();

        assert_eq!(copied, file_size * 2);
        assert_eq!(
            crate::lock_util::recover(&progress).copied_bytes,
            file_size * 2
        );
    }

    #[test]
    fn parallel_directory_scan_honors_cancellation_before_allocating_the_tree() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let source = src.dir("folder");
        src.file("folder/a.txt", "a");
        let destination = dst.path().join("folder");
        let progress = Arc::new(Mutex::new(TransferProgress::new(1, 1)));
        crate::lock_util::recover(&progress).cancelled = true;

        let error =
            copy_dir_buffered_parallel(&source, &destination, &progress, 2, false).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert!(!destination.exists());
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
    fn source_cleanup_restores_an_object_changed_after_its_identity_fence() {
        let temp = TempDir::new();
        let source = temp.file("source.txt", "reviewed");
        let expected = PathIdentity::observe_deep(&source).unwrap();
        std::fs::write(&source, "changed concurrently").unwrap();

        let error = cleanup_moved_source(
            &source,
            &expected,
            crate::filesystem_policy::SymlinkPolicy::Preserve,
        )
        .unwrap_err();

        assert!(error.to_string().contains("changed"));
        assert_eq!(
            std::fs::read_to_string(&source).unwrap(),
            "changed concurrently"
        );
        assert!(std::fs::read_dir(temp.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("cmdr-source-cleanup")
        }));
    }

    #[test]
    fn overwrite_quarantine_restores_an_object_changed_inside_the_commit_fence() {
        let temp = TempDir::new();
        let destination = temp.file("destination.txt", "reviewed");
        let expected = PathIdentity::observe_deep(&destination).unwrap();
        std::fs::write(&destination, "changed concurrently").unwrap();
        let quarantine = temp.path().join(".destination.backup");

        let error = quarantine_expected_path(&destination, &quarantine, &expected).unwrap_err();

        assert!(error.to_string().contains("changed"));
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "changed concurrently"
        );
        assert!(!quarantine.exists());
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
    fn production_transfer_completes_and_cleans_a_journaled_overwrite() {
        let (src, dst, journal_root) = (TempDir::new(), TempDir::new(), TempDir::new());
        let source = src.file("a.txt", "new contents");
        dst.file("a.txt", "old contents");
        let _journal = crate::operation_journal::use_test_journal(
            journal_root.path().join("operation-journal.json"),
        );
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&source)],
            dst.path(),
            vec!["a.txt".to_string()],
            OverwritePolicy::OverwriteAll,
        );
        request.journal_enabled = true;
        let operation_id = request.operation_id.clone();

        let state = run(request);

        assert!(state.failures.is_empty(), "{:?}", state.failures);
        assert_eq!(
            std::fs::read_to_string(dst.path().join("a.txt")).unwrap(),
            "new contents"
        );
        let record = crate::operation_journal::operation(&operation_id).unwrap();
        assert_eq!(
            record.status,
            crate::operation_journal::OperationStatus::Completed
        );
        assert_eq!(
            record.steps[0].status,
            crate::operation_journal::StepStatus::Completed
        );
        assert!(record.steps[0].replacement.is_none());
        assert!(std::fs::read_dir(dst.path()).unwrap().all(|entry| {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            !name.contains(".cmdr-tmp.") && !name.contains(".cmdr-quarantine.")
        }));
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
    fn source_modified_between_copy_and_commit_is_rejected_as_stale() {
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

        assert!(
            state
                .failures
                .iter()
                .any(|failure| failure.class == FailureClass::IntegrityUncertain)
        );
        assert_eq!(state.requeued_files, 0);
        assert!(!dst.path().join("a.txt").exists());
        assert_eq!(
            std::fs::read_to_string(file).unwrap(),
            "second version after copy"
        );
    }

    #[test]
    fn directory_staging_modified_after_backend_receipt_is_rejected() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let folder = src.dir("folder");
        src.file("folder/data.txt", "copied bytes");
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&folder)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        request.before_commit = Some(Arc::new(move |_, landing| {
            let staging = std::fs::read_dir(landing.parent().unwrap())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy().contains(".cmdr-tmp."))
                })
                .expect("backend staging directory");
            std::fs::write(staging.join("data.txt"), "changed after receipt").unwrap();
        }));

        let state = run(request);

        assert!(!dst.path().join("folder").exists());
        assert!(
            state
                .errors
                .iter()
                .any(|error| error.contains("staging artifact changed"))
        );
        assert_eq!(
            std::fs::read_to_string(src.path().join("folder/data.txt")).unwrap(),
            "copied bytes"
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
    fn destination_directory_child_modified_after_review_is_preserved() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let incoming = src.dir("folder");
        src.file("folder/incoming.txt", "incoming");
        dst.dir("folder");
        let existing = dst.file("folder/existing.txt", "reviewed");
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&incoming)],
            dst.path(),
            vec!["folder".to_string()],
            OverwritePolicy::OverwriteAll,
        );
        request.before_commit = Some(Arc::new(move |_, landing| {
            std::fs::write(landing.join("existing.txt"), "changed concurrently").unwrap();
        }));

        let state = run(request);

        assert_eq!(
            std::fs::read_to_string(&existing).unwrap(),
            "changed concurrently"
        );
        assert!(!dst.path().join("folder/incoming.txt").exists());
        assert!(
            state
                .failures
                .iter()
                .any(|failure| failure.class == FailureClass::UserDecision)
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
    fn confirmed_preflight_size_is_reused_by_the_worker_plan() {
        let (source, target) = (TempDir::new(), TempDir::new());
        let directory = source.dir("folder");
        source.file("folder/value.bin", "123");
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Native,
            vec![entry_for(&directory)],
            target.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        request.preflight_bytes = Some(987_654);

        assert_eq!(planned_total_bytes(&request), 987_654);
    }

    #[test]
    fn confirmed_size_without_source_proofs_fails_closed() {
        let (source, target) = (TempDir::new(), TempDir::new());
        let file = source.file("value.bin", "123");
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Native,
            vec![entry_for(&file)],
            target.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        request.preflight_bytes = Some(3);
        request.expectations.clear();

        let state = run(request);

        assert!(
            state
                .errors
                .iter()
                .any(|error| error.contains("preflight snapshot is incomplete"))
        );
        assert!(!target.path().join("value.bin").exists());
    }

    #[cfg(unix)]
    #[test]
    fn worker_fallback_sizes_followed_symlink_bytes() {
        let (source, target) = (TempDir::new(), TempDir::new());
        source.file("target/value.bin", "1234567");
        let link = source.path().join("linked");
        std::os::unix::fs::symlink("target", &link).unwrap();
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Native,
            vec![entry_for(&link)],
            target.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        request.symlink_policy = crate::filesystem_policy::SymlinkPolicy::Follow;
        request.preflight_bytes = None;

        assert_eq!(planned_total_bytes(&request), 7);
    }

    #[cfg(unix)]
    #[test]
    fn worker_never_upgrades_an_incomplete_follow_proof() {
        let (source, target) = (TempDir::new(), TempDir::new());
        source.file("target/value.bin", "1234567");
        let link = source.path().join("linked");
        std::os::unix::fs::symlink("target", &link).unwrap();
        let entries = vec![entry_for(&link)];
        let preserve = crate::scan::transfer_preflight(
            &entries,
            crate::filesystem_policy::SymlinkPolicy::Preserve,
        );
        let mut expectations = expectations_from_source_identities(
            &entries,
            target.path(),
            preserve.source_identities,
        )
        .unwrap();
        let followed = crate::scan::transfer_preflight(
            &entries,
            crate::filesystem_policy::SymlinkPolicy::Follow,
        );

        let failure =
            rebind_expectations_to_worker_scan(&mut expectations, followed.source_identities)
                .unwrap_err();

        assert_eq!(failure.kind, crate::ports::NativeFailureKind::Stale);
    }

    #[test]
    fn unknown_progress_becomes_known_without_changing_its_model() {
        let mut progress = TransferProgress::unknown(2);
        assert_eq!(progress.phase, crate::operation_view::OperationPhase::Scan);
        assert_eq!(progress.progress_fraction(), None);

        progress.total_known = true;
        progress.set_phase(crate::operation_view::OperationPhase::Plan);
        assert_eq!(progress.progress_fraction(), Some(0.0));
        progress.files_done = 1;
        assert_eq!(progress.progress_fraction(), Some(0.5));
        progress.files_done = 2;
        assert_eq!(progress.progress_fraction(), Some(1.0));
    }

    #[test]
    fn phase_eta_is_only_published_for_measured_transfer_work() {
        let mut progress = TransferProgress::new(1_000, 1);
        progress.started_at = std::time::Instant::now() - std::time::Duration::from_secs(6);
        progress.speed_samples = vec![(0.0, 0.0), (4.0, 400.0)];
        progress.copied_bytes = 600;
        progress.set_phase(crate::operation_view::OperationPhase::Plan);
        assert_eq!(progress.phase_eta_secs(), None);
        progress.set_phase(crate::operation_view::OperationPhase::Transfer);
        assert!(progress.phase_eta_secs().is_some_and(|eta| eta > 0.0));
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

        assert!(s.errors.is_empty(), "{:?}", s.errors);
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
    fn symlink_backend_never_removes_an_existing_staging_entry() {
        let root = TempDir::new();
        let target = root.file("target.txt", "target");
        let source = root.path().join("source-link");
        std::os::unix::fs::symlink(&target, &source).unwrap();
        let occupied = root.file("occupied", "foreign");

        let error = copy_symlink(&source, &occupied).unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(occupied).unwrap(), "foreign");
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
        bind_preflight(&mut request);

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
    fn followed_target_replacement_after_preflight_never_starts_copy() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let target = src.file("target.txt", "original");
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
        bind_preflight(&mut request);
        std::fs::remove_file(&target).unwrap();
        std::fs::write(&target, "replacement").unwrap();

        let state = run(request);

        assert!(
            state
                .errors
                .iter()
                .any(|error| error.contains("source changed after transfer preflight"))
        );
        assert!(!dst.path().join("alias.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn verified_follow_accepts_nested_symlinks_as_copied_bytes() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let tree = src.dir("tree");
        let followed = src.file("payload.txt", "followed contents");
        std::os::unix::fs::symlink(&followed, tree.join("alias.txt")).unwrap();
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&tree)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        request.symlink_policy = crate::filesystem_policy::SymlinkPolicy::Follow;
        request.durability = DurabilityProfile::Verified;
        bind_preflight(&mut request);

        let state = run(request);

        assert!(state.errors.is_empty(), "{:?}", state.errors);
        let copied = dst.path().join("tree/alias.txt");
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
    fn verified_skip_ignores_nested_symlinks_on_both_sides_of_verification() {
        let (src, dst) = (TempDir::new(), TempDir::new());
        let tree = src.dir("tree");
        src.file("tree/kept.txt", "kept");
        let skipped = src.file("payload.txt", "skipped");
        std::os::unix::fs::symlink(&skipped, tree.join("alias.txt")).unwrap();
        let mut request = spec(
            TransferKind::Copy,
            CopyMethod::Buffered,
            vec![entry_for(&tree)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        );
        request.symlink_policy = crate::filesystem_policy::SymlinkPolicy::Skip;
        request.durability = DurabilityProfile::Verified;
        bind_preflight(&mut request);

        let state = run(request);

        assert!(state.errors.is_empty(), "{:?}", state.errors);
        assert_eq!(
            std::fs::read_to_string(dst.path().join("tree/kept.txt")).unwrap(),
            "kept"
        );
        assert!(!dst.path().join("tree/alias.txt").exists());
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
        request.expectations.clear();

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
