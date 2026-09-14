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
    crate::native_copy::rename_noreplace(&destination, &backup).unwrap();
    crate::fs_util::sync_parent_namespace(&destination).unwrap();
    mark_replacement_backed_up(&spec.operation_id, &key).unwrap();
    crate::native_copy::rename_noreplace(&staging, &destination).unwrap();
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
    crate::native_copy::rename_noreplace(&destination, &backup).unwrap();
    crate::fs_util::sync_parent_namespace(&destination).unwrap();
    mark_replacement_backed_up(&spec.operation_id, &key).unwrap();
    // Crash between rename-into-place and `mark_replacement_placed`.
    crate::native_copy::rename_noreplace(&staging, &destination).unwrap();
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
    crate::native_copy::rename_noreplace(&staging, &destination).unwrap();
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
    crate::native_copy::rename_noreplace(&staging, &destination).unwrap();
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
    crate::native_copy::rename_noreplace(&destination, &backup).unwrap();
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
    crate::native_copy::rename_noreplace(&receipt.quarantine, &source).unwrap();
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
