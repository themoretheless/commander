use super::*;
use crate::testutil::TempDir;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Barrier, Mutex, mpsc};

struct ScriptedTrashPort {
    outcomes: Mutex<VecDeque<crate::ports::TrashItemOutcome>>,
    calls: Mutex<Vec<PathBuf>>,
}

/// Trashes one item at a time, each only after the test releases it, so the
/// worker's progress is deterministic while the test inspects it.
struct GatedTrashPort {
    release: Mutex<mpsc::Receiver<()>>,
}

impl crate::ports::TrashPort for GatedTrashPort {
    fn move_to_trash(&self, target: &crate::ports::TrashTarget) -> crate::ports::TrashItemOutcome {
        self.release
            .lock()
            .unwrap()
            .recv()
            .expect("test releases the Trash call");
        let _ = std::fs::remove_file(&target.path);
        crate::ports::TrashItemOutcome::Trashed
    }
}

struct FixedFreeSpacePort {
    outcome: crate::ports::SpaceProbeOutcome,
    relation: crate::ports::VolumeRelation,
}

struct InjectedPersistence {
    bytes: Mutex<Vec<u8>>,
    commits: AtomicUsize,
}

impl crate::persistence::Persist for InjectedPersistence {
    fn read(
        &self,
        _path: &Path,
        max_bytes: usize,
    ) -> Result<crate::persistence::ReadOutcome, crate::persistence::ReadFailure> {
        let bytes = self.bytes.lock().unwrap().clone();
        assert!(bytes.len() <= max_bytes);
        let revision = crate::persistence::Revision::from_bytes(&bytes);
        Ok(crate::persistence::ReadOutcome::Present { bytes, revision })
    }

    fn commit(
        &self,
        _path: &Path,
        bytes: &[u8],
        _expected: crate::persistence::ExpectedRevision,
    ) -> Result<crate::persistence::AtomicWriteOutcome, crate::persistence::PreCommitError> {
        self.commits.fetch_add(1, Ordering::Relaxed);
        *self.bytes.lock().unwrap() = bytes.to_vec();
        Ok(crate::persistence::AtomicWriteOutcome::Durable)
    }
}

impl crate::ports::FreeSpacePort for FixedFreeSpacePort {
    fn probe(&self, _path: &Path) -> crate::ports::SpaceProbeOutcome {
        self.outcome.clone()
    }

    fn volume_relation(&self, _source: &Path, _target: &Path) -> crate::ports::VolumeRelation {
        self.relation.clone()
    }
}

impl ScriptedTrashPort {
    fn new(outcomes: impl IntoIterator<Item = crate::ports::TrashItemOutcome>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl crate::ports::TrashPort for ScriptedTrashPort {
    fn move_to_trash(&self, target: &crate::ports::TrashTarget) -> crate::ports::TrashItemOutcome {
        self.calls.lock().unwrap().push(target.path.clone());
        let outcome = self
            .outcomes
            .lock()
            .unwrap()
            .pop_front()
            .expect("scripted Trash outcome");
        if outcome == crate::ports::TrashItemOutcome::Trashed {
            let _ = if target.path.is_dir() {
                std::fs::remove_dir_all(&target.path)
            } else {
                std::fs::remove_file(&target.path)
            };
        }
        outcome
    }
}

fn ready_trash_item(path: &Path) -> crate::ports::TrashBatchItem {
    crate::ports::TrashBatchItem::Ready(crate::ports::TrashTarget {
        path: path.to_path_buf(),
        expected: crate::path_identity::PathIdentity::observe(path).unwrap(),
    })
}

fn workspace_with_trash(
    left: &TempDir,
    right: &TempDir,
    trash: Arc<impl crate::ports::TrashPort + 'static>,
) -> Workspace {
    let mut workspace = Workspace::with_ports(
        left.path().to_path_buf(),
        right.path().to_path_buf(),
        trash,
        Arc::new(TestFreeSpacePort),
    );
    workspace.left.refresh();
    workspace.right.refresh();
    workspace
}

fn workspace(left: &TempDir, right: &TempDir) -> Workspace {
    let mut ws = Workspace::new(left.path().to_path_buf(), right.path().to_path_buf());
    ws.left.refresh();
    ws.right.refresh();
    ws
}

#[test]
fn workspace_uses_the_injected_persistence_for_bookmark_load_and_save() {
    let left = TempDir::new();
    let right = TempDir::new();
    let persistence = Arc::new(InjectedPersistence {
        bytes: Mutex::new(
            br#"{"items":[{"name":"Injected","path":"/injected","slot":3}]}"#.to_vec(),
        ),
        commits: AtomicUsize::new(0),
    });
    let mut workspace = Workspace::with_ports_and_views(
        left.path().to_path_buf(),
        right.path().to_path_buf(),
        [crate::panel::ViewConfig::default(); 2],
        Arc::new(TestTrashPort),
        Arc::new(TestFreeSpacePort),
        persistence.clone(),
    );

    assert_eq!(workspace.bookmarks.items[0].name, "Injected");
    workspace.bookmarks.add("Saved", "/saved");
    workspace.save_bookmarks();

    assert_eq!(persistence.commits.load(Ordering::Relaxed), 1);
    let written: serde_json::Value =
        serde_json::from_slice(&persistence.bytes.lock().unwrap()).unwrap();
    assert_eq!(written["store"], "commander.bookmarks");
    assert_eq!(written["payload"]["items"].as_array().unwrap().len(), 2);
}

fn test_transfer_spec(operation_id: &str, target: &Path) -> TransferSpec {
    TransferSpec {
        operation_id: crate::operation::OperationId(operation_id.to_string()),
        group_id: None,
        kind: TransferKind::Move,
        entries: Vec::new(),
        expectations: Vec::new(),
        target: target.to_path_buf(),
        policy: OverwritePolicy::Ask,
        method: CopyMethod::Native,
        durability: crate::operation::DurabilityProfile::default(),
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

fn interrupt_history_replay(
    workspace: &mut Workspace,
    operation_id: &crate::operation::OperationId,
    target: &Path,
    placements: Vec<(PathBuf, PathBuf)>,
) {
    workspace.record_history_for_test(crate::undo::Action::Rename {
        from: target.join("history-old"),
        to: target.join("history-new"),
    });
    let replay = workspace.begin_history_replay_for_test(crate::undo::ReplayDirection::Undo);
    let progress = workspace.launch_test_transfer(
        test_transfer_spec(&operation_id.0, target),
        transfer_queue::HistoryIntent::replay(replay.reservation),
    );
    {
        let mut progress = crate::lock_util::recover(&progress);
        progress.finished = true;
        progress.errors.push("interrupted replay".to_string());
        progress.placements = placements;
    }
    assert!(workspace.poll_transfer(|| {}).terminal.is_none());
    workspace.dismiss_transfer(|| {});
}

fn assert_mount_wait_interruption(label: &str, cancel: bool, reconnect_wins: bool) {
    let journal = TempDir::new();
    let _journal = crate::operation_journal::use_test_journal(journal.path().join("journal.json"));
    let (left, right) = (TempDir::new(), TempDir::new());
    let source = left.file("mount-race.txt", "payload");
    let entry =
        FileEntry::from_meta(source.clone(), &std::fs::symlink_metadata(&source).unwrap()).unwrap();
    let operation_id = crate::operation::OperationId::new();
    let mut spec = test_transfer_spec(&operation_id.0, right.path());
    spec.kind = TransferKind::Copy;
    spec.entries = vec![entry.clone()];
    spec.expectations = transfer::capture_expectations(std::slice::from_ref(&entry), right.path());
    spec.journal_enabled = true;

    let calls = Arc::new(AtomicUsize::new(0));
    let wait_calls = Arc::clone(&calls);
    let barrier = Arc::new(Barrier::new(2));
    let wait_barrier = Arc::clone(&barrier);
    let (waiting_tx, waiting_rx) = mpsc::sync_channel(1);
    spec.mount_wait_override = Some(Arc::new(move || {
        match wait_calls.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(()), // initial operation-level mount check
            1 => {
                let _ = waiting_tx.send(());
                wait_barrier.wait();
                if reconnect_wins {
                    Ok(())
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "user interrupted deterministic mount wait",
                    ))
                }
            }
            call => panic!("unexpected mount wait call {call}"),
        }
    }));

    let workload =
        crate::workload::DeterministicWorkload::new(crate::workload::SchedulerLimits::default());
    let mut ws = workspace(&left, &right);
    ws.enqueue_with_history(spec, transfer_queue::HistoryIntent::None);
    ws.pump_queue_with_workload(workload.handle(), || {});
    let progress = ws
        .active_transfer_view()
        .expect("mount-wait transfer is active")
        .progress;
    let runner = workload.clone();
    let worker = std::thread::spawn(move || {
        assert!(runner.run_next());
    });
    waiting_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("worker reached per-entry mount wait");

    if cancel {
        ws.cancel_transfer();
    } else {
        ws.stop_transfer_after_current();
    }
    barrier.wait();
    worker.join().expect("deterministic worker");

    let state = crate::lock_util::recover(&progress);
    assert!(state.finished, "{label}");
    assert_eq!(state.cancelled, cancel, "{label}");
    assert_eq!(state.stopped, !cancel, "{label}");
    assert_eq!(state.files_done, 0, "{label}");
    assert_eq!(state.copied_bytes, 0, "{label}");
    assert!(state.errors.is_empty(), "{label}: {:?}", state.errors);
    assert!(state.failures.is_empty(), "{label}: {:?}", state.failures);
    drop(state);

    assert!(source.is_file(), "{label}: source was mutated");
    assert!(
        !right.path().join("mount-race.txt").exists(),
        "{label}: destination was mutated"
    );
    let operation =
        crate::operation_journal::operation(&operation_id).expect("interrupted journal");
    assert_eq!(
        operation.status,
        crate::operation_journal::OperationStatus::Stopped,
        "{label}"
    );
    assert_eq!(operation.steps.len(), 1, "{label}");
    assert_eq!(
        operation.steps[0].status,
        crate::operation_journal::StepStatus::Planned,
        "{label}"
    );
    assert_eq!(operation.steps[0].attempts, 0, "{label}");
    assert!(operation.steps[0].failure.is_none(), "{label}");

    let report = ws
        .poll_transfer(|| {})
        .terminal
        .expect("interrupted terminal report");
    assert_eq!(
        report.terminal,
        if cancel {
            TransferTerminalState::Cancelled
        } else {
            TransferTerminalState::Stopped
        },
        "{label}"
    );
    assert!(
        ws.poll_transfer(|| {}).terminal.is_none(),
        "{label}: terminal report repeated"
    );
}

#[test]
fn mount_reconnect_does_not_beat_cancel_or_stop() {
    assert_mount_wait_interruption("cancel after reconnect", true, true);
    assert_mount_wait_interruption("stop after reconnect", false, true);
}

#[test]
fn user_interrupted_mount_wait_does_not_complete_or_fail_step() {
    assert_mount_wait_interruption("cancelled Interrupted", true, false);
    assert_mount_wait_interruption("stopped Interrupted", false, false);
}

#[test]
fn command_context_counts_only_actionable_filtered_selection() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let file = left.file("report.txt", "data");
    let folder = left.dir("Archive");
    let mut ws = workspace(&left, &right);

    ws.left.select_path(file.clone());
    let cursor = ws
        .left
        .filtered_entries()
        .iter()
        .position(|entry| entry.path == folder)
        .unwrap()
        + 1;
    ws.left.set_cursor(cursor);
    let context = ws.command_context();
    let action_bar = ws.action_bar_command_context();
    assert_eq!(context.visible_entries, 2);
    assert_eq!(context.selected_entries, 1);
    assert_eq!(context.picked_entries, 1);
    assert!(context.cursor_is_dir);
    assert!(context.can_transfer_into_cursor_folder);
    assert_eq!(action_bar.selected_entries, 1);
    assert!(action_bar.can_transfer_into_cursor_folder);

    ws.left.set_search_query("does-not-match");
    let filtered = ws.command_context();
    assert_eq!(filtered.visible_entries, 0);
    assert_eq!(filtered.selected_entries, 0);
    assert_eq!(filtered.picked_entries, 0);
    assert_eq!(filtered.listing_entries, 0);
    assert_eq!(ws.action_bar_command_context().picked_entries, 0);
}

fn apply_batch_rename(
    ws: &mut Workspace,
    rule: &crate::rename::RenameRule,
) -> Result<usize, String> {
    let context = ws
        .batch_rename_context()
        .ok_or("Nothing selected to rename")?;
    ws.apply_batch_rename_in(&context, rule)
}

fn apply_sync(ws: &mut Workspace, actions: &[crate::sync::SyncAction]) {
    let left_dir = ws.left.current_path.clone();
    let right_dir = ws.right.current_path.clone();
    ws.apply_sync_between(actions, &left_dir, &right_dir, || {});
}

fn wait_transfer(ws: &mut Workspace) {
    if ws.active_transfer().is_none() && matches!(ws.pending_op, Some(PendingOp::Transfer(_))) {
        ws.finish_space_probe();
        if ws.active_transfer().is_none() {
            ws.start_transfer(|| {});
        }
    }
    let state = ws
        .active_transfer()
        .cloned()
        .expect("transfer should be running");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !state.lock().unwrap().finished {
        assert!(std::time::Instant::now() < deadline, "transfer timed out");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    ws.poll_transfer(|| {});
}

/// Drive the queue to completion: wait out the active transfer and any jobs
/// queued behind it, polling between each.
fn drain_transfers(ws: &mut Workspace) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while ws.active_transfer().is_some() || matches!(ws.pending_op, Some(PendingOp::Transfer(_))) {
        wait_transfer(ws);
        assert!(
            std::time::Instant::now() < deadline,
            "queue drain timed out"
        );
    }
}

#[test]
fn terminal_publication_waits_for_journal_finalization() {
    let journal = TempDir::new();
    let _journal = crate::operation_journal::use_test_journal(journal.path().join("journal.json"));
    let (left, right) = (TempDir::new(), TempDir::new());
    let source = left.file("boundary.txt", "boundary");
    let entry =
        FileEntry::from_meta(source.clone(), &std::fs::symlink_metadata(&source).unwrap()).unwrap();
    let operation_id = crate::operation::OperationId::new();
    let mut spec = test_transfer_spec(&operation_id.0, right.path());
    spec.kind = TransferKind::Copy;
    spec.expectations = transfer::capture_expectations(std::slice::from_ref(&entry), right.path());
    spec.entries = vec![entry];
    spec.journal_enabled = true;

    let barrier = Arc::new(Barrier::new(2));
    let worker_barrier = Arc::clone(&barrier);
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    spec.before_terminal_publish = Some(Arc::new(move || {
        let _ = entered_tx.send(());
        worker_barrier.wait();
    }));

    let workload =
        crate::workload::DeterministicWorkload::new(crate::workload::SchedulerLimits::default());
    let mut ws = workspace(&left, &right);
    ws.enqueue_with_history(spec, transfer_queue::HistoryIntent::None);
    ws.pump_queue_with_workload(workload.handle(), || {});
    let progress = ws
        .active_transfer_view()
        .expect("boundary transfer active")
        .progress;
    let runner = workload.clone();
    let worker = std::thread::spawn(move || {
        assert!(runner.run_next());
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("worker reached terminal publication barrier");

    let before = ws.poll_transfer(|| {});
    let journal_is_terminal = crate::operation_journal::operation(&operation_id)
        .map(|operation| operation.status.is_terminal())
        .unwrap_or(false);
    let progress_is_terminal = crate::lock_util::recover(&progress).finished;
    ws.cancel_transfer();
    let late_cancelled = crate::lock_util::recover(&progress).cancelled;
    barrier.wait();
    worker.join().expect("deterministic worker");

    assert!(journal_is_terminal, "journal finalized before the barrier");
    assert!(!progress_is_terminal, "finished leaked before finalization");
    assert!(
        !late_cancelled,
        "completed finalization rejects a late cancellation"
    );
    assert!(before.terminal.is_none(), "poll detached a live worker");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !crate::lock_util::recover(&progress).finished {
        assert!(std::time::Instant::now() < deadline, "transfer timed out");
        std::thread::yield_now();
    }
    assert_eq!(
        ws.poll_transfer(|| {})
            .terminal
            .expect("clean terminal report")
            .terminal,
        TransferTerminalState::Done
    );
}

#[test]
fn zero_entry_recovery_stop_before_worker_keeps_required_cleanup_unfinished() {
    let journal = TempDir::new();
    let _journal = crate::operation_journal::use_test_journal(journal.path().join("journal.json"));
    let (left, right) = (TempDir::new(), TempDir::new());
    let cleanup = left.dir("recovery-cleanup");
    let operation_id = crate::operation::OperationId::new();
    let mut spec = test_transfer_spec(&operation_id.0, right.path());
    spec.post_success = Some(PostTransferAction::RemoveEmptyDir(cleanup.clone()));
    spec.journal_enabled = true;
    let workload =
        crate::workload::DeterministicWorkload::new(crate::workload::SchedulerLimits::default());
    let mut ws = workspace(&left, &right);
    ws.enqueue_with_history(spec, transfer_queue::HistoryIntent::None);
    ws.pump_queue_with_workload(workload.handle(), || {});
    let progress = ws
        .active_transfer_view()
        .expect("zero-entry recovery is active")
        .progress;

    ws.stop_transfer_after_current();
    assert!(workload.run_next());

    let state = crate::lock_util::recover(&progress);
    assert!(state.finished);
    assert!(state.stopped);
    assert!(state.stop_requested);
    drop(state);
    assert!(
        cleanup.exists(),
        "a stopped recovery must not pretend required cleanup completed"
    );
    assert_eq!(
        crate::operation_journal::operation(&operation_id)
            .expect("recovery journal")
            .status,
        crate::operation_journal::OperationStatus::Stopped
    );
    let report = ws
        .poll_transfer(|| {})
        .terminal
        .expect("stopped recovery report");
    assert_eq!(report.terminal, TransferTerminalState::Stopped);
    assert!(ws.poll_transfer(|| {}).terminal.is_none());
}

#[test]
fn cancellation_after_dequeue_finishes_once_before_worker_body_runs() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let workload =
        crate::workload::DeterministicWorkload::new(crate::workload::SchedulerLimits::default());
    let mut ws = workspace(&left, &right);
    ws.enqueue_with_history(
        test_transfer_spec("cancel-after-dequeue", right.path()),
        transfer_queue::HistoryIntent::None,
    );
    ws.pump_queue_with_workload(workload.handle(), || {});
    let progress = ws
        .active_transfer_view()
        .expect("dequeued transfer is active")
        .progress;
    let barrier = Arc::new(Barrier::new(2));
    let worker_barrier = Arc::clone(&barrier);
    let (dequeued_tx, dequeued_rx) = mpsc::sync_channel(1);
    let runner = workload.clone();
    let worker = std::thread::spawn(move || {
        assert!(runner.run_next_after_dequeue(|| {
            let _ = dequeued_tx.send(());
            worker_barrier.wait();
        }));
    });
    dequeued_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("workload dequeued transfer");

    ws.cancel_transfer();
    assert!(
        !crate::lock_util::recover(&progress).finished,
        "running cancellation waits for the worker terminal path"
    );
    barrier.wait();
    worker.join().expect("deterministic worker");

    let state = crate::lock_util::recover(&progress);
    assert!(state.finished);
    assert!(state.cancelled);
    assert!(state.errors.is_empty());
    drop(state);
    let report = ws
        .poll_transfer(|| {})
        .terminal
        .expect("cancelled terminal report");
    assert_eq!(report.terminal, TransferTerminalState::Cancelled);
    assert!(ws.active_transfer_view().is_none());
    assert!(
        ws.poll_transfer(|| {}).terminal.is_none(),
        "dequeued cancellation publishes exactly once"
    );
}

#[test]
fn interruption_before_required_post_success_is_not_normalized_to_completion() {
    let journal = TempDir::new();
    let _journal = crate::operation_journal::use_test_journal(journal.path().join("journal.json"));
    for (label, cancel, expected_terminal) in [
        (
            "cancel",
            true,
            crate::workspace::TransferTerminalState::Cancelled,
        ),
        (
            "stop",
            false,
            crate::workspace::TransferTerminalState::Stopped,
        ),
    ] {
        let (left, right) = (TempDir::new(), TempDir::new());
        let source_folder_name = format!("{label}-source");
        let source_folder = left.dir(&source_folder_name);
        let source_path = format!("{source_folder_name}/file.txt");
        let source = left.file(&source_path, "payload");
        let entry =
            FileEntry::from_meta(source.clone(), &std::fs::symlink_metadata(&source).unwrap())
                .unwrap();
        let operation_id = crate::operation::OperationId::new();
        let mut spec = test_transfer_spec(&operation_id.0, right.path());
        spec.entries = vec![entry.clone()];
        spec.expectations =
            transfer::capture_expectations(std::slice::from_ref(&entry), right.path());
        spec.post_success = Some(PostTransferAction::RemoveEmptyDir(source_folder.clone()));
        spec.journal_enabled = true;
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = Arc::clone(&barrier);
        let (finalizing_tx, finalizing_rx) = mpsc::sync_channel(1);
        spec.before_post_success = Some(Arc::new(move || {
            let _ = finalizing_tx.send(());
            worker_barrier.wait();
        }));
        let workload = crate::workload::DeterministicWorkload::new(
            crate::workload::SchedulerLimits::default(),
        );
        let mut ws = workspace(&left, &right);
        ws.enqueue_with_history(spec, transfer_queue::HistoryIntent::None);
        ws.pump_queue_with_workload(workload.handle(), || {});
        let progress = ws
            .active_transfer_view()
            .expect("boundary transfer is active")
            .progress;
        let runner = workload.clone();
        let worker = std::thread::spawn(move || {
            assert!(runner.run_next());
        });
        finalizing_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("worker reached required post-success boundary");
        assert_eq!(crate::lock_util::recover(&progress).files_done, 1);
        assert!(right.path().join("file.txt").is_file());
        assert!(source_folder.exists());

        if cancel {
            ws.cancel_transfer();
        } else {
            ws.stop_transfer_after_current();
        }
        barrier.wait();
        worker.join().expect("deterministic worker");

        let state = crate::lock_util::recover(&progress);
        assert!(state.finished);
        assert_eq!(state.cancelled, cancel);
        assert_eq!(state.stopped, !cancel);
        drop(state);
        assert!(
            source_folder.exists(),
            "{label} skipped mandatory directory cleanup"
        );
        assert_eq!(
            crate::operation_journal::operation(&operation_id)
                .expect("boundary operation journal")
                .status,
            crate::operation_journal::OperationStatus::Stopped
        );
        let report = ws
            .poll_transfer(|| {})
            .terminal
            .expect("interrupted terminal report");
        assert_eq!(report.terminal, expected_terminal);
        assert!(ws.poll_transfer(|| {}).terminal.is_none());
    }
}

#[test]
fn cancelled_and_stopped_errors_emit_one_canonical_terminal_report() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let mut ws = workspace(&left, &right);

    for (operation, stopped, expected) in [
        ("cancelled-report", false, TransferTerminalState::Cancelled),
        ("stopped-report", true, TransferTerminalState::Stopped),
    ] {
        let progress = ws.launch_test_transfer(
            test_transfer_spec(operation, right.path()),
            transfer_queue::HistoryIntent::None,
        );
        {
            let mut progress = crate::lock_util::recover(&progress);
            progress.finished = true;
            progress.cancelled = !stopped;
            progress.stopped = stopped;
            progress.errors.push("terminal error".to_string());
            progress
                .failures
                .push(crate::operation::ClassifiedFailure::message(
                    crate::operation::FailureClass::Blocked,
                    None,
                    "terminal error",
                ));
        }

        let outcome = ws.poll_transfer(|| {});
        let report = outcome.terminal.expect("terminal report");
        assert_eq!(report.operation_id.0, operation);
        assert_eq!(report.terminal, expected);
        assert_eq!(report.errors, ["terminal error"]);
        assert_eq!(report.failures.len(), 1);
        assert!(ws.active_transfer_view().is_none());
        assert!(
            ws.poll_transfer(|| {}).terminal.is_none(),
            "terminal report must be exactly once"
        );
    }
}

#[test]
fn cancelling_an_admitted_queued_transfer_reports_once_without_running_work() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let workload =
        crate::workload::DeterministicWorkload::new(crate::workload::SchedulerLimits::default());
    let mut ws = workspace(&left, &right);
    ws.enqueue_with_history(
        test_transfer_spec("queued-cancel", right.path()),
        transfer_queue::HistoryIntent::None,
    );
    ws.pump_queue_with_workload(workload.handle(), || {});
    let progress = ws
        .active_transfer_view()
        .expect("queued transfer is controller-active")
        .progress;

    ws.cancel_transfer();

    let state = crate::lock_util::recover(&progress);
    assert!(state.finished);
    assert!(state.cancelled);
    assert!(state.errors.is_empty());
    drop(state);
    assert!(!workload.run_next(), "cancelled queued work never executes");
    assert_eq!(workload.stats().queued, 0);

    let report = ws
        .poll_transfer(|| {})
        .terminal
        .expect("cancelled terminal report");
    assert_eq!(report.operation_id.0, "queued-cancel");
    assert_eq!(report.terminal, TransferTerminalState::Cancelled);
    assert!(report.errors.is_empty());
    assert!(ws.active_transfer_view().is_none());
    assert!(
        ws.poll_transfer(|| {}).terminal.is_none(),
        "cancelled report is emitted exactly once"
    );
}

#[test]
fn panicking_transfer_finishes_in_needs_review_without_wedging_workspace() {
    let journal = TempDir::new();
    let _journal = crate::operation_journal::use_test_journal(journal.path().join("journal.json"));
    let (left, right) = (TempDir::new(), TempDir::new());
    let source = left.file("panic.txt", "panic boundary");
    let entry =
        FileEntry::from_meta(source.clone(), &std::fs::symlink_metadata(&source).unwrap()).unwrap();
    let operation_id = crate::operation::OperationId::new();
    let mut spec = test_transfer_spec(&operation_id.0, right.path());
    spec.kind = TransferKind::Copy;
    spec.expectations = transfer::capture_expectations(std::slice::from_ref(&entry), right.path());
    spec.entries = vec![entry];
    spec.journal_enabled = true;
    spec.before_commit = Some(Arc::new(|_, _| panic!("injected worker panic")));

    let notifications = Arc::new(AtomicUsize::new(0));
    let notify_count = Arc::clone(&notifications);
    let workload =
        crate::workload::DeterministicWorkload::new(crate::workload::SchedulerLimits::default());
    let mut ws = workspace(&left, &right);
    ws.enqueue_with_history(spec, transfer_queue::HistoryIntent::None);
    ws.pump_queue_with_workload(workload.handle(), move || {
        notify_count.fetch_add(1, Ordering::SeqCst);
    });
    let progress = ws
        .active_transfer_view()
        .expect("panic transfer active")
        .progress;
    assert!(workload.run_next(), "panic worker was admitted");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !crate::lock_util::recover(&progress).finished {
        assert!(std::time::Instant::now() < deadline, "panic path wedged");
        std::thread::yield_now();
    }

    let state = crate::lock_util::recover(&progress);
    assert!(state.failures.iter().any(|failure| {
        failure.class == crate::operation::FailureClass::IntegrityUncertain
            && failure.message.contains("panicked")
    }));
    drop(state);
    assert!(notifications.load(Ordering::SeqCst) > 0);
    assert_eq!(
        crate::operation_journal::operation(&operation_id)
            .expect("panic journal")
            .status,
        crate::operation_journal::OperationStatus::NeedsReview
    );

    let outcome = ws.poll_transfer(|| {});
    assert!(outcome.terminal.is_none(), "panic error remains reviewable");
    assert_eq!(
        ws.safe_state.as_ref().map(|state| &state.operation_id),
        Some(&operation_id)
    );
    ws.acknowledge_safe_state();
    let report = ws
        .try_dismiss_transfer(|| {})
        .expect("reviewed panic can be dismissed");
    assert_eq!(report.operation_id, operation_id);
    assert!(
        report
            .failures
            .iter()
            .any(|failure| failure.class == crate::operation::FailureClass::IntegrityUncertain)
    );
    assert!(ws.active_transfer_view().is_none());
}

#[test]
fn switch_panel_toggles_active() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let mut ws = workspace(&l, &r);
    assert!(ws.active == ActivePanel::Left);
    ws.execute(Command::SwitchPanel);
    assert!(ws.active == ActivePanel::Right);
    ws.execute(Command::SwitchPanel);
    assert!(ws.active == ActivePanel::Left);
}

#[test]
fn every_fixed_ui_command_emits_the_expected_typed_request() {
    use crate::clipboard::PathStyle;

    let cases = [
        (
            Command::MoveIntoCursorFolder,
            UiRequest::TransferIntoCursorFolder(TransferKind::Move),
        ),
        (
            Command::CopyIntoCursorFolder,
            UiRequest::TransferIntoCursorFolder(TransferKind::Copy),
        ),
        (Command::BeginBatchRename, UiRequest::BatchRename),
        (Command::BeginSync, UiRequest::Sync),
        (Command::FindDuplicates, UiRequest::FindDuplicates),
        (Command::DiffFiles, UiRequest::DiffFiles),
        (Command::DiskTreemap, UiRequest::DiskTreemap),
        (Command::BeginFind, UiRequest::Find),
        (Command::OpenSavedSearch, UiRequest::SavedSearch),
        (
            Command::OpenProjectCollections,
            UiRequest::ProjectCollections,
        ),
        (Command::CopyPath, UiRequest::CopyPaths(PathStyle::FullPath)),
        (Command::CopyName, UiRequest::CopyPaths(PathStyle::NameOnly)),
        (
            Command::CopyParentPath,
            UiRequest::CopyPaths(PathStyle::ParentPath),
        ),
        (
            Command::CopyFileUrl,
            UiRequest::CopyPaths(PathStyle::FileUrl),
        ),
        (
            Command::CopyShellPath,
            UiRequest::CopyPaths(PathStyle::ShellEscaped),
        ),
        (
            Command::CopyRelativePath,
            UiRequest::CopyPaths(PathStyle::RelativeToOther),
        ),
        (Command::BeginSelectMask, UiRequest::SelectMask),
        (Command::BeginRunBar, UiRequest::RunCommand),
        (Command::GatherIntoFolder, UiRequest::GatherIntoFolder),
        (Command::BeginGoToPath, UiRequest::GoToPath),
        (Command::BeginRecent, UiRequest::Recent),
        (Command::BeginPalette, UiRequest::Palette),
        (Command::Undo, UiRequest::Undo),
        (Command::Redo, UiRequest::Redo),
        (Command::ToggleQueuePanel, UiRequest::ToggleQueuePanel),
        (Command::OpenReceipts, UiRequest::OperationHistory),
        (Command::OpenRecoveryCenter, UiRequest::OpenRecoveryCenter),
    ];
    let (left, right) = (TempDir::new(), TempDir::new());
    let mut ws = workspace(&left, &right);

    for (command, expected) in cases {
        ws.execute(command);
        assert_eq!(ws.drain_ui_requests(), vec![expected], "{command:?}");
    }
}

#[test]
fn same_frame_ui_commands_preserve_fifo_and_duplicates() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let mut ws = workspace(&left, &right);

    ws.execute(Command::ToggleQueuePanel);
    ws.execute(Command::ToggleQueuePanel);
    ws.execute(Command::BeginPalette);
    ws.execute(Command::BeginRecent);

    assert_eq!(
        ws.drain_ui_requests(),
        vec![
            UiRequest::ToggleQueuePanel,
            UiRequest::ToggleQueuePanel,
            UiRequest::Palette,
            UiRequest::Recent,
        ]
    );
}

#[test]
fn toggle_hidden_command_emits_typed_success_feedback() {
    let (left, right) = (TempDir::new(), TempDir::new());
    left.file(".hidden.txt", "hidden");
    let mut ws = workspace(&left, &right);

    ws.execute(Command::ToggleHidden);

    assert!(ws.left.show_hidden());
    assert_eq!(
        ws.drain_ui_requests(),
        vec![UiRequest::HiddenFilesOutcome(
            crate::panel::ViewApplyOutcome::Applied
        )]
    );
}

#[test]
fn toggle_hidden_command_reports_rejection_without_changing_the_view() {
    let root = TempDir::new();
    let left = root.dir("left");
    root.file("left/visible.txt", "visible");
    let right = TempDir::new();
    let mut ws = Workspace::new(left.clone(), right.path().to_path_buf());
    ws.left.refresh();
    ws.right.refresh();
    let config = ws.left.view_config();
    let revision = ws.left.entries_gen();
    let status = ws.left.dir_status();
    std::fs::remove_dir_all(&left).unwrap();

    ws.execute(Command::ToggleHidden);

    assert_eq!(ws.left.view_config(), config);
    assert_eq!(ws.left.entries_gen(), revision);
    assert_eq!(ws.left.dir_status(), status);
    assert_eq!(
        ws.drain_ui_requests(),
        vec![UiRequest::HiddenFilesOutcome(
            crate::panel::ViewApplyOutcome::ReadRejected(crate::panel::DirStatus::Gone)
        )]
    );
}

#[test]
fn conditional_and_listing_commands_emit_payload_requests() {
    let (left, right) = (TempDir::new(), TempDir::new());
    left.file("listed.txt", "x");
    let mut ws = workspace(&left, &right);

    for (command, format_label) in [
        (Command::CopyListingText, "text"),
        (Command::CopyListingCsv, "CSV"),
        (Command::CopyListingMarkdown, "Markdown"),
    ] {
        ws.execute(command);
        let requests = ws.drain_ui_requests();
        let [UiRequest::CopyText { text, label }] = requests.as_slice() else {
            panic!("{command:?} did not emit one CopyText request: {requests:?}");
        };
        assert!(text.contains("listed.txt"));
        assert!(label.contains(format_label));
    }
}

#[test]
fn equalize_points_inactive_panel_at_active_dir() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let mut ws = workspace(&l, &r);
    assert_ne!(ws.left.current_path, ws.right.current_path);

    // Active is Left; equalize sends Right to Left's directory.
    ws.execute(Command::EqualizePanels);
    assert_eq!(ws.right.current_path, l.path());
    assert_eq!(ws.left.current_path, l.path());
}

#[test]
fn swap_exchanges_panels_and_keeps_focus_on_content() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "x");
    let mut ws = workspace(&l, &r);
    ws.left.set_cursor(1);

    ws.execute(Command::SwapPanels);

    // Left's content (and cursor) is now on the right, and focus follows.
    assert_eq!(ws.right.current_path, l.path());
    assert_eq!(ws.left.current_path, r.path());
    assert_eq!(ws.right.cursor(), 1);
    assert!(ws.active == ActivePanel::Right);
}

#[test]
fn cursor_moves_are_clamped() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "x");
    l.file("b.txt", "x");
    let mut ws = workspace(&l, &r);

    ws.execute(Command::CursorUp);
    assert_eq!(ws.left.cursor(), 0, "cursor must not go below 0");

    for _ in 0..10 {
        ws.execute(Command::CursorDown);
    }
    assert_eq!(ws.left.cursor(), 2, "cursor must stop at the last entry");
}

#[test]
fn parent_focus_never_targets_the_first_entry() {
    let (left, right) = (TempDir::new(), TempDir::new());
    left.file("first.txt", "x");
    let mut ws = workspace(&left, &right);
    ws.left.set_cursor(0);

    assert!(ws.left.cursor_entry().is_none());
    assert!(ws.left.selected_or_cursor().unwrap().is_empty());
    assert!(!ws.command_context().cursor_entry);

    ws.execute(Command::TogglePreview);
    ws.execute(Command::BeginRename);
    assert!(ws.right.preview.is_none());
    assert!(ws.pending_ui_requests().is_empty());
}

#[test]
fn cursor_move_jumps_by_a_signed_count_and_clamps() {
    let (l, r) = (TempDir::new(), TempDir::new());
    for n in 0..10 {
        l.file(&format!("f{n:02}.txt"), "x");
    }
    let mut ws = workspace(&l, &r);

    ws.execute(Command::CursorMove(5));
    assert_eq!(ws.left.cursor(), 5, "5j-style jump moves 5 rows down");

    ws.execute(Command::CursorMove(-2));
    assert_eq!(ws.left.cursor(), 3, "negative delta moves up");

    ws.execute(Command::CursorMove(100));
    assert_eq!(ws.left.cursor(), 10, "clamped to the last entry");

    ws.execute(Command::CursorMove(-100));
    assert_eq!(ws.left.cursor(), 0, "clamped to the first row");

    // A ten-digit vim count must not overflow (debug panic, release wrap).
    ws.execute(Command::CursorMove(3));
    ws.execute(Command::CursorMove(i32::MAX));
    assert_eq!(ws.left.cursor(), 10, "a huge count lands on the last entry");
    ws.execute(Command::CursorMove(i32::MIN));
    assert_eq!(ws.left.cursor(), 0, "a huge negative count lands on top");
}

#[test]
fn home_end_and_page_navigation() {
    let (l, r) = (TempDir::new(), TempDir::new());
    for n in 0..20 {
        l.file(&format!("f{n:02}.txt"), "x");
    }
    let mut ws = workspace(&l, &r);
    ws.left.set_page_rows(5);

    ws.execute(Command::CursorEnd);
    assert_eq!(ws.left.cursor(), 20, "End jumps to the last row");

    ws.execute(Command::CursorHome);
    assert_eq!(ws.left.cursor(), 0, "Home jumps to the top");

    ws.execute(Command::CursorPageDown);
    assert_eq!(ws.left.cursor(), 5, "PageDown moves by one page");

    ws.execute(Command::CursorPageUp);
    assert_eq!(ws.left.cursor(), 0, "PageUp moves back, clamped at 0");
}

#[test]
fn shift_arrows_build_a_contiguous_selection() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "x");
    l.file("b.txt", "x");
    l.file("c.txt", "x");
    let mut ws = workspace(&l, &r);

    ws.left.set_cursor(1); // a.txt
    ws.execute(Command::ExtendSelectDown); // select a, move to b, select b
    ws.execute(Command::ExtendSelectDown); // select b, move to c, select c

    assert_eq!(ws.left.cursor(), 3);
    assert_eq!(ws.left.selected_count(), 3, "a, b and c are selected");
}

#[test]
fn activate_dir_navigates_into_it() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let sub = l.dir("sub");
    l.file("sub/inner.txt", "x");
    let mut ws = workspace(&l, &r);

    ws.left.set_cursor(1); // dirs sort first, so "sub" is the first row
    ws.execute(Command::Activate);
    assert_eq!(ws.left.current_path, sub);
    assert_eq!(ws.left.entries().len(), 1);
}

#[test]
fn activate_file_emits_typed_open_intent() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let path = l.file("a.txt", "x");
    let mut ws = Workspace::new(l.path().to_path_buf(), r.path().to_path_buf());
    ws.left.refresh();

    ws.left.set_cursor(1);
    ws.execute(Command::Activate);
    assert_eq!(
        ws.pending_ui_requests(),
        vec![UiRequest::OpenExternal(
            crate::ports::OpenRequest::OpenPath(path)
        )]
    );
}

#[test]
fn activate_zip_requests_the_read_only_archive_browser() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let archive = left.file("bundle.zip", "placeholder");
    let mut workspace = Workspace::new(left.path().to_path_buf(), right.path().to_path_buf());
    workspace.left.refresh();

    workspace.left.set_cursor(1);
    workspace.execute(Command::Activate);

    assert_eq!(
        workspace.pending_ui_requests(),
        vec![UiRequest::Archive(archive)]
    );
}

#[test]
fn copy_flow_end_to_end() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "hello");
    let mut ws = workspace(&l, &r);

    ws.left.set_cursor(1);
    ws.execute(Command::RequestCopy);
    assert!(matches!(ws.pending_op, Some(PendingOp::Transfer(_))));

    ws.finish_space_probe();
    ws.confirm_pending_op(|| {});
    wait_transfer(&mut ws);

    assert!(ws.active_transfer().is_none(), "clean transfer auto-closes");
    let copied = std::fs::read_to_string(r.path().join("a.txt")).unwrap();
    assert_eq!(copied, "hello");
    assert!(l.path().join("a.txt").exists(), "copy must keep the source");
}

#[test]
fn move_flow_removes_source() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "hello");
    let mut ws = workspace(&l, &r);

    ws.left.set_cursor(1);
    ws.execute(Command::RequestMove);
    ws.finish_space_probe();
    ws.confirm_pending_op(|| {});
    wait_transfer(&mut ws);

    assert!(r.path().join("a.txt").exists());
    assert!(
        !l.path().join("a.txt").exists(),
        "move must delete the source"
    );
}

#[test]
fn request_delete_builds_pending_op() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "x");
    let mut ws = workspace(&l, &r);

    ws.left.set_cursor(1);
    ws.execute(Command::RequestDelete);
    match &ws.pending_op {
        Some(PendingOp::Delete { entries, .. }) => {
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].name, "a.txt");
        }
        _ => panic!("expected a pending delete"),
    }
}

#[test]
fn context_menu_trash_on_a_selected_row_takes_the_selection_from_every_folder() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let sub = left.dir("sub");
    let inner = left.file("sub/inner.txt", "i");
    let here = left.file("here.txt", "h");
    let mut ws = workspace(&left, &right);
    ws.left.navigate_to(sub);
    ws.left.select_path(inner.clone());
    ws.left.navigate_to(left.path().to_path_buf());
    ws.left.select_path(here.clone());

    ws.request_context_delete(ActivePanel::Left, &here);

    match &ws.pending_op {
        Some(PendingOp::Delete { entries, .. }) => {
            let mut paths: Vec<_> = entries.iter().map(|e| e.path.clone()).collect();
            paths.sort();
            // The same items the Delete key would trash.
            let mut expected = vec![here, inner];
            expected.sort();
            assert_eq!(paths, expected);
        }
        _ => panic!("expected a pending delete"),
    }
}

#[test]
fn context_menu_trash_routes_through_confirmation_and_background_port() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let path = left.file("a.txt", "x");
    let trash = Arc::new(ScriptedTrashPort::new([
        crate::ports::TrashItemOutcome::Trashed,
    ]));
    let mut workspace = workspace_with_trash(&left, &right, Arc::clone(&trash));

    workspace.request_context_delete(ActivePanel::Left, &path);
    assert!(matches!(
        workspace.pending_op,
        Some(PendingOp::Delete { .. })
    ));
    assert!(trash.calls.lock().unwrap().is_empty());

    assert!(workspace.confirm_pending_op(|| {}));
    let outcome = workspace.finish_delete().expect("delete outcome");
    assert_eq!(outcome.trashed, 1);
    assert_eq!(*trash.calls.lock().unwrap(), [path]);
}

#[test]
fn versioned_trash_delete_records_undo_and_restores_through_version_store() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let path = left.file("keep-me.txt", "payload");
    let versions = TempDir::new();
    let _guard = crate::version_store::use_test_versions_dir(versions.path().to_path_buf());
    let trash = Arc::new(ScriptedTrashPort::new([
        crate::ports::TrashItemOutcome::Trashed,
        crate::ports::TrashItemOutcome::Trashed, // redo
    ]));
    let mut workspace = Workspace::with_versioned_delete_ports(
        left.path().to_path_buf(),
        right.path().to_path_buf(),
        trash.clone(),
        Arc::new(TestFreeSpacePort),
    );
    workspace.left.refresh();
    workspace.right.refresh();
    workspace.left.set_cursor(1);
    workspace.request_delete();
    assert!(workspace.confirm_pending_op(|| {}));
    let outcome = workspace.finish_delete().expect("delete outcome");
    assert_eq!(outcome.trashed, 1);
    assert!(!path.exists(), "scripted trash removes the live file");
    assert!(
        matches!(
            workspace.top_undo_action(),
            Some(crate::undo::Action::Trash { .. })
        ),
        "Versioned trash must land on the undo stack"
    );

    workspace
        .perform_undo(|| {})
        .expect("restore from version store");
    assert!(path.is_file(), "undo restores the original path");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "payload");

    workspace.perform_redo(|| {}).expect("re-trash");
    assert!(!path.exists(), "redo trashes the restored file again");
    assert_eq!(trash.calls.lock().unwrap().len(), 2);
}

#[test]
fn delete_uses_visible_listing_identity_and_rejects_a_replacement() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let path = left.file("a.txt", "old");
    let trash = Arc::new(ScriptedTrashPort::new([
        crate::ports::TrashItemOutcome::Trashed,
    ]));
    let mut workspace = workspace_with_trash(&left, &right, Arc::clone(&trash));
    workspace.left.set_cursor(1);

    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, "replacement object").unwrap();
    workspace.request_delete();
    assert!(workspace.confirm_pending_op(|| {}));
    let outcome = workspace.finish_delete().expect("delete outcome");

    assert_eq!(
        outcome.items[0].outcome,
        crate::ports::TrashItemOutcome::StaleBinding
    );
    assert!(trash.calls.lock().unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "replacement object");
}

#[test]
fn safe_state_and_busy_queue_never_call_the_trash_port() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let path = left.file("a.txt", "x");
    let trash = Arc::new(ScriptedTrashPort::new([
        crate::ports::TrashItemOutcome::Trashed,
    ]));
    let mut workspace = workspace_with_trash(&left, &right, Arc::clone(&trash));
    workspace.safe_state = Some(crate::operation::SafeState {
        operation_id: crate::operation::OperationId("blocked-delete".to_string()),
        reason: "review required".to_string(),
        paths: vec![path.clone()],
        failures: Vec::new(),
    });
    assert!(!workspace.trash_entries(vec![ready_trash_item(&path)], || {}));

    workspace.safe_state = None;
    workspace.launch_test_transfer(
        test_transfer_spec("busy-delete", right.path()),
        transfer_queue::HistoryIntent::None,
    );
    assert!(!workspace.trash_entries(vec![ready_trash_item(&path)], || {}));
    assert!(trash.calls.lock().unwrap().is_empty());
}

#[test]
fn failed_delete_batch_does_not_request_panel_refresh() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let path = left.file("a.txt", "x");
    let trash = Arc::new(ScriptedTrashPort::new([
        crate::ports::TrashItemOutcome::Failed(crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Denied,
            message: "denied".to_string(),
        }),
    ]));
    let mut workspace = workspace_with_trash(&left, &right, trash);

    assert!(workspace.trash_entries(vec![ready_trash_item(&path)], || {}));
    let outcome = workspace.finish_delete().expect("delete outcome");
    assert_eq!(outcome.trashed, 0);
    assert_eq!(outcome.failed, 1);
    assert!(!outcome.refresh_required());
}

#[test]
fn active_delete_blocks_other_mutation_commits_until_retirement() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let path = left.file("a.txt", "x");
    let (release, gate) = mpsc::channel();
    let trash = Arc::new(GatedTrashPort {
        release: Mutex::new(gate),
    });
    let mut workspace = workspace_with_trash(&left, &right, trash);
    workspace.left.set_cursor(1);
    workspace.request_delete();
    assert!(workspace.confirm_pending_op(|| {}));

    assert!(workspace.delete_active());
    assert!(workspace.mutations_blocked());
    assert_eq!(
        workspace.delete_activity(),
        Some(DeleteActivity {
            completed: 0,
            total: 1,
            cancel_requested: false,
        })
    );
    let context = workspace.command_context();
    assert!(context.active_mutation);
    assert!(!context.safe_state);
    workspace.create_dir();
    assert!(!left.path().join("New Folder").exists());
    let rename = workspace.commit_rename(&path, "renamed.txt");
    assert!(rename.unwrap_err().contains("Trash operation"));

    release.send(()).unwrap();
    workspace.finish_delete().expect("delete retirement");
    assert!(!workspace.delete_active());
}

#[test]
fn file_op_requests_do_not_replace_an_existing_confirmation() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "a");
    let mut ws = workspace(&l, &r);
    ws.left.set_cursor(1);

    ws.request_copy();
    assert!(matches!(
        ws.pending_op,
        Some(PendingOp::Transfer(PendingTransfer {
            kind: TransferKind::Copy,
            ..
        }))
    ));

    ws.request_move();
    ws.request_delete();
    assert!(matches!(
        ws.pending_op,
        Some(PendingOp::Transfer(PendingTransfer {
            kind: TransferKind::Copy,
            ..
        }))
    ));

    ws.pending_op = None;
    ws.launch_test_transfer(
        test_transfer_spec("request-guard", r.path()),
        transfer_queue::HistoryIntent::None,
    );
    assert!(
        ws.can_request_transfer(),
        "Copy/Move may queue while active"
    );
    ws.request_delete();
    assert!(ws.pending_op.is_none(), "Delete is blocked while active");
}

#[test]
fn create_dir_picks_first_free_name() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let mut ws = workspace(&l, &r);

    ws.create_dir();
    assert!(l.path().join("New Folder").is_dir());
    ws.create_dir();
    assert!(l.path().join("New Folder 1").is_dir());
}

#[test]
fn select_same_named_adds_common_names_keeping_prior_picks() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let shared = l.file("report.txt", "x");
    let only_here = l.file("draft.txt", "y");
    r.file("Report.TXT", "z"); // same name, different case -> still a match
    let mut ws = workspace(&l, &r);

    // A pre-existing manual pick must survive the union.
    ws.left.select_path(only_here.clone());
    ws.select_same_named();

    assert!(ws.left.is_selected(&shared), "common name selected");
    assert!(ws.left.is_selected(&only_here), "prior pick kept");
    assert_eq!(ws.left.selected_count(), 2, "no spurious selections");
}

#[test]
fn apply_batch_rename_renames_only_the_selection() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "1");
    l.file("b.txt", "2");
    l.file("keep.log", "3"); // not selected
    let mut ws = workspace(&l, &r);
    ws.left.select_path(l.path().join("a.txt"));
    ws.left.select_path(l.path().join("b.txt"));

    let rule = crate::rename::RenameRule {
        prefix: "x_".into(),
        ..Default::default()
    };
    let n = apply_batch_rename(&mut ws, &rule).unwrap();
    assert_eq!(n, 2);
    assert!(l.path().join("x_a.txt").is_file());
    assert!(l.path().join("x_b.txt").is_file());
    assert!(!l.path().join("a.txt").exists());
    assert!(
        l.path().join("keep.log").is_file(),
        "non-selected untouched"
    );
    assert!(
        ws.left.selection_is_empty(),
        "selection cleared after rename"
    );
}

#[test]
fn batch_rename_context_does_not_follow_a_later_panel_switch() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let left_file = l.file("left.txt", "left");
    let right_file = r.file("right.txt", "right");
    let mut ws = workspace(&l, &r);
    ws.left.select_path(left_file);
    let context = ws.batch_rename_context().unwrap();

    ws.active = ActivePanel::Right;
    ws.right.select_path(right_file);
    let rule = crate::rename::RenameRule {
        prefix: "renamed_".into(),
        ..Default::default()
    };
    assert_eq!(ws.apply_batch_rename_in(&context, &rule).unwrap(), 1);

    assert!(l.path().join("renamed_left.txt").is_file());
    assert!(r.path().join("right.txt").is_file());
    assert!(!r.path().join("renamed_right.txt").exists());
}

#[test]
fn treemap_snapshot_keeps_its_opening_directory() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("left.txt", "left");
    r.file("right.txt", "right");
    let mut ws = workspace(&l, &r);

    let snapshot = ws.treemap_snapshot();
    ws.active = ActivePanel::Right;

    assert_eq!(snapshot.dir, l.path());
    assert_eq!(snapshot.items.len(), 1);
    assert_eq!(snapshot.items[0].0.name, "left.txt");
}

#[test]
fn apply_batch_rename_refuses_a_colliding_plan() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("report_v1.txt", "a");
    l.file("report_v2.txt", "b");
    let mut ws = workspace(&l, &r);
    ws.left.select_path(l.path().join("report_v1.txt"));
    ws.left.select_path(l.path().join("report_v2.txt"));

    // "v1" -> "v2" maps report_v1 onto report_v2's name while report_v2
    // stays put: a duplicate/sibling collision, so the plan is rejected.
    let dup = crate::rename::RenameRule {
        find: "v1".into(),
        replace: "v2".into(),
        ..Default::default()
    };
    let err = apply_batch_rename(&mut ws, &dup);
    assert!(err.is_err(), "colliding plan rejected: {err:?}");
    // Both files are left untouched on refusal.
    assert!(l.path().join("report_v1.txt").is_file());
    assert!(l.path().join("report_v2.txt").is_file());
}

#[test]
fn apply_batch_rename_rejects_invalid_regex_before_mutation() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let original = l.file("report.txt", "content");
    let mut ws = workspace(&l, &r);
    ws.left.select_path(original.clone());
    let invalid = crate::rename::RenameRule {
        find: "(".to_string(),
        replace: "renamed".to_string(),
        regex: true,
        ..Default::default()
    };

    let error = apply_batch_rename(&mut ws, &invalid).unwrap_err();

    assert!(error.starts_with("Invalid regex:"), "{error}");
    assert!(original.is_file());
    assert!(!l.path().join("renamed").exists());
    assert!(!ws.can_undo());
}

#[test]
fn apply_sync_mirror_copies_left_only_file_right() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("new.txt", "hello");
    let mut ws = workspace(&l, &r);
    let actions = ws.build_sync_actions(crate::sync::SyncPolicy::MirrorLeftToRight);
    apply_sync(&mut ws, &actions);
    wait_transfer(&mut ws);
    assert!(r.path().join("new.txt").is_file(), "left -> right copied");
}

#[test]
fn sync_snapshot_does_not_follow_later_panel_navigation() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let elsewhere = TempDir::new();
    l.file("a.txt", "left");
    let mut ws = workspace(&l, &r);
    let actions = ws.build_sync_actions(crate::sync::SyncPolicy::MirrorLeftToRight);
    let left_dir = ws.left.current_path.clone();
    let right_dir = ws.right.current_path.clone();

    ws.right.navigate_to(elsewhere.path().to_path_buf());
    ws.apply_sync_between(&actions, &left_dir, &right_dir, || {});
    wait_transfer(&mut ws);

    assert!(r.path().join("a.txt").is_file());
    assert!(!elsewhere.path().join("a.txt").exists());
}

#[test]
fn two_way_sync_runs_both_passes() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("left.txt", "L");
    r.file("right.txt", "R");
    let mut ws = workspace(&l, &r);
    let actions = ws.build_sync_actions(crate::sync::SyncPolicy::TwoWay);
    apply_sync(&mut ws, &actions);

    // Both passes are enqueued; the first runs now, the second waits behind
    // it on the queue (no more ad-hoc follow-up handling).
    assert!(ws.active_transfer().is_some(), "first pass running");
    assert_eq!(ws.queued_count(), 1, "second pass queued behind the first");
    let groups = ws.transfer_group_ids();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0], groups[1], "both passes share one intent id");

    // Drive the queue to completion; poll_transfer drains the second pass.
    drain_transfers(&mut ws);
    assert!(r.path().join("left.txt").is_file(), "left -> right");
    assert!(l.path().join("right.txt").is_file(), "right -> left");
    assert_eq!(ws.queued_count(), 0, "queue fully drained");
}

#[test]
fn guarded_sync_rejects_a_stale_baseline_before_enqueue() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("report.txt", "first");
    let mut ws = workspace(&l, &r);
    let policy = crate::sync::SyncPolicy::MirrorLeftToRight;
    let (actions, stamp) = ws.build_guarded_sync_plan(policy).unwrap();
    l.file("report.txt", "changed after review");
    let guard = crate::sync_guard::GuardPolicy::default();
    let settings = crate::sync_guard::settings_fingerprint(policy, &guard, stamp.filter_key());

    let error = ws
        .apply_sync_guarded(
            crate::sync_guard::GuardedPlan {
                actions: &actions,
                stamp: &stamp,
                policy,
                guard: &guard,
                expected_settings: settings,
                allow_large_plan: false,
            },
            || {},
        )
        .unwrap_err();

    assert!(error.contains("stale"), "unexpected error: {error}");
    assert!(ws.active_transfer().is_none());
    assert_eq!(ws.queued_count(), 0);
    assert!(!r.path().join("report.txt").exists());
}

#[test]
fn guarded_sync_requires_the_marker_on_the_receiving_root() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("report.txt", "ready");
    let mut ws = workspace(&l, &r);
    let policy = crate::sync::SyncPolicy::MirrorLeftToRight;
    let (actions, stamp) = ws.build_guarded_sync_plan(policy).unwrap();
    let mut guard = crate::sync_guard::GuardPolicy::default();
    guard.set_marker(".sync-root").unwrap();
    let settings = crate::sync_guard::settings_fingerprint(policy, &guard, stamp.filter_key());

    let error = ws
        .apply_sync_guarded(
            crate::sync_guard::GuardedPlan {
                actions: &actions,
                stamp: &stamp,
                policy,
                guard: &guard,
                expected_settings: settings,
                allow_large_plan: false,
            },
            || {},
        )
        .unwrap_err();

    assert!(error.contains("Health marker"), "unexpected error: {error}");
    assert!(ws.active_transfer().is_none());
    assert_eq!(ws.queued_count(), 0);
}

#[test]
fn guarded_sync_needs_explicit_review_for_an_excessive_plan() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("report.txt", "left version");
    r.file("report.txt", "x");
    let mut ws = workspace(&l, &r);
    let policy = crate::sync::SyncPolicy::MirrorLeftToRight;
    let (actions, stamp) = ws.build_guarded_sync_plan(policy).unwrap();
    let guard = crate::sync_guard::GuardPolicy {
        max_change_fraction: 0.0,
        minimum_changed: 1,
        ..Default::default()
    };
    let settings = crate::sync_guard::settings_fingerprint(policy, &guard, stamp.filter_key());

    let error = ws
        .apply_sync_guarded(
            crate::sync_guard::GuardedPlan {
                actions: &actions,
                stamp: &stamp,
                policy,
                guard: &guard,
                expected_settings: settings,
                allow_large_plan: false,
            },
            || {},
        )
        .unwrap_err();
    assert!(error.contains("changes"), "unexpected error: {error}");
    assert!(ws.active_transfer().is_none());
    assert_eq!(ws.queued_count(), 0);

    ws.apply_sync_guarded(
        crate::sync_guard::GuardedPlan {
            actions: &actions,
            stamp: &stamp,
            policy,
            guard: &guard,
            expected_settings: settings,
            allow_large_plan: true,
        },
        || {},
    )
    .unwrap();
    drain_transfers(&mut ws);
    assert_eq!(
        std::fs::read_to_string(r.path().join("report.txt")).unwrap(),
        "left version"
    );
}

#[test]
fn gather_into_folder_moves_selection_and_undoes() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("IMG_1.jpg", "a");
    l.file("IMG_2.jpg", "b");
    l.file("keep.txt", "c"); // not selected
    let mut ws = workspace(&l, &r);
    ws.left.select_path(l.path().join("IMG_1.jpg"));
    ws.left.select_path(l.path().join("IMG_2.jpg"));

    ws.gather_into_folder(|| {});
    drain_transfers(&mut ws);

    // The selection moved into a new "IMG" subfolder; the rest stays put.
    let folder = l.path().join("IMG");
    assert!(folder.is_dir(), "gather folder created");
    assert!(folder.join("IMG_1.jpg").is_file());
    assert!(folder.join("IMG_2.jpg").is_file());
    assert!(!l.path().join("IMG_1.jpg").exists(), "originals moved out");
    assert!(l.path().join("keep.txt").is_file(), "unselected untouched");

    // Cmd+Z moves them back out and removes the now-empty folder.
    ws.perform_undo(|| {}).unwrap();
    drain_transfers(&mut ws);
    assert!(l.path().join("IMG_1.jpg").is_file(), "undo restored IMG_1");
    assert!(l.path().join("IMG_2.jpg").is_file(), "undo restored IMG_2");
    assert!(!folder.exists(), "undo removes the empty gather folder");

    // Cmd+Shift+Z recreates the exact folder and gathers the same files.
    ws.perform_redo(|| {}).unwrap();
    drain_transfers(&mut ws);
    assert!(folder.join("IMG_1.jpg").is_file());
    assert!(folder.join("IMG_2.jpg").is_file());
    assert!(!l.path().join("IMG_1.jpg").exists());
}

#[test]
fn second_transfer_queues_and_runs_after_the_first() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let a = l.file("a.txt", "AAA");
    let b = l.file("b.txt", "BBBB");
    let mut ws = workspace(&l, &r);
    let entry = |p: &std::path::Path| {
        let meta = std::fs::metadata(p).unwrap();
        FileEntry::from_meta(p.to_path_buf(), &meta).unwrap()
    };

    // Fire two copies into the right dir. The first starts; the second is
    // queued behind it instead of being dropped (active_transfer stays set
    // until poll_transfer closes it).
    ws.start_copy(
        vec![entry(&a)],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
        || {},
    );
    ws.start_copy(
        vec![entry(&b)],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
        || {},
    );
    assert!(ws.active_transfer().is_some(), "first transfer running");
    assert_eq!(ws.queued_count(), 1, "second transfer queued, not dropped");

    drain_transfers(&mut ws);
    assert!(ws.active_transfer().is_none());
    assert_eq!(ws.queued_count(), 0);
    assert!(r.path().join("a.txt").is_file(), "first copy landed");
    assert!(r.path().join("b.txt").is_file(), "queued copy ran after");
}

#[test]
fn queue_snapshot_reports_running_and_pending_jobs() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let a = l.file("a.txt", "AAA");
    let b = l.file("b.txt", "BBBB");
    let mut ws = workspace(&l, &r);
    let entry = |p: &std::path::Path| {
        let meta = std::fs::metadata(p).unwrap();
        FileEntry::from_meta(p.to_path_buf(), &meta).unwrap()
    };

    ws.start_copy(
        vec![entry(&a)],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
        || {},
    );
    ws.start_copy(
        vec![entry(&b)],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
        || {},
    );

    let rows = ws.queue_snapshot();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].state, crate::opqueue::JobState::Running);
    assert_eq!(rows[1].state, crate::opqueue::JobState::Pending);
    assert!(rows[0].label.contains("Copy 1 item"));

    drain_transfers(&mut ws);
}

#[test]
fn queue_pause_resume_and_reorder_only_touch_pending_jobs() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let a = l.file("a.txt", "A");
    let b = l.file("b.txt", "B");
    let c = l.file("c.txt", "C");
    let mut ws = workspace(&l, &r);
    let entry = |p: &std::path::Path| {
        let meta = std::fs::metadata(p).unwrap();
        FileEntry::from_meta(p.to_path_buf(), &meta).unwrap()
    };
    ws.start_copy(
        vec![entry(&a)],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
        || {},
    ); // running
    ws.start_copy(
        vec![entry(&b)],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
        || {},
    ); // pending
    ws.start_copy(
        vec![entry(&c)],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
        || {},
    ); // pending

    let rows = ws.queue_snapshot();
    let (running_id, pending1, pending2) = (rows[0].id, rows[1].id, rows[2].id);

    // Workspace only supports pausing work that has not started; attempting
    // to pause the live row must not desynchronise it from active_transfer.
    ws.queue_pause(running_id);
    assert_eq!(
        ws.queue_snapshot()[0].state,
        crate::opqueue::JobState::Running
    );

    // Pause the first pending job; the running one remains untouched.
    ws.queue_pause(pending1);
    let rows = ws.queue_snapshot();
    assert_eq!(rows[0].state, crate::opqueue::JobState::Running);
    assert_eq!(rows[1].state, crate::opqueue::JobState::Paused);

    // Move the second pending job ahead of the paused one.
    ws.queue_move(pending2, -1);
    let ids_after_move: Vec<_> = ws.queue_snapshot().iter().map(|row| row.id).collect();
    assert_eq!(ids_after_move, vec![running_id, pending2, pending1]);

    // Resume the paused job.
    ws.queue_resume(pending1, || {});
    let resumed = ws
        .queue_snapshot()
        .into_iter()
        .find(|row| row.id == pending1)
        .unwrap();
    assert_eq!(resumed.state, crate::opqueue::JobState::Pending);

    drain_transfers(&mut ws);
    assert!(r.path().join("a.txt").is_file());
    assert!(r.path().join("b.txt").is_file());
    assert!(r.path().join("c.txt").is_file());
}

#[test]
fn resuming_the_only_paused_job_starts_it_when_idle() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let source = l.file("paused.txt", "content");
    let meta = std::fs::metadata(&source).unwrap();
    let entry = FileEntry::from_meta(source, &meta).unwrap();
    let mut ws = workspace(&l, &r);
    ws.enqueue_copy(
        vec![entry],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
    );
    let id = ws.queue_snapshot()[0].id;
    ws.queue_pause(id);

    assert!(ws.active_transfer().is_none());
    assert_eq!(ws.queued_count(), 0, "paused work is not runnable");
    assert_eq!(ws.unfinished_queue_count(), 1);

    let notifications = Arc::new(AtomicUsize::new(0));
    let notify_count = Arc::clone(&notifications);
    ws.queue_resume(id, move || {
        notify_count.fetch_add(1, Ordering::SeqCst);
    });

    assert!(ws.active_transfer().is_some(), "resume fills the idle slot");
    assert_eq!(
        ws.queue_snapshot()[0].state,
        crate::opqueue::JobState::Running
    );
    drain_transfers(&mut ws);
    assert!(r.path().join("paused.txt").is_file());
    assert!(
        notifications.load(Ordering::SeqCst) > 0,
        "resumed worker forwards repaint notifications"
    );
}

#[test]
fn paused_queue_blocks_recovery_history_and_synchronous_mutations() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let source = l.file("held.txt", "content");
    let meta = std::fs::metadata(&source).unwrap();
    let entry = FileEntry::from_meta(source, &meta).unwrap();
    let mut ws = workspace(&l, &r);
    ws.enqueue_copy(
        vec![entry],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
    );
    let id = ws.queue_snapshot()[0].id;
    ws.queue_pause(id);

    assert!(ws.active_transfer().is_none());
    assert_eq!(ws.queued_count(), 0);
    assert_eq!(ws.unfinished_queue_count(), 1);
    assert!(ws.has_unfinished_transfer_work());
    assert!(!ws.can_request_delete());
    let command_context = ws.command_context();
    assert!(command_context.transfer_queue_busy);
    assert!(!crate::command::availability(Command::RequestDelete, &command_context).enabled);

    let operation_id = crate::operation::OperationId("paused-guard".to_string());
    let resume_error = ws.resume_recovery(&operation_id, || {}).unwrap_err();
    assert!(resume_error.contains("Wait for the transfer queue"));
    let rollback_error = ws.rollback_recovery(&operation_id).unwrap_err();
    assert!(rollback_error.contains("Wait for the transfer queue"));
    let undo_error = ws.perform_undo(|| {}).unwrap_err();
    assert!(undo_error.contains("Wait for the transfer queue"));
}

#[test]
fn queue_cancel_on_the_running_job_reports_a_truthful_outcome() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let a = l.file("a.txt", "A");
    let mut ws = workspace(&l, &r);
    let entry = |p: &std::path::Path| {
        let meta = std::fs::metadata(p).unwrap();
        FileEntry::from_meta(p.to_path_buf(), &meta).unwrap()
    };
    ws.start_copy(
        vec![entry(&a)],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
        || {},
    );
    let running_id = ws.queue_snapshot()[0].id;
    let progress = ws.active_transfer().cloned().unwrap();

    ws.queue_cancel(running_id);
    // Cancelling the running job routes through `cancel_transfer`: the
    // queue bookkeeping only catches up once the live worker actually
    // stops and `poll_transfer` retires it (same as the transfer
    // dialog's own Cancel button).
    assert_eq!(
        ws.queue_snapshot()[0].state,
        crate::opqueue::JobState::Running
    );

    drain_transfers(&mut ws);
    assert!(ws.active_transfer().is_none());
    let state = progress.lock().unwrap();
    assert!(state.finished);
    if state.cancelled {
        assert!(
            !r.path().join("a.txt").is_file(),
            "a cancelled copy must clean its partial destination"
        );
    } else {
        assert_eq!(
            std::fs::read_to_string(r.path().join("a.txt")).unwrap(),
            "A",
            "a copy that beat cancellation must finish cleanly"
        );
    }
}

#[test]
fn queue_cancel_on_a_pending_job_drops_it_without_touching_the_running_one() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let a = l.file("a.txt", "A");
    let b = l.file("b.txt", "B");
    let mut ws = workspace(&l, &r);
    let entry = |p: &std::path::Path| {
        let meta = std::fs::metadata(p).unwrap();
        FileEntry::from_meta(p.to_path_buf(), &meta).unwrap()
    };
    ws.start_copy(
        vec![entry(&a)],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
        || {},
    );
    ws.start_copy(
        vec![entry(&b)],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
        || {},
    );
    let pending_id = ws.queue_snapshot()[1].id;

    ws.queue_cancel(pending_id);
    assert_eq!(
        ws.queue_snapshot().len(),
        1,
        "cancelled pending job is dropped immediately"
    );
    assert_eq!(
        ws.queue_snapshot()[0].state,
        crate::opqueue::JobState::Running
    );

    drain_transfers(&mut ws);
    assert!(r.path().join("a.txt").is_file());
    assert!(!r.path().join("b.txt").is_file(), "cancelled job never ran");
}

#[test]
fn dismissing_an_errored_transfer_retires_the_job_and_drains_the_queue() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("x.txt", "X");
    r.file("x.txt", "old"); // conflict: first copy errors under Ask
    l.file("y.txt", "Y");
    let mut ws = workspace(&l, &r);
    let entry = |p: &std::path::Path| {
        let meta = std::fs::metadata(p).unwrap();
        FileEntry::from_meta(p.to_path_buf(), &meta).unwrap()
    };
    // First copy refuses x.txt (dest exists, policy Ask) -> finishes with an
    // error and is NOT cancelled. Second copy is queued behind it.
    ws.start_copy(
        vec![entry(&l.path().join("x.txt"))],
        r.path().to_path_buf(),
        OverwritePolicy::Ask,
        || {},
    );
    ws.start_copy(
        vec![entry(&l.path().join("y.txt"))],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
        || {},
    );
    assert_eq!(ws.queued_count(), 1);

    // Wait for the first to finish; an errored run stays open (poll does not
    // retire it), so the queue must not advance yet.
    let st = ws.active_transfer().cloned().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !st.lock().unwrap().finished {
        assert!(std::time::Instant::now() < deadline, "timed out");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    ws.poll_transfer(|| {});
    assert!(
        ws.active_transfer().is_some(),
        "errored transfer stays open for OK"
    );
    assert_eq!(ws.queued_count(), 1, "queue waits while the error is shown");

    // Clicking OK must retire the job and start the queued copy (before the
    // fix this left the job Running forever and wedged the whole queue).
    ws.dismiss_transfer(|| {});
    drain_transfers(&mut ws);
    assert!(ws.active_transfer().is_none());
    assert!(
        r.path().join("y.txt").is_file(),
        "queued copy ran after dismiss"
    );
    assert_eq!(
        std::fs::read_to_string(r.path().join("x.txt")).unwrap(),
        "old",
        "the refused copy left the existing file intact"
    );
}

#[test]
fn integrity_uncertain_failure_enters_safe_state_until_reviewed() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let affected = l.file("affected.txt", "data");
    let mut ws = workspace(&l, &r);
    let progress = ws.launch_test_transfer(
        test_transfer_spec("uncertain-op", r.path()),
        transfer_queue::HistoryIntent::None,
    );
    {
        let mut state = crate::lock_util::recover(&progress);
        state.finished = true;
        state.errors.push("placement uncertain".to_string());
        state
            .failures
            .push(crate::operation::ClassifiedFailure::message(
                crate::operation::FailureClass::IntegrityUncertain,
                Some(affected.clone()),
                "placement uncertain",
            ));
    }

    assert!(
        ws.poll_transfer(|| {}).terminal.is_none(),
        "errored transfer stays visible"
    );
    let safe = ws.safe_state.as_ref().expect("safe state raised");
    assert_eq!(safe.operation_id.0, "uncertain-op");
    assert_eq!(safe.paths, vec![affected]);
    ws.execute(Command::CreateDir);
    assert!(!l.path().join("New Folder").exists());
    assert!(ws.perform_undo(|| {}).is_err());

    ws.acknowledge_safe_state();
    assert!(
        ws.poll_transfer(|| {}).terminal.is_none(),
        "review token prevents a reopen loop"
    );
    assert!(ws.safe_state.is_none());
    ws.execute(Command::CreateDir);
    assert!(l.path().join("New Folder").is_dir());
}

#[test]
fn integrity_safe_state_cancels_a_paused_tail() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let source = l.file("held.txt", "content");
    let meta = std::fs::metadata(&source).unwrap();
    let entry = FileEntry::from_meta(source.clone(), &meta).unwrap();
    let mut ws = workspace(&l, &r);
    ws.enqueue_copy(
        vec![entry],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
    );
    let paused_id = ws.queue_snapshot()[0].id;
    ws.queue_pause(paused_id);

    let progress = ws.launch_test_transfer(
        test_transfer_spec("uncertain-with-paused-tail", r.path()),
        transfer_queue::HistoryIntent::None,
    );
    {
        let mut state = crate::lock_util::recover(&progress);
        state.finished = true;
        state.errors.push("placement uncertain".to_string());
        state
            .failures
            .push(crate::operation::ClassifiedFailure::message(
                crate::operation::FailureClass::IntegrityUncertain,
                Some(source),
                "placement uncertain",
            ));
    }

    assert!(ws.poll_transfer(|| {}).terminal.is_none());
    assert!(ws.safe_state.is_some());
    assert_eq!(
        ws.unfinished_queue_count(),
        1,
        "the retained active error remains unfinished until dismiss"
    );
    let paused_tail = ws
        .queue_snapshot()
        .into_iter()
        .find(|row| row.id == paused_id)
        .unwrap();
    assert_eq!(paused_tail.state, crate::opqueue::JobState::Cancelled);
}

#[test]
fn direct_dismiss_requires_review_and_cancels_waiting_tail_before_poll() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let mut ws = workspace(&left, &right);
    let progress = ws.launch_test_transfer(
        test_transfer_spec("dismiss-review-active", right.path()),
        transfer_queue::HistoryIntent::None,
    );
    ws.enqueue_with_history(
        test_transfer_spec("dismiss-review-pending", right.path()),
        transfer_queue::HistoryIntent::None,
    );
    let pending_id = ws.queue_snapshot().last().expect("pending tail").id;
    ws.enqueue_with_history(
        test_transfer_spec("dismiss-review-paused", right.path()),
        transfer_queue::HistoryIntent::None,
    );
    let paused_id = ws.queue_snapshot().last().expect("paused tail").id;
    ws.queue_pause(paused_id);
    {
        let mut state = crate::lock_util::recover(&progress);
        state.operation_id = Some(crate::operation::OperationId(
            "foreign-progress-identity".to_string(),
        ));
        state.finished = true;
        state.errors.push("retained error".to_string());
    }
    let notifications = Arc::new(AtomicUsize::new(0));
    let rejected_notifications = Arc::clone(&notifications);

    let rejection = ws
        .try_dismiss_transfer(move || {
            rejected_notifications.fetch_add(1, Ordering::SeqCst);
        })
        .expect_err("unreviewed integrity failure cannot be dismissed");
    let transfer_queue::DismissRejection::ReviewRequired(safe_state) = rejection else {
        panic!("expected review-required rejection");
    };

    assert_eq!(safe_state.operation_id.0, "dismiss-review-active");
    assert!(safe_state.reason.contains("foreign-progress-identity"));
    assert_eq!(
        ws.safe_state.as_ref().map(|state| &state.operation_id),
        Some(&safe_state.operation_id)
    );
    assert!(ws.active_transfer_view().is_some());
    assert_eq!(
        notifications.load(Ordering::SeqCst),
        0,
        "dismiss did not pump"
    );
    for tail_id in [pending_id, paused_id] {
        assert_eq!(
            ws.queue_snapshot()
                .into_iter()
                .find(|row| row.id == tail_id)
                .map(|row| row.state),
            Some(crate::opqueue::JobState::Cancelled)
        );
    }

    ws.acknowledge_safe_state();
    let report = ws
        .try_dismiss_transfer(|| {})
        .expect("exact review allows the repeated dismiss");
    assert_eq!(report.operation_id.0, "dismiss-review-active");
    assert!(ws.active_transfer_view().is_none());
    assert!(ws.queue_snapshot().is_empty());
}

#[test]
fn faithfully_undoable_drops_keep_both_renames() {
    let pairs = vec![
        // A clean move kept its name and is reversible.
        (PathBuf::from("/src/a.txt"), PathBuf::from("/dst/a.txt")),
        // A Keep Both conflict landed at "b copy.txt" and is dropped.
        (
            PathBuf::from("/src/b.txt"),
            PathBuf::from("/dst/b copy.txt"),
        ),
    ];
    let kept = faithfully_undoable(pairs);
    assert_eq!(
        kept,
        vec![(PathBuf::from("/src/a.txt"), PathBuf::from("/dst/a.txt"))]
    );
}

#[test]
fn keep_both_move_undo_does_not_relocate_the_existing_file() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("dup.txt", "moved"); // source to move
    r.file("dup.txt", "existing"); // name conflict in the destination
    let mut ws = workspace(&l, &r);
    // Select the source and move it into the right (inactive) panel.
    ws.left.select_path(l.path().join("dup.txt"));
    ws.request_move();
    // Resolve the conflict as Keep Both: the moved file lands at "dup copy.txt".
    assert!(ws.resolve_pending_conflicts(crate::conflict::RelationPolicy::KeepBoth));
    ws.start_transfer(|| {});
    drain_transfers(&mut ws);

    assert_eq!(
        std::fs::read_to_string(r.path().join("dup.txt")).unwrap(),
        "existing",
        "the pre-existing destination is left intact"
    );
    assert_eq!(
        std::fs::read_to_string(r.path().join("dup copy.txt")).unwrap(),
        "moved",
        "the moved file landed under a Keep Both name"
    );
    assert!(!l.path().join("dup.txt").exists(), "source moved out");

    // A Keep Both rename is not faithfully reversible, so no undo is recorded
    // and Cmd+Z must not relocate the pre-existing file (the old bug did).
    assert!(
        ws.top_undo_action().is_none(),
        "no bogus undo recorded for an all-KeepBoth move"
    );
    let _ = ws.perform_undo(|| {});
    drain_transfers(&mut ws);
    assert_eq!(
        std::fs::read_to_string(r.path().join("dup.txt")).unwrap(),
        "existing",
        "undo left the existing file in place"
    );
}

#[test]
fn cancelling_a_pipeline_drops_its_paused_tail() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let a = l.file("a.txt", "AAA");
    let b = l.file("b.txt", "BBBB");
    let mut ws = workspace(&l, &r);
    let entry = |p: &std::path::Path| {
        let m = std::fs::metadata(p).unwrap();
        FileEntry::from_meta(p.to_path_buf(), &m).unwrap()
    };
    ws.start_copy(
        vec![entry(&a)],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
        || {},
    );
    ws.start_copy(
        vec![entry(&b)],
        r.path().to_path_buf(),
        OverwritePolicy::KeepBoth,
        || {},
    );
    let tail_id = ws.queue_snapshot()[1].id;
    ws.queue_pause(tail_id);
    assert_eq!(ws.queued_count(), 0, "paused tail is not runnable");
    assert_eq!(ws.unfinished_queue_count(), 2);

    // Simulate the user cancelling the active transfer and the worker
    // stopping: flag it cancelled+finished, then poll.
    {
        let st = ws.active_transfer().cloned().unwrap();
        let mut s = st.lock().unwrap();
        s.cancelled = true;
        s.finished = true;
    }
    ws.poll_transfer(|| {});

    assert!(ws.active_transfer().is_none(), "cancelled transfer closed");
    assert_eq!(ws.unfinished_queue_count(), 0);
    assert!(ws.queue_snapshot().is_empty());
    assert!(
        !r.path().join("b.txt").exists(),
        "the queued copy never started"
    );
}

#[test]
fn find_duplicates_groups_identical_files_in_active_dir() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "same content");
    l.file("b.txt", "same content"); // byte-identical dup of a
    l.file("c.txt", "unique bytes"); // same length, different bytes
    l.file("d.txt", "x"); // unique size
    let ws = workspace(&l, &r);

    let groups = ws.find_duplicates();
    assert_eq!(groups.len(), 1, "only a.txt/b.txt are byte-identical");
    assert_eq!(groups[0].files.len(), 2);
    let names: Vec<String> = groups[0]
        .files
        .iter()
        .filter_map(|f| f.path.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    assert!(names.contains(&"a.txt".to_string()));
    assert!(names.contains(&"b.txt".to_string()));
}

#[test]
fn find_duplicates_skips_empty_files_and_hard_links() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("__init__.py", "");
    l.file(".gitkeep", "");
    let original = l.file("original.bin", "shared bytes");
    std::fs::hard_link(&original, l.path().join("link.bin")).unwrap();
    let mut ws = workspace(&l, &r);
    if !ws.left.show_hidden() {
        let _ = ws.left.toggle_hidden();
    }

    // Empty markers are not clutter, and one inode frees nothing when one
    // of its names is trashed.
    assert!(ws.find_duplicates().is_empty());

    l.file("copy.bin", "shared bytes");
    ws.left.refresh();
    let groups = ws.find_duplicates();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].files.len(), 2, "{:?}", groups[0].files);
}

#[test]
fn duplicate_groups_are_confirmed_by_the_full_digest() {
    let key = |name: &str| crate::dedup::FileKey {
        path: PathBuf::from(format!("/dups/{name}")),
        identity: crate::panel::ListingIdentity::Unavailable,
        size: 4,
        // Every key shares one 64-bit prefix, as in a prefix collision.
        hash: 7,
        modified: None,
    };
    let group = crate::dedup::DupGroup {
        size: 4,
        files: vec![key("a"), key("b"), key("c"), key("d")],
    };
    let digests: std::collections::HashMap<PathBuf, crate::verified_hash::VerifiedHash> = [
        ("a", [1_u8; 32]),
        ("b", [2_u8; 32]),
        ("c", [1_u8; 32]),
        // "d" could not be hashed and must never be offered.
    ]
    .into_iter()
    .map(|(name, digest)| (PathBuf::from(format!("/dups/{name}")), digest))
    .collect();

    let confirmed = Workspace::confirm_dup_group(group, &digests);

    assert_eq!(confirmed.len(), 1);
    let paths: Vec<&Path> = confirmed[0]
        .files
        .iter()
        .map(|f| f.path.as_path())
        .collect();
    assert_eq!(paths, [Path::new("/dups/a"), Path::new("/dups/c")]);
}

#[test]
fn diff_targets_picks_two_selected_or_same_named() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let a = l.file("a.txt", "1");
    let b = l.file("b.txt", "2");
    r.file("a.txt", "9"); // same name on the other side
    let mut ws = workspace(&l, &r);

    // Two selected in the active panel -> that pair.
    ws.left.select_path(a.clone());
    ws.left.select_path(b.clone());
    let (x, y) = ws.diff_targets().unwrap();
    let names: Vec<String> = [&x, &y]
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    assert!(names.contains(&"a.txt".to_string()) && names.contains(&"b.txt".to_string()));

    // One selected -> pair with the same-named file in the other panel.
    ws.left.clear_selection();
    ws.left.select_path(a.clone());
    let (x, y) = ws.diff_targets().unwrap();
    assert_eq!(y, a, "active file is the second target");
    assert_eq!(x, r.path().join("a.txt"), "other-panel same name is first");
}

#[test]
fn diff_pairs_one_selected_file_per_pane_and_refuses_three() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let a = l.file("a.txt", "1");
    let b = l.file("b.txt", "2");
    let c = l.file("c.txt", "3");
    let other = r.file("other.txt", "9");
    let mut ws = workspace(&l, &r);

    // One selected file in each pane, names differ: that pair.
    ws.left.select_path(a.clone());
    ws.right.select_path(other.clone());
    assert_eq!(ws.diff_targets(), Some((other, a.clone())));
    assert!(ws.command_context().can_diff);

    // Three selected: no pair, and the command says so.
    ws.left.select_path(b);
    ws.left.select_path(c);
    assert_eq!(ws.diff_targets(), None);
    assert!(!ws.command_context().can_diff);
}

#[test]
fn select_same_named_matches_rows_the_other_filter_hides() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let shared = l.file("shared.txt", "l");
    r.file("shared.txt", "r");
    r.file("visible.txt", "r");
    let mut ws = workspace(&l, &r);
    ws.right.set_search_query("visible");

    ws.execute(Command::SelectSameNamed);

    assert!(ws.left.is_selected(&shared));
}

#[test]
fn move_within_one_folder_explains_instead_of_listing_conflicts() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "x");
    let mut ws = workspace(&l, &r);
    ws.right.navigate_to(l.path().to_path_buf());
    ws.left.set_cursor(1);
    ws.drain_ui_requests();

    ws.execute(Command::RequestMove);

    assert!(ws.pending_op.is_none());
    assert_eq!(
        ws.drain_ui_requests(),
        vec![UiRequest::Notice {
            message: "Both panes show this folder: open the destination in the other pane first"
                .to_string(),
            error: false,
        }]
    );
    // Copy still offers its conflict sheet (Keep Both duplicates).
    ws.execute(Command::RequestCopy);
    assert!(ws.pending_op.is_some());
}

#[test]
fn get_info_fills_in_a_folder_size_once_it_is_measured() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let folder = l.dir("box");
    l.file("box/a.bin", "12345");
    let mut ws = workspace(&l, &r);
    let entry = ws
        .left
        .entries()
        .iter()
        .find(|entry| entry.name == "box")
        .cloned()
        .unwrap();
    // As built while the size was still being measured.
    let card = crate::panel::make_info(&entry, None, None);
    assert_eq!(card.size, crate::panel::MEASURING);
    ws.right.preview = Some(PreviewContent::Info(card));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while ws.left.size_snapshot().size_of(&folder).is_none()
        || ws.left.size_snapshot().count_of(&folder).is_none()
    {
        assert!(
            std::time::Instant::now() < deadline,
            "folder never measured"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    ws.sync_preview();

    let Some(PreviewContent::Info(card)) = &ws.right.preview else {
        panic!("info card stays open");
    };
    assert_eq!(card.size, "5 B");
    assert_eq!(card.children, Some(1));
    assert_eq!(card.pending, None);
}

#[test]
fn select_by_relation_picks_only_here_and_differing() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("only.txt", "x"); // only in the active (left) panel
    l.file("both.txt", "AAA"); // present both sides, different size -> differing
    r.file("both.txt", "BBBBB");
    let mut ws = workspace(&l, &r);
    ws.left.refresh();
    ws.right.refresh();

    ws.execute(Command::SelectOnlyHere);
    assert_eq!(
        ws.left.selected_paths().clone(),
        [l.path().join("only.txt")].into_iter().collect()
    );

    ws.execute(Command::SelectDiffering);
    assert_eq!(
        ws.left.selected_paths().clone(),
        [l.path().join("both.txt")].into_iter().collect()
    );
}

#[test]
fn select_by_relation_respects_the_active_filter() {
    let (l, r) = (TempDir::new(), TempDir::new());
    // Both differ from the other side; only "alpha" will be visible.
    l.file("alpha.txt", "A");
    r.file("alpha.txt", "AA");
    l.file("beta.txt", "B");
    r.file("beta.txt", "BB");
    let mut ws = workspace(&l, &r);
    ws.left.refresh();
    ws.right.refresh();
    // Narrow the active view to just "alpha".
    ws.left.set_search_query("alpha");

    ws.execute(Command::SelectDiffering);
    // Only the visible differing entry is selected; the filtered-out
    // "beta.txt" is not, even though it also differs.
    assert_eq!(
        ws.left.selected_paths().clone(),
        [l.path().join("alpha.txt")].into_iter().collect()
    );
}

#[test]
fn jump_slot_navigates_active_panel_to_the_bookmarked_dir() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let project = l.dir("project");
    let mut ws = workspace(&l, &r);
    // Bookmark `project` in slot 1 (set the store directly; the execute
    // path for AssignSlot persists to the real config, so it is not used
    // in tests).
    ws.bookmarks.add("project", project.clone());
    assert!(ws.bookmarks.assign_slot(&project, 1));

    // Active panel elsewhere, then Cmd+1 jumps it to the bookmark.
    ws.left.navigate_to(l.path().to_path_buf());
    assert_eq!(ws.active_panel_ref().current_path, l.path());
    ws.execute(Command::JumpSlot(1));
    assert_eq!(ws.active_panel_ref().current_path, project);

    // An empty slot is a no-op (no panic, no navigation).
    let before = ws.active_panel_ref().current_path.clone();
    ws.execute(Command::JumpSlot(7));
    assert_eq!(ws.active_panel_ref().current_path, before);
}

#[test]
fn jump_slot_reports_a_bookmark_whose_directory_disappeared() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let missing = left.dir("removed-project");
    let mut workspace = workspace(&left, &right);
    workspace.bookmarks.add("removed", missing.clone());
    assert!(workspace.bookmarks.assign_slot(&missing, 2));
    std::fs::remove_dir(&missing).unwrap();

    let before = workspace.active_panel_ref().current_path.clone();
    workspace.execute(Command::JumpSlot(2));

    assert_eq!(workspace.active_panel_ref().current_path, before);
    assert_eq!(
        workspace.drain_ui_requests(),
        vec![UiRequest::Notice {
            message: format!("Quick jump 2 is unavailable: {}", missing.display()),
            error: true,
        }]
    );
}

#[test]
fn move_pairs_maps_source_to_dest() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let a = l.file("a.txt", "x");
    let b = l.file("b.txt", "y");
    let meta_a = std::fs::metadata(&a).unwrap();
    let meta_b = std::fs::metadata(&b).unwrap();
    let entries = vec![
        FileEntry::from_meta(a.clone(), &meta_a).unwrap(),
        FileEntry::from_meta(b.clone(), &meta_b).unwrap(),
    ];
    let pairs = move_pairs(&entries, r.path());
    // (from, to): from the entry's current path to target/name.
    assert_eq!(pairs[0], (a, r.path().join("a.txt")));
    assert_eq!(pairs[1], (b, r.path().join("b.txt")));
}

#[test]
fn move_then_undo_restores_the_source() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let f = l.file("doc.txt", "data");
    let mut ws = workspace(&l, &r);
    ws.left.set_cursor(1);

    ws.execute(Command::RequestMove);
    ws.finish_space_probe();
    ws.confirm_pending_op(|| {});
    wait_transfer(&mut ws);
    assert!(!f.exists(), "move removed the source");
    assert!(r.path().join("doc.txt").exists());
    assert!(ws.can_undo(), "a clean move is undoable");

    let _ = ws.perform_undo(|| {});
    wait_transfer(&mut ws);
    assert!(f.exists(), "undo restored the source");
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "data");
    assert!(
        !r.path().join("doc.txt").exists(),
        "undo emptied the target"
    );

    // Redo re-applies the move.
    assert!(ws.can_redo(), "the undone move is redoable");
    let _ = ws.perform_redo(|| {});
    wait_transfer(&mut ws);
    assert!(!f.exists(), "redo re-moved the source away");
    assert!(
        r.path().join("doc.txt").exists(),
        "redo restored the target"
    );
}

#[test]
fn batch_rename_is_undoable_and_redoable() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "1");
    l.file("b.txt", "2");
    let mut ws = workspace(&l, &r);
    ws.left.select_path(l.path().join("a.txt"));
    ws.left.select_path(l.path().join("b.txt"));

    let rule = crate::rename::RenameRule {
        prefix: "x_".into(),
        ..Default::default()
    };
    assert_eq!(apply_batch_rename(&mut ws, &rule).unwrap(), 2);
    assert!(l.path().join("x_a.txt").is_file());
    assert!(ws.can_undo());

    let _ = ws.perform_undo(|| {});
    assert!(l.path().join("a.txt").is_file(), "undo restored names");
    assert!(!l.path().join("x_a.txt").exists());

    let _ = ws.perform_redo(|| {});
    assert!(l.path().join("x_a.txt").is_file(), "redo re-applied names");
    assert!(!l.path().join("a.txt").exists());
}

#[test]
fn pending_transfer_overflow_logic() {
    let mk = |kind: TransferKind, method: CopyMethod, need: u64, free: Option<u64>, same: bool| {
        PendingTransfer {
            kind,
            entries: vec![],
            expectations: vec![],
            target: PathBuf::from("/t"),
            conflicts: vec![],
            policy: OverwritePolicy::Ask,
            method,
            durability: crate::operation::DurabilityProfile::Fast,
            version_retention: crate::operation::VersionRetentionPolicy::default(),
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            filesystem: filesystem_preflight(&[], Path::new("/"), Default::default()),
            flat: scan::pending_flat_list(),
            space: TransferSpaceState::Ready {
                generation: 1,
                need_bytes: need,
                free: free.map_or_else(
                    || {
                        crate::ports::SpaceProbeOutcome::Unknown(
                            crate::ports::NativeFailure::unsupported("probe unavailable"),
                        )
                    },
                    |bytes| crate::ports::SpaceProbeOutcome::Known {
                        bytes,
                        precision: crate::ports::SpacePrecision::Exact,
                    },
                ),
                relation: if same {
                    crate::ports::VolumeRelation::Same
                } else {
                    crate::ports::VolumeRelation::Different
                },
            },
            start_when_ready: false,
        }
    };
    use CopyMethod::{Buffered, Native};
    // Cross-volume copy needing more than free overflows.
    assert!(mk(TransferKind::Copy, Native, 100, Some(50), false).overflows());
    // Cross-volume copy that fits does not.
    assert!(!mk(TransferKind::Copy, Native, 40, Some(50), false).overflows());
    // Same-volume move never overflows (instant rename).
    assert!(!mk(TransferKind::Move, Native, 100, Some(50), true).overflows());
    // Cross-volume move behaves like copy.
    assert!(mk(TransferKind::Move, Native, 100, Some(50), false).overflows());
    // Unknown free space is non-blocking but explicitly indeterminate.
    assert!(!mk(TransferKind::Copy, Native, 100, None, false).overflows());
    assert_eq!(
        mk(TransferKind::Copy, Native, 100, None, false).space_verdict(),
        crate::fs_util::SpaceVerdict::Indeterminate
    );

    // Native copy budgets its full logical size because clonefile may fall
    // back to a byte copy.
    let clone = mk(TransferKind::Copy, Native, 1_000, Some(10), true);
    assert!(clone.overflows());
    assert!(!clone.needs_no_space());
    // A same-volume BUFFERED copy writes every byte, so it can overflow.
    assert!(mk(TransferKind::Copy, Buffered, 1_000, Some(10), true).overflows());
    assert!(!mk(TransferKind::Copy, Buffered, 1_000, Some(10), true).needs_no_space());
    // A normal same-volume move can rename without allocating data.
    assert!(mk(TransferKind::Move, Native, 1_000, Some(10), true).needs_no_space());
    let mut followed_move = mk(TransferKind::Move, Native, 1_000, Some(10), true);
    followed_move.symlink_policy = crate::filesystem_policy::SymlinkPolicy::Follow;
    assert!(!followed_move.needs_no_space());
    assert!(matches!(
        followed_move.space_verdict(),
        crate::fs_util::SpaceVerdict::WontFit { .. }
    ));
    assert_eq!(
        mk(TransferKind::Move, Native, 1_000, None, true).space_verdict(),
        crate::fs_util::SpaceVerdict::Indeterminate
    );
    let mut same_volume_move = mk(TransferKind::Move, Native, 1_000, None, true);
    same_volume_move.start_when_ready = true;
    assert!(should_auto_start_after_preflight(&same_volume_move));

    let mut unknown_copy = mk(TransferKind::Copy, Native, 1_000, None, false);
    unknown_copy.start_when_ready = true;
    assert!(!should_auto_start_after_preflight(&unknown_copy));
}

#[test]
fn conflict_resolution_recomputes_the_space_budget() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let conflict = l.file("conflict.txt", &"x".repeat(100));
    let fresh = l.file("fresh.txt", &"y".repeat(10));
    r.file("conflict.txt", "existing");
    let mut ws = workspace(&l, &r);
    ws.left.extend_selection([conflict, fresh.clone()]);
    ws.request_copy();

    let Some(PendingOp::Transfer(tr)) = &mut ws.pending_op else {
        panic!("copy should be pending");
    };
    tr.method = CopyMethod::Buffered;
    tr.space = TransferSpaceState::Ready {
        generation: 1,
        need_bytes: 110,
        free: crate::ports::SpaceProbeOutcome::Known {
            bytes: 10,
            precision: crate::ports::SpacePrecision::Exact,
        },
        relation: crate::ports::VolumeRelation::Different,
    };
    assert_eq!(tr.need_bytes(), Some(110));
    assert!(tr.overflows());

    assert!(ws.resolve_pending_conflicts(crate::conflict::RelationPolicy::SkipAll));
    let Some(PendingOp::Transfer(tr)) = &ws.pending_op else {
        panic!("non-conflicting copy should remain pending");
    };
    assert_eq!(tr.entries.len(), 1);
    assert_eq!(tr.entries[0].path, fresh);
    assert!(matches!(tr.space, TransferSpaceState::Pending { .. }));
    assert!(tr.conflicts.is_empty());
}

#[test]
fn transfer_confirmation_waits_for_one_complete_background_resource_snapshot() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let directory = left.dir("folder");
    left.file("folder/nested.bin", "1234567");
    let file = left.file("plain.bin", "12345");
    let mut workspace = workspace(&left, &right);
    workspace
        .left
        .extend_selection([directory.clone(), file.clone()]);
    workspace.request_copy();

    let generation = match &workspace.pending_op {
        Some(PendingOp::Transfer(transfer)) => {
            assert!(!transfer.space_ready());
            assert!(transfer.expectations.is_empty());
            match transfer.space {
                TransferSpaceState::Pending { generation } => generation,
                TransferSpaceState::Ready { .. } | TransferSpaceState::Failed { .. } => {
                    unreachable!()
                }
            }
        }
        _ => panic!("copy should be pending"),
    };
    assert!(!workspace.confirm_pending_op(|| {}));
    assert!(matches!(workspace.pending_op, Some(PendingOp::Transfer(_))));

    workspace.finish_space_probe();
    let Some(PendingOp::Transfer(transfer)) = &workspace.pending_op else {
        panic!("copy should remain pending after preflight");
    };
    assert!(transfer.space_ready());
    assert_eq!(transfer.need_bytes(), Some(12));
    assert_eq!(transfer.expectations.len(), transfer.entries.len());
    assert!(matches!(
        transfer.space,
        TransferSpaceState::Ready {
            generation: ready_generation,
            ..
        } if ready_generation == generation
    ));
}

#[test]
fn replacement_before_worker_scan_fails_stale_without_starting_transfer_backend() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let source = left.file("source.txt", "original");
    let replacement = left.file("replacement.txt", "replacement");
    let mut workspace = workspace(&left, &right);
    workspace.left.extend_selection([source.clone()]);
    let hook_source = source.clone();
    workspace.set_space_probe_before_scan(Arc::new(move || {
        std::fs::rename(&replacement, &hook_source).unwrap();
    }));

    workspace.request_copy();
    workspace.finish_space_probe();

    let Some(PendingOp::Transfer(transfer)) = &workspace.pending_op else {
        panic!("stale transfer should remain pending for review");
    };
    assert!(matches!(
        &transfer.space,
        TransferSpaceState::Failed { failure, .. }
            if failure.kind == crate::ports::NativeFailureKind::Stale
    ));
    assert!(transfer.expectations.is_empty());
    assert!(!workspace.confirm_pending_op(|| {}));
    assert!(workspace.active_transfer_view().is_none());
    assert!(!right.path().join("source.txt").exists());
    assert_eq!(std::fs::read_to_string(source).unwrap(), "replacement");
}

#[cfg(unix)]
#[test]
fn changing_symlink_policy_restarts_the_bound_size_snapshot() {
    let (left, right) = (TempDir::new(), TempDir::new());
    left.file("target/value.bin", "1234567");
    let link = left.path().join("linked");
    std::os::unix::fs::symlink("target", &link).unwrap();
    let mut workspace = Workspace::with_ports(
        left.path().to_path_buf(),
        right.path().to_path_buf(),
        Arc::new(TestTrashPort),
        Arc::new(FixedFreeSpacePort {
            outcome: crate::ports::SpaceProbeOutcome::Known {
                bytes: u64::MAX,
                precision: crate::ports::SpacePrecision::Exact,
            },
            relation: crate::ports::VolumeRelation::Different,
        }),
    );
    workspace.left.refresh();
    workspace.right.refresh();
    workspace.left.select_path(link);
    workspace.request_copy();
    workspace.finish_space_probe();

    let previous_generation = match &workspace.pending_op {
        Some(PendingOp::Transfer(PendingTransfer {
            space: TransferSpaceState::Ready { generation, .. },
            ..
        })) => *generation,
        _ => panic!("initial preflight should be ready"),
    };
    assert!(
        workspace
            .set_pending_symlink_policy(crate::filesystem_policy::SymlinkPolicy::Follow, || {},)
    );
    assert!(matches!(
        &workspace.pending_op,
        Some(PendingOp::Transfer(PendingTransfer {
            space: TransferSpaceState::Pending { generation },
            ..
        })) if *generation != previous_generation
    ));

    workspace.finish_space_probe();
    let Some(PendingOp::Transfer(transfer)) = &workspace.pending_op else {
        panic!("updated preflight should remain pending confirmation");
    };
    assert_eq!(transfer.need_bytes(), Some(7));
}

#[test]
fn stale_space_report_cannot_bind_to_a_newer_pending_plan() {
    let (left, right) = (TempDir::new(), TempDir::new());
    left.file("value.txt", "123");
    let mut workspace = workspace(&left, &right);
    workspace.left.set_cursor(1);
    workspace.request_copy();
    let (generation, target) = match &workspace.pending_op {
        Some(PendingOp::Transfer(transfer)) => match transfer.space {
            TransferSpaceState::Pending { generation } => (generation, transfer.target.clone()),
            TransferSpaceState::Ready { .. } | TransferSpaceState::Failed { .. } => {
                unreachable!()
            }
        },
        _ => panic!("copy should be pending"),
    };

    workspace.apply_space_probe(
        space_probe::SpaceProbeReport {
            generation: generation.saturating_add(1),
            target,
            need_bytes: Ok(0),
            expectations: Ok(Vec::new()),
            free: crate::ports::SpaceProbeOutcome::Known {
                bytes: u64::MAX,
                precision: crate::ports::SpacePrecision::Exact,
            },
            relation: crate::ports::VolumeRelation::Same,
        },
        || {},
    );

    assert!(matches!(
        &workspace.pending_op,
        Some(PendingOp::Transfer(PendingTransfer {
            space: TransferSpaceState::Pending {
                generation: retained
            },
            ..
        })) if *retained == generation
    ));
    workspace.finish_space_probe();
}

#[test]
fn unknown_free_space_is_ready_but_never_fits() {
    let (left, right) = (TempDir::new(), TempDir::new());
    left.file("value.txt", "123");
    let mut workspace = Workspace::with_ports(
        left.path().to_path_buf(),
        right.path().to_path_buf(),
        Arc::new(TestTrashPort),
        Arc::new(FixedFreeSpacePort {
            outcome: crate::ports::SpaceProbeOutcome::Unknown(
                crate::ports::NativeFailure::unsupported("probe unavailable"),
            ),
            relation: crate::ports::VolumeRelation::Different,
        }),
    );
    workspace.left.refresh();
    workspace.right.refresh();
    workspace.left.set_cursor(1);
    workspace.request_copy();
    workspace.finish_space_probe();

    let Some(PendingOp::Transfer(transfer)) = &workspace.pending_op else {
        panic!("copy should be pending");
    };
    assert!(transfer.space_ready());
    assert_eq!(
        transfer.space_verdict(),
        crate::fs_util::SpaceVerdict::Indeterminate
    );
    assert!(!transfer.overflows());
}

#[test]
fn conflict_free_drag_does_not_auto_start_with_unknown_space() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let file = left.file("value.txt", "123");
    let mut workspace = Workspace::with_ports(
        left.path().to_path_buf(),
        right.path().to_path_buf(),
        Arc::new(TestTrashPort),
        Arc::new(FixedFreeSpacePort {
            outcome: crate::ports::SpaceProbeOutcome::Unknown(
                crate::ports::NativeFailure::unsupported("probe unavailable"),
            ),
            relation: crate::ports::VolumeRelation::Different,
        }),
    );
    workspace.left.refresh();
    workspace.right.refresh();
    workspace.left.drag.set(vec![file]);
    workspace
        .right
        .drag
        .set_drop_target(right.path().to_path_buf());

    workspace.drop_dragged(|| {});
    workspace.finish_space_probe();

    assert!(workspace.active_transfer().is_none());
    let Some(PendingOp::Transfer(transfer)) = &workspace.pending_op else {
        panic!("indeterminate drag should remain pending");
    };
    assert_eq!(
        transfer.space_verdict(),
        crate::fs_util::SpaceVerdict::Indeterminate
    );
}

#[test]
fn begin_rename_targets_the_cursor_entry() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let f = l.file("a.txt", "x");
    let mut ws = workspace(&l, &r);
    ws.left.set_cursor(1);

    ws.execute(Command::BeginRename);
    assert_eq!(ws.pending_ui_requests(), vec![UiRequest::Rename(f)]);
}

#[test]
fn commit_rename_moves_the_file_and_follows_cursor() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let f = l.file("old.txt", "data");
    let mut ws = workspace(&l, &r);
    ws.left.set_cursor(1);

    ws.commit_rename(&f, "new.txt").unwrap();

    assert!(!f.exists());
    let renamed = l.path().join("new.txt");
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "data");
    // Cursor follows the renamed file by path.
    assert_eq!(
        ws.left.filtered_get(ws.left.cursor() - 1).unwrap().name,
        "new.txt"
    );
}

#[test]
fn commit_rename_is_undoable_and_redoable() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let old = l.file("old.txt", "data");
    let new = l.path().join("new.txt");
    let mut ws = workspace(&l, &r);

    ws.commit_rename(&old, "new.txt").unwrap();
    assert!(ws.can_undo());
    ws.perform_undo(|| {}).unwrap();
    assert!(old.is_file());
    assert!(!new.exists());

    ws.perform_redo(|| {}).unwrap();
    assert!(new.is_file());
    assert!(!old.exists());
}

#[test]
fn commit_rename_does_not_follow_the_active_panel() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let left = l.file("left.txt", "left");
    r.file("right.txt", "right");
    let mut ws = workspace(&l, &r);
    ws.active = ActivePanel::Right;

    ws.commit_rename(&left, "renamed.txt").unwrap();

    assert!(l.path().join("renamed.txt").is_file());
    assert!(r.path().join("right.txt").is_file());
    assert!(!r.path().join("renamed.txt").exists());
}

#[test]
fn commit_rename_rejects_a_collision_without_touching_disk() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let f = l.file("a.txt", "A");
    l.file("b.txt", "B");
    let mut ws = workspace(&l, &r);

    let err = ws.commit_rename(&f, "b.txt");
    assert!(err.is_err());
    assert!(f.exists(), "source untouched on collision");
    assert_eq!(
        std::fs::read_to_string(l.path().join("b.txt")).unwrap(),
        "B"
    );
}

#[test]
fn commit_rename_refuses_to_clobber_a_file_only_on_disk() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let a = l.file("a.txt", "1");
    let mut ws = workspace(&l, &r);
    // Create the destination on disk AFTER the panel was loaded, so it is
    // not in the in-memory sibling list, exercising the disk probe.
    l.file("b.txt", "2");
    let err = ws.commit_rename(&a, "b.txt");
    assert!(err.is_err(), "must refuse to overwrite an existing file");
    assert_eq!(
        std::fs::read_to_string(l.path().join("b.txt")).unwrap(),
        "2"
    );
    assert!(a.exists(), "source untouched on refusal");
}

#[test]
fn commit_rename_rejects_nul_before_touching_disk_or_history() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let original = left.file("original.txt", "content");
    let mut workspace = workspace(&left, &right);

    let error = workspace
        .commit_rename(&original, "bad\0name.txt")
        .unwrap_err();

    assert_eq!(error, "Name cannot contain NUL");
    assert!(original.is_file());
    assert!(!workspace.can_undo());
}

#[test]
fn commit_rename_uses_the_same_trimmed_basename_as_live_validation() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let original = left.file("original.txt", "content");
    let mut workspace = workspace(&left, &right);

    workspace
        .commit_rename(&original, "  renamed.txt  ")
        .unwrap();

    assert!(left.path().join("renamed.txt").is_file());
    assert!(!left.path().join("  renamed.txt  ").exists());
}

#[test]
fn apply_rename_order_refuses_to_clobber_unrelated_target() {
    let tmp = TempDir::new();
    tmp.file("a.txt", "1");
    tmp.file("b.txt", "2"); // not part of the batch
    let map = vec![("a.txt".to_string(), "b.txt".to_string())];
    let existing = fileops::dir_names(tmp.path());
    let r = fileops::apply_rename_order(tmp.path(), &map, &existing);
    assert!(r.is_err(), "renaming onto an untouched sibling is refused");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "1"
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("b.txt")).unwrap(),
        "2"
    );
}

#[test]
fn batch_rename_undo_surfaces_a_failed_rename() {
    // A rename whose target clobbers an unrelated sibling is refused by
    // apply_rename_order; the undo/redo path (execute_action) must surface
    // that error instead of swallowing it via `let _ =` (audit #20).
    let (l, r) = (TempDir::new(), TempDir::new());
    let dir = l.path().to_path_buf();
    l.file("a.txt", "1");
    l.file("b.txt", "2"); // unrelated existing target the rename would clobber
    let mut ws = workspace(&l, &r);
    let action = crate::undo::Action::BatchRename {
        dir: dir.clone(),
        pairs: vec![("a.txt".to_string(), "b.txt".to_string())],
    };
    ws.record_history_for_test(crate::undo::invert(&action).unwrap());
    let plan = ws.begin_history_replay_for_test(crate::undo::ReplayDirection::Undo);
    assert_eq!(plan.action, action);
    let result = ws.execute_action(plan.action, plan.reservation, || {});
    assert!(
        result.is_err(),
        "a clobbering rename during undo must surface an error, not be swallowed"
    );
    // The refusal leaves both files untouched.
    assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "1");
    assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "2");
}

#[test]
fn failed_batch_rename_rollback_enters_safe_state_without_advancing_history() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let mut names = ["a", "b"]
        .into_iter()
        .map(str::to_string)
        .collect::<std::collections::HashSet<_>>();
    let map = vec![
        ("a".to_string(), "x".to_string()),
        ("b".to_string(), "y".to_string()),
    ];
    let existing = names.clone();
    let error = fileops::apply_rename_order_using(left.path(), &map, &existing, |from, to| {
        if (from, to) == ("b", "y") {
            return Err("forward failure");
        }
        if (from, to) == ("x", "a") {
            return Err("rollback failure");
        }
        assert!(names.remove(from));
        names.insert(to.to_string());
        Ok(())
    })
    .unwrap_err();
    assert!(error.integrity_uncertain);
    assert_eq!(
        names,
        ["x", "b"]
            .into_iter()
            .map(str::to_string)
            .collect::<std::collections::HashSet<_>>()
    );

    let mut workspace = workspace(&left, &right);
    workspace.record_history_for_test(crate::undo::Action::BatchRename {
        dir: left.path().to_path_buf(),
        pairs: map,
    });
    let replay = workspace.begin_history_replay_for_test(crate::undo::ReplayDirection::Undo);
    fileops::latch_rename_execution_error(&mut workspace, &error);
    workspace.undo.abort_immediate(replay.reservation).unwrap();

    assert!(workspace.can_undo());
    assert!(!workspace.can_redo());
    assert!(workspace.safe_state.is_some());
    assert!(workspace.mutations_blocked());
}

#[test]
fn blocked_undo_keeps_the_history_pointer_unchanged() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let original = l.path().join("old.txt");
    let renamed = l.file("new.txt", "completed rename");
    std::fs::write(&original, "foreign replacement").unwrap();
    let mut ws = workspace(&l, &r);
    ws.record_history_for_test(crate::undo::Action::Rename {
        from: original,
        to: renamed,
    });

    let error = ws.perform_undo(|| {}).unwrap_err();
    assert!(error.contains("occupied"), "{error}");
    assert!(ws.can_undo());
    assert!(!ws.can_redo());
}

#[test]
fn async_history_transition_commits_only_for_a_clean_worker() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let mut ws = workspace(&l, &r);
    ws.record_history_for_test(crate::undo::Action::Rename {
        from: l.path().join("a.txt"),
        to: l.path().join("b.txt"),
    });
    let undo = ws.begin_history_replay_for_test(crate::undo::ReplayDirection::Undo);
    let clean = ws.launch_test_transfer(
        test_transfer_spec("clean-undo", r.path()),
        transfer_queue::HistoryIntent::replay(undo.reservation),
    );
    crate::lock_util::recover(&clean).finished = true;

    ws.poll_transfer(|| {});
    assert!(!ws.can_undo());
    assert!(ws.can_redo());

    let replay_folder = l.path().join("gathered");
    std::fs::create_dir(&replay_folder).unwrap();
    let redo = ws.begin_history_replay_for_test(crate::undo::ReplayDirection::Redo);
    let failed = ws.launch_test_transfer(
        test_transfer_spec("failed-redo", r.path()),
        transfer_queue::HistoryIntent::replay(redo.reservation),
    );
    {
        let mut progress = crate::lock_util::recover(&failed);
        progress.finished = true;
        progress.errors.push("worker failed".to_string());
    }
    ws.poll_transfer(|| {});
    ws.dismiss_transfer(|| {});
    assert!(!ws.can_undo(), "failed redo was not committed");
    assert!(ws.can_redo());
    assert!(
        replay_folder.exists(),
        "history settlement must not remove a folder by path without executor ownership proof"
    );
}

#[test]
fn partial_async_replay_enters_recovery_state_without_advancing_or_unlocking_history() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let operation_id = crate::operation::OperationId("partial-history-replay".to_string());
    let source = left.path().join("source.txt");
    let landing = right.path().join("source.txt");
    let mut workspace = workspace(&left, &right);

    interrupt_history_replay(
        &mut workspace,
        &operation_id,
        right.path(),
        vec![(source, landing)],
    );

    assert!(workspace.can_undo());
    assert!(!workspace.can_redo());
    assert!(workspace.mutations_blocked());
    let safe_state = workspace
        .safe_state
        .as_ref()
        .expect("partial replay must require recovery");
    assert_eq!(safe_state.operation_id, operation_id);
    assert!(safe_state.reason.contains("resume or roll back"));

    workspace.acknowledge_safe_state();
    assert!(
        workspace.mutations_blocked(),
        "review acknowledgement must not discard an interrupted replay"
    );
}

#[test]
fn cancelling_a_waiting_replay_aborts_only_its_bound_reservation() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let mut workspace = workspace(&left, &right);
    workspace.record_history_for_test(crate::undo::Action::Rename {
        from: left.path().join("old.txt"),
        to: left.path().join("new.txt"),
    });
    let replay = workspace.begin_history_replay_for_test(crate::undo::ReplayDirection::Undo);
    workspace
        .enqueue_bound_replay(
            test_transfer_spec("waiting-history-replay", right.path()),
            transfer_queue::HistoryIntent::replay(replay.reservation),
        )
        .unwrap();
    let job_id = workspace.queue_snapshot()[0].id;
    workspace.queue_pause(job_id);

    workspace.queue_cancel(job_id);

    assert!(workspace.can_undo());
    assert!(!workspace.can_redo());
    assert!(!workspace.mutations_blocked());
    assert!(workspace.queue_snapshot().is_empty());
}

#[test]
fn matching_paused_replay_can_resume_while_ordinary_mutations_remain_blocked() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let mut workspace = workspace(&left, &right);
    workspace.record_history_for_test(crate::undo::Action::Rename {
        from: left.path().join("old.txt"),
        to: left.path().join("new.txt"),
    });
    let replay = workspace.begin_history_replay_for_test(crate::undo::ReplayDirection::Undo);
    workspace
        .enqueue_bound_replay(
            test_transfer_spec("paused-history-replay", right.path()),
            transfer_queue::HistoryIntent::replay(replay.reservation),
        )
        .unwrap();
    let job_id = workspace.queue_snapshot()[0].id;
    workspace.queue_pause(job_id);

    workspace.queue_resume(job_id, || {});

    assert_eq!(
        workspace
            .active_transfer_view()
            .map(|view| view.operation_id),
        Some(crate::operation::OperationId(
            "paused-history-replay".to_string()
        ))
    );
}

#[test]
fn journal_resume_keeps_the_original_replay_reservation_until_clean_completion() {
    let journal = TempDir::new();
    let _journal = crate::operation_journal::use_test_journal(journal.path().join("journal.json"));
    let (left, right) = (TempDir::new(), TempDir::new());
    let source = left.file("resume.txt", "payload");
    let destination = right.path().join("resume.txt");
    let entry =
        FileEntry::from_meta(source.clone(), &std::fs::symlink_metadata(&source).unwrap()).unwrap();
    let operation_id = crate::operation::OperationId("history-resume".to_string());
    let mut spec = test_transfer_spec(&operation_id.0, right.path());
    spec.entries = vec![entry.clone()];
    spec.expectations = transfer::capture_expectations(std::slice::from_ref(&entry), right.path());
    crate::operation_journal::begin(&spec).unwrap();
    crate::operation_journal::finish(
        &operation_id,
        crate::operation_journal::OperationStatus::Stopped,
    )
    .unwrap();
    let mut workspace = workspace(&left, &right);
    interrupt_history_replay(
        &mut workspace,
        &operation_id,
        right.path(),
        vec![(source.clone(), destination.clone())],
    );

    assert_eq!(workspace.resume_recovery(&operation_id, || {}), Ok(1));
    let progress = workspace
        .active_transfer_view()
        .expect("recovery replay should start")
        .progress;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !crate::lock_util::recover(&progress).finished {
        assert!(
            std::time::Instant::now() < deadline,
            "recovery replay did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    workspace.poll_transfer(|| {});

    assert!(!workspace.can_undo());
    assert!(workspace.can_redo());
    assert!(!workspace.mutations_blocked());
    assert!(!source.exists());
    assert!(destination.exists());
}

#[test]
fn journal_rollback_aborts_interrupted_replay_without_advancing_the_cursor() {
    let journal = TempDir::new();
    let _journal = crate::operation_journal::use_test_journal(journal.path().join("journal.json"));
    let (left, right) = (TempDir::new(), TempDir::new());
    let source = left.file("rollback.txt", "payload");
    let destination = right.path().join("rollback.txt");
    let entry =
        FileEntry::from_meta(source.clone(), &std::fs::symlink_metadata(&source).unwrap()).unwrap();
    let operation_id = crate::operation::OperationId("history-rollback".to_string());
    let mut spec = test_transfer_spec(&operation_id.0, right.path());
    spec.entries = vec![entry.clone()];
    spec.expectations = transfer::capture_expectations(std::slice::from_ref(&entry), right.path());
    crate::operation_journal::begin(&spec).unwrap();
    let key = crate::operation_journal::operation(&operation_id)
        .unwrap()
        .steps[0]
        .key
        .clone();
    crate::operation_journal::mark_running(
        &operation_id,
        &key,
        &right.path().join(".rollback-staging"),
        &destination,
        crate::path_identity::PathIdentity::missing(&destination),
    )
    .unwrap();
    crate::fs_at::rename_noreplace(&source, &destination).unwrap();
    crate::operation_journal::mark_completed(
        &operation_id,
        &key,
        &destination,
        crate::transfer_tuning::FastPath::Rename,
    )
    .unwrap();
    crate::operation_journal::finish(
        &operation_id,
        crate::operation_journal::OperationStatus::Failed,
    )
    .unwrap();
    let mut workspace = workspace(&left, &right);
    interrupt_history_replay(
        &mut workspace,
        &operation_id,
        right.path(),
        vec![(source.clone(), destination.clone())],
    );

    let plan = workspace.rollback_recovery(&operation_id).unwrap();

    assert!(plan.remaining.is_empty());
    assert!(workspace.can_undo());
    assert!(!workspace.can_redo());
    assert!(!workspace.mutations_blocked());
    assert!(source.exists());
    assert!(!destination.exists());
}

#[test]
fn filesystem_mutation_is_blocked_before_disk_while_async_replay_is_reserved() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let original = left.file("untouched.txt", "content");
    let renamed = left.path().join("changed.txt");
    let mut workspace = workspace(&left, &right);
    workspace.record_history_for_test(crate::undo::Action::Rename {
        from: left.path().join("old.txt"),
        to: left.path().join("new.txt"),
    });
    let replay = workspace.begin_history_replay_for_test(crate::undo::ReplayDirection::Undo);
    let _active = workspace.launch_test_transfer(
        test_transfer_spec("pending-history-replay", right.path()),
        transfer_queue::HistoryIntent::replay(replay.reservation),
    );

    let error = workspace
        .commit_rename(&original, "changed.txt")
        .expect_err("rename must be rejected before touching disk");

    assert!(error.contains("history replay"), "{error}");
    assert_eq!(std::fs::read_to_string(&original).unwrap(), "content");
    assert!(!renamed.exists());
    assert!(workspace.can_undo());
    assert!(!workspace.can_redo());
}

#[test]
fn foreign_async_replay_completion_enters_safe_state_without_advancing_history() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let mut workspace = workspace(&left, &right);
    workspace.record_history_for_test(crate::undo::Action::Rename {
        from: left.path().join("old.txt"),
        to: left.path().join("new.txt"),
    });
    let _current = workspace.begin_history_replay_for_test(crate::undo::ReplayDirection::Undo);

    let mut foreign = crate::undo::UndoCenter::default();
    foreign
        .record(crate::undo::Action::Rename {
            from: right.path().join("foreign-old.txt"),
            to: right.path().join("foreign-new.txt"),
        })
        .unwrap();
    let foreign_replay = foreign
        .begin(crate::undo::ReplayDirection::Undo)
        .unwrap()
        .unwrap();
    let active = workspace.launch_unbound_test_transfer(
        test_transfer_spec("foreign-history-outcome", right.path()),
        transfer_queue::HistoryIntent::replay(foreign_replay.reservation),
    );
    crate::lock_util::recover(&active).finished = true;

    workspace.poll_transfer(|| {});

    let safe_state = workspace
        .safe_state
        .as_ref()
        .expect("foreign settlement must fail closed");
    assert!(safe_state.reason.contains("reservation does not match"));
    assert!(workspace.can_undo());
    assert!(!workspace.can_redo());
    assert!(workspace.mutations_blocked());
}

#[test]
fn redo_invalidation_names_the_later_action() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let mut ws = workspace(&l, &r);
    ws.record_history_for_test(crate::undo::Action::Rename {
        from: l.path().join("old.txt"),
        to: l.path().join("new.txt"),
    });
    let undo = ws.begin_history_replay_for_test(crate::undo::ReplayDirection::Undo);
    ws.commit_history_replay_for_test(undo.reservation);
    ws.record_history_for_test(crate::undo::Action::Move {
        pairs: vec![(l.path().join("a.txt"), r.path().join("a.txt"))],
    });

    let reason = ws.redo_unavailable_reason().unwrap();
    assert!(reason.contains("Moved (1 item)"), "{reason}");
    assert!(reason.contains("1 undone action"), "{reason}");
    assert_eq!(ws.perform_redo(|| {}).unwrap_err(), reason);
}

#[test]
fn move_replay_refuses_all_sources_before_starting_a_partial_undo() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let existing = l.file("existing.txt", "data");
    let missing = l.path().join("missing.txt");
    let mut ws = workspace(&l, &r);
    let action = crate::undo::Action::Move {
        pairs: vec![
            (existing.clone(), r.path().join("existing.txt")),
            (missing, r.path().join("missing.txt")),
        ],
    };

    ws.record_history_for_test(crate::undo::invert(&action).unwrap());
    let plan = ws.begin_history_replay_for_test(crate::undo::ReplayDirection::Undo);
    assert_eq!(plan.action, action);
    let result = ws.execute_action(plan.action, plan.reservation, || {});

    assert!(result.is_err());
    assert!(existing.is_file());
    assert!(!r.path().join("existing.txt").exists());
    assert!(ws.active_transfer().is_none());
}

#[test]
fn apply_rename_order_swaps_two_files() {
    let tmp = TempDir::new();
    tmp.file("a.txt", "A");
    tmp.file("b.txt", "B");
    let map = vec![
        ("a.txt".to_string(), "b.txt".to_string()),
        ("b.txt".to_string(), "a.txt".to_string()),
    ];
    let existing = fileops::dir_names(tmp.path());
    let n = fileops::apply_rename_order(tmp.path(), &map, &existing).unwrap();
    assert_eq!(n, 2);
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "B"
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("b.txt")).unwrap(),
        "A"
    );
}

#[test]
fn apply_batch_rename_allows_case_only_rename() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("readme.md", "x");
    let mut ws = workspace(&l, &r);
    ws.left.select_path(l.path().join("readme.md"));
    // Upper-case the stem: readme.md -> README.md (a case-only change the
    // old studio refused on a case-insensitive volume).
    let rule = crate::rename::RenameRule {
        case: crate::rename::CaseMode::Upper,
        ..Default::default()
    };
    let n = apply_batch_rename(&mut ws, &rule).unwrap();
    assert_eq!(n, 1);
    // The on-disk name now reads with the upper-cased stem.
    let names = fileops::dir_names(l.path());
    assert!(names.contains("README.md"), "names: {names:?}");
}

#[test]
fn apply_batch_rename_undo_restores_a_case_only_rename() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("readme.md", "x");
    let mut ws = workspace(&l, &r);
    ws.left.select_path(l.path().join("readme.md"));
    let rule = crate::rename::RenameRule {
        case: crate::rename::CaseMode::Upper,
        ..Default::default()
    };
    apply_batch_rename(&mut ws, &rule).unwrap();
    assert!(fileops::dir_names(l.path()).contains("README.md"));
    // Undo puts the lower-case name back (itself a case-only rename).
    let _ = ws.perform_undo(|| {});
    let names = fileops::dir_names(l.path());
    assert!(names.contains("readme.md"), "after undo: {names:?}");
    assert!(!names.contains("README.md"));
}

#[test]
fn commit_rename_noop_on_unchanged_name() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let f = l.file("a.txt", "A");
    let mut ws = workspace(&l, &r);
    assert!(ws.commit_rename(&f, "a.txt").is_ok());
    assert!(f.exists());
    assert!(!ws.can_undo());
}

#[test]
fn commit_rename_allows_a_case_only_change() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let f = l.file("readme.md", "x");
    let mut ws = workspace(&l, &r);
    // On a case-insensitive volume "README.md" resolves to "readme.md"; the
    // inline rename used to refuse this legitimate change as "Name already in
    // use". Staging through a temp makes the case actually flip.
    ws.commit_rename(&f, "README.md").unwrap();
    let names = fileops::dir_names(l.path());
    assert!(names.contains("README.md"), "names: {names:?}");
    assert!(!names.contains("readme.md"), "old case gone: {names:?}");

    ws.perform_undo(|| {}).unwrap();
    let names = fileops::dir_names(l.path());
    assert!(names.contains("readme.md"), "after undo: {names:?}");
    assert!(!names.contains("README.md"));
}

#[test]
fn drop_prefers_source_panel_target() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let file = l.file("a.txt", "x");
    let sub = l.dir("sub");
    let mut ws = workspace(&l, &r);

    // Dragging within the left panel onto its own subdirectory:
    // the right panel must not steal the drop.
    ws.left.drag.set(vec![file.clone()]);
    ws.left.drag.set_drop_target(sub.clone());
    ws.drop_dragged(|| {});
    wait_transfer(&mut ws);

    assert!(
        sub.join("a.txt").exists(),
        "file lands in the hovered subdir"
    );
    assert!(!r.path().join("a.txt").exists());
    assert!(ws.left.drag.is_empty());
}

#[test]
fn cancel_drag_clears_both_sources_and_targets() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let mut ws = workspace(&l, &r);
    ws.left.drag.set(vec![l.path().join("left.txt")]);
    ws.right.drag.set(vec![r.path().join("right.txt")]);
    ws.left.drag.set_drop_target(l.path().join("left-target"));
    ws.right.drag.set_drop_target(r.path().join("right-target"));

    ws.cancel_drag();

    assert!(ws.left.drag.is_empty());
    assert!(ws.right.drag.is_empty());
    assert!(!ws.left.drag.has_drop_target());
    assert!(!ws.right.drag.has_drop_target());
}

#[test]
fn preview_follows_cursor_without_reading_text_on_the_workspace_thread() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "alpha");
    l.file("b.txt", "beta");
    let mut ws = workspace(&l, &r);

    ws.left.set_cursor(1); // a.txt
    ws.execute(Command::TogglePreview);
    match &ws.right.preview {
        Some(PreviewContent::Pending(identity)) => {
            assert_eq!(identity.path, l.path().join("a.txt"));
        }
        other => panic!("expected pending text preview, got {other:?}"),
    }

    // Preview follows the cursor by replacing only the pending identity.
    ws.left.set_cursor(2); // b.txt
    ws.sync_preview();
    let identity = match &ws.right.preview {
        Some(PreviewContent::Pending(identity)) => {
            assert_eq!(identity.path, l.path().join("b.txt"));
            identity.clone()
        }
        other => panic!("expected pending text preview, got {other:?}"),
    };
    ws.right.preview = Some(PreviewContent::Text {
        identity,
        content: std::sync::Arc::from("beta"),
    });

    // A ready preview survives while the cached listing identity is stable;
    // sync_preview performs no filesystem read of its own.
    std::fs::remove_file(l.path().join("b.txt")).unwrap();
    ws.sync_preview();
    match &ws.right.preview {
        Some(PreviewContent::Text { content, .. }) => assert_eq!(content.as_ref(), "beta"),
        _ => panic!("preview must survive while the cursor is unchanged"),
    }
}

#[test]
fn drop_to_explicit_other_panel_target_moves_the_file() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let file = l.file("a.txt", "x");
    let mut ws = workspace(&l, &r);

    ws.left.drag.set(vec![file]);
    ws.right.drag.set_drop_target(r.path().to_path_buf());
    ws.drop_dragged(|| {});
    wait_transfer(&mut ws);

    assert!(r.path().join("a.txt").exists());
    assert!(!l.path().join("a.txt").exists(), "drop is a move");
}

#[test]
fn option_drop_copy_effect_keeps_the_source() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let file = l.file("a.txt", "x");
    let mut ws = workspace(&l, &r);

    ws.left.drag.set(vec![file.clone()]);
    ws.right.drag.set_drop_target(r.path().to_path_buf());
    ws.drop_dragged_as(TransferKind::Copy, || {});
    wait_transfer(&mut ws);

    assert!(r.path().join("a.txt").exists());
    assert!(file.exists(), "copy effect keeps the source");
}

#[test]
fn keyboard_drop_moves_selection_into_cursor_folder() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let file = l.file("a.txt", "x");
    let sub = l.dir("sub");
    let mut ws = workspace(&l, &r);
    ws.left.select_path(file.clone());
    let cursor = ws
        .left
        .filtered_entries()
        .iter()
        .position(|entry| entry.path == sub)
        .expect("subfolder is visible")
        + 1;
    ws.left.set_cursor(cursor);

    ws.execute(Command::MoveIntoCursorFolder);
    assert_eq!(
        ws.drain_ui_requests(),
        vec![UiRequest::TransferIntoCursorFolder(TransferKind::Move)]
    );
    ws.transfer_selection_into_cursor_folder(TransferKind::Move, || {});
    wait_transfer(&mut ws);

    assert!(sub.join("a.txt").is_file());
    assert!(!file.exists());
}

#[test]
fn keyboard_copy_into_cursor_folder_keeps_the_source() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let file = l.file("a.txt", "x");
    let sub = l.dir("sub");
    let mut ws = workspace(&l, &r);
    ws.left.select_path(file.clone());
    let cursor = ws
        .left
        .filtered_entries()
        .iter()
        .position(|entry| entry.path == sub)
        .expect("subfolder is visible")
        + 1;
    ws.left.set_cursor(cursor);

    ws.execute(Command::CopyIntoCursorFolder);
    assert_eq!(
        ws.drain_ui_requests(),
        vec![UiRequest::TransferIntoCursorFolder(TransferKind::Copy)]
    );
    ws.transfer_selection_into_cursor_folder(TransferKind::Copy, || {});
    wait_transfer(&mut ws);

    assert!(sub.join("a.txt").is_file());
    assert!(file.exists());
}

#[test]
fn drop_without_an_explicit_target_is_cancelled() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let file = l.file("a.txt", "x");
    let mut ws = workspace(&l, &r);

    ws.left.drag.set(vec![file.clone()]);
    ws.drop_dragged(|| {});

    assert!(file.exists());
    assert!(ws.active_transfer().is_none());
    assert!(ws.pending_op.is_none());
    assert!(ws.left.drag.is_empty());
}

#[test]
fn drop_with_conflict_opens_dialog_instead_of_moving() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let file = l.file("a.txt", "new");
    r.file("a.txt", "old");
    let mut ws = workspace(&l, &r);

    ws.left.drag.set(vec![file]);
    ws.right.drag.set_drop_target(r.path().to_path_buf());
    ws.drop_dragged(|| {});

    // A conflicting drop must NOT move immediately; it stages a
    // confirmation instead, leaving both sides intact.
    assert!(ws.active_transfer().is_none());
    assert!(matches!(ws.pending_op, Some(PendingOp::Transfer(_))));
    assert!(l.path().join("a.txt").exists());
    assert_eq!(
        std::fs::read_to_string(r.path().join("a.txt")).unwrap(),
        "old"
    );
}

#[cfg(unix)]
#[test]
fn skip_conflict_in_unopened_subfolder_handles_a_broken_symlink() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let file = l.file("a.txt", "new");
    let sub = r.dir("sub");
    std::os::unix::fs::symlink("missing-target", sub.join("a.txt")).unwrap();
    let mut ws = workspace(&l, &r);

    ws.left.drag.set(vec![file.clone()]);
    ws.right.drag.set_drop_target(sub);
    ws.drop_dragged(|| {});

    assert_eq!(ws.pending_conflicts().len(), 1);
    assert!(!ws.resolve_pending_conflicts(crate::conflict::RelationPolicy::SkipAll));
    assert!(file.is_file());
    assert!(
        ws.pending_op.is_none(),
        "an empty conflict resolution must retire the old plan"
    );
}

#[test]
fn drag_session_resolves_source_side_and_prefers_its_own_target() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let mut ws = workspace(&l, &r);
    assert!(ws.drag_session().is_none());

    let dragged = r.path().join("b.txt");
    ws.right.drag.set(vec![dragged.clone()]);
    ws.left.drag.set_drop_target(l.path().to_path_buf());
    let session = ws.drag_session().expect("right pane is dragging");
    assert_eq!(session.source, ActivePanel::Right);
    assert_eq!(session.entries, [dragged]);
    assert_eq!(session.target, Some(&l.path().to_path_buf()));

    let own = r.path().join("sub");
    ws.right.drag.set_drop_target(own.clone());
    assert_eq!(ws.drag_session().unwrap().target, Some(&own));
}

#[test]
fn selection_gathered_across_folders_feeds_one_transfer() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let first = l.dir("first");
    let second = l.dir("second");
    std::fs::write(first.join("a.txt"), "a").unwrap();
    std::fs::write(second.join("b.txt"), "b").unwrap();
    let mut ws = workspace(&l, &r);

    ws.left.navigate_to(first.clone());
    ws.left.refresh();
    ws.left.toggle_select(first.join("a.txt"));
    ws.left.navigate_to(second.clone());
    ws.left.refresh();
    ws.left.toggle_select(second.join("b.txt"));

    assert_eq!(ws.left.selected_elsewhere_count(), 1);
    assert_eq!(ws.left.selection_folder_count(), 2);
    assert_eq!(ws.command_context().picked_entries, 2);

    ws.request_copy();
    let Some(PendingOp::Transfer(pending)) = &ws.pending_op else {
        panic!("copy should stage a transfer");
    };
    let mut names: Vec<_> = pending
        .entries
        .iter()
        .map(|entry| entry.name.clone())
        .collect();
    names.sort();
    assert_eq!(names, ["a.txt", "b.txt"]);

    ws.pending_op = None;
    ws.execute(Command::ClearSelection);
    assert_eq!(ws.left.selected_elsewhere_count(), 0);
    assert!(ws.left.selection_is_empty());
}

#[test]
fn repeat_last_is_gated_like_the_command_it_replays() {
    let (left, right) = (TempDir::new(), TempDir::new());
    left.file("a.txt", "x");
    let mut ws = workspace(&left, &right);
    ws.execute(Command::BeginSync);
    assert!(matches!(
        ws.drain_ui_requests().as_slice(),
        [UiRequest::Sync]
    ));

    // Blocked mutations stay blocked when replayed with `.`.
    ws.safe_state = Some(crate::operation::SafeState {
        operation_id: crate::operation::OperationId("review".to_string()),
        reason: "review required".to_string(),
        paths: Vec::new(),
        failures: Vec::new(),
    });
    ws.execute(Command::RepeatLast);
    assert!(ws.drain_ui_requests().is_empty());
    ws.safe_state = None;

    // So do commands whose context no longer applies.
    ws.left.set_cursor(1);
    ws.execute(Command::CopyPath);
    assert_eq!(ws.drain_ui_requests().len(), 1);
    ws.left.set_cursor(0);
    let context = ws.command_context();
    assert!(!crate::command::availability(Command::CopyPath, &context).enabled);
    ws.execute(Command::RepeatLast);
    assert!(ws.drain_ui_requests().is_empty());
}

#[test]
fn preview_stays_open_while_the_cursor_passes_over_a_folder() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "alpha");
    l.dir("folder");
    l.file("z.txt", "zulu");
    let mut ws = workspace(&l, &r);
    let row = |ws: &Workspace, name: &str| {
        ws.left
            .filtered_entries()
            .iter()
            .position(|entry| entry.name == name)
            .unwrap()
            + 1
    };

    ws.left.set_cursor(row(&ws, "a.txt"));
    ws.execute(Command::TogglePreview);
    assert!(ws.right.preview.is_some());

    ws.left.set_cursor(row(&ws, "folder"));
    ws.sync_preview();
    assert!(
        ws.right.preview.is_some(),
        "preview mode survives a folder row"
    );
    // The declined row is remembered, so later frames do not retry it.
    assert!(
        ws.preview_declined
            .as_ref()
            .is_some_and(|declined| declined.path == l.path().join("folder"))
    );
    ws.sync_preview();

    ws.left.set_cursor(row(&ws, "z.txt"));
    ws.sync_preview();
    match &ws.right.preview {
        Some(PreviewContent::Pending(identity)) => {
            assert_eq!(identity.path, l.path().join("z.txt"));
        }
        other => panic!("expected the preview to follow again, got {other:?}"),
    }
    assert!(ws.preview_declined.is_none());
}

#[test]
fn opening_a_preview_from_the_preview_side_moves_it_instead_of_doubling() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "alpha");
    r.file("b.txt", "beta");
    let mut ws = workspace(&l, &r);
    ws.left.set_cursor(1);
    ws.execute(Command::TogglePreview);
    assert!(ws.right.preview.is_some());

    // Tab into the pane showing the preview, then preview from there.
    ws.execute(Command::SwitchPanel);
    ws.right.set_cursor(1);
    ws.execute(Command::TogglePreview);
    assert!(ws.left.preview.is_some());
    assert!(ws.right.preview.is_none(), "only one side previews");

    // Same for the Get-Info card.
    ws.execute(Command::SwitchPanel);
    ws.left.set_cursor(1);
    ws.execute(Command::ToggleInfo);
    assert!(matches!(ws.right.preview, Some(PreviewContent::Info(_))));
    assert!(ws.left.preview.is_none(), "only one side shows info");
}

#[test]
fn reveal_lands_on_the_file_when_the_listing_is_asynchronous() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.dir("deep");
    for n in 0..50 {
        l.file(&format!("deep/f{n:02}.txt"), "x");
    }
    let target = l.path().join("deep/f42.txt");
    let mut ws = workspace(&l, &r);
    let runtime =
        crate::workload::WorkloadRuntime::new(crate::workload::SchedulerLimits::default());
    ws.left
        .set_workload(crate::workload::WorkloadHandle::from_runtime(runtime));
    ws.left.set_notify(Arc::new(|| {}));

    ws.reveal(&target);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while ws.left.filtered_count() == 0 {
        assert!(std::time::Instant::now() < deadline, "listing never landed");
        ws.left.poll_listing();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        ws.left.cursor_entry().map(|entry| entry.path.clone()),
        Some(target)
    );
}

#[cfg(unix)]
#[test]
fn sync_sources_keep_symlinks_lexical() {
    let l = TempDir::new();
    l.dir("folder");
    let link = l.path().join("link");
    std::os::unix::fs::symlink("folder", &link).unwrap();

    let entry = Workspace::sync_source_entry(&link).unwrap();
    assert!(!entry.is_dir, "the link is synced as a link, not a folder");
    assert!(entry.is_symlink());
}

#[test]
fn undo_of_a_cross_folder_gather_is_refused_instead_of_misplacing_items() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let sub = l.dir("sub");
    let top = l.file("top.txt", "t");
    let inner = l.file("sub/inner.txt", "i");
    let mut ws = workspace(&l, &r);
    ws.left.select_path(top);
    ws.left.navigate_to(sub.clone());
    ws.left.select_path(inner);

    ws.gather_into_folder(|| {});
    drain_transfers(&mut ws);
    let folder = std::fs::read_dir(&sub)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.is_dir())
        .expect("gather folder");
    assert!(folder.join("top.txt").is_file());
    assert!(folder.join("inner.txt").is_file());

    // One undo transfer cannot return items to two different folders.
    assert!(ws.perform_undo(|| {}).is_err());
    drain_transfers(&mut ws);
    assert!(!sub.join("top.txt").exists(), "top.txt is not misplaced");
    assert!(folder.join("top.txt").is_file());
    assert!(folder.join("inner.txt").is_file());
}

fn versioned_workspace_with_two_trashed(
    left: &TempDir,
    right: &TempDir,
    trash: Arc<ScriptedTrashPort>,
) -> (Workspace, PathBuf, PathBuf) {
    let first = left.file("first.txt", "one");
    let second = left.file("second.txt", "two");
    let mut ws = Workspace::with_versioned_delete_ports(
        left.path().to_path_buf(),
        right.path().to_path_buf(),
        trash,
        Arc::new(TestFreeSpacePort),
    );
    ws.left.refresh();
    ws.right.refresh();
    ws.left.select_all();
    ws.request_delete();
    assert!(ws.confirm_pending_op(|| {}));
    assert_eq!(ws.finish_delete().expect("delete outcome").trashed, 2);
    (ws, first, second)
}

#[test]
fn partially_failed_trash_redo_refreshes_and_latches_safe_state() {
    let (left, right) = (TempDir::new(), TempDir::new());
    let versions = TempDir::new();
    let _guard = crate::version_store::use_test_versions_dir(versions.path().to_path_buf());
    let trash = Arc::new(ScriptedTrashPort::new([
        crate::ports::TrashItemOutcome::Trashed,
        crate::ports::TrashItemOutcome::Trashed,
        crate::ports::TrashItemOutcome::Trashed, // redo: first
        crate::ports::TrashItemOutcome::Failed(crate::ports::NativeFailure {
            kind: crate::ports::NativeFailureKind::Denied,
            message: "scripted".to_string(),
        }),
    ]));
    let (mut ws, first, second) = versioned_workspace_with_two_trashed(&left, &right, trash);
    ws.perform_undo(|| {}).expect("restore both");
    assert!(first.is_file() && second.is_file());

    assert!(ws.perform_redo(|| {}).is_err());
    assert!(!first.exists(), "the first item was trashed again");
    assert!(
        !ws.left.entries().iter().any(|entry| entry.path == first),
        "listing reflects the partial redo"
    );
    assert!(ws.safe_state.is_some(), "partial replay needs review");
}

#[test]
fn relation_policy_sees_dotfile_conflicts_the_destination_panel_hides() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let source = l.file(".env", "small");
    let existing = r.file(".env", "the larger existing destination");
    r.file("visible.txt", "listed");
    let mut ws = workspace(&l, &r);
    assert_eq!(ws.left.toggle_hidden(), panel::ViewApplyOutcome::Applied);
    assert!(!ws.right.show_hidden(), "destination hides dotfiles");
    ws.left.select_path(source.clone());
    ws.request_copy();

    assert_eq!(ws.pending_conflicts().len(), 1);
    // Keep Larger keeps the larger destination: nothing is left to copy.
    assert!(!ws.resolve_pending_conflicts(crate::conflict::RelationPolicy::KeepLarger));
    assert_eq!(
        std::fs::read_to_string(existing).unwrap(),
        "the larger existing destination"
    );
}

#[test]
fn conflict_detail_and_resolution_read_the_destination_fresh() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let source = l.file("a.txt", "a medium source");
    let existing = r.file("a.txt", "tiny");
    let mut ws = workspace(&l, &r);
    ws.left.select_path(source);
    ws.request_copy();

    let shown = ws.pending_conflicts();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].dst_size, 4);
    // The destination grows while the dialog is open: the next read sees it
    // (the dialog throttles these reads itself)...
    std::fs::write(&existing, "a much larger destination than the source").unwrap();
    assert_eq!(ws.pending_conflicts()[0].dst_size, 41);
    // ...and Keep Larger decides on the destination as it is now.
    assert!(!ws.resolve_pending_conflicts(crate::conflict::RelationPolicy::KeepLarger));
    assert_eq!(
        std::fs::read_to_string(existing).unwrap(),
        "a much larger destination than the source"
    );
}

#[test]
fn resolving_conflicts_recomputes_the_filesystem_preflight() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("CON.txt", "reserved on Windows");
    l.file("ok.txt", "fine");
    r.file("CON.txt", "existing");
    let mut ws = workspace(&l, &r);
    ws.left.select_all();
    ws.request_copy();
    let portable = |ws: &Workspace| match &ws.pending_op {
        Some(PendingOp::Transfer(tr)) => tr
            .filesystem
            .portability
            .iter()
            .any(|issue| issue.name == "CON.txt"),
        _ => panic!("pending copy"),
    };
    assert!(portable(&ws), "audit covers the reserved name");

    assert!(ws.resolve_pending_conflicts(crate::conflict::RelationPolicy::SkipAll));
    assert!(!portable(&ws), "skipped entries leave the audit");
}

#[test]
fn browse_archive_on_the_parent_row_explains_what_to_highlight() {
    let (left, right) = (TempDir::new(), TempDir::new());
    left.file("notes.txt", "x");
    let mut ws = workspace(&left, &right);
    ws.left.set_cursor(0); // the `..` row has no entry
    ws.drain_ui_requests();

    ws.execute(Command::BrowseArchive);

    assert_eq!(
        ws.drain_ui_requests(),
        vec![UiRequest::Notice {
            message: "Highlight a ZIP or tar.gz archive first".to_string(),
            error: true,
        }]
    );
}

#[test]
fn gather_into_folder_reports_when_the_folder_cannot_be_created() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let base = l.dir("work");
    std::fs::write(base.join("IMG_1.jpg"), "a").unwrap();
    let mut ws = Workspace::new(base.clone(), r.path().to_path_buf());
    ws.left.refresh();
    ws.left.select_path(base.join("IMG_1.jpg"));
    ws.drain_ui_requests();
    // The folder vanished under the listing: the new subfolder cannot be made.
    std::fs::remove_dir_all(&base).unwrap();

    ws.gather_into_folder(|| {});

    let requests = ws.drain_ui_requests();
    assert!(
        matches!(
            requests.as_slice(),
            [UiRequest::Notice { message, error: true }]
                if message.starts_with("Could not create folder")
                    && message.contains(&base.display().to_string())
        ),
        "{requests:?}"
    );
}

#[test]
fn link_report_message_shows_the_first_failure() {
    let symlinks = ("symlink", "symlinks");
    assert_eq!(link_report_message(symlinks, 2, &[]), "Created 2 symlinks");
    assert_eq!(link_report_message(symlinks, 1, &[]), "Created 1 symlink");
    let one = vec!["Could not symlink /d/a -> /s/a: File exists (os error 17)".to_string()];
    assert_eq!(
        link_report_message(symlinks, 0, &one),
        "Created 0 symlinks; Could not symlink /d/a -> /s/a: File exists (os error 17)"
    );
    let three = vec![one[0].clone(), "b".to_string(), "c".to_string()];
    let message = link_report_message(("hard link", "hard links"), 1, &three);
    assert!(message.starts_with("Created 1 hard link;"), "{message}");
    assert!(message.ends_with("(os error 17) (+2 more)"));
}

#[test]
fn feedback_text_uses_singular_and_plural_nouns() {
    assert_eq!(count_noun(1, "row", "rows"), "1 row");
    assert_eq!(count_noun(0, "row", "rows"), "0 rows");
    assert_eq!(
        smart_select_message(1, ("empty file", "empty files"), "none"),
        "Selected 1 empty file"
    );
    assert_eq!(
        smart_select_message(4, ("clutter file", "clutter files"), "none"),
        "Selected 4 clutter files"
    );
    assert_eq!(
        smart_select_message(0, ("a", "b"), "No empty files"),
        "No empty files"
    );
    assert_eq!(
        sort_notice("kind", crate::panel::SortOrder::Desc),
        "Sorted by kind, descending"
    );
    assert_eq!(toggle_notice("Folders first", false), "Folders first: off");
    assert_eq!(
        assign_slot_message(3, "src", None),
        "Quick jump 3 \u{2192} src"
    );
    assert_eq!(
        assign_slot_message(3, "src", Some("docs")),
        "Quick jump 3 \u{2192} src (was docs)"
    );
    let root = Path::new("/x/y");
    assert_eq!(
        trust_notice(root, crate::trust::TrustLabel::Restricted),
        "Marked /x/y as Restricted: external providers and automatic archive inspection are blocked"
    );
    assert_eq!(
        trust_notice(root, crate::trust::TrustLabel::Untrusted),
        "Marked /x/y as Untrusted: Run command, external providers and automatic archive inspection are blocked"
    );
    assert_eq!(
        trust_notice(root, crate::trust::TrustLabel::Trusted),
        "Marked /x/y as Trusted: everything is allowed"
    );
}

#[test]
fn listing_copy_label_counts_a_single_row() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("only.txt", "x");
    let mut ws = workspace(&l, &r);
    ws.drain_ui_requests();

    ws.execute(Command::CopyListingCsv);

    let requests = ws.drain_ui_requests();
    assert!(
        matches!(
            requests.as_slice(),
            [UiRequest::CopyText { label, .. }] if label == "listing (1 row as CSV)"
        ),
        "{requests:?}"
    );
}

#[test]
fn smart_select_reports_what_it_selected() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.file("a.txt", "x");
    l.file("empty.txt", "");
    let mut ws = workspace(&l, &r);
    ws.drain_ui_requests();

    ws.execute(Command::SelectEmptyFiles);
    ws.execute(Command::SelectJunk);
    ws.left.set_cursor(0);
    ws.execute(Command::SelectLikeCursor);

    let messages: Vec<String> = ws
        .drain_ui_requests()
        .into_iter()
        .filter_map(|request| match request {
            UiRequest::Notice {
                message,
                error: false,
            } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(
        messages,
        [
            "Selected 1 empty file",
            "No clutter files to select here",
            "Highlight a file with an extension first",
        ]
    );
}

#[test]
fn sort_and_view_toggles_say_what_changed() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let mut ws = workspace(&l, &r);
    ws.drain_ui_requests();

    ws.execute(Command::SortByExtension);
    ws.execute(Command::SortByExtension);
    ws.execute(Command::ToggleNaturalSort);

    let messages: Vec<String> = ws
        .drain_ui_requests()
        .into_iter()
        .filter_map(|request| match request {
            UiRequest::Notice { message, .. } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(
        messages,
        [
            "Sorted by extension, ascending",
            "Sorted by extension, descending",
            "Natural name sort: off",
        ]
    );
}

#[test]
fn bookmark_and_quick_jump_commands_confirm_what_they_did() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let docs = l.dir("docs");
    let mut ws = workspace(&l, &r);
    ws.left.navigate_to(docs.clone());
    ws.drain_ui_requests();

    ws.execute(Command::JumpSlot(4));
    ws.execute(Command::BookmarkCurrentDir);
    ws.execute(Command::BookmarkCurrentDir);
    ws.execute(Command::AssignSlot(4));
    ws.left.navigate_to(l.path().to_path_buf());
    ws.execute(Command::AssignSlot(4));

    let messages: Vec<String> = ws
        .drain_ui_requests()
        .into_iter()
        .filter_map(|request| match request {
            UiRequest::Notice {
                message,
                error: false,
            } => Some(message),
            _ => None,
        })
        .collect();
    let root_name = l.path().file_name().unwrap().to_string_lossy().to_string();
    assert_eq!(
        messages,
        [
            "Quick jump 4 is empty: Cmd+Shift+4 assigns the current folder".to_string(),
            "Added to Favorites".to_string(),
            "Removed from Favorites".to_string(),
            "Quick jump 4 \u{2192} docs".to_string(),
            format!("Quick jump 4 \u{2192} {root_name} (was docs)"),
        ]
    );
}

#[test]
fn nav_lock_says_when_the_other_pane_cannot_follow() {
    let (l, r) = (TempDir::new(), TempDir::new());
    l.dir("only-left");
    let mut ws = workspace(&l, &r);
    ws.execute(Command::ToggleNavLock);
    ws.left.refresh();
    ws.drain_ui_requests();
    let row = ws
        .left
        .filtered_entries()
        .iter()
        .position(|entry| entry.name == "only-left")
        .unwrap();
    ws.left.set_cursor(row + 1);

    ws.execute(Command::Activate);

    assert_eq!(ws.right.current_path, r.path());
    assert_eq!(
        ws.drain_ui_requests(),
        vec![UiRequest::Notice {
            message: "Navigation lock: no folder \u{201c}only-left\u{201d} in the other pane"
                .to_string(),
            error: false,
        }]
    );
}

#[test]
fn profile_applied_message_names_missing_folders() {
    assert_eq!(
        profile_applied_message("default", &[]),
        "Applied workspace profile \"default\""
    );
    let gone = PathBuf::from("/gone/left");
    let message = profile_applied_message("default", &[&gone]);
    assert!(message.contains("/gone/left"), "{message}");
    assert!(message.contains("missing"), "{message}");
}

#[test]
fn applying_a_profile_with_a_missing_root_leaves_that_side_alone() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let saved_right = r.dir("saved");
    let mut ws = workspace(&l, &r);
    ws.left.set_search_query("keep");
    let left_before = ws.left.current_path.clone();
    let gone = l.path().join("removed");
    let mut profile = crate::workspace_profile::WorkspaceProfile::new(
        "default",
        gone.clone(),
        saved_right.clone(),
    );
    profile.left_filter = "from-profile".to_string();
    profile.right_filter = "right-filter".to_string();
    ws.drain_ui_requests();

    ws.apply_profile(&profile);

    assert_eq!(ws.left.current_path, left_before);
    assert_eq!(ws.left.search_query(), "keep");
    assert_eq!(ws.right.current_path, saved_right);
    assert_eq!(ws.right.search_query(), "right-filter");
    let requests = ws.drain_ui_requests();
    assert!(
        requests.iter().any(|request| matches!(
            request,
            UiRequest::Notice { message, error: true }
                if message.contains(&gone.display().to_string())
        )),
        "{requests:?}"
    );
}

#[test]
fn recovery_on_a_history_only_stop_names_the_stop_not_an_operation() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let mut ws = workspace(&l, &r);
    ws.latch_history_failure(None, crate::undo::HistoryError::TimelineRejected);

    let operation_id = crate::operation::OperationId("op-real".to_string());
    let error = ws.resume_recovery(&operation_id, || {}).unwrap_err();
    assert!(error.contains("History settlement failed"), "{error}");
    assert!(!error.contains("belongs to operation"), "{error}");
}

#[test]
fn sync_reports_sources_that_vanished_after_the_plan() {
    let (l, r) = (TempDir::new(), TempDir::new());
    let gone = l.file("gone.txt", "L");
    let mut ws = workspace(&l, &r);
    let actions = ws.build_sync_actions(crate::sync::SyncPolicy::MirrorLeftToRight);
    assert!(!actions.is_empty());
    std::fs::remove_file(&gone).unwrap();
    ws.drain_ui_requests();

    apply_sync(&mut ws, &actions);

    let requests = ws.drain_ui_requests();
    assert!(
        requests.iter().any(|request| matches!(
            request,
            UiRequest::Notice { message, error: true }
                if message.starts_with("Sync skipped")
                    && message.contains(&gone.display().to_string())
                    && message.contains("no longer exists")
        )),
        "{requests:?}"
    );
}
