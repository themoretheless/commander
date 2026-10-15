use std::path::Path;
use std::sync::Arc;

use super::{CheckpointLayout, CopyMethod, ResumeCheckpoint, TransferState};
use crate::filesystem_policy::SymlinkPolicy;
use crate::operation::DurabilityProfile;
use crate::path_identity::PathIdentity;
use crate::transfer_tuning::{FastPath, TuningSnapshot, VolumeRule};
use crate::volume_profile::VolumeProfile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BackendPlan {
    NativeClone,
    Delta(crate::delta_copy::DeltaMode),
    Sparse,
    Buffered,
}

#[derive(Clone, Copy)]
pub(super) struct PlanInput {
    pub method: CopyMethod,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub source_size: Option<u64>,
    pub basis_size: Option<u64>,
    pub source_is_sparse: bool,
    pub resume_layout: Option<CheckpointLayout>,
    pub symlink_policy: SymlinkPolicy,
    pub delta_capable: bool,
    pub sparse_capable: bool,
    pub resumable_capable: bool,
    pub slow_link: bool,
    pub bandwidth_limited: bool,
    pub tuning: TuningSnapshot,
}

pub(super) struct BackendPlanner;

impl BackendPlanner {
    pub(super) fn select(input: PlanInput) -> std::io::Result<BackendPlan> {
        if input.is_symlink {
            return match input.symlink_policy {
                SymlinkPolicy::Preserve | SymlinkPolicy::Follow => Ok(BackendPlan::Buffered),
                SymlinkPolicy::Skip => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "skipped symlink reached the copy backend",
                )),
            };
        }

        let resume_delta = input.resume_layout.and_then(CheckpointLayout::delta_mode);
        if resume_delta.is_some() && input.basis_size.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "delta checkpoint lost its basis",
            ));
        }
        if !input.is_dir
            && (resume_delta.is_some() || !input.bandwidth_limited)
            && let (Some(source_size), Some(basis_size)) = (input.source_size, input.basis_size)
        {
            let mode = resume_delta.or_else(|| {
                if input.resume_layout.is_none() {
                    crate::delta_copy::select(
                        input.delta_capable,
                        source_size,
                        basis_size,
                        input.tuning,
                    )
                } else {
                    None
                }
            });
            if let Some(mode) = mode {
                return Ok(BackendPlan::Delta(mode));
            }
        }

        let force_resumable = input.source_size.is_some_and(|size| {
            input.resumable_capable && input.slow_link && size >= crate::delta_copy::DELTA_MIN_BYTES
        });
        // Symlinks returned above, so the policy only matters for folders,
        // whose native copy cannot follow or skip links inside the tree. A
        // plain file keeps its clone/native path under every policy.
        let force_buffered = input.bandwidth_limited
            || force_resumable
            || (input.is_dir && input.symlink_policy != SymlinkPolicy::Preserve);
        if !input.is_dir
            && input.resume_layout.is_none()
            && !force_resumable
            && input.sparse_capable
            && input.source_is_sparse
        {
            return Ok(BackendPlan::Sparse);
        }
        match input.method {
            CopyMethod::Native if !force_buffered && input.resume_layout.is_none() => {
                Ok(BackendPlan::NativeClone)
            }
            CopyMethod::Native | CopyMethod::Buffered => Ok(BackendPlan::Buffered),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct StageRequest<'a> {
    pub source: &'a Path,
    pub is_dir: bool,
    pub staging: &'a Path,
    pub progress: &'a BackendProgress,
    pub base_bytes: u64,
    pub profile: &'a VolumeProfile,
    pub rule: VolumeRule,
    pub tuning: TuningSnapshot,
    pub basis: Option<&'a Path>,
    pub resume: Option<&'a ResumeCheckpoint>,
    pub symlink_policy: SymlinkPolicy,
    pub durability: DurabilityProfile,
}

pub(super) struct BackendProgress {
    state: TransferState,
}

impl BackendProgress {
    pub(super) const fn new(state: TransferState) -> Self {
        Self { state }
    }

    const fn state(&self) -> &TransferState {
        &self.state
    }

    #[cfg(test)]
    pub(super) fn complete_file(&self, base_bytes: u64, bytes: u64) {
        let mut progress = crate::lock_util::recover(&self.state);
        progress.current_file_copied = bytes;
        progress.copied_bytes = base_bytes.saturating_add(bytes);
    }

    #[cfg(test)]
    pub(super) fn request_cancel(&self) {
        crate::lock_util::recover(&self.state).request_cancel();
    }
}

#[derive(Clone, Debug)]
pub(super) struct StageReceipt {
    pub bytes: u64,
    pub fast_path: FastPath,
    pub artifact: PathIdentity,
    pub durable: bool,
}

pub(super) trait CheckpointSink {
    fn publish(&mut self, checkpoint: ResumeCheckpoint) -> std::io::Result<()>;
}

pub(super) trait NativeCloneBackend: Send + Sync {
    fn stage(&self, request: StageRequest<'_>) -> std::io::Result<StageReceipt>;
}

pub(super) trait DeltaBackend: Send + Sync {
    fn stage(
        &self,
        mode: crate::delta_copy::DeltaMode,
        request: StageRequest<'_>,
        checkpoints: &mut dyn CheckpointSink,
    ) -> std::io::Result<StageReceipt>;
}

pub(super) trait SparseBackend: Send + Sync {
    fn stage(&self, request: StageRequest<'_>) -> std::io::Result<StageReceipt>;
}

pub(super) trait BufferedBackend: Send + Sync {
    fn stage(
        &self,
        request: StageRequest<'_>,
        checkpoints: &mut dyn CheckpointSink,
    ) -> std::io::Result<StageReceipt>;
}

#[derive(Clone)]
pub(super) struct BackendPorts {
    native_clone: Arc<dyn NativeCloneBackend>,
    delta: Arc<dyn DeltaBackend>,
    sparse: Arc<dyn SparseBackend>,
    buffered: Arc<dyn BufferedBackend>,
}

impl BackendPorts {
    pub(super) fn production() -> Self {
        Self {
            native_clone: Arc::new(ProductionNativeClone),
            delta: Arc::new(ProductionDelta),
            sparse: Arc::new(ProductionSparse),
            buffered: Arc::new(ProductionBuffered),
        }
    }

    #[cfg(test)]
    pub(super) fn new(
        native_clone: Arc<dyn NativeCloneBackend>,
        delta: Arc<dyn DeltaBackend>,
        sparse: Arc<dyn SparseBackend>,
        buffered: Arc<dyn BufferedBackend>,
    ) -> Self {
        Self {
            native_clone,
            delta,
            sparse,
            buffered,
        }
    }

    pub(super) fn plan(
        &self,
        request: &StageRequest<'_>,
        method: CopyMethod,
    ) -> std::io::Result<BackendPlan> {
        let metadata = std::fs::symlink_metadata(request.source)?;
        let is_symlink = metadata.file_type().is_symlink();
        let source_size = (!request.is_dir && !is_symlink).then_some(metadata.len());
        let basis_size = request
            .basis
            .and_then(|basis| basis.metadata().ok())
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len());
        BackendPlanner::select(PlanInput {
            method,
            is_dir: request.is_dir,
            is_symlink,
            source_size,
            basis_size,
            source_is_sparse: !request.is_dir
                && !is_symlink
                && super::sparse::is_sparse_file(request.source),
            resume_layout: request.resume.map(|checkpoint| checkpoint.layout),
            symlink_policy: request.symlink_policy,
            delta_capable: request.profile.capabilities.delta,
            sparse_capable: request.profile.capabilities.sparse,
            resumable_capable: request.profile.capabilities.resumable,
            slow_link: request.profile.backend.is_slow_link(),
            bandwidth_limited: request.rule.max_bytes_per_second.is_some(),
            tuning: request.tuning,
        })
    }

    pub(super) fn stage(
        &self,
        method: CopyMethod,
        request: StageRequest<'_>,
        checkpoints: &mut dyn CheckpointSink,
    ) -> std::io::Result<StageReceipt> {
        match self.plan(&request, method)? {
            BackendPlan::NativeClone => self.native_clone.stage(request),
            BackendPlan::Delta(mode) => self.delta.stage(mode, request, checkpoints),
            BackendPlan::Sparse => self.sparse.stage(request),
            BackendPlan::Buffered => self.buffered.stage(request, checkpoints),
        }
    }
}

struct ProductionNativeClone;

impl NativeCloneBackend for ProductionNativeClone {
    fn stage(&self, request: StageRequest<'_>) -> std::io::Result<StageReceipt> {
        let (bytes, fast_path) = if request.is_dir {
            (
                crate::native_copy::copy_dir_native(
                    request.source,
                    request.staging,
                    request.progress.state(),
                    request.base_bytes,
                )?,
                FastPath::Native,
            )
        } else {
            let outcome = crate::native_copy::copy_file_native(
                request.source,
                request.staging,
                request.progress.state(),
                request.base_bytes,
                request.profile.capabilities.clone,
            )?;
            (
                outcome.bytes,
                if outcome.cloned {
                    FastPath::Clone
                } else {
                    FastPath::Native
                },
            )
        };
        receipt(request.staging, bytes, fast_path, request.durability)
    }
}

struct ProductionDelta;

impl DeltaBackend for ProductionDelta {
    fn stage(
        &self,
        mode: crate::delta_copy::DeltaMode,
        request: StageRequest<'_>,
        checkpoints: &mut dyn CheckpointSink,
    ) -> std::io::Result<StageReceipt> {
        let basis = request.basis.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "delta backend lost its basis",
            )
        })?;
        let layout = match mode {
            crate::delta_copy::DeltaMode::Fixed => CheckpointLayout::DeltaFixed,
            crate::delta_copy::DeltaMode::ContentDefined => CheckpointLayout::DeltaCdc,
        };
        // The delta copy proves both prefixes against this digest before
        // resuming, so the backend does not re-read them here as well.
        let resume = request
            .resume
            .map(|checkpoint| {
                let content_digest = checkpoint.content_digest.ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Delta checkpoint has no content proof",
                    )
                })?;
                Ok::<_, std::io::Error>(crate::delta_copy::DeltaResume {
                    offset: checkpoint.offset,
                    content_digest,
                })
            })
            .transpose()?;
        let source = request.source;
        let staging = request.staging;
        let mut publish = |offset, content_digest| {
            checkpoints.publish(ResumeCheckpoint::observe(
                source,
                staging,
                offset,
                layout,
                content_digest,
            )?)
        };
        let stats = crate::delta_copy::copy_file(
            crate::delta_copy::DeltaRequest {
                source,
                basis,
                destination: staging,
                mode,
                state: request.progress.state(),
                base_bytes: request.base_bytes,
                allow_clone_seed: request.profile.capabilities.clone,
                resume,
                bandwidth: request.rule,
                reverify_staging: request.durability.verifies(),
            },
            &mut publish,
        )?;
        receipt(
            staging,
            stats.logical_bytes,
            mode.fast_path(),
            request.durability,
        )
    }
}

struct ProductionSparse;

impl SparseBackend for ProductionSparse {
    fn stage(&self, request: StageRequest<'_>) -> std::io::Result<StageReceipt> {
        let mut limiter = crate::transfer_tuning::BandwidthLimiter::new(request.rule);
        let bytes = super::sparse::copy_file_sparse(
            request.source,
            request.staging,
            request.progress.state(),
            &mut limiter,
        )?;
        receipt(request.staging, bytes, FastPath::Sparse, request.durability)
    }
}

struct ProductionBuffered;

impl BufferedBackend for ProductionBuffered {
    fn stage(
        &self,
        request: StageRequest<'_>,
        checkpoints: &mut dyn CheckpointSink,
    ) -> std::io::Result<StageReceipt> {
        let metadata = std::fs::symlink_metadata(request.source)?;
        if metadata.file_type().is_symlink() {
            return match request.symlink_policy {
                SymlinkPolicy::Preserve => {
                    crate::fs_util::copy_symlink(request.source, request.staging)?;
                    receipt(request.staging, 0, FastPath::Buffered, request.durability)
                }
                SymlinkPolicy::Follow => {
                    let followed = std::fs::canonicalize(request.source)?;
                    let metadata = std::fs::symlink_metadata(&followed)?;
                    self.stage(
                        StageRequest {
                            source: &followed,
                            is_dir: metadata.is_dir(),
                            ..request
                        },
                        checkpoints,
                    )
                }
                SymlinkPolicy::Skip => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "skipped symlink reached the copy backend",
                )),
            };
        }

        let mut limiter = crate::transfer_tuning::BandwidthLimiter::new(request.rule);
        let bytes = if request.is_dir {
            let workers = request.tuning.concurrency;
            if workers > 1
                && request.rule.max_bytes_per_second.is_none()
                && request.symlink_policy == SymlinkPolicy::Preserve
            {
                super::parallel_tree::copy_dir_buffered_parallel(
                    request.source,
                    request.staging,
                    request.progress.state(),
                    workers,
                    request.profile.capabilities.sparse,
                )
            } else {
                super::buffered::copy_dir_buffered_with_limiter(
                    request.source,
                    request.staging,
                    request.progress.state(),
                    &mut limiter,
                    request.profile.capabilities.sparse,
                    request.symlink_policy,
                )
            }?
        } else {
            super::buffered::copy_file_buffered_with_limiter(
                request.source,
                request.staging,
                request.progress.state(),
                &mut limiter,
                request.resume,
                Some(checkpoints),
                false,
            )?
        };
        receipt(
            request.staging,
            bytes,
            if request.resume.is_some() {
                FastPath::Resumed
            } else {
                FastPath::Buffered
            },
            request.durability,
        )
    }
}

pub(super) fn receipt(
    staging: &Path,
    bytes: u64,
    fast_path: FastPath,
    durability: DurabilityProfile,
) -> std::io::Result<StageReceipt> {
    let before_sync = PathIdentity::observe_deep(staging)?;
    let (artifact, durable) = if durability.verifies() {
        sync_staged_artifact(staging)?;
        let after_sync = PathIdentity::observe_deep(staging)?;
        if !before_sync.same_binding(&after_sync) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "staging artifact changed while durability was established",
            ));
        }
        (after_sync, true)
    } else {
        (before_sync, false)
    };
    Ok(StageReceipt {
        bytes,
        fast_path,
        artifact,
        durable,
    })
}

fn sync_staged_artifact(staging: &Path) -> std::io::Result<()> {
    let mut pending = vec![staging.to_path_buf()];
    let mut directories = Vec::new();
    while let Some(path) = pending.pop() {
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            directories.push(path.clone());
            let mut children = std::fs::read_dir(&path)?.collect::<Result<Vec<_>, _>>()?;
            children.sort_by_key(|entry| entry.file_name());
            pending.extend(children.into_iter().rev().map(|entry| entry.path()));
        } else if metadata.is_file() {
            sync_staged_file(&path)?;
        }
    }
    for directory in directories.into_iter().rev() {
        #[cfg(unix)]
        std::fs::File::open(&directory)?.sync_all()?;
        #[cfg(not(unix))]
        let _ = directory;
    }
    crate::fs_util::sync_parent_namespace(staging)
}

/// Flush a staged file's data. Windows' `FlushFileBuffers` needs a writable
/// handle, so a read-only open there always failed. A staged file that
/// inherited the source's read-only attribute refuses a writable open, so the
/// attribute is cleared for the sync and put back afterwards; a failed sync
/// still fails closed.
fn sync_staged_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(path)?.sync_data()
    }
    #[cfg(not(unix))]
    {
        let open_writable = || std::fs::OpenOptions::new().write(true).open(path);
        let refused = match open_writable() {
            Ok(file) => return file.sync_data(),
            Err(error) if error.kind() != std::io::ErrorKind::PermissionDenied => {
                return Err(error);
            }
            Err(error) => error,
        };
        let mut permissions = std::fs::metadata(path)?.permissions();
        if !permissions.readonly() {
            return Err(refused);
        }
        // Only reached off Unix, where this clears the read-only attribute
        // rather than granting world write.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(path, permissions.clone())?;
        let synced = open_writable().and_then(|file| file.sync_data());
        permissions.set_readonly(true);
        let _ = std::fs::set_permissions(path, permissions);
        synced
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input(method: CopyMethod) -> PlanInput {
        PlanInput {
            method,
            is_dir: false,
            is_symlink: false,
            source_size: Some(1024),
            basis_size: None,
            source_is_sparse: false,
            resume_layout: None,
            symlink_policy: SymlinkPolicy::Preserve,
            delta_capable: false,
            sparse_capable: false,
            resumable_capable: false,
            slow_link: false,
            bandwidth_limited: false,
            tuning: TuningSnapshot {
                concurrency: 1,
                p95_latency_ms: 0.0,
                samples: 0,
                ..TuningSnapshot::default()
            },
        }
    }

    #[test]
    #[allow(clippy::permissions_set_readonly_false)]
    fn staged_sync_handles_read_only_files_and_keeps_the_attribute() {
        // A read-only source makes a read-only staged file; on Windows the
        // sync used to fail for it because FlushFileBuffers needs a
        // writable handle.
        let tmp = crate::testutil::TempDir::new();
        let staging = tmp.dir("staged");
        let file = staging.join("object");
        std::fs::write(&file, b"packed").unwrap();
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions.clone()).unwrap();

        sync_staged_artifact(&staging).unwrap();

        assert!(std::fs::metadata(&file).unwrap().permissions().readonly());
        assert_eq!(std::fs::read(&file).unwrap(), b"packed");
        permissions.set_readonly(false);
        std::fs::set_permissions(&file, permissions).unwrap();
    }

    #[test]
    fn selector_matrix_preserves_native_and_buffered_forcing_rules() {
        let native = base_input(CopyMethod::Native);
        assert_eq!(
            BackendPlanner::select(native).unwrap(),
            BackendPlan::NativeClone
        );
        assert_eq!(
            BackendPlanner::select(PlanInput {
                bandwidth_limited: true,
                ..native
            })
            .unwrap(),
            BackendPlan::Buffered
        );
        for policy in [SymlinkPolicy::Follow, SymlinkPolicy::Skip] {
            assert_eq!(
                BackendPlanner::select(PlanInput {
                    is_dir: true,
                    source_size: None,
                    symlink_policy: policy,
                    ..native
                })
                .unwrap(),
                BackendPlan::Buffered,
                "{policy:?} folder"
            );
        }
        assert_eq!(
            BackendPlanner::select(base_input(CopyMethod::Buffered)).unwrap(),
            BackendPlan::Buffered
        );
    }

    #[test]
    fn selector_matrix_keeps_delta_sparse_and_resume_precedence() {
        let mut input = base_input(CopyMethod::Native);
        input.source_size = Some(crate::delta_copy::DELTA_MIN_BYTES);
        input.basis_size = input.source_size;
        input.delta_capable = true;
        input.tuning.p95_latency_ms = crate::delta_copy::DELTA_MIN_P95_MS;
        assert_eq!(
            BackendPlanner::select(input).unwrap(),
            BackendPlan::Delta(crate::delta_copy::DeltaMode::Fixed)
        );
        assert_eq!(
            BackendPlanner::select(PlanInput {
                resume_layout: Some(CheckpointLayout::DeltaFixed),
                bandwidth_limited: true,
                ..input
            })
            .unwrap(),
            BackendPlan::Delta(crate::delta_copy::DeltaMode::Fixed)
        );

        input.basis_size = None;
        input.delta_capable = false;
        input.source_is_sparse = true;
        input.sparse_capable = true;
        assert_eq!(BackendPlanner::select(input).unwrap(), BackendPlan::Sparse);

        input.resume_layout = Some(CheckpointLayout::Prefix);
        assert_eq!(
            BackendPlanner::select(input).unwrap(),
            BackendPlan::Buffered
        );
        input.resume_layout = Some(CheckpointLayout::DeltaFixed);
        assert_eq!(
            BackendPlanner::select(input).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );

        input.resume_layout = None;
        input.source_size = Some(crate::delta_copy::DELTA_MIN_BYTES);
        input.basis_size = None;
        input.delta_capable = false;
        input.source_is_sparse = true;
        input.sparse_capable = true;
        input.resumable_capable = true;
        input.slow_link = true;
        assert_eq!(
            BackendPlanner::select(input).unwrap(),
            BackendPlan::Buffered
        );
    }

    #[test]
    fn selector_matrix_routes_symlinks_without_touching_native_backends() {
        let input = PlanInput {
            is_symlink: true,
            source_is_sparse: true,
            sparse_capable: true,
            ..base_input(CopyMethod::Native)
        };
        assert_eq!(
            BackendPlanner::select(input).unwrap(),
            BackendPlan::Buffered
        );
        assert!(
            BackendPlanner::select(PlanInput {
                symlink_policy: SymlinkPolicy::Skip,
                ..input
            })
            .is_err()
        );
    }

    #[test]
    fn non_preserve_policies_keep_native_for_plain_files() {
        for policy in [SymlinkPolicy::Follow, SymlinkPolicy::Skip] {
            assert_eq!(
                BackendPlanner::select(PlanInput {
                    symlink_policy: policy,
                    ..base_input(CopyMethod::Native)
                })
                .unwrap(),
                BackendPlan::NativeClone,
                "{policy:?} file"
            );
        }
        assert_eq!(
            BackendPlanner::select(PlanInput {
                is_dir: true,
                source_size: None,
                ..base_input(CopyMethod::Native)
            })
            .unwrap(),
            BackendPlan::NativeClone
        );
    }

    struct NoCheckpoints;

    impl CheckpointSink for NoCheckpoints {
        fn publish(&mut self, _checkpoint: ResumeCheckpoint) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn buffered_folder_copy_uses_the_requested_worker_count() {
        let tmp = crate::testutil::TempDir::new();
        let source = tmp.dir("src");
        for index in 0..4 {
            tmp.file(&format!("src/{index}.txt"), "x");
        }
        let profile = crate::volume_profile::profile(tmp.path());
        let live = crate::transfer_tuning::snapshot(&profile);
        // Ask for a worker count the live snapshot would not pick.
        let requested = if live.concurrency > 1 { 1 } else { 3 };
        let state = Arc::new(std::sync::Mutex::new(super::super::TransferProgress::new(
            4, 1,
        )));
        let progress = BackendProgress::new(Arc::clone(&state));
        let staging = tmp.path().join("staging");

        ProductionBuffered
            .stage(
                StageRequest {
                    source: &source,
                    is_dir: true,
                    staging: &staging,
                    progress: &progress,
                    base_bytes: 0,
                    profile: &profile,
                    rule: VolumeRule::default(),
                    tuning: TuningSnapshot {
                        concurrency: requested,
                        ..live
                    },
                    basis: None,
                    resume: None,
                    symlink_policy: SymlinkPolicy::Preserve,
                    durability: DurabilityProfile::Fast,
                },
                &mut NoCheckpoints,
            )
            .unwrap();

        assert_eq!(crate::lock_util::recover(&state).peak_workers, requested);
        assert_eq!(std::fs::read_to_string(staging.join("3.txt")).unwrap(), "x");
    }

    #[test]
    fn verified_receipt_syncs_a_staged_tree() {
        let tmp = crate::testutil::TempDir::new();
        let staging = tmp.dir("staging");
        tmp.file("staging/a.txt", "a");
        tmp.file("staging/nested/b.txt", "b");
        let receipt =
            receipt(&staging, 2, FastPath::Buffered, DurabilityProfile::Verified).unwrap();
        assert!(receipt.durable);
        assert_eq!(receipt.bytes, 2);
    }
}
