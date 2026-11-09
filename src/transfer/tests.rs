use super::*;
use crate::testutil::TempDir;

fn hung_mount_guard() -> crate::mount_guard::MountGuard {
    crate::mount_guard::MountGuard {
        volume_id: 1,
        generation: 1,
        mount_point: PathBuf::from("/fixture"),
        policy: crate::mount_guard::ReconnectPolicy {
            timeout_ms: 50,
            poll_interval_ms: 5,
        },
    }
}

fn hung_probe() -> crate::mount_guard::MountAvailability {
    std::thread::sleep(std::time::Duration::from_secs(3_600));
    crate::mount_guard::MountAvailability::Available
}

#[test]
fn wait_for_mount_times_out_when_the_first_probe_hangs() {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let progress: TransferState = Arc::new(Mutex::new(TransferProgress::new(1, 1)));
        let paused = std::sync::atomic::AtomicBool::new(false);
        let result = wait_for_mount_probed(
            &hung_mount_guard(),
            hung_probe,
            "Source volume",
            &progress,
            &crate::workload::CancellationToken::new(),
            None,
            &|| {
                if crate::lock_util::recover(&progress).pause_reason.is_some() {
                    paused.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            },
        );
        let cleared = crate::lock_util::recover(&progress).pause_reason.is_none();
        let outcome = match result {
            Err(MountWaitError::Unavailable(error)) => Some(error.kind()),
            _ => None,
        };
        let _ = done.send((
            outcome,
            paused.load(std::sync::atomic::Ordering::SeqCst),
            cleared,
        ));
    });

    let (outcome, paused, cleared) = finished
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("a hung first probe blocked wait_for_mount");

    assert_eq!(outcome, Some(std::io::ErrorKind::TimedOut));
    assert!(paused, "the pause reason was never published");
    assert!(cleared, "the pause reason outlived the wait");
}

#[test]
fn wait_for_mount_honours_cancel_while_the_probe_hangs() {
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let progress: TransferState = Arc::new(Mutex::new(TransferProgress::new(1, 1)));
        let mut guard = hung_mount_guard();
        guard.policy.timeout_ms = 3_600_000;
        let result = wait_for_mount_probed(
            &guard,
            hung_probe,
            "Destination volume",
            &progress,
            &crate::workload::CancellationToken::new(),
            None,
            &|| crate::lock_util::recover(&progress).cancelled = true,
        );
        let _ = done.send(matches!(result, Err(MountWaitError::Interrupted)));
    });

    let interrupted = finished
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("a hung probe blocked cancel");
    assert!(interrupted);
}
use std::io::{Seek, SeekFrom, Write};
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
    spec.expectations =
        expectations_from_source_identities(&spec.entries, &spec.target, scan.source_identities)
            .unwrap();
    spec.preflight_bytes = Some(bytes);
}

/// Fresh progress state sized for `spec`'s entries.
fn progress_for(spec: &TransferSpec) -> TransferState {
    let total = total_bytes(&spec.entries);
    Arc::new(Mutex::new(TransferProgress::new(total, spec.entries.len())))
}

/// Poll `progress` until the worker finishes and return the final state.
fn wait_finished(progress: &TransferState) -> TransferProgress {
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

/// Run a transfer to completion and return the final progress state.
fn run(spec: TransferSpec) -> TransferProgress {
    let progress = progress_for(&spec);
    spawn_transfer(spec, progress.clone(), || {});
    wait_finished(&progress)
}

fn run_with_backends(spec: TransferSpec, backends: backend::BackendPorts) -> TransferProgress {
    let progress = progress_for(&spec);
    TransferExecutor::with_backends(
        crate::workload::global_handle(),
        spec,
        Arc::clone(&progress),
        || {},
        backends,
    )
    .submit();
    wait_finished(&progress)
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
    fn stage(&self, request: backend::StageRequest<'_>) -> std::io::Result<backend::StageReceipt> {
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
    fn stage(&self, request: backend::StageRequest<'_>) -> std::io::Result<backend::StageReceipt> {
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

fn fake_ports_without_durability(calls: Arc<Mutex<Vec<&'static str>>>) -> backend::BackendPorts {
    let fake = Arc::new(FakeStageBackend {
        calls,
        cancel_after_stage: false,
        durable_receipt: false,
    });
    backend::BackendPorts::new(fake.clone(), fake.clone(), fake.clone(), fake)
}

/// Native directory stage that leaves a successful sibling on disk, then
/// fails — the D23 residual the executor must not wipe away.
struct PartialDirNativeBackend;

impl backend::NativeCloneBackend for PartialDirNativeBackend {
    fn stage(&self, request: backend::StageRequest<'_>) -> std::io::Result<backend::StageReceipt> {
        assert!(
            request.is_dir,
            "partial-dir fixture stages directories only"
        );
        std::fs::create_dir(request.staging)?;
        std::fs::write(request.staging.join("good.txt"), "kept sibling")?;
        Err(std::io::Error::other("Failed to copy: bad.txt"))
    }
}

struct UnusedBackend(&'static str);

impl backend::DeltaBackend for UnusedBackend {
    fn stage(
        &self,
        _mode: crate::delta_copy::DeltaMode,
        _request: backend::StageRequest<'_>,
        _checkpoints: &mut dyn backend::CheckpointSink,
    ) -> std::io::Result<backend::StageReceipt> {
        unreachable!("{} backend must not be selected", self.0)
    }
}

impl backend::SparseBackend for UnusedBackend {
    fn stage(&self, _request: backend::StageRequest<'_>) -> std::io::Result<backend::StageReceipt> {
        unreachable!("{} backend must not be selected", self.0)
    }
}

impl backend::BufferedBackend for UnusedBackend {
    fn stage(
        &self,
        _request: backend::StageRequest<'_>,
        _checkpoints: &mut dyn backend::CheckpointSink,
    ) -> std::io::Result<backend::StageReceipt> {
        unreachable!("{} backend must not be selected", self.0)
    }
}

#[test]
fn native_dir_child_failure_preserves_successful_staging_siblings() {
    let (src, dst) = (TempDir::new(), TempDir::new());
    let dir = src.dir("folder");
    src.file("folder/good.txt", "kept sibling");
    src.file("folder/bad.txt", "will fail in the fake backend");

    let state = run_with_backends(
        spec(
            TransferKind::Copy,
            CopyMethod::Native,
            vec![entry_for(&dir)],
            dst.path(),
            vec![],
            OverwritePolicy::Ask,
        ),
        backend::BackendPorts::new(
            Arc::new(PartialDirNativeBackend),
            Arc::new(UnusedBackend("delta")),
            Arc::new(UnusedBackend("sparse")),
            Arc::new(UnusedBackend("buffered")),
        ),
    );

    assert!(!state.errors.is_empty(), "{:?}", state.errors);
    assert!(
        !dst.path().join("folder").exists(),
        "incomplete tree must not be placed at the landing name"
    );
    let staging = std::fs::read_dir(dst.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().contains(".cmdr-tmp."))
        })
        .expect("partial staging tree must be preserved");
    assert_eq!(
        std::fs::read_to_string(staging.join("good.txt")).unwrap(),
        "kept sibling",
        "successfully copied siblings must survive the child failure"
    );
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
        expectations: capture_expectations(&[entry_for(&file)], root.path()),
        durability: DurabilityProfile::Fast,
        post_success: Some(PostTransferAction::RemoveEmptyDir(folder.clone())),
        ..TransferSpec::new(
            TransferKind::Move,
            vec![entry_for(&file)],
            root.path().to_path_buf(),
        )
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
        durability: DurabilityProfile::Fast,
        post_success: Some(PostTransferAction::RemoveEmptyDir(folder.clone())),
        ..TransferSpec::new(TransferKind::Move, Vec::new(), root.path().to_path_buf())
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
        expectations: capture_expectations(&[entry_for(&file)], root.path()),
        durability: DurabilityProfile::Fast,
        post_success: Some(PostTransferAction::RemoveEmptyDir(folder.clone())),
        ..TransferSpec::new(
            TransferKind::Move,
            vec![entry_for(&file)],
            root.path().to_path_buf(),
        )
    });

    assert!(root.path().join("a.txt").is_file());
    assert!(folder.join("foreign.txt").is_file());
    assert!(state.errors.is_empty(), "{:?}", state.errors);
    assert!(state.finished);
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
        expectations: capture_expectations(&entries, target),
        policy,
        method,
        durability: DurabilityProfile::Fast,
        ..TransferSpec::new(kind, entries, target.to_path_buf())
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
    let plan = |profile: &crate::volume_profile::VolumeProfile, size: u64| {
        backend::BackendPlanner::select(backend::PlanInput {
            method: CopyMethod::Native,
            is_dir: false,
            is_symlink: false,
            source_size: Some(size),
            basis_size: None,
            source_is_sparse: false,
            resume_layout: None,
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::Preserve,
            delta_capable: false,
            sparse_capable: false,
            clone_same_volume: false,
            resumable_capable: profile.capabilities.resumable,
            slow_link: profile.backend.is_slow_link(),
            bandwidth_limited: false,
            tuning: crate::transfer_tuning::TuningSnapshot::default(),
        })
        .unwrap()
    };

    let threshold = crate::delta_copy::DELTA_MIN_BYTES;
    assert_eq!(plan(&profile, threshold), backend::BackendPlan::Buffered);
    assert_eq!(
        plan(&profile, threshold - 1),
        backend::BackendPlan::NativeClone
    );
    profile.capabilities.resumable = false;
    assert_eq!(plan(&profile, threshold), backend::BackendPlan::NativeClone);
    profile.capabilities.resumable = true;
    profile.backend = crate::volume_profile::BackendKind::LocalFast;
    assert_eq!(plan(&profile, threshold), backend::BackendPlan::NativeClone);
}

fn prefix_checkpoint_fixture(temp: &TempDir) -> (PathBuf, PathBuf, ResumeCheckpoint) {
    let source = temp.path().join("checkpoint-source.bin");
    let staging = temp.path().join(".checkpoint-source.bin.cmdr-tmp.0");
    std::fs::write(&source, b"0123456789abcdef").unwrap();
    std::fs::write(&staging, b"01234567").unwrap();
    let checkpoint = ResumeCheckpoint::observe(
        &source,
        &staging,
        8,
        CheckpointLayout::Prefix,
        prefix_digest(&staging, 8).unwrap(),
    )
    .unwrap();
    (source, staging, checkpoint)
}

#[test]
fn checkpoint_verify_accepts_a_prefix_staging_with_an_unproved_tail() {
    let temp = TempDir::new();
    let (source, staging, mut checkpoint) = prefix_checkpoint_fixture(&temp);
    std::fs::write(&staging, b"01234567crash").unwrap();

    assert_eq!(checkpoint.verify(&source, &staging), Ok(()));
    // Without file ids (Windows) the grown staging is still accepted: the
    // prefix digests prove the resumed bytes.
    checkpoint.partial.volume = None;
    checkpoint.partial.file_id = None;
    assert_eq!(checkpoint.verify(&source, &staging), Ok(()));
}

#[test]
fn checkpoint_verify_says_why_a_checkpoint_is_unusable() {
    let temp = TempDir::new();
    let (source, staging, checkpoint) = prefix_checkpoint_fixture(&temp);
    std::fs::write(&staging, b"0123XXXX").unwrap();
    assert_eq!(
        checkpoint.verify(&source, &staging),
        Err("checkpoint bytes changed after proof".to_string())
    );

    let (source, staging, checkpoint) = prefix_checkpoint_fixture(&temp);
    std::fs::write(&source, b"0123456789abcdef-grown").unwrap();
    assert_eq!(
        checkpoint.verify(&source, &staging),
        Err("source changed after checkpoint".to_string())
    );

    let (source, staging, checkpoint) = prefix_checkpoint_fixture(&temp);
    std::fs::write(&staging, b"0123").unwrap();
    assert_eq!(
        checkpoint.verify(&source, &staging),
        Err("staging changed after checkpoint".to_string())
    );

    let (source, staging, mut checkpoint) = prefix_checkpoint_fixture(&temp);
    checkpoint.content_digest = None;
    assert_eq!(
        checkpoint.verify(&source, &staging),
        Err("checkpoint has no content proof".to_string())
    );
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
    let checkpoint = ResumeCheckpoint::observe(
        &source,
        &staging,
        offset as u64,
        CheckpointLayout::Prefix,
        prefix_digest(&staging, offset as u64).unwrap(),
    )
    .unwrap();
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

    let error = copy_dir_buffered_parallel(&source, &destination, &progress, 2, false).unwrap_err();

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

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
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
fn failed_preflight_does_not_rescan_a_directory_for_progress() {
    let tmp = TempDir::new();
    let directory = tmp.dir("folder");
    tmp.file("folder/value.bin", "12345678");
    let entry = entry_for(&directory);
    let listing_size = entry.size;
    let failed = Err("source could not be inspected".to_string());

    assert_eq!(execution_entry_size(&entry, &failed), listing_size);
    assert_ne!(listing_size, fs_util::dir_size_recursive(&directory));
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
    let mut expectations =
        expectations_from_source_identities(&entries, target.path(), preserve.source_identities)
            .unwrap();
    let followed =
        crate::scan::transfer_preflight(&entries, crate::filesystem_policy::SymlinkPolicy::Follow);

    let failure = rebind_expectations_to_worker_scan(&mut expectations, followed.source_identities)
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

/// A journaled Running step with a verified 5-byte prefix checkpoint.
struct CheckpointFixture {
    _journal: crate::operation_journal::TestJournalPathGuard,
    temp: TempDir,
    operation_id: OperationId,
    key: IdempotencyKey,
    source: PathBuf,
    staging: PathBuf,
}

impl CheckpointFixture {
    fn new() -> Self {
        let temp = TempDir::new();
        let journal = crate::operation_journal::use_test_journal(temp.path().join("journal.json"));
        let target = temp.dir("target");
        // Canonical, so a Follow resume resolves to the recorded path even
        // when the temp root sits behind a symlink (macOS /tmp).
        let source = std::fs::canonicalize(temp.file("source.bin", "0123456789")).unwrap();
        let staging = temp.file("target/.source.bin.cmdr-tmp.0", "01234");
        let spec = TransferSpec::new(TransferKind::Copy, vec![entry_for(&source)], target);
        crate::operation_journal::begin(&spec).unwrap();
        let operation_id = spec.operation_id.clone();
        let key = crate::operation_journal::operation(&operation_id)
            .unwrap()
            .steps[0]
            .key
            .clone();
        let landing = spec.target.join("source.bin");
        crate::operation_journal::mark_running(
            &operation_id,
            &key,
            &staging,
            &landing,
            PathIdentity::missing(&landing),
        )
        .unwrap();
        let checkpoint = ResumeCheckpoint::observe(
            &source,
            &staging,
            5,
            CheckpointLayout::Prefix,
            prefix_digest(&staging, 5).unwrap(),
        )
        .unwrap();
        crate::operation_journal::mark_checkpoint(&operation_id, &key, checkpoint).unwrap();
        Self {
            _journal: journal,
            temp,
            operation_id,
            key,
            source,
            staging,
        }
    }

    fn check(
        &self,
        source: &Path,
        staging: &Path,
        policy: crate::filesystem_policy::SymlinkPolicy,
    ) -> Option<ResumeCheckpoint> {
        valid_runtime_checkpoint(&self.operation_id, &self.key, source, staging, policy, true)
    }

    fn check_default(&self) -> Option<ResumeCheckpoint> {
        self.check(
            &self.source,
            &self.staging,
            crate::filesystem_policy::SymlinkPolicy::Preserve,
        )
    }
}

#[test]
fn runtime_checkpoint_accepts_an_untouched_verified_prefix() {
    let fixture = CheckpointFixture::new();
    let checkpoint = fixture.check_default().expect("valid checkpoint rejected");
    assert_eq!(checkpoint.offset, 5);
    assert_eq!(checkpoint.staging, fixture.staging);
    assert!(
        valid_runtime_checkpoint(
            &fixture.operation_id,
            &fixture.key,
            &fixture.source,
            &fixture.staging,
            crate::filesystem_policy::SymlinkPolicy::Preserve,
            false,
        )
        .is_none(),
        "a transfer without a journal must never resume"
    );
}

#[test]
fn runtime_checkpoint_rejects_unknown_steps_and_other_staging_paths() {
    let fixture = CheckpointFixture::new();
    let other = fixture.temp.file("target/.other.cmdr-tmp.0", "01234");
    assert!(
        fixture
            .check(
                &fixture.source,
                &other,
                crate::filesystem_policy::SymlinkPolicy::Preserve
            )
            .is_none()
    );
    assert!(
        valid_runtime_checkpoint(
            &OperationId::new(),
            &fixture.key,
            &fixture.source,
            &fixture.staging,
            crate::filesystem_policy::SymlinkPolicy::Preserve,
            true,
        )
        .is_none()
    );
}

#[test]
fn runtime_checkpoint_rejects_a_changed_source() {
    let fixture = CheckpointFixture::new();
    std::fs::write(&fixture.source, "0123456789abcdef").unwrap();
    assert!(fixture.check_default().is_none());
}

#[test]
fn runtime_checkpoint_rejects_a_replaced_or_non_file_partial() {
    {
        let fixture = CheckpointFixture::new();
        // Same bytes, new object: created before the swap so the inode
        // cannot be recycled.
        let replacement = fixture.temp.file("target/replacement", "01234");
        std::fs::rename(&replacement, &fixture.staging).unwrap();
        assert!(fixture.check_default().is_none());
    }

    // Each fixture holds the test-journal lock, so the first must drop.
    let fixture = CheckpointFixture::new();
    std::fs::remove_file(&fixture.staging).unwrap();
    std::fs::create_dir(&fixture.staging).unwrap();
    assert!(fixture.check_default().is_none());
}

#[test]
fn runtime_checkpoint_rejects_partial_bytes_that_no_longer_match() {
    let fixture = CheckpointFixture::new();
    let mut partial = std::fs::OpenOptions::new()
        .write(true)
        .open(&fixture.staging)
        .unwrap();
    partial.write_all(b"x").unwrap();
    drop(partial);
    assert!(fixture.check_default().is_none());
}

#[cfg(unix)]
#[test]
fn runtime_checkpoint_follows_a_symlinked_source_only_under_follow() {
    let fixture = CheckpointFixture::new();
    let link = fixture.temp.path().join("link.bin");
    std::os::unix::fs::symlink(&fixture.source, &link).unwrap();
    assert!(
        fixture
            .check(
                &link,
                &fixture.staging,
                crate::filesystem_policy::SymlinkPolicy::Follow
            )
            .is_some()
    );
    assert!(
        fixture
            .check(
                &link,
                &fixture.staging,
                crate::filesystem_policy::SymlinkPolicy::Preserve
            )
            .is_none()
    );
}

#[test]
fn entries_completed_without_copying_add_no_throughput() {
    let progress: TransferState = Arc::new(Mutex::new(TransferProgress::new(20_000, 2)));
    {
        let mut state = progress.lock().unwrap();
        state.speed_samples = vec![(0.0, 0.0), (4.0, 400.0)];
        state.copied_bytes = 600;
    }
    let mut completed = 600;
    complete_without_copy(
        &progress,
        &mut completed,
        10_000,
        "skipped",
        None,
        None,
        &|| {},
    );

    let state = progress.lock().unwrap();
    assert_eq!(completed, 10_600);
    assert_eq!(state.copied_bytes, 10_600);
    assert_eq!(state.files_done, 1);
    // Still the 100 B/s actually copied, not a 5 KB/s spike.
    assert!((state.speed_bps_at(6.0) - 100.0).abs() < 1e-9);
}

#[test]
fn step_failure_reports_its_cause_before_a_journal_failure() {
    let temp = TempDir::new();
    let _journal = crate::operation_journal::use_test_journal(temp.path().join("journal.json"));
    let progress: TransferState = Arc::new(Mutex::new(TransferProgress::new(0, 1)));
    record_step_failure(
        &progress,
        &OperationId::new(),
        &IdempotencyKey("unknown-step".to_string()),
        "a.txt",
        ClassifiedFailure::message(FailureClass::Blocked, None, "disk full".to_string()),
        true,
    );

    let state = progress.lock().unwrap();
    assert_eq!(state.errors.len(), 2, "{:?}", state.errors);
    assert_eq!(state.errors[0], "a.txt: disk full");
    assert!(
        state.errors[1].contains("operation journal update failed"),
        "{:?}",
        state.errors
    );
    assert_eq!(state.failures[0].class, FailureClass::Blocked);
}

#[test]
fn a_volume_replaced_before_any_wait_is_not_reported_as_paused() {
    let progress: TransferState = Arc::new(Mutex::new(TransferProgress::new(1, 1)));
    // A generous poll interval keeps a slow thread spawn on a loaded machine
    // from timing out the first probe and reporting Disconnected (which
    // would set the paused flag before Replaced arrives).
    let guard = crate::mount_guard::MountGuard {
        policy: crate::mount_guard::ReconnectPolicy {
            timeout_ms: 10_000,
            poll_interval_ms: 1_000,
        },
        ..hung_mount_guard()
    };
    let result = wait_for_mount_probed(
        &guard,
        || crate::mount_guard::MountAvailability::Replaced,
        "Destination volume",
        &progress,
        &crate::workload::CancellationToken::new(),
        None,
        &|| {},
    );

    let Err(MountWaitError::Unavailable(error)) = result else {
        panic!("a replaced volume must fail the wait");
    };
    assert_eq!(error.kind(), std::io::ErrorKind::StaleNetworkFileHandle);
    let message = error.to_string();
    assert!(message.contains("Destination volume"), "{message}");
    assert!(!message.contains("paused"), "{message}");
    assert!(crate::lock_util::recover(&progress).pause_reason.is_none());
}

#[test]
fn cancel_is_offered_only_until_finalization_starts() {
    let mut progress = TransferProgress::new(1, 1);
    assert!(progress.can_cancel());
    progress.finalization = Some(FinalizationOutcome::NotReached);
    assert!(!progress.can_cancel());
    assert!(!progress.request_cancel());
    progress.request_stop();
    assert!(!progress.cancelled && !progress.stop_requested);

    let mut finished = TransferProgress::new(1, 1);
    finished.finished = true;
    assert!(!finished.can_cancel());
}

#[test]
fn known_byte_total_drives_progress_and_clamps_overshoot() {
    let mut progress = TransferProgress::new(1_000, 4);
    progress.files_done = 4;
    assert_eq!(progress.progress_fraction(), Some(0.0));
    progress.copied_bytes = 500;
    assert_eq!(progress.progress_fraction(), Some(0.5));
    progress.copied_bytes = 1_500;
    assert_eq!(progress.progress_fraction(), Some(1.0));
}

#[test]
fn disconnect_classification_covers_only_transport_failures() {
    use std::io::ErrorKind;
    for kind in [
        ErrorKind::NotFound,
        ErrorKind::TimedOut,
        ErrorKind::ConnectionAborted,
        ErrorKind::ConnectionReset,
        ErrorKind::BrokenPipe,
        ErrorKind::UnexpectedEof,
        ErrorKind::NetworkDown,
        ErrorKind::NotConnected,
        ErrorKind::HostUnreachable,
        ErrorKind::StaleNetworkFileHandle,
    ] {
        assert!(
            is_disconnect_error(&std::io::Error::from(kind)),
            "{kind:?} must wait for the mount"
        );
    }
    for kind in [
        ErrorKind::PermissionDenied,
        ErrorKind::StorageFull,
        ErrorKind::Interrupted,
        ErrorKind::AlreadyExists,
        ErrorKind::InvalidData,
        ErrorKind::Other,
    ] {
        assert!(
            !is_disconnect_error(&std::io::Error::from(kind)),
            "{kind:?} must not be retried as a disconnect"
        );
    }
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

#[cfg(unix)]
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

#[cfg(unix)]
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

#[test]
fn verification_accepts_identical_trees() {
    let (src, dst) = (TempDir::new(), TempDir::new());
    src.dir("tree/sub");
    src.file("tree/sub/a.txt", "same");
    dst.dir("tree/sub");
    dst.file("tree/sub/a.txt", "same");
    assert!(transfer_paths_equal(
        &src.path().join("tree"),
        &dst.path().join("tree"),
        crate::filesystem_policy::SymlinkPolicy::Preserve,
    ));
}

#[test]
fn verification_rejects_a_file_where_a_directory_was_copied() {
    let (src, dst) = (TempDir::new(), TempDir::new());
    src.dir("item");
    dst.file("item", "");
    let policy = crate::filesystem_policy::SymlinkPolicy::Preserve;
    assert!(!transfer_paths_equal(
        &src.path().join("item"),
        &dst.path().join("item"),
        policy,
    ));
    assert!(!transfer_paths_equal(
        &dst.path().join("item"),
        &src.path().join("item"),
        policy,
    ));
}

#[test]
fn verification_rejects_an_extra_destination_child() {
    let (src, dst) = (TempDir::new(), TempDir::new());
    src.dir("tree");
    src.file("tree/a.txt", "a");
    dst.dir("tree");
    dst.file("tree/a.txt", "a");
    dst.file("tree/stray.txt", "");
    assert!(!transfer_paths_equal(
        &src.path().join("tree"),
        &dst.path().join("tree"),
        crate::filesystem_policy::SymlinkPolicy::Preserve,
    ));
}

#[test]
fn verification_rejects_differing_file_contents() {
    let (src, dst) = (TempDir::new(), TempDir::new());
    let source = src.file("a.txt", "abcd");
    let destination = dst.file("a.txt", "abce");
    assert!(!transfer_paths_equal(
        &source,
        &destination,
        crate::filesystem_policy::SymlinkPolicy::Preserve,
    ));
}

#[test]
fn verification_failure_names_the_differing_entry_and_reason() {
    let (src, dst) = (TempDir::new(), TempDir::new());
    src.file("tree/sub/a.txt", "abcd");
    dst.file("tree/sub/a.txt", "abce");
    let policy = crate::filesystem_policy::SymlinkPolicy::Preserve;
    let (source, destination) = (src.path().join("tree"), dst.path().join("tree"));
    let mismatch = transfer_paths_mismatch(&source, &destination, policy).unwrap();
    assert_eq!(
        mismatch,
        format!(
            "{}: contents differ",
            Path::new("sub").join("a.txt").display()
        )
    );

    dst.file("tree/sub/stray.txt", "");
    assert_eq!(
        transfer_paths_mismatch(&source, &destination, policy).unwrap(),
        "sub: folder contents differ"
    );

    let file = src.file("one.txt", "x");
    assert_eq!(
        transfer_paths_mismatch(&file, &dst.path().join("missing.txt"), policy).unwrap(),
        "copy is missing"
    );
}

fn names(names: &[&str]) -> Vec<std::ffi::OsString> {
    names.iter().map(std::ffi::OsString::from).collect()
}

#[test]
fn child_pairing_tolerates_another_unicode_normalization_only() {
    let nfc = "caf\u{e9}.txt";
    let nfd = "cafe\u{301}.txt";
    let pairs = pair_child_names(names(&["a", nfc]), names(&["a", nfd])).unwrap();
    assert!(pairs.contains(&(nfc.into(), nfd.into())));
    assert!(pairs.contains(&("a".into(), "a".into())));

    assert!(pair_child_names(names(&["a", nfc]), names(&["a", "cafe.txt"])).is_none());
    assert!(pair_child_names(names(&["a"]), names(&["a", "b"])).is_none());
    // Two destination names folding to one source name stay ambiguous.
    assert!(pair_child_names(names(&[nfc, "z"]), names(&[nfc, nfd])).is_none());
}

#[test]
fn apple_double_companions_are_dropped_only_for_copied_names() {
    let source = names(&["a.txt", "._b.txt"]);
    let destination = names(&["._a.txt", "._b.txt", "._c.txt", "a.txt"]);
    let destination = drop_apple_double_companions(&source, destination);
    assert_eq!(destination, names(&["._b.txt", "._c.txt", "a.txt"]));
}

#[test]
fn verification_pairs_children_stored_in_another_normalization() {
    let (src, dst) = (TempDir::new(), TempDir::new());
    src.file("tree/caf\u{e9}.txt", "same");
    dst.file("tree/cafe\u{301}.txt", "same");
    let policy = crate::filesystem_policy::SymlinkPolicy::Preserve;
    let (source, destination) = (src.path().join("tree"), dst.path().join("tree"));
    assert!(transfer_paths_equal(&source, &destination, policy));
    std::fs::write(dst.path().join("tree/cafe\u{301}.txt"), "diff").unwrap();
    assert!(!transfer_paths_equal(&source, &destination, policy));
}

#[cfg(unix)]
#[test]
fn verification_rejects_a_preserved_link_with_another_target() {
    let (src, dst) = (TempDir::new(), TempDir::new());
    std::os::unix::fs::symlink("one", src.path().join("link")).unwrap();
    std::os::unix::fs::symlink("two", dst.path().join("link")).unwrap();
    std::os::unix::fs::symlink("one", dst.path().join("same")).unwrap();
    let policy = crate::filesystem_policy::SymlinkPolicy::Preserve;
    assert!(!transfer_paths_equal(
        &src.path().join("link"),
        &dst.path().join("link"),
        policy,
    ));
    assert!(transfer_paths_equal(
        &src.path().join("link"),
        &dst.path().join("same"),
        policy,
    ));
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

#[cfg(unix)]
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
