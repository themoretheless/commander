use super::*;
use crate::testutil::TempDir;

#[test]
fn operation_status_labels_do_not_claim_a_running_record_is_interrupted() {
    assert_eq!(OperationStatus::Planned.label(), "Planned");
    assert_eq!(OperationStatus::Running.label(), "Running / interrupted");
    assert_eq!(OperationStatus::Stopped.label(), "Stopped");
    assert_eq!(OperationStatus::Failed.label(), "Failed");
    assert_eq!(OperationStatus::NeedsReview.label(), "Review required");
    assert_eq!(OperationStatus::Completed.label(), "Completed");
    assert_eq!(OperationStatus::RolledBack.label(), "Rolled back");
}

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
            source_followed: Vec::new(),
            source_logical_bytes: None,
            source_proof_complete: false,
            destination_before: Some(PathIdentity::observe_deep(destination).unwrap()),
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
        preflight_bytes: None,
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

fn remove_object_field(value: &mut serde_json::Value, field: &str) {
    value.as_object_mut().unwrap().remove(field);
}

fn strip_schema_2_fields(value: &mut serde_json::Value) {
    let operations = value
        .get_mut("operations")
        .and_then(serde_json::Value::as_array_mut)
        .unwrap();
    for operation in operations {
        for field in [
            "version_retention",
            "name_policy",
            "symlink_policy",
            "post_success",
            "rollback_cleanup",
            "rollback_cleanup_identity",
            "rollback_cleanup_quarantine",
        ] {
            remove_object_field(operation, field);
        }
        for step in operation
            .get_mut("steps")
            .and_then(serde_json::Value::as_array_mut)
            .unwrap()
        {
            for field in [
                "checkpoint",
                "fast_path",
                "replacement",
                "rollback",
                "rollback_quarantine",
            ] {
                remove_object_field(step, field);
            }
        }
    }
}

fn completed_overwrite(
    temp: &TempDir,
    label: &str,
    kind: TransferKind,
) -> (OperationId, IdempotencyKey, PathBuf, PathBuf, PathIdentity) {
    let _target = temp.dir(&format!("{label}-target"));
    let source = temp.file(&format!("{label}-source.txt"), "new bytes");
    let destination = temp.file(&format!("{label}-target/item.txt"), "old bytes");
    let mut record = incomplete_record(&source, &destination, StepStatus::Completed);
    record.id = OperationId(format!("{label}-operation"));
    record.kind = kind;
    record.status = OperationStatus::NeedsReview;
    let key = IdempotencyKey(format!("{label}-step"));
    record.steps[0].key = key.clone();

    crate::version_store::preserve_with_policy(
        &destination,
        &record.id,
        key.clone(),
        crate::operation::VersionRetentionPolicy::Forever,
    )
    .unwrap();
    std::fs::remove_file(&destination).unwrap();
    if kind == TransferKind::Move {
        std::fs::rename(&source, &destination).unwrap();
    } else {
        std::fs::write(&destination, "new bytes").unwrap();
    }
    let completed = PathIdentity::observe_deep(&destination).unwrap();
    record.steps[0].landing = Some(destination.clone());
    record.steps[0].landing_before = record.steps[0].destination_before.clone();
    record.steps[0].destination_after = Some(completed.clone());
    record.steps[0].fast_path = Some(if kind == TransferKind::Move {
        crate::transfer_tuning::FastPath::Rename
    } else {
        crate::transfer_tuning::FastPath::Buffered
    });
    save_at(
        &journal_path(),
        &Journal {
            operations: vec![record],
            ..Journal::default()
        },
    )
    .unwrap();
    (
        OperationId(format!("{label}-operation")),
        key,
        source,
        destination,
        completed,
    )
}

#[test]
fn journal_round_trips_atomically() {
    let temp = TempDir::new();
    let path = temp.path().join("journal.json");
    let journal = Journal::default();
    save_at(&path, &journal).unwrap();
    assert_eq!(load_at(&path).unwrap().schema, JOURNAL_SCHEMA);
    assert!(!path.with_extension("json.tmp").exists());

    let root: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(root["format"], "commander.persist");
    assert_eq!(root["store"], "commander.operation_journal");
    assert_eq!(root["schema"], 1);
    assert_eq!(root["payload"]["schema"], JOURNAL_SCHEMA);
    assert!(root["generation"].as_u64().unwrap() >= 1);
}

#[test]
fn journal_envelope_upgrades_legacy_on_save_and_rejects_stale_revision() {
    let temp = TempDir::new();
    let path = temp.path().join("legacy-journal.json");
    let legacy = Journal::default();
    std::fs::write(&path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();

    let mut first = load_journal_at(&path).unwrap();
    assert_eq!(first.gate.status(), crate::persistence::LoadStatus::Legacy);
    first.journal.schema = JOURNAL_SCHEMA;
    save_journal_at(&path, &first.journal, &mut first.gate).unwrap();

    let current = load_journal_at(&path).unwrap();
    assert_eq!(
        current.gate.status(),
        crate::persistence::LoadStatus::Current
    );
    assert_eq!(current.journal.schema, JOURNAL_SCHEMA);

    let mut stale = load_journal_at(&path).unwrap();
    let mut winner = load_journal_at(&path).unwrap();
    winner.journal.operations.clear();
    save_journal_at(&path, &winner.journal, &mut winner.gate).unwrap();

    stale.journal.operations.clear();
    let error = save_journal_at(&path, &stale.journal, &mut stale.gate).unwrap_err();
    assert!(
        error.contains("revision mismatch"),
        "expected revision mismatch, got {error}"
    );
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
    let mut record = incomplete_record(&source, &folder.join("source.txt"), StepStatus::Planned);
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
fn settled_steps_snapshot_answers_from_one_load_and_still_proves_effects() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("settled-journal.json"));
    let target = temp.dir("settled-target");
    let done = temp.file("done.txt", "done");
    let todo = temp.file("todo.txt", "todo");
    let entries = [&done, &todo]
        .into_iter()
        .map(|path| FileEntry::from_meta(path.clone(), &path.symlink_metadata().unwrap()).unwrap())
        .collect();
    let spec = transfer_spec("settled-snapshot", entries, &target);
    begin(&spec).unwrap();
    let done_destination = target.join("done.txt");
    let done_key = step_key(&spec, 0, &done_destination);
    let todo_key = step_key(&spec, 1, &target.join("todo.txt"));
    mark_running(
        &spec.operation_id,
        &done_key,
        &target.join(".done.txt.cmdr-tmp.0"),
        &done_destination,
        PathIdentity::missing(&done_destination),
    )
    .unwrap();
    std::fs::write(&done_destination, "done").unwrap();
    mark_completed(
        &spec.operation_id,
        &done_key,
        &done_destination,
        crate::transfer_tuning::FastPath::Buffered,
    )
    .unwrap();

    let settled = SettledSteps::load(&spec.operation_id).unwrap();

    assert!(settled.is_settled(&done_key).unwrap());
    assert!(!settled.is_settled(&todo_key).unwrap());
    assert!(
        !settled
            .is_settled(&IdempotencyKey("unknown".to_string()))
            .unwrap()
    );
    // The effect proof is checked against the live destination, not the
    // snapshot.
    std::fs::write(&done_destination, "changed").unwrap();
    assert!(
        settled
            .is_settled(&done_key)
            .unwrap_err()
            .contains("changed since operation")
    );
    assert!(SettledSteps::load(&OperationId("missing".to_string())).is_err());
}

#[test]
fn unsupported_journal_schema_fails_closed() {
    let temp = TempDir::new();
    let path = temp.path().join("journal.json");
    std::fs::write(&path, r#"{"schema":99,"operations":[]}"#).unwrap();
    assert!(load_at(&path).unwrap_err().contains("Unsupported"));
}

#[test]
fn schema_2_completed_record_without_fast_path_does_not_block_upgrade() {
    let temp = TempDir::new();
    let path = temp.path().join("schema-2.json");
    let source = temp.file("source.txt", "source");
    let destination = temp.file("destination.txt", "completed");
    let mut record = incomplete_record(&source, &destination, StepStatus::Completed);
    record.status = OperationStatus::Completed;
    record.steps[0].landing = Some(destination.clone());
    record.steps[0].destination_after = Some(PathIdentity::observe_deep(&destination).unwrap());
    record.steps[0].fast_path = None;
    let mut value = serde_json::to_value(Journal {
        schema: 2,
        operations: vec![record],
    })
    .unwrap();
    strip_schema_2_fields(&mut value);
    std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    let loaded = load_at(&path).unwrap();

    assert_eq!(loaded.schema, JOURNAL_SCHEMA);
    assert_eq!(loaded.operations[0].status, OperationStatus::Completed);
    assert_eq!(loaded.operations[0].steps[0].status, StepStatus::Completed);
    assert!(loaded.operations[0].steps[0].fast_path.is_none());
}

#[test]
fn schema_3_terminal_legacy_containers_become_reviewable_without_blocking_load() {
    let temp = TempDir::new();
    let path = temp.path().join("schema-3.json");
    let source = temp.file("source.txt", "source");
    let destination = temp.file("destination.txt", "completed");
    let cleanup = temp.dir("legacy-container");
    let mut completed = incomplete_record(&source, &destination, StepStatus::Completed);
    completed.id = OperationId("legacy-completed".to_string());
    completed.status = OperationStatus::Completed;
    completed.rollback_cleanup = Some(cleanup.clone());
    completed.steps[0].landing = Some(destination.clone());
    completed.steps[0].destination_after = Some(PathIdentity::observe_deep(&destination).unwrap());
    completed.steps[0].fast_path = Some(crate::transfer_tuning::FastPath::Buffered);

    let mut rolled_back = completed.clone();
    rolled_back.id = OperationId("legacy-rolled-back".to_string());
    rolled_back.steps[0].key = IdempotencyKey("legacy-rolled-back-step".to_string());
    rolled_back.status = OperationStatus::RolledBack;
    rolled_back.steps[0].status = StepStatus::RolledBack;

    let mut value = serde_json::to_value(Journal {
        schema: 3,
        operations: vec![completed, rolled_back],
    })
    .unwrap();
    for operation in value
        .get_mut("operations")
        .and_then(serde_json::Value::as_array_mut)
        .unwrap()
    {
        for field in ["rollback_cleanup_identity", "rollback_cleanup_quarantine"] {
            remove_object_field(operation, field);
        }
        for step in operation
            .get_mut("steps")
            .and_then(serde_json::Value::as_array_mut)
            .unwrap()
        {
            for field in ["replacement", "rollback", "rollback_quarantine"] {
                remove_object_field(step, field);
            }
        }
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

    let loaded = load_at(&path).unwrap();

    assert_eq!(loaded.operations.len(), 2);
    assert!(
        loaded
            .operations
            .iter()
            .all(|operation| operation.status == OperationStatus::NeedsReview)
    );
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
        record.steps[0].staging = Some(temp.path().join(format!("staging-{index}")));
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
fn retention_retires_only_effect_free_failed_or_stopped_operations() {
    let temp = TempDir::new();
    let source = temp.file("source.txt", "source");
    let destination = temp.path().join("destination.txt");
    let mut journal = Journal::default();
    for index in 0..=MAX_OPERATIONS {
        let mut record = incomplete_record(&source, &destination, StepStatus::Failed);
        record.id = OperationId(format!("effect-free-{index}"));
        record.status = if index % 2 == 0 {
            OperationStatus::Failed
        } else {
            OperationStatus::Stopped
        };
        journal.operations.push(record);
    }

    prune_terminal_history(&mut journal).unwrap();

    assert_eq!(journal.operations.len(), MAX_OPERATIONS);
    assert_eq!(journal.operations[0].id.0, "effect-free-1");
    assert!(journal.operations.iter().all(|operation| matches!(
        operation.status,
        OperationStatus::Failed | OperationStatus::Stopped
    )));
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

#[cfg(unix)]
#[test]
fn follow_source_proof_round_trips_through_recovery() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new();
    let journal_path = temp.path().join("journal.json");
    let _journal = use_test_journal(journal_path);
    let target = temp.dir("target");
    let followed = temp.dir("followed");
    temp.file("followed/payload.txt", "payload");
    let nested_target = temp.dir("nested-target");
    temp.file("nested-target/nested.txt", "nested");
    symlink(&nested_target, followed.join("nested-link")).unwrap();
    let source = temp.path().join("source-link");
    symlink(&followed, &source).unwrap();
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let scan = crate::scan::transfer_preflight(
        std::slice::from_ref(&entry),
        crate::filesystem_policy::SymlinkPolicy::Follow,
    );
    let logical_bytes = scan.need_bytes.clone().unwrap();
    let source_proof = scan.source_identities[0].as_ref().unwrap().clone();
    let expectations = crate::transfer::expectations_from_source_identities(
        std::slice::from_ref(&entry),
        &target,
        scan.source_identities,
    )
    .unwrap();
    let mut spec = transfer_spec("follow-proof", vec![entry], &target);
    spec.symlink_policy = crate::filesystem_policy::SymlinkPolicy::Follow;
    spec.preflight_bytes = Some(logical_bytes);
    spec.expectations = expectations;

    begin(&spec).unwrap();
    let record = load().unwrap().operations.remove(0);
    let step = &record.steps[0];
    assert!(step.source_proof_complete);
    assert_eq!(step.source_before.as_ref(), Some(&source_proof.lexical));
    assert_eq!(step.source_followed, source_proof.followed);
    assert_eq!(step.source_followed.len(), 2);
    assert_eq!(step.source_logical_bytes, Some(logical_bytes));

    let resumed = build_resume_spec_from(record).unwrap();
    assert_eq!(resumed.preflight_bytes, Some(logical_bytes));
    assert_eq!(resumed.expectations[0].source, Ok(source_proof));
}

#[test]
fn recovery_revalidates_a_shallow_directory_destination_as_shallow() {
    let temp = TempDir::new();
    let source = temp.file("source.txt", "source");
    let destination = temp.dir("target/source.txt");
    temp.file("target/source.txt/existing.txt", "existing");
    let mut record = incomplete_record(&source, &destination, StepStatus::Planned);
    record.steps[0].destination_before = Some(PathIdentity::observe(&destination).unwrap());

    let resumed = build_resume_spec_from(record).unwrap();

    assert_eq!(
        resumed.expectations[0].destination,
        Ok(PathIdentity::observe(&destination).unwrap())
    );
}

#[test]
fn terminal_completion_proof_is_structural_immutable_and_stale_safe() {
    let temp = TempDir::new();
    let journal_path = temp.path().join("journal.json");
    let _journal = use_test_journal(journal_path);
    let target = temp.dir("target");
    let source = temp.file("source.txt", "source");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
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
            crate::transfer::prefix_digest(&staging, staging.metadata().unwrap().len()).unwrap(),
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
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
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
    prepare_replacement(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        &backup,
        &destination_before,
    )
    .unwrap();
    crate::fs_at::rename_noreplace(&destination, &backup).unwrap();
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
fn restart_completes_overwrite_after_placement_before_mark_completed() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("placed-journal.json"));
    let target = temp.dir("placed-target");
    let source = temp.file("source.txt", "new bytes");
    let destination = temp.file("placed-target/source.txt", "old bytes");
    let staging = temp.file("placed-target/.source.txt.cmdr-tmp.0", "new bytes");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("overwrite-placed", vec![entry], &target);
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
    let backup = target.join(".source.txt.cmdr-tmp.backup");
    prepare_replacement(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        &backup,
        &destination_before,
    )
    .unwrap();
    crate::fs_at::rename_noreplace(&destination, &backup).unwrap();
    crate::fs_util::sync_parent_namespace(&destination).unwrap();
    mark_replacement_backed_up(&spec.operation_id, &key).unwrap();
    crate::fs_at::rename_noreplace(&staging, &destination).unwrap();
    crate::fs_util::sync_parent_namespace(&destination).unwrap();
    mark_replacement_placed(&spec.operation_id, &key).unwrap();
    finish(&spec.operation_id, OperationStatus::NeedsReview).unwrap();

    let resumed = build_resume_spec(&spec.operation_id).unwrap();

    assert!(resumed.entries.is_empty(), "{:?}", resumed.entries);
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new bytes");
    assert!(!backup.exists());
    let record = operation(&spec.operation_id).unwrap();
    assert_eq!(record.steps[0].status, StepStatus::Completed);
    assert!(record.steps[0].replacement.is_none());
    assert!(record.steps[0].staging.is_none());
    assert_eq!(
        record.steps[0].fast_path,
        Some(crate::transfer_tuning::FastPath::Resumed)
    );
    assert!(
        record.steps[0]
            .destination_after
            .as_ref()
            .unwrap()
            .same_binding(&PathIdentity::observe_deep(&destination).unwrap())
    );
}

#[test]
fn completion_reports_backup_cleanup_failure_as_a_warning_not_a_journal_error() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("cleanup-journal.json"));
    let target = temp.dir("cleanup-target");
    let source = temp.file("source.txt", "new bytes");
    let destination = temp.file("cleanup-target/source.txt", "old bytes");
    let staging = temp.file("cleanup-target/.source.txt.cmdr-tmp.0", "new bytes");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("cleanup-warning", vec![entry], &target);
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
    let backup = target.join(".source.txt.cmdr-tmp.backup");
    prepare_replacement(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        &backup,
        &destination_before,
    )
    .unwrap();
    crate::fs_at::rename_noreplace(&destination, &backup).unwrap();
    crate::fs_util::sync_parent_namespace(&destination).unwrap();
    mark_replacement_backed_up(&spec.operation_id, &key).unwrap();
    crate::fs_at::rename_noreplace(&staging, &destination).unwrap();
    crate::fs_util::sync_parent_namespace(&destination).unwrap();
    mark_replacement_placed(&spec.operation_id, &key).unwrap();
    // Something else replaces the backup before cleanup runs.
    std::fs::remove_file(&backup).unwrap();
    std::fs::write(&backup, "foreign").unwrap();

    let warning = mark_completed(
        &spec.operation_id,
        &key,
        &destination,
        crate::transfer_tuning::FastPath::Buffered,
    )
    .expect("the completed copy is not a journal failure")
    .expect("the kept backup is reported");

    assert!(warning.contains("foreign data was preserved"), "{warning}");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), "foreign");
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new bytes");
    let record = operation(&spec.operation_id).unwrap();
    assert_eq!(record.steps[0].status, StepStatus::Completed);
    assert!(record.steps[0].replacement.is_some());
}

#[test]
fn restart_completes_overwrite_when_placement_landed_before_placed_phase() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("landed-journal.json"));
    let target = temp.dir("landed-target");
    let source = temp.file("source.txt", "new bytes");
    let destination = temp.file("landed-target/source.txt", "old bytes");
    let staging = temp.file("landed-target/.source.txt.cmdr-tmp.0", "new bytes");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("overwrite-landed", vec![entry], &target);
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
    let backup = target.join(".source.txt.cmdr-tmp.backup");
    prepare_replacement(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        &backup,
        &destination_before,
    )
    .unwrap();
    crate::fs_at::rename_noreplace(&destination, &backup).unwrap();
    crate::fs_util::sync_parent_namespace(&destination).unwrap();
    mark_replacement_backed_up(&spec.operation_id, &key).unwrap();
    // Crash between rename-into-place and `mark_replacement_placed`.
    crate::fs_at::rename_noreplace(&staging, &destination).unwrap();
    crate::fs_util::sync_parent_namespace(&destination).unwrap();
    finish(&spec.operation_id, OperationStatus::NeedsReview).unwrap();
    assert_eq!(
        operation(&spec.operation_id).unwrap().steps[0]
            .replacement
            .as_ref()
            .unwrap()
            .phase,
        ReplacementPhase::OriginalBackedUp
    );

    let resumed = build_resume_spec(&spec.operation_id).unwrap();

    assert!(resumed.entries.is_empty(), "{:?}", resumed.entries);
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new bytes");
    assert!(!backup.exists());
    let record = operation(&spec.operation_id).unwrap();
    assert_eq!(record.steps[0].status, StepStatus::Completed);
    assert!(record.steps[0].replacement.is_none());
    assert!(
        record.steps[0]
            .destination_after
            .as_ref()
            .unwrap()
            .same_binding(&PathIdentity::observe_deep(&destination).unwrap())
    );
}

#[test]
fn restart_completes_non_overwrite_after_placement_before_mark_completed() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("place-journal.json"));
    let target = temp.dir("place-target");
    let source = temp.file("source.txt", "new bytes");
    let destination = target.join("source.txt");
    let staging = temp.file("place-target/.source.txt.cmdr-tmp.0", "new bytes");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("place-placed", vec![entry], &target);
    begin(&spec).unwrap();
    let key = step_key(&spec, 0, &destination);
    mark_running(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        PathIdentity::missing(&destination),
    )
    .unwrap();
    prepare_placement(&spec.operation_id, &key, &staging, &destination).unwrap();
    crate::fs_at::rename_noreplace(&staging, &destination).unwrap();
    crate::fs_util::sync_parent_namespace(&destination).unwrap();
    mark_placement_placed(&spec.operation_id, &key).unwrap();
    finish(&spec.operation_id, OperationStatus::NeedsReview).unwrap();

    let resumed = build_resume_spec(&spec.operation_id).unwrap();

    assert!(resumed.entries.is_empty(), "{:?}", resumed.entries);
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new bytes");
    let record = operation(&spec.operation_id).unwrap();
    assert_eq!(record.steps[0].status, StepStatus::Completed);
    assert!(record.steps[0].placement.is_none());
    assert!(record.steps[0].staging.is_none());
    assert_eq!(
        record.steps[0].fast_path,
        Some(crate::transfer_tuning::FastPath::Resumed)
    );
    assert!(
        record.steps[0]
            .destination_after
            .as_ref()
            .unwrap()
            .same_binding(&PathIdentity::observe_deep(&destination).unwrap())
    );
}

#[test]
fn restart_completes_non_overwrite_when_placement_landed_before_placed_phase() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("place-landed-journal.json"));
    let target = temp.dir("place-landed-target");
    let source = temp.file("source.txt", "new bytes");
    let destination = target.join("source.txt");
    let staging = temp.file("place-landed-target/.source.txt.cmdr-tmp.0", "new bytes");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("place-landed", vec![entry], &target);
    begin(&spec).unwrap();
    let key = step_key(&spec, 0, &destination);
    mark_running(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        PathIdentity::missing(&destination),
    )
    .unwrap();
    prepare_placement(&spec.operation_id, &key, &staging, &destination).unwrap();
    // Crash between rename-into-place and `mark_placement_placed`.
    crate::fs_at::rename_noreplace(&staging, &destination).unwrap();
    crate::fs_util::sync_parent_namespace(&destination).unwrap();
    finish(&spec.operation_id, OperationStatus::NeedsReview).unwrap();
    assert_eq!(
        operation(&spec.operation_id).unwrap().steps[0]
            .placement
            .as_ref()
            .unwrap()
            .phase,
        PlacementPhase::Prepared
    );

    let resumed = build_resume_spec(&spec.operation_id).unwrap();

    assert!(resumed.entries.is_empty(), "{:?}", resumed.entries);
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "new bytes");
    let record = operation(&spec.operation_id).unwrap();
    assert_eq!(record.steps[0].status, StepStatus::Completed);
    assert!(record.steps[0].placement.is_none());
    assert!(
        record.steps[0]
            .destination_after
            .as_ref()
            .unwrap()
            .same_binding(&PathIdentity::observe_deep(&destination).unwrap())
    );
}

#[test]
fn overwrite_backup_proof_rejects_same_inode_content_tampering() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("journal.json"));
    let target = temp.dir("target");
    let source = temp.file("source.txt", "new bytes");
    let destination = temp.file("target/source.txt", "old bytes");
    let staging = temp.file("target/.source.txt.cmdr-tmp.0", "new bytes");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("overwrite-tamper", vec![entry], &target);
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
    let backup = target.join(".source.txt.cmdr-tmp.backup");
    prepare_replacement(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        &backup,
        &destination_before,
    )
    .unwrap();
    crate::fs_at::rename_noreplace(&destination, &backup).unwrap();
    std::fs::write(&backup, "tampered").unwrap();

    let error = mark_replacement_backed_up(&spec.operation_id, &key).unwrap_err();

    assert!(error.contains("proven original"), "{error}");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), "tampered");
    assert!(!destination.exists());
}

#[test]
fn overwrite_preparation_rejects_a_replacement_after_version_review() {
    let temp = TempDir::new();
    let journal_path = temp.path().join("journal.json");
    let _journal = use_test_journal(journal_path);
    let target = temp.dir("target");
    let source = temp.file("source.txt", "new bytes");
    let destination = temp.file("target/source.txt", "reviewed bytes");
    let staging = temp.file("target/.source.txt.cmdr-tmp.0", "new bytes");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("overwrite-stale", vec![entry], &target);
    begin(&spec).unwrap();
    let key = step_key(&spec, 0, &destination);
    let expected = PathIdentity::observe(&destination).unwrap();
    mark_running(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        expected.clone(),
    )
    .unwrap();
    let replacement = temp.file("replacement.txt", "foreign replacement");
    std::fs::remove_file(&destination).unwrap();
    std::fs::rename(replacement, &destination).unwrap();
    let backup = target.join(".source.txt.cmdr-tmp.backup");

    let error = prepare_replacement(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        &backup,
        &expected,
    )
    .unwrap_err();

    assert!(error.contains("changed after conflict review"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "foreign replacement"
    );
    assert!(!backup.exists());
    assert!(
        operation(&spec.operation_id).unwrap().steps[0]
            .replacement
            .is_none()
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
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
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
    let quarantine = prepare_rollback_receipt(&spec.operation_id, &key, &destination)
        .unwrap()
        .quarantine;
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
    assert_eq!(
        record.steps[0]
            .rollback
            .as_ref()
            .map(|receipt| receipt.phase),
        Some(RollbackPhase::Complete)
    );
}

#[test]
fn copy_overwrite_restart_restores_destination_after_legacy_disposal_window() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("copy-journal.json"));
    let _versions = crate::version_store::use_test_versions_dir(temp.dir("copy-versions"));
    let (operation_id, key, source, destination, completed) =
        completed_overwrite(&temp, "copy-crash", TransferKind::Copy);
    let receipt = prepare_rollback_receipt(&operation_id, &key, &destination).unwrap();
    detach_expected_path(&destination, &completed, &receipt.quarantine).unwrap();
    record_rollback_phase(&operation_id, &key, RollbackPhase::EffectDetached, None).unwrap();

    // Reproduce the old unsafe ordering: the replacement disappeared
    // before the preserved destination was restored, then the process died.
    remove_detached_effect(&receipt.quarantine, &completed).unwrap();
    assert!(!destination.exists());

    let plan = rollback(&operation_id).unwrap();

    assert!(plan.remaining.is_empty(), "{:?}", plan.remaining);
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "old bytes");
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "new bytes");
    let record = operation(&operation_id).unwrap();
    assert_eq!(record.status, OperationStatus::RolledBack);
    let receipt = record.steps[0].rollback.as_ref().unwrap();
    assert_eq!(receipt.phase, RollbackPhase::Complete);
    assert!(
        receipt
            .restored_destination
            .as_ref()
            .unwrap()
            .same_binding(&PathIdentity::observe_deep(&destination).unwrap())
    );
}

#[test]
fn move_overwrite_restart_restores_destination_after_source_was_moved_back() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("move-journal.json"));
    let _versions = crate::version_store::use_test_versions_dir(temp.dir("move-versions"));
    let (operation_id, key, source, destination, completed) =
        completed_overwrite(&temp, "move-crash", TransferKind::Move);
    let receipt = prepare_rollback_receipt(&operation_id, &key, &destination).unwrap();
    detach_expected_path(&destination, &completed, &receipt.quarantine).unwrap();
    record_rollback_phase(&operation_id, &key, RollbackPhase::EffectDetached, None).unwrap();
    crate::fs_at::rename_noreplace(&receipt.quarantine, &source).unwrap();
    crate::fs_util::sync_parent_namespace(&source).unwrap();
    crate::fs_util::sync_parent_namespace(&receipt.quarantine).unwrap();
    assert!(!destination.exists());
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "new bytes");

    // Restart while the durable phase still says EffectDetached. Recovery
    // must infer the source receipt and still restore the old destination.
    let plan = rollback(&operation_id).unwrap();

    assert!(plan.remaining.is_empty(), "{:?}", plan.remaining);
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "new bytes");
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "old bytes");
    let record = operation(&operation_id).unwrap();
    assert_eq!(record.status, OperationStatus::RolledBack);
    let receipt = record.steps[0].rollback.as_ref().unwrap();
    assert_eq!(receipt.phase, RollbackPhase::Complete);
    assert!(
        receipt
            .restored_destination
            .as_ref()
            .unwrap()
            .same_binding(&PathIdentity::observe_deep(&destination).unwrap())
    );
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
        content_digest: Some(crate::transfer::prefix_digest(&staging, 0).unwrap()),
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
fn delta_checkpoint_rejects_processed_prefix_tampering() {
    let temp = TempDir::new();
    let source = temp.file("source-delta-proof.txt", "source contents");
    let destination = temp.path().join("destination-delta-proof.txt");
    let staging = temp.file(".destination-delta-proof.cmdr-tmp.0", "source contents");
    let offset = staging.metadata().unwrap().len();
    let mut record = incomplete_record(&source, &destination, StepStatus::Running);
    record.steps[0].staging = Some(staging.clone());
    record.steps[0].checkpoint = Some(ResumeCheckpoint {
        staging: staging.clone(),
        offset,
        source: PathIdentity::observe_deep(&source).unwrap(),
        partial: PathIdentity::observe_deep(&staging).unwrap(),
        layout: crate::transfer::CheckpointLayout::DeltaFixed,
        content_digest: Some(crate::transfer::prefix_digest(&staging, offset).unwrap()),
    });

    std::fs::write(&staging, "xxxxxx contents").unwrap();

    let error = build_resume_spec_from(record).err().unwrap();
    assert!(error.contains("bytes changed"), "{error}");
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
    let mut record = incomplete_record(&source, &folder.join("source.txt"), StepStatus::Planned);
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
    let mut record = incomplete_record(&source, &folder.join("source.txt"), StepStatus::Planned);
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

fn save_test_journal(records: Vec<OperationRecord>) {
    save_at(
        &journal_path(),
        &Journal {
            operations: records,
            ..Journal::default()
        },
    )
    .unwrap();
}

/// A completed copy step that created `{label}-target/item.txt`.
fn completed_created_copy(temp: &TempDir, label: &str) -> (OperationRecord, PathBuf) {
    let source = temp.file(&format!("{label}-source.txt"), "bytes");
    let target = temp.dir(&format!("{label}-target"));
    let destination = target.join("item.txt");
    let mut record = incomplete_record(&source, &destination, StepStatus::Completed);
    record.id = OperationId(format!("{label}-operation"));
    record.status = OperationStatus::NeedsReview;
    record.steps[0].key = IdempotencyKey(format!("{label}-step"));
    std::fs::write(&destination, "bytes").unwrap();
    record.steps[0].landing = Some(destination.clone());
    record.steps[0].landing_before = record.steps[0].destination_before.clone();
    record.steps[0].destination_after = Some(PathIdentity::observe_deep(&destination).unwrap());
    record.steps[0].fast_path = Some(crate::transfer_tuning::FastPath::Buffered);
    (record, destination)
}

#[test]
fn removing_a_replaced_expected_path_restores_the_foreign_object_untouched() {
    let temp = TempDir::new();
    let path = temp.file("item.txt", "ours");
    let expected = PathIdentity::observe_deep(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, "foreign replacement").unwrap();

    let error = remove_expected_path(&path, &expected).unwrap_err();

    assert!(error.contains("replaced before quarantine"), "{error}");
    assert!(error.contains("restored untouched"), "{error}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "foreign replacement"
    );
    let leftovers = std::fs::read_dir(temp.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains("cmdr-quarantine")
        })
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn validation_rejects_each_malformed_source_proof() {
    let temp = TempDir::new();
    let source = temp.file("source.txt", "source");
    let elsewhere = temp.file("elsewhere.txt", "elsewhere");
    let destination = temp.path().join("destination.txt");
    let record = incomplete_record(&source, &destination, StepStatus::Planned);
    let followed = |link: &Path| crate::path_identity::FollowedPathIdentity {
        link: link.to_path_buf(),
        target: PathIdentity::observe(&elsewhere).unwrap(),
    };
    type Corrupt<'a> = Box<dyn Fn(&mut OperationRecord) + 'a>;
    let cases: Vec<(&str, Corrupt)> = vec![
        (
            "unreachable followed proof",
            Box::new(|record: &mut OperationRecord| {
                record.symlink_policy = crate::filesystem_policy::SymlinkPolicy::Follow;
                let step = &mut record.steps[0];
                step.source_proof_complete = true;
                step.source_logical_bytes = Some(6);
                step.source_followed = vec![followed(&elsewhere)];
            }),
        ),
        (
            "incomplete source proof",
            Box::new(|record: &mut OperationRecord| {
                record.steps[0].source_proof_complete = true;
                record.steps[0].source_logical_bytes = None;
            }),
        ),
        (
            "unbound source proof fields",
            Box::new(|record: &mut OperationRecord| {
                record.steps[0].source_proof_complete = false;
                record.steps[0].source_logical_bytes = Some(6);
            }),
        ),
        (
            "followed proofs under another symlink policy",
            Box::new(|record: &mut OperationRecord| {
                record.symlink_policy = crate::filesystem_policy::SymlinkPolicy::Preserve;
                let step = &mut record.steps[0];
                step.source_proof_complete = true;
                step.source_logical_bytes = Some(6);
                step.source_followed = vec![followed(&source)];
            }),
        ),
    ];
    for (expected, corrupt) in cases {
        let mut record = record.clone();
        corrupt(&mut record);
        let error = validate_step(&record, &record.steps[0]).unwrap_err();
        assert!(error.contains(expected), "{expected}: {error}");
    }
    // The same followed proof is accepted under the follow policy.
    let mut valid = record;
    valid.symlink_policy = crate::filesystem_policy::SymlinkPolicy::Follow;
    valid.steps[0].source_proof_complete = true;
    valid.steps[0].source_logical_bytes = Some(6);
    valid.steps[0].source_followed = vec![followed(&source)];
    validate_step(&valid, &valid.steps[0]).unwrap();
}

#[test]
fn recovery_inventory_lists_unsettled_operations_newest_first_and_hides_referenced_staging() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("inventory-journal.json"));
    let target = temp.dir("target");
    let source = temp.file("source.txt", "source");
    let destination = target.join("source.txt");

    let mut older = incomplete_record(&source, &destination, StepStatus::Failed);
    older.id = OperationId("older".to_string());
    older.steps[0].key = IdempotencyKey("older-step".to_string());
    older.updated_at_secs = 5;
    let referenced = temp.file("target/.source.txt.cmdr-tmp.0", "partial");
    older.steps[0].staging = Some(referenced.clone());

    let mut newer = incomplete_record(&source, &destination, StepStatus::Failed);
    newer.id = OperationId("newer".to_string());
    newer.steps[0].key = IdempotencyKey("newer-step".to_string());
    newer.updated_at_secs = 9;

    // A terminal rollback whose overwrite proof was never reconciled stays
    // listed so its backup is not lost from view.
    let backup = temp.file("target/.source.txt.cmdr-tmp.1", "original");
    let mut pending = incomplete_record(&source, &destination, StepStatus::Failed);
    pending.id = OperationId("pending-overwrite".to_string());
    pending.steps[0].key = IdempotencyKey("pending-step".to_string());
    pending.status = OperationStatus::RolledBack;
    pending.updated_at_secs = 7;
    pending.steps[0].replacement = Some(ReplacementBackup {
        path: backup.clone(),
        original: PathIdentity::missing(&destination),
        replacement: PathIdentity::missing(&destination),
        phase: ReplacementPhase::Prepared,
    });

    // A settled terminal operation is hidden; its staging stays referenced.
    let terminal_staging = temp.file("target/.source.txt.cmdr-tmp.2", "partial");
    let mut terminal = incomplete_record(&source, &destination, StepStatus::Failed);
    terminal.id = OperationId("terminal".to_string());
    terminal.steps[0].key = IdempotencyKey("terminal-step".to_string());
    terminal.status = OperationStatus::RolledBack;
    terminal.updated_at_secs = 11;
    terminal.steps[0].staging = Some(terminal_staging);

    let orphan = temp.file("target/.other.txt.cmdr-tmp.0", "orphan");
    save_test_journal(vec![older, newer, pending, terminal]);

    let inventory = recovery_inventory(&[]).unwrap();

    let listed = inventory
        .operations
        .iter()
        .map(|operation| operation.id.0.as_str())
        .collect::<Vec<_>>();
    assert_eq!(listed, ["newer", "pending-overwrite", "older"]);
    let orphans = inventory
        .orphans
        .into_iter()
        .map(|orphan| orphan.path)
        .collect::<Vec<_>>();
    assert_eq!(orphans, vec![orphan]);
}

#[test]
fn recovery_inventory_skips_source_and_settled_history_roots() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("bounded-inventory-journal.json"));
    let source_root = temp.dir("offline-source");
    let source = temp.file("offline-source/source.txt", "source");
    let target = temp.dir("active-target");
    let destination = target.join("source.txt");
    let mut recoverable = incomplete_record(&source, &destination, StepStatus::Failed);
    recoverable.id = OperationId("recoverable".to_string());
    let source_orphan = temp.file("offline-source/.source-only.cmdr-tmp.0", "partial");
    let target_orphan = temp.file("active-target/.target.cmdr-tmp.0", "partial");

    let settled_target = temp.dir("settled-target");
    let settled_destination = settled_target.join("source.txt");
    let mut settled = incomplete_record(&source, &settled_destination, StepStatus::Failed);
    settled.id = OperationId("settled".to_string());
    settled.status = OperationStatus::RolledBack;
    let settled_orphan = temp.file("settled-target/.settled.cmdr-tmp.0", "partial");
    save_test_journal(vec![recoverable, settled]);

    let inventory = recovery_inventory(&[]).unwrap();
    let found = inventory
        .orphans
        .iter()
        .map(|orphan| orphan.path.clone())
        .collect::<Vec<_>>();
    assert_eq!(found, vec![target_orphan]);
    assert!(!found.contains(&source_orphan));
    assert!(!found.contains(&settled_orphan));

    let with_explicit_source = recovery_inventory(&[source_root]).unwrap();
    assert!(
        with_explicit_source
            .orphans
            .iter()
            .any(|orphan| orphan.path == source_orphan)
    );
}

#[test]
fn repair_plan_offers_automatic_removal_of_an_unchanged_created_copy() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("plan-journal.json"));
    let (record, destination) = completed_created_copy(&temp, "plan-copy");
    let id = record.id.clone();
    save_test_journal(vec![record]);

    let plan = repair_plan(&id).unwrap();

    assert_eq!(plan.operation_id, Some(id));
    assert!(plan.completed.is_empty());
    assert_eq!(
        plan.remaining,
        vec![RepairItem {
            path: destination,
            action: "Remove copy created by operation".to_string(),
            automatic: true,
        }]
    );
}

#[test]
fn repair_plan_requires_inspection_of_a_changed_completed_effect() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("plan-journal.json"));
    let (record, destination) = completed_created_copy(&temp, "plan-changed");
    let id = record.id.clone();
    save_test_journal(vec![record]);
    std::fs::write(&destination, "changed after the operation").unwrap();

    let plan = repair_plan(&id).unwrap();

    assert_eq!(plan.remaining.len(), 1);
    assert_eq!(plan.remaining[0].path, destination);
    assert!(
        plan.remaining[0]
            .action
            .contains("changed completed effect"),
        "{:?}",
        plan.remaining
    );
    assert!(!plan.remaining[0].automatic);
}

#[test]
fn repair_plan_moves_back_only_while_the_source_is_free() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("plan-journal.json"));
    let (mut record, destination) = completed_created_copy(&temp, "plan-move");
    record.kind = TransferKind::Move;
    let source = record.steps[0].source.clone();
    std::fs::remove_file(&source).unwrap();
    let id = record.id.clone();
    save_test_journal(vec![record]);

    let plan = repair_plan(&id).unwrap();
    assert_eq!(
        plan.remaining,
        vec![RepairItem {
            path: destination.clone(),
            action: format!("Move back to {}", source.display()),
            automatic: true,
        }]
    );

    std::fs::write(&source, "repopulated").unwrap();
    let plan = repair_plan(&id).unwrap();
    assert_eq!(plan.remaining.len(), 1);
    assert!(!plan.remaining[0].automatic);
}

#[test]
fn repair_plan_restores_a_replaced_copy_destination_from_its_version() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("plan-journal.json"));
    let _versions = crate::version_store::use_test_versions_dir(temp.dir("plan-versions"));
    let (operation_id, _key, _source, destination, _completed) =
        completed_overwrite(&temp, "plan-overwrite", TransferKind::Copy);

    let plan = repair_plan(&operation_id).unwrap();

    assert_eq!(
        plan.remaining,
        vec![RepairItem {
            path: destination,
            action: "Restore replaced local version".to_string(),
            automatic: true,
        }]
    );
}

#[test]
fn repair_plan_keeps_a_replaced_copy_destination_manual_without_a_version() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("plan-journal.json"));
    let versions = crate::version_store::use_test_versions_dir(temp.dir("plan-versions"));
    let (operation_id, _key, _source, destination, _completed) =
        completed_overwrite(&temp, "plan-no-version", TransferKind::Copy);
    drop(versions);
    let _empty = crate::version_store::use_test_versions_dir(temp.dir("empty-versions"));

    let plan = repair_plan(&operation_id).unwrap();

    assert_eq!(
        plan.remaining,
        vec![RepairItem {
            path: destination,
            action: "No preserved version of the replaced original was found; inspect manually"
                .to_string(),
            automatic: false,
        }]
    );
}

#[test]
fn rollback_reports_an_unreadable_version_store_not_a_missing_version() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("unreadable-journal.json"));
    let versions_dir = temp.dir("unreadable-versions");
    let _versions = crate::version_store::use_test_versions_dir(versions_dir.clone());
    let (operation_id, key, _source, destination, _completed) =
        completed_overwrite(&temp, "unreadable", TransferKind::Copy);
    std::fs::write(versions_dir.join("manifest.json"), b"{ not json").unwrap();

    let error = prepare_rollback_receipt(&operation_id, &key, &destination).unwrap_err();
    let plan = repair_plan(&operation_id).unwrap();

    assert_eq!(
        error,
        "Could not read the version store: version manifest is corrupt"
    );
    assert_eq!(
        plan.remaining,
        vec![RepairItem {
            path: destination,
            action: "Could not read the version store (version manifest is corrupt); \
                     inspect manually"
                .to_string(),
            automatic: false,
        }]
    );
}

#[test]
fn repair_plan_lists_the_version_restore_of_an_overwriting_move() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("plan-journal.json"));
    let versions = crate::version_store::use_test_versions_dir(temp.dir("plan-versions"));
    let (operation_id, _key, source, destination, _completed) =
        completed_overwrite(&temp, "plan-move-overwrite", TransferKind::Move);
    let move_back = RepairItem {
        path: destination.clone(),
        action: format!("Move back to {}", source.display()),
        automatic: true,
    };

    let plan = repair_plan(&operation_id).unwrap();
    assert_eq!(
        plan.remaining,
        vec![
            move_back.clone(),
            RepairItem {
                path: destination.clone(),
                action: "Restore replaced local version".to_string(),
                automatic: true,
            },
        ]
    );

    drop(versions);
    let _empty = crate::version_store::use_test_versions_dir(temp.dir("empty-versions"));
    let plan = repair_plan(&operation_id).unwrap();
    assert_eq!(
        plan.remaining,
        vec![
            move_back,
            RepairItem {
                path: destination,
                action: "No preserved version of the replaced original was found; inspect manually"
                    .to_string(),
                automatic: false,
            },
        ]
    );
}

#[test]
fn repair_plan_retries_incomplete_steps_unless_integrity_is_uncertain() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("plan-journal.json"));
    let source = temp.file("source.txt", "source");
    let destination = temp.path().join("destination.txt");
    let mut record = incomplete_record(&source, &destination, StepStatus::Failed);
    let id = record.id.clone();
    let mut uncertain = record.steps[0].clone();
    uncertain.key = IdempotencyKey("uncertain".to_string());
    uncertain.failure = Some(ClassifiedFailure::message(
        FailureClass::IntegrityUncertain,
        None,
        "commit state unknown",
    ));
    record.steps.push(uncertain);
    save_test_journal(vec![record]);

    let plan = repair_plan(&id).unwrap();

    let retry = |automatic| RepairItem {
        path: source.clone(),
        action: "Retry incomplete manifest entry".to_string(),
        automatic,
    };
    // Steps are planned newest-first.
    assert_eq!(plan.remaining, vec![retry(false), retry(true)]);
}

#[test]
fn repair_plan_removes_an_operation_created_folder_only_when_it_holds_only_effects() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("plan-journal.json"));
    let (mut record, destination) = completed_created_copy(&temp, "plan-container");
    let folder = destination.parent().unwrap().to_path_buf();
    record.rollback_cleanup = Some(folder.clone());
    record.rollback_cleanup_identity = Some(PathIdentity::observe_deep(&folder).unwrap());
    let id = record.id.clone();
    save_test_journal(vec![record]);

    let plan = repair_plan(&id).unwrap();
    assert_eq!(plan.remaining.len(), 2);
    assert_eq!(
        plan.remaining[1],
        RepairItem {
            path: folder.clone(),
            action: "Remove operation-created folder after restoring entries".to_string(),
            automatic: true,
        }
    );

    std::fs::write(folder.join("foreign.txt"), "foreign").unwrap();
    let plan = repair_plan(&id).unwrap();
    assert_eq!(plan.remaining[1].path, folder);
    assert!(!plan.remaining[1].automatic);
}

#[test]
fn store_lock_excludes_another_handle_until_released() {
    let temp = TempDir::new();
    let path = temp.path().join("locked").join("operation-journal.json");
    let held = acquire_store_lock(&path).unwrap();
    let lock_path = path.parent().unwrap().join(".operation-journal.lock");
    let other = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();

    assert!(matches!(
        other.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(held);
    other.try_lock().unwrap();
}

#[test]
fn begin_accepts_a_resume_manifest_that_omits_settled_steps() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("partial-resume-journal.json"));
    let target = temp.dir("target");
    let first = temp.file("first.txt", "first");
    let second = temp.file("second.txt", "second");
    let third = temp.file("third.txt", "third");
    let entries = [&first, &second, &third]
        .into_iter()
        .map(|path| FileEntry::from_meta(path.clone(), &path.symlink_metadata().unwrap()).unwrap())
        .collect::<Vec<_>>();
    let spec = transfer_spec("partial-resume", entries, &target);
    begin(&spec).unwrap();
    let completed = target.join("first.txt");
    let completed_key = step_key(&spec, 0, &completed);
    mark_running(
        &spec.operation_id,
        &completed_key,
        &target.join(".first.txt.cmdr-tmp.0"),
        &completed,
        PathIdentity::missing(&completed),
    )
    .unwrap();
    std::fs::write(&completed, "first").unwrap();
    mark_completed(
        &spec.operation_id,
        &completed_key,
        &completed,
        crate::transfer_tuning::FastPath::Buffered,
    )
    .unwrap();
    let failed_key = step_key(&spec, 1, &target.join("second.txt"));
    mark_failed(
        &spec.operation_id,
        &failed_key,
        ClassifiedFailure::message(FailureClass::Blocked, None, "disk full"),
    )
    .unwrap();
    finish(&spec.operation_id, OperationStatus::Failed).unwrap();

    let resumed = build_resume_spec(&spec.operation_id).unwrap();
    assert_eq!(resumed.entries.len(), 2);

    // Dropping an unsettled step or reordering the rest is still refused.
    let mut omitted = resumed.clone();
    omitted.entries.pop();
    omitted.expectations.pop();
    assert!(
        begin(&omitted)
            .unwrap_err()
            .contains("different manifest length")
    );
    let mut reordered = resumed.clone();
    reordered.entries.swap(0, 1);
    reordered.expectations.swap(0, 1);
    assert!(begin(&reordered).unwrap_err().contains("reordered"));

    begin(&resumed).unwrap();
    let record = operation(&spec.operation_id).unwrap();
    assert_eq!(record.status, OperationStatus::Running);
    assert_eq!(record.steps[0].status, StepStatus::Completed);
}

#[test]
fn a_journal_from_a_newer_version_names_the_upgrade_and_the_path() {
    let temp = TempDir::new();
    let path = temp.path().join("journal.json");
    std::fs::write(&path, r#"{"schema":99,"operations":[]}"#).unwrap();

    let error = load_at(&path).unwrap_err();

    assert!(error.contains("Unsupported"), "{error}");
    assert!(error.contains("newer Commander version"), "{error}");
    assert!(error.contains(&path.display().to_string()), "{error}");
}

#[test]
fn journal_save_errors_read_as_sentences_with_the_os_error_and_path() {
    let path = Path::new("/config/operation-journal.json");
    let error = map_journal_save_error(
        path,
        crate::persistence::JsonSaveError::PreCommit(crate::persistence::PreCommitError::Io {
            stage: crate::persistence::SaveStage::SyncFile,
            source: std::io::Error::new(std::io::ErrorKind::StorageFull, "No space left"),
            cleanup: Some(std::io::Error::other("busy")),
        }),
    );

    assert_eq!(
        error,
        format!(
            "Operation journal {} was not committed: could not sync temporary file: No space left; cleanup also failed: busy",
            path.display()
        )
    );
}

#[test]
fn repair_plan_reports_why_a_completed_effect_could_not_be_inspected() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("plan-journal.json"));
    let (mut record, _destination) = completed_created_copy(&temp, "plan-unreadable");
    let blocker = temp.file("blocker", "a file, not a folder");
    let landing = blocker.join("item.txt");
    record.steps[0].landing = Some(landing.clone());
    record.steps[0].landing_before = None;
    record.steps[0].destination_after.as_mut().unwrap().path = landing.clone();
    let id = record.id.clone();
    save_test_journal(vec![record]);

    let plan = repair_plan(&id).unwrap();

    assert_eq!(plan.remaining.len(), 1);
    assert_eq!(plan.remaining[0].path, landing);
    assert!(
        plan.remaining[0]
            .action
            .starts_with("Could not inspect completed effect: "),
        "{:?}",
        plan.remaining
    );
    assert!(!plan.remaining[0].automatic);
}

#[test]
fn rollback_reports_why_a_completed_effect_could_not_be_verified() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("rollback-journal.json"));
    let (record, destination) = completed_created_copy(&temp, "rollback-changed");
    let id = record.id.clone();
    save_test_journal(vec![record]);
    std::fs::write(&destination, "changed after the operation").unwrap();

    let plan = rollback(&id).unwrap();

    assert_eq!(plan.remaining.len(), 1);
    let action = &plan.remaining[0].action;
    assert!(
        action.starts_with("Could not verify completed effect: "),
        "{action}"
    );
    assert!(action.contains("changed since operation"), "{action}");
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "changed after the operation"
    );
}

#[test]
fn rollback_names_both_paths_when_a_move_cannot_go_back() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("move-back-journal.json"));
    let source = temp.file("gone-folder/item.txt", "moved bytes");
    let target = temp.dir("move-target");
    let destination = target.join("item.txt");
    let mut record = incomplete_record(&source, &destination, StepStatus::Completed);
    record.kind = TransferKind::Move;
    record.status = OperationStatus::NeedsReview;
    std::fs::rename(&source, &destination).unwrap();
    std::fs::remove_dir(source.parent().unwrap()).unwrap();
    record.steps[0].landing = Some(destination.clone());
    record.steps[0].landing_before = record.steps[0].destination_before.clone();
    record.steps[0].destination_after = Some(PathIdentity::observe_deep(&destination).unwrap());
    record.steps[0].fast_path = Some(crate::transfer_tuning::FastPath::Rename);
    let id = record.id.clone();
    save_test_journal(vec![record]);

    let plan = rollback(&id).unwrap();

    assert_eq!(plan.remaining.len(), 1);
    let action = &plan.remaining[0].action;
    assert!(action.starts_with("Could not move "), "{action}");
    assert!(
        action.contains(&format!("back to {}", source.display())),
        "{action}"
    );
    let receipt = operation(&id).unwrap().steps[0].rollback.clone().unwrap();
    assert_eq!(
        std::fs::read_to_string(&receipt.quarantine).unwrap(),
        "moved bytes"
    );
}

#[test]
fn a_failure_after_prepared_placement_keeps_the_staging_its_proof_binds() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("fail-placement-journal.json"));
    let target = temp.dir("target");
    let source = temp.file("source.txt", "new bytes");
    let destination = target.join("source.txt");
    let staging = temp.file("target/.source.txt.cmdr-tmp.0", "new bytes");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("fail-after-placement", vec![entry], &target);
    begin(&spec).unwrap();
    let key = step_key(&spec, 0, &destination);
    mark_running(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        PathIdentity::missing(&destination),
    )
    .unwrap();
    prepare_placement(&spec.operation_id, &key, &staging, &destination).unwrap();

    mark_failed(
        &spec.operation_id,
        &key,
        ClassifiedFailure::message(FailureClass::Blocked, None, "final placement failed"),
    )
    .unwrap();

    let step = &operation(&spec.operation_id).unwrap().steps[0];
    assert_eq!(step.status, StepStatus::Failed);
    assert_eq!(step.staging.as_ref(), Some(&staging));
    assert_eq!(
        step.placement.as_ref().map(|placement| placement.phase),
        Some(PlacementPhase::Prepared)
    );
}

#[test]
fn skipping_a_step_with_a_placement_proof_fails_closed() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("skip-placement-journal.json"));
    let target = temp.dir("target");
    let source = temp.file("source.txt", "new bytes");
    let destination = target.join("source.txt");
    let staging = temp.file("target/.source.txt.cmdr-tmp.0", "new bytes");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("skip-placement", vec![entry], &target);
    begin(&spec).unwrap();
    let key = step_key(&spec, 0, &destination);
    mark_running(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        PathIdentity::missing(&destination),
    )
    .unwrap();
    prepare_placement(&spec.operation_id, &key, &staging, &destination).unwrap();

    let error = mark_skipped(&spec.operation_id, &key).unwrap_err();
    assert!(error.contains("unresolved placement proof"), "{error}");
    let step = &operation(&spec.operation_id).unwrap().steps[0];
    assert_eq!(step.status, StepStatus::Running);
    assert_eq!(step.staging.as_ref(), Some(&staging));
    assert!(step.placement.is_some());
}

#[test]
fn skipping_a_failed_step_clears_its_stale_failure() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("skip-journal.json"));
    let target = temp.dir("target");
    let source = temp.file("source.txt", "source");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("skip-after-failure", vec![entry], &target);
    begin(&spec).unwrap();
    let key = step_key(&spec, 0, &target.join("source.txt"));
    mark_failed(
        &spec.operation_id,
        &key,
        ClassifiedFailure::message(FailureClass::IntegrityUncertain, None, "commit unknown"),
    )
    .unwrap();

    mark_skipped(&spec.operation_id, &key).unwrap();

    let step = &operation(&spec.operation_id).unwrap().steps[0];
    assert_eq!(step.status, StepStatus::Skipped);
    assert!(step.failure.is_none(), "{:?}", step.failure);
}

#[test]
fn a_rollback_retry_reuses_the_version_its_receipt_already_holds() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("retry-journal.json"));
    let versions = crate::version_store::use_test_versions_dir(temp.dir("retry-versions"));
    let (operation_id, key, _source, destination, _completed) =
        completed_overwrite(&temp, "retry", TransferKind::Copy);
    let first = prepare_rollback_receipt(&operation_id, &key, &destination).unwrap();
    assert!(first.version.is_some());
    drop(versions);
    // The version-store record is gone (pruned or blocked manifest), but the
    // durable receipt still carries the version and its identity.
    let _empty = crate::version_store::use_test_versions_dir(temp.dir("pruned-versions"));
    assert!(
        crate::version_store::record_for_key(&key)
            .unwrap()
            .is_none()
    );

    let retried = prepare_rollback_receipt(&operation_id, &key, &destination).unwrap();

    assert_eq!(retried, first);
}

fn created_container_record(temp: &TempDir) -> (OperationRecord, PathBuf) {
    let source = temp.file("source.txt", "source");
    let folder = temp.dir("gathered");
    let mut record = incomplete_record(&source, &folder.join("source.txt"), StepStatus::Planned);
    record.rollback_cleanup = Some(folder.clone());
    record.rollback_cleanup_identity = Some(PathIdentity::observe_deep(&folder).unwrap());
    (record, folder)
}

#[test]
fn rollback_keeps_the_created_folder_in_place_while_entries_remain() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("keep-folder-journal.json"));
    let (mut record, destination) = completed_created_copy(&temp, "keep-folder");
    let folder = destination.parent().unwrap().to_path_buf();
    record.rollback_cleanup = Some(folder.clone());
    record.rollback_cleanup_identity = Some(PathIdentity::observe_deep(&folder).unwrap());
    let id = record.id.clone();
    save_test_journal(vec![record]);
    std::fs::write(&destination, "changed after the operation").unwrap();

    let plan = rollback(&id).unwrap();

    assert_eq!(plan.remaining.len(), 2, "{:?}", plan.remaining);
    assert_eq!(plan.remaining[1].path, folder);
    assert!(
        plan.remaining[1].action.contains("kept until"),
        "{:?}",
        plan.remaining
    );
    assert!(destination.exists());
    assert!(
        operation(&id)
            .unwrap()
            .rollback_cleanup_quarantine
            .is_none()
    );
}

#[test]
fn container_rollback_picks_a_fresh_quarantine_when_a_foreign_object_holds_the_old_one() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("foreign-quarantine-journal.json"));
    let (mut record, folder) = created_container_record(&temp);
    let taken = temp.file(".gathered.cmdr-quarantine.0", "foreign");
    record.rollback_cleanup_quarantine = Some(taken.clone());
    save_test_journal(vec![record.clone()]);
    let mut plan = RepairPlan::default();

    rollback_created_container(&record, &mut plan);

    assert!(plan.remaining.is_empty(), "{:?}", plan.remaining);
    assert_eq!(plan.completed.len(), 1);
    assert!(!folder.exists());
    assert_eq!(std::fs::read_to_string(&taken).unwrap(), "foreign");
    assert!(
        operation(&record.id)
            .unwrap()
            .rollback_cleanup_quarantine
            .is_none()
    );
}

#[test]
fn container_rollback_keeps_an_uninspectable_quarantine_record() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("uninspectable-journal.json"));
    let (mut record, folder) = created_container_record(&temp);
    std::fs::remove_dir(&folder).unwrap();
    let blocker = temp.file("blocker", "a file, not a folder");
    let quarantine = blocker.join(".gathered.cmdr-quarantine.0");
    record.rollback_cleanup_quarantine = Some(quarantine.clone());
    save_test_journal(vec![record.clone()]);
    let mut plan = RepairPlan::default();

    rollback_created_container(&record, &mut plan);

    assert_eq!(plan.remaining.len(), 1);
    assert!(
        plan.remaining[0].action.starts_with("Could not inspect"),
        "{:?}",
        plan.remaining
    );
    assert_eq!(
        operation(&record.id).unwrap().rollback_cleanup_quarantine,
        Some(quarantine)
    );
}

#[test]
fn orphan_discovery_ignores_user_files_that_only_contain_a_marker() {
    let temp = TempDir::new();
    let staging = temp.file(".copy.txt.cmdr-tmp.12", "staged");
    temp.file("notes.cmdr-tmp.txt", "user file");
    temp.file(
        "report.cmdr-quarantine.1",
        "user file without a leading dot",
    );
    temp.file(".config.cmdr-tmp.backup", "user file with a word suffix");
    temp.file(".cmdr-tmp.3", "no stem");

    let found = discover_orphan_staging_with(
        &[temp.path().to_path_buf()],
        &std::collections::HashSet::new(),
    )
    .into_iter()
    .map(|orphan| orphan.path)
    .collect::<Vec<_>>();

    assert_eq!(found, vec![staging]);
}

#[test]
fn quarantine_names_of_long_files_stay_within_the_name_limit() {
    let parent = Path::new("/folder");
    for name in ["a".repeat(250), "é".repeat(125)] {
        let quarantine = quarantine_path(&parent.join(&name));
        let file_name = quarantine.file_name().unwrap().to_str().unwrap();
        assert!(file_name.len() <= 255, "{} bytes", file_name.len());
        assert!(is_commander_temporary_name(file_name), "{file_name}");
        assert_eq!(quarantine.parent(), Some(parent));
    }
    assert_eq!(
        quarantine_path(&parent.join("short.txt")),
        parent.join(".short.txt.cmdr-quarantine.0")
    );
}

#[test]
fn restart_reports_a_destination_taken_while_placement_was_prepared() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("occupied-journal.json"));
    let target = temp.dir("target");
    let source = temp.file("source.txt", "new bytes");
    let destination = target.join("source.txt");
    let staging = temp.file("target/.source.txt.cmdr-tmp.0", "new bytes");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("occupied-placement", vec![entry], &target);
    begin(&spec).unwrap();
    let key = step_key(&spec, 0, &destination);
    mark_running(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        PathIdentity::missing(&destination),
    )
    .unwrap();
    prepare_placement(&spec.operation_id, &key, &staging, &destination).unwrap();
    finish(&spec.operation_id, OperationStatus::NeedsReview).unwrap();
    std::fs::write(&destination, "foreign").unwrap();

    let error = build_resume_spec(&spec.operation_id).err().unwrap();

    assert!(error.contains("occupied by another object"), "{error}");
    assert!(error.contains(&staging.display().to_string()), "{error}");
    assert_eq!(std::fs::read_to_string(&staging).unwrap(), "new bytes");
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "foreign");
}

#[test]
fn restart_finishes_a_sealed_prepared_placement_without_the_source() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("sealed-placement-journal.json"));
    let target = temp.dir("target");
    let source = temp.file("source.txt", "only copy");
    let destination = target.join("source.txt");
    let staging = target.join(".source.txt.cmdr-tmp.0");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("sealed-placement", vec![entry], &target);
    begin(&spec).unwrap();
    let key = step_key(&spec, 0, &destination);
    std::fs::rename(&source, &staging).unwrap();
    mark_running(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        PathIdentity::missing(&destination),
    )
    .unwrap();
    prepare_placement(&spec.operation_id, &key, &staging, &destination).unwrap();
    finish(&spec.operation_id, OperationStatus::NeedsReview).unwrap();

    let resumed = build_resume_spec(&spec.operation_id).unwrap();

    assert!(resumed.entries.is_empty(), "{:?}", resumed.entries);
    assert!(!source.exists());
    assert!(!staging.exists());
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "only copy");
    let step = &operation(&spec.operation_id).unwrap().steps[0];
    assert_eq!(step.status, StepStatus::Completed);
    assert!(step.placement.is_none());
    assert!(step.staging.is_none());
}

#[test]
fn restart_finishes_a_sealed_prepared_directory_placement() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("sealed-directory-journal.json"));
    let target = temp.dir("target");
    let source = temp.dir("source-dir");
    std::fs::write(source.join("nested.txt"), "nested bytes").unwrap();
    let destination = target.join("source-dir");
    let staging = target.join(".source-dir.cmdr-tmp.0");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("sealed-directory", vec![entry], &target);
    begin(&spec).unwrap();
    let key = step_key(&spec, 0, &destination);
    std::fs::rename(&source, &staging).unwrap();
    mark_running(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        PathIdentity::missing(&destination),
    )
    .unwrap();
    prepare_placement(&spec.operation_id, &key, &staging, &destination).unwrap();
    finish(&spec.operation_id, OperationStatus::NeedsReview).unwrap();

    let resumed = build_resume_spec(&spec.operation_id).unwrap();

    assert!(resumed.entries.is_empty(), "{:?}", resumed.entries);
    assert!(!source.exists());
    assert!(!staging.exists());
    assert_eq!(
        std::fs::read_to_string(destination.join("nested.txt")).unwrap(),
        "nested bytes"
    );
    assert_eq!(
        operation(&spec.operation_id).unwrap().steps[0].status,
        StepStatus::Completed
    );
}

#[test]
fn restart_keeps_a_changed_prepared_staging_for_review() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("changed-placement-journal.json"));
    let target = temp.dir("target");
    let source = temp.file("source.txt", "original bytes");
    let destination = target.join("source.txt");
    let staging = temp.file("target/.source.txt.cmdr-tmp.0", "original bytes");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("changed-placement", vec![entry], &target);
    begin(&spec).unwrap();
    let key = step_key(&spec, 0, &destination);
    mark_running(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        PathIdentity::missing(&destination),
    )
    .unwrap();
    prepare_placement(&spec.operation_id, &key, &staging, &destination).unwrap();
    finish(&spec.operation_id, OperationStatus::NeedsReview).unwrap();
    std::fs::write(&staging, "foreign bytes").unwrap();

    let error = build_resume_spec(&spec.operation_id).err().unwrap();

    assert!(error.contains("staging changed"), "{error}");
    assert_eq!(std::fs::read_to_string(&staging).unwrap(), "foreign bytes");
    assert!(!destination.exists());
    assert!(
        operation(&spec.operation_id).unwrap().steps[0]
            .placement
            .is_some()
    );
}

#[test]
fn resume_without_file_ids_accepts_an_appended_prefix_tail_but_proves_its_bytes() {
    let temp = TempDir::new();
    let source = temp.file("source.txt", "source contents");
    let destination = temp.path().join("destination.txt");
    let staging = temp.file(".destination.cmdr-tmp.0", "source");
    let offset = staging.metadata().unwrap().len();
    let mut partial = PathIdentity::observe_deep(&staging).unwrap();
    // Platforms without stable file ids record none.
    partial.volume = None;
    partial.file_id = None;
    let mut record = incomplete_record(&source, &destination, StepStatus::Running);
    record.steps[0].staging = Some(staging.clone());
    record.steps[0].checkpoint = Some(ResumeCheckpoint {
        staging: staging.clone(),
        offset,
        source: PathIdentity::observe_deep(&source).unwrap(),
        partial,
        layout: crate::transfer::CheckpointLayout::Prefix,
        content_digest: Some(crate::transfer::prefix_digest(&staging, offset).unwrap()),
    });
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&staging)
        .unwrap()
        .write_all(b" contents written after the checkpoint")
        .unwrap();

    let spec = build_resume_spec_from(record.clone()).unwrap();
    assert_eq!(
        spec.expectations[0]
            .resume
            .as_ref()
            .map(|checkpoint| checkpoint.offset),
        Some(offset)
    );

    let mut tampered = std::fs::read(&staging).unwrap();
    tampered[0] = b'x';
    std::fs::write(&staging, tampered).unwrap();
    let error = build_resume_spec_from(record).err().unwrap();
    assert!(error.contains("bytes changed"), "{error}");
}

/// Begin a one-step overwrite of `target/source.txt` and back up the
/// original, stopping just before the staged copy is renamed into place.
fn overwrite_backed_up(
    temp: &TempDir,
    label: &str,
) -> (OperationId, IdempotencyKey, PathBuf, PathBuf, PathBuf) {
    let target = temp.dir(&format!("{label}-target"));
    let source = temp.file(&format!("{label}-source.txt"), "new bytes");
    let destination = temp.file(&format!("{label}-target/{label}-source.txt"), "old bytes");
    let staging = temp.file(
        &format!("{label}-target/.{label}-source.txt.cmdr-tmp.0"),
        "new bytes",
    );
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec(label, vec![entry], &target);
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
    let backup = target.join(format!(".{label}-source.txt.cmdr-tmp.1"));
    prepare_replacement(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        &backup,
        &destination_before,
    )
    .unwrap();
    crate::fs_at::rename_noreplace(&destination, &backup).unwrap();
    mark_replacement_backed_up(&spec.operation_id, &key).unwrap();
    (spec.operation_id, key, destination, staging, backup)
}

#[test]
fn skipping_a_step_with_an_overwrite_proof_fails_closed() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("skip-overwrite-journal.json"));
    let (operation_id, key, _destination, staging, backup) =
        overwrite_backed_up(&temp, "skip-overwrite");

    let error = mark_skipped(&operation_id, &key).unwrap_err();
    assert!(error.contains("unresolved placement proof"), "{error}");
    let step = &operation(&operation_id).unwrap().steps[0];
    assert_eq!(step.status, StepStatus::Running);
    assert_eq!(step.staging.as_ref(), Some(&staging));
    assert!(step.replacement.is_some());
    assert!(backup.exists());
}

#[test]
fn replacement_placement_requires_the_staged_object_and_the_unchanged_backup() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("placed-proof-journal.json"));
    let (operation_id, key, destination, staging, backup) =
        overwrite_backed_up(&temp, "placed-proof");

    std::fs::write(&destination, "foreign").unwrap();
    let error = mark_replacement_placed(&operation_id, &key).unwrap_err();
    assert!(error.contains("not the proven staged object"), "{error}");
    std::fs::remove_file(&destination).unwrap();

    crate::fs_at::rename_noreplace(&staging, &destination).unwrap();
    std::fs::write(&backup, "tampered original").unwrap();
    let error = mark_replacement_placed(&operation_id, &key).unwrap_err();
    assert!(error.contains("changed in its backup"), "{error}");
    let step = &operation(&operation_id).unwrap().steps[0];
    assert_eq!(
        step.replacement.as_ref().unwrap().phase,
        ReplacementPhase::OriginalBackedUp
    );
}

#[test]
fn replacement_placement_is_idempotent_and_rejects_a_stale_phase() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("placed-phase-journal.json"));
    let (operation_id, key, destination, staging, _backup) =
        overwrite_backed_up(&temp, "placed-phase");
    crate::fs_at::rename_noreplace(&staging, &destination).unwrap();

    mark_replacement_placed(&operation_id, &key).unwrap();
    // A duplicate callback succeeds without re-proving the filesystem.
    std::fs::write(&destination, "changed later").unwrap();
    mark_replacement_placed(&operation_id, &key).unwrap();

    let mut journal = load().unwrap();
    journal.operations[0].steps[0]
        .replacement
        .as_mut()
        .unwrap()
        .phase = ReplacementPhase::Prepared;
    save_at(&journal_path(), &journal).unwrap();
    let error = mark_replacement_placed(&operation_id, &key).unwrap_err();
    assert!(error.contains("stale phase"), "{error}");
}

#[test]
fn non_overwrite_placement_requires_the_staged_object_and_is_idempotent() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("place-proof-journal.json"));
    let target = temp.dir("target");
    let source = temp.file("source.txt", "new bytes");
    let destination = target.join("source.txt");
    let staging = temp.file("target/.source.txt.cmdr-tmp.0", "new bytes");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("place-proof", vec![entry], &target);
    begin(&spec).unwrap();
    let key = step_key(&spec, 0, &destination);
    let error = mark_placement_placed(&spec.operation_id, &key).unwrap_err();
    assert!(error.contains("was not prepared"), "{error}");
    mark_running(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        PathIdentity::missing(&destination),
    )
    .unwrap();
    prepare_placement(&spec.operation_id, &key, &staging, &destination).unwrap();

    std::fs::write(&destination, "foreign").unwrap();
    let error = mark_placement_placed(&spec.operation_id, &key).unwrap_err();
    assert!(error.contains("not the proven staged object"), "{error}");
    std::fs::remove_file(&destination).unwrap();

    crate::fs_at::rename_noreplace(&staging, &destination).unwrap();
    mark_placement_placed(&spec.operation_id, &key).unwrap();
    std::fs::write(&destination, "changed later").unwrap();
    mark_placement_placed(&spec.operation_id, &key).unwrap();
    assert_eq!(
        operation(&spec.operation_id).unwrap().steps[0]
            .placement
            .as_ref()
            .unwrap()
            .phase,
        PlacementPhase::Placed
    );
}

#[test]
fn finishing_as_completed_reproves_every_completed_effect() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("finish-proof-journal.json"));
    let target = temp.dir("target");
    let source = temp.file("source.txt", "source");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("finish-proof", vec![entry], &target);
    begin(&spec).unwrap();
    let destination = target.join("source.txt");
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
    std::fs::write(&destination, "changed before finish").unwrap();

    let error = finish(&spec.operation_id, OperationStatus::Completed).unwrap_err();

    assert!(error.contains("changed since operation"), "{error}");
    assert_eq!(
        operation(&spec.operation_id).unwrap().status,
        OperationStatus::Running
    );
    finish(&spec.operation_id, OperationStatus::NeedsReview).unwrap();
}

#[test]
fn duplicate_completion_must_match_the_recorded_proof() {
    use crate::transfer_tuning::FastPath;
    let temp = TempDir::new();
    let (record, destination) = completed_created_copy(&temp, "duplicate");
    let step = &record.steps[0];
    let proof = step.destination_after.clone().unwrap();

    check_duplicate_completion(step, &destination, &proof, Some(FastPath::Buffered)).unwrap();
    check_duplicate_completion(step, &destination, &proof, None).unwrap();

    let conflicts = [
        check_duplicate_completion(step, &destination, &proof, Some(FastPath::Clone)),
        check_duplicate_completion(
            step,
            &temp.path().join("elsewhere.txt"),
            &proof,
            Some(FastPath::Buffered),
        ),
        {
            std::fs::write(&destination, "changed").unwrap();
            let changed = PathIdentity::observe_deep(&destination).unwrap();
            check_duplicate_completion(step, &destination, &changed, None)
        },
    ];
    for conflict in conflicts {
        assert!(conflict.unwrap_err().contains("immutable proof"));
    }

    let mut unproved = step.clone();
    unproved.destination_after = None;
    assert!(
        check_duplicate_completion(&unproved, &destination, &proof, None)
            .unwrap_err()
            .contains("no immutable effect proof")
    );
}

#[test]
fn a_placement_failure_that_undid_its_staging_resumes_as_a_retry() {
    let temp = TempDir::new();
    let _journal = use_test_journal(temp.path().join("retry-placement-journal.json"));
    let target = temp.dir("target");
    let source = temp.file("source.txt", "new bytes");
    let destination = target.join("source.txt");
    let staging = temp.file("target/.source.txt.cmdr-tmp.0", "new bytes");
    let entry = FileEntry::from_meta(source.clone(), &source.symlink_metadata().unwrap()).unwrap();
    let spec = transfer_spec("retry-after-placement", vec![entry], &target);
    begin(&spec).unwrap();
    let key = step_key(&spec, 0, &destination);
    mark_running(
        &spec.operation_id,
        &key,
        &staging,
        &destination,
        PathIdentity::missing(&destination),
    )
    .unwrap();
    prepare_placement(&spec.operation_id, &key, &staging, &destination).unwrap();
    // The executor's undo_placement removes the copy staging before the
    // failure is recorded.
    std::fs::remove_file(&staging).unwrap();
    mark_failed(
        &spec.operation_id,
        &key,
        ClassifiedFailure::message(FailureClass::Blocked, None, "final placement failed"),
    )
    .unwrap();
    finish(&spec.operation_id, OperationStatus::Failed).unwrap();

    let resumed = build_resume_spec(&spec.operation_id).unwrap();

    assert_eq!(resumed.expectations.len(), 1);
    assert_eq!(resumed.expectations[0].key.as_ref(), Some(&key));
    assert!(resumed.expectations[0].resume.is_none());
    let step = &operation(&spec.operation_id).unwrap().steps[0];
    assert_eq!(step.status, StepStatus::Failed);
    assert!(step.placement.is_none());
    assert!(step.staging.is_none());
}
