//! Background transfer engine: copy/move with progress reporting.
//!
//! This module is UI-agnostic. Progress is shared through [`TransferState`]
//! and the caller supplies a `notify` callback (e.g. a repaint request), so
//! the engine never depends on egui.

mod backend;
mod buffered;
mod executor;
mod parallel_tree;
mod sparse;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::fs_util;
use crate::operation::{
    ClassifiedFailure, DurabilityProfile, FailureClass, IdempotencyKey, OperationGroupId,
    OperationId,
};
use crate::panel::FileEntry;
use crate::path_identity::PathIdentity;
use serde::{Deserialize, Serialize};

const MAX_MOUNT_RETRIES: usize = 2;

pub use crate::verified_hash::prefix as prefix_digest;

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
    /// Native macOS `copyfile()` with APFS clone support, xattr/ACL preservation.
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
    /// undo can target where files really landed: a `KeepBoth` conflict lands at
    /// "name copy.ext", not "name". Empty for copies.
    pub placements: Vec<(PathBuf, PathBuf)>,
    finalization: Option<FinalizationOutcome>,
    scheduler_task: Option<crate::workload::TaskHandle>,
}

pub type TransferState = Arc<Mutex<TransferProgress>>;

/// The error every copy primitive returns when the user cancels.
pub fn cancelled_error() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled")
}

/// `Err(cancelled)` once the user has cancelled this transfer.
pub fn check_cancelled(state: &Mutex<TransferProgress>) -> std::io::Result<()> {
    if crate::lock_util::recover(state).cancelled {
        Err(cancelled_error())
    } else {
        Ok(())
    }
}

impl TransferProgress {
    /// Report `path` as the file now being copied, `size` bytes, none done.
    pub(crate) fn begin_file(&mut self, path: &Path, size: u64) {
        self.current_file = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        self.current_file_size = size;
        self.current_file_copied = 0;
    }

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

    /// Whether Cancel (or Stop after current file) still has an effect: once
    /// finalization starts the outcome is committed and requests are ignored.
    pub const fn can_cancel(&self) -> bool {
        !self.finished && self.finalization.is_none()
    }

    pub const fn request_cancel(&mut self) -> bool {
        if !self.can_cancel() {
            return false;
        }
        self.cancelled = true;
        true
    }

    pub const fn request_stop(&mut self) {
        if self.can_cancel() {
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

    pub const fn set_phase(&mut self, phase: crate::operation_view::OperationPhase) {
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

    /// Estimate the remainder of the current phase. Only Transfer publishes
    /// an ETA (from its rolling byte-rate); the other phases have no measured
    /// throughput and return `None`.
    pub fn phase_eta_secs(&self) -> Option<f64> {
        use crate::operation_view::OperationPhase;
        match self.phase {
            OperationPhase::Transfer => {
                let eta = self.eta_secs();
                (eta > 0.0).then_some(eta)
            }
            OperationPhase::Scan
            | OperationPhase::Plan
            | OperationPhase::Verify
            | OperationPhase::Finalize => None,
        }
    }

    /// Move `copied_bytes` to `copied` for work that moved no data (a skip,
    /// a failure or a settled entry). Every speed sample shifts by the same
    /// amount so the credited bytes never show up as throughput.
    fn credit_without_throughput(&mut self, copied: u64) {
        let credited = copied as f64 - self.copied_bytes as f64;
        for (_, bytes) in &mut self.speed_samples {
            *bytes += credited;
        }
        self.copied_bytes = copied;
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

pub fn request_cancel(progress: &TransferState) {
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

/// Cleanup that runs only after every entry of a transfer succeeded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PostTransferAction {
    /// Remove a directory only if it is empty after a successful transfer.
    /// Used by Undo for "New Folder with Selection"; `remove_dir` deliberately
    /// preserves the folder if another process added anything to it.
    RemoveEmptyDir(PathBuf),
}

/// A copy/move request, fully described and detached from any UI state.
///
/// Note there is no conflict list here: the engine checks the destination
/// live at copy time (the confirmation dialog may sit open while the
/// filesystem changes), driven only by [`OverwritePolicy`].
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

impl TransferSpec {
    /// A fresh, ungrouped operation with default policies, nothing captured
    /// and no post-success or rollback work. Callers override the rest with
    /// struct-update syntax.
    pub fn new(kind: TransferKind, entries: Vec<FileEntry>, target: PathBuf) -> Self {
        Self {
            operation_id: OperationId::new(),
            group_id: None,
            kind,
            entries,
            expectations: Vec::new(),
            target,
            policy: OverwritePolicy::Ask,
            method: CopyMethod::Native,
            durability: DurabilityProfile::default(),
            version_retention: crate::operation::VersionRetentionPolicy::default(),
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            preflight_bytes: None,
            post_success: None,
            rollback_cleanup: None,
            rollback_cleanup_identity: None,
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
        }
    }
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

impl ResumeCheckpoint {
    /// Record a checkpoint for `staging` at `offset`, observing the source and
    /// partial identities now. `content_digest` is BLAKE3 of the first
    /// `offset` bytes.
    pub fn observe(
        source: &Path,
        staging: &Path,
        offset: u64,
        layout: CheckpointLayout,
        content_digest: [u8; 32],
    ) -> std::io::Result<Self> {
        Ok(Self {
            staging: staging.to_path_buf(),
            offset,
            source: PathIdentity::observe_deep(source)?,
            partial: PathIdentity::observe_deep(staging)?,
            layout,
            content_digest: Some(content_digest),
        })
    }

    /// Whether the partial file as observed now can still hold this
    /// checkpoint: a prefix may have grown past the offset, while delta
    /// layouts preallocate and must keep their recorded size.
    pub const fn size_fits(&self, partial_now: &PathIdentity) -> bool {
        match self.layout {
            CheckpointLayout::Prefix => partial_now.size >= self.offset,
            CheckpointLayout::DeltaFixed | CheckpointLayout::DeltaCdc => {
                partial_now.size == self.partial.size && partial_now.size >= self.offset
            }
        }
    }

    /// Re-prove this checkpoint against `source` and `staging` as they are
    /// now: the source keeps its binding, the staging file is still the same
    /// object and can hold the offset, and both prefixes hash to the recorded
    /// proof. Shared by runtime resume and journal recovery; the error says
    /// why the checkpoint cannot be used.
    pub fn verify(&self, source: &Path, staging: &Path) -> Result<(), String> {
        let expected = self
            .content_digest
            .ok_or_else(|| "checkpoint has no content proof".to_string())?;
        let source_now = PathIdentity::observe_deep(source)
            .map_err(|error| format!("could not inspect checkpoint source: {error}"))?;
        if !self.source.same_binding(&source_now) {
            return Err("source changed after checkpoint".to_string());
        }
        let partial_now = PathIdentity::observe_deep(staging)
            .map_err(|error| format!("could not inspect checkpoint staging: {error}"))?;
        if !self.same_partial(&partial_now)
            || partial_now.kind != Some(crate::path_identity::PathKind::File)
            || !self.size_fits(&partial_now)
        {
            return Err("staging changed after checkpoint".to_string());
        }
        let partial_digest = prefix_digest(staging, self.offset)
            .map_err(|error| format!("could not hash checkpoint staging: {error}"))?;
        let source_digest = prefix_digest(source, self.offset)
            .map_err(|error| format!("could not hash checkpoint source: {error}"))?;
        if partial_digest != expected || source_digest != expected {
            return Err("checkpoint bytes changed after proof".to_string());
        }
        Ok(())
    }

    /// Whether `partial_now` is still the staging object this checkpoint
    /// recorded. Without file ids (Windows) a prefix staging cannot be
    /// followed by identity, and the tail written after the last checkpoint
    /// changes its size and mtime; the prefix digests prove the resumed bytes.
    fn same_partial(&self, partial_now: &PathIdentity) -> bool {
        let ids_known = self.partial.volume.zip(self.partial.file_id).is_some()
            && partial_now.volume.zip(partial_now.file_id).is_some();
        if !ids_known && self.layout == CheckpointLayout::Prefix {
            self.partial.exists && partial_now.exists && self.partial.kind == partial_now.kind
        } else {
            self.partial.same_object(partial_now)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckpointLayout {
    #[default]
    Prefix,
    DeltaFixed,
    DeltaCdc,
}

impl CheckpointLayout {
    const fn delta_mode(self) -> Option<crate::delta_copy::DeltaMode> {
        match self {
            Self::Prefix => None,
            Self::DeltaFixed => Some(crate::delta_copy::DeltaMode::Fixed),
            Self::DeltaCdc => Some(crate::delta_copy::DeltaMode::ContentDefined),
        }
    }

    pub const fn label(self) -> &'static str {
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

pub fn expectations_from_source_identities(
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
    let probe_guard = guard.clone();
    wait_for_mount_probed(
        guard,
        move || probe_guard.check(),
        label,
        progress,
        scheduler_cancel,
        wait_override,
        notify,
    )
}

/// Every probe, including the first, goes through the bounded reconnect wait
/// so a hung hard-mounted share cannot block the worker past the timeout or
/// a cancel. The pause reason is published once the first probe fails.
fn wait_for_mount_probed(
    guard: &crate::mount_guard::MountGuard,
    probe: impl Fn() -> crate::mount_guard::MountAvailability + Send + Clone + 'static,
    label: &str,
    progress: &TransferState,
    scheduler_cancel: &crate::workload::CancellationToken,
    wait_override: Option<&(dyn Fn() -> std::io::Result<()> + Send + Sync)>,
    notify: &impl Fn(),
) -> Result<(), MountWaitError> {
    let result = if let Some(wait) = wait_override {
        wait()
    } else {
        let mut paused = false;
        let result = guard.wait_bounded(probe, || {
            if !paused {
                paused = true;
                let mut state = crate::lock_util::recover(progress);
                let reason = crate::operation_view::PauseReason::MountDisconnected {
                    label: label.to_string(),
                    timeout_secs: guard.policy.timeout_ms / 1_000,
                };
                state.waiting_reason = Some(reason.label());
                state.pause_reason = Some(reason);
            }
            notify();
            !interruption_requested(scheduler_cancel, progress)
        });
        if paused {
            {
                let mut state = crate::lock_util::recover(progress);
                state.waiting_reason = None;
                state.pause_reason = None;
            }
            notify();
            result
        } else {
            // The generic error says "while the operation was paused"; a
            // volume found swapped before any wait is reported as just that.
            result.map_err(|error| {
                if error.kind() == std::io::ErrorKind::StaleNetworkFileHandle {
                    std::io::Error::new(
                        error.kind(),
                        format!("{label} changed: a different volume is mounted there"),
                    )
                } else {
                    error
                }
            })
        }
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
    let checkpoint = match crate::operation_journal::step_checkpoint(operation_id, key) {
        Ok(checkpoint) => checkpoint?,
        Err(error) => {
            // Resume is an optimisation: an unreadable journal only costs a
            // full recopy, but say why the checkpoint was not used.
            log::warn!(
                target: "commander::transfer",
                "ignoring resume checkpoint for {}: {error}",
                staging.display()
            );
            return None;
        }
    };
    if checkpoint.staging != staging {
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
    if let Err(reason) = checkpoint.verify(&checkpoint_source, staging) {
        log::debug!(
            target: "commander::transfer",
            "not resuming {}: {reason}",
            staging.display()
        );
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
    let journal_error = if journal_enabled {
        crate::operation_journal::mark_failed(operation_id, key, failure.clone()).err()
    } else {
        None
    };
    // The real cause first; a journal failure is secondary to it.
    let path = failure.path.clone();
    record_failure(progress, entry_name, failure);
    if let Some(error) = journal_error {
        record_journal_error(progress, entry_name, path, error);
    }
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
    *completed_bytes = completed_bytes.saturating_add(size);
    let mut state = crate::lock_util::recover(progress);
    state.credit_without_throughput(*completed_bytes);
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
const fn journal_enabled(spec: &TransferSpec) -> bool {
    spec.journal_enabled
}

#[cfg(not(test))]
const fn journal_enabled(_spec: &TransferSpec) -> bool {
    true
}

/// Size of one entry: its byte length, or the recursive size of a directory.
#[cfg(test)]
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

/// Progress credit for one work item. A successful preflight already owns the
/// exact recursive logical size. If preflight failed, the item will be
/// rejected without copying, so use its cheap listing size instead of walking
/// a directory tree that cannot be transferred.
fn execution_entry_size(
    entry: &FileEntry,
    source: &Result<crate::path_identity::TransferSourceIdentity, String>,
) -> u64 {
    source
        .as_ref()
        .map_or(entry.size, |source| source.logical_bytes)
}

#[cfg(test)]
fn transfer_paths_equal(
    source: &Path,
    destination: &Path,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
) -> bool {
    transfer_paths_mismatch(source, destination, symlink_policy).is_none()
}

/// Compare a finished copy with its source. `None` means they match;
/// otherwise the first difference found, naming the entry (relative to the
/// copied item) and why it differs.
fn transfer_paths_mismatch(
    source: &Path,
    destination: &Path,
    symlink_policy: crate::filesystem_policy::SymlinkPolicy,
) -> Option<String> {
    enum Task {
        Compare(PathBuf, PathBuf),
        ExitDirectory(PathBuf),
    }

    // The copy may still sit at a hidden staging name, so a difference at
    // the root is reported by reason alone; the caller names the item.
    let root = destination;
    let mismatch = |at: &Path, reason: &str| -> Option<String> {
        match at.strip_prefix(root) {
            Ok(relative) if relative.as_os_str().is_empty() => Some(reason.to_string()),
            Ok(relative) => Some(format!("{}: {reason}", relative.display())),
            Err(_) => Some(format!("{}: {reason}", at.display())),
        }
    };
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
                    return mismatch(&destination, "source can no longer be read");
                };
                let Ok(destination_metadata) = std::fs::symlink_metadata(&destination) else {
                    return mismatch(&destination, "copy is missing");
                };
                if source_metadata.file_type().is_symlink() {
                    match symlink_policy {
                        crate::filesystem_policy::SymlinkPolicy::Preserve => {
                            if !destination_metadata.file_type().is_symlink() {
                                return mismatch(&destination, "copy is not a symbolic link");
                            }
                            if std::fs::read_link(&source).ok()
                                != std::fs::read_link(&destination).ok()
                            {
                                return mismatch(&destination, "symbolic link target differs");
                            }
                        }
                        crate::filesystem_policy::SymlinkPolicy::Skip => {
                            return mismatch(&destination, "skipped symbolic link was copied");
                        }
                        crate::filesystem_policy::SymlinkPolicy::Follow => {
                            if destination_metadata.file_type().is_symlink() {
                                return mismatch(
                                    &destination,
                                    "copy is a symbolic link, not its target",
                                );
                            }
                            let Ok(followed) = std::fs::canonicalize(&source) else {
                                return mismatch(&destination, "source link cannot be followed");
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
                        return mismatch(&destination, "copy is not a folder");
                    }
                    let Ok(canonical) = std::fs::canonicalize(&source) else {
                        return mismatch(&destination, "source folder cannot be resolved");
                    };
                    if !ancestors.insert(canonical.clone()) {
                        return mismatch(&destination, "source folder loops back on itself");
                    }
                    let Some(source_names) = transfer_child_names(&source, Some(symlink_policy))
                    else {
                        return mismatch(&destination, "source folder cannot be listed");
                    };
                    let Some(destination_names) = transfer_child_names(&destination, None) else {
                        return mismatch(&destination, "copied folder cannot be listed");
                    };
                    let Some(pairs) = pair_child_names(source_names, destination_names) else {
                        return mismatch(&destination, "folder contents differ");
                    };
                    tasks.push(Task::ExitDirectory(canonical));
                    for (source_name, destination_name) in pairs.into_iter().rev() {
                        tasks.push(Task::Compare(
                            source.join(source_name),
                            destination.join(destination_name),
                        ));
                    }
                    continue;
                }
                if destination_metadata.is_dir() || destination_metadata.file_type().is_symlink() {
                    return mismatch(&destination, "copy is not a regular file");
                }
                if !crate::fs_util::files_equal(&source, &destination) {
                    return mismatch(&destination, "contents differ");
                }
            }
        }
    }
    None
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

/// Pair every source child with the destination child that holds its copy,
/// or `None` when the two directories do not list the same entries.
///
/// Names normally match exactly. A destination that stores names in another
/// Unicode normalization (HFS+ and some SMB servers store NFD) still pairs
/// one-to-one by NFC form; on macOS the `._name` `AppleDouble` files the kernel
/// writes for extended attributes on exFAT/FAT/SMB are not extra entries.
fn pair_child_names(
    source: Vec<std::ffi::OsString>,
    destination: Vec<std::ffi::OsString>,
) -> Option<Vec<(std::ffi::OsString, std::ffi::OsString)>> {
    use unicode_normalization::UnicodeNormalization;

    #[cfg(target_os = "macos")]
    let destination = drop_apple_double_companions(&source, destination);
    if source == destination {
        return Some(
            source
                .into_iter()
                .map(|name| (name.clone(), name))
                .collect(),
        );
    }
    if source.len() != destination.len() {
        return None;
    }
    let nfc = |name: &std::ffi::OsStr| name.to_str().map(|name| name.nfc().collect::<String>());
    let mut by_form = std::collections::HashMap::with_capacity(destination.len());
    for name in destination {
        if by_form.insert(nfc(&name)?, name).is_some() {
            return None;
        }
    }
    source
        .into_iter()
        .map(|name| {
            let destination = by_form.remove(&nfc(&name)?)?;
            Some((name, destination))
        })
        .collect()
}

/// Remove destination `._X` names that are `AppleDouble` companions of a
/// copied `X` rather than entries of the source itself.
#[cfg(any(test, target_os = "macos"))]
fn drop_apple_double_companions(
    source: &[std::ffi::OsString],
    mut destination: Vec<std::ffi::OsString>,
) -> Vec<std::ffi::OsString> {
    destination.retain(|name| {
        let companion_of = name
            .to_str()
            .and_then(|name| name.strip_prefix("._"))
            .map(std::ffi::OsStr::new);
        !companion_of.is_some_and(|base| {
            source.iter().any(|source| source == base) && !source.contains(name)
        })
    });
    destination
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

/// How the optional post-success action ended for a finished transfer.
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
    const fn permits_late_interruption_success(self) -> bool {
        matches!(
            self,
            Self::Reached(PostSuccessOutcome::NotRequired | PostSuccessOutcome::Succeeded)
        )
    }
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
    executor::spawn_transfer(spec, progress, notify);
}

#[cfg(test)]
pub fn spawn_transfer_with_workload(
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

#[cfg(test)]
use crate::fs_util::copy_symlink;
#[cfg(test)]
use buffered::copy_file_buffered_with_limiter;
#[cfg(test)]
use parallel_tree::copy_dir_buffered_parallel;
#[cfg(test)]
use sparse::{copy_file_sparse, is_sparse_file};

#[cfg(test)]
mod tests;
