//! Repair plans and rollback of journaled effects.

use super::*;

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

pub(super) fn cleanup_contains_only_operation_effects(
    record: &OperationRecord,
    cleanup: &Path,
) -> bool {
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

pub(super) fn quarantine_path(path: &Path) -> PathBuf {
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

pub(super) fn detach_expected_path(
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

pub(super) fn remove_expected_path(path: &Path, expected: &PathIdentity) -> Result<(), String> {
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

pub(super) fn prepare_rollback_receipt(
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
        operation.updated_at_secs = crate::clock::now_secs();
        Ok(receipt)
    })
}

pub(super) fn record_rollback_phase(
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

pub(super) fn remove_detached_effect(path: &Path, expected: &PathIdentity) -> Result<(), String> {
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

pub(super) fn rollback_created_container(record: &OperationRecord, plan: &mut RepairPlan) {
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
            operation.updated_at_secs = crate::clock::now_secs();
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
        operation.updated_at_secs = crate::clock::now_secs();
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
        operation.updated_at_secs = crate::clock::now_secs();
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
        remaining: settle_incomplete_steps_for_rollback(operation_id, &record)?,
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

fn settle_incomplete_steps_for_rollback(
    operation_id: &OperationId,
    record: &OperationRecord,
) -> Result<Vec<RepairItem>, String> {
    let interrupted = record
        .steps
        .iter()
        .filter(|step| step.status == StepStatus::Running)
        .map(|step| step.key.clone())
        .collect::<HashSet<_>>();
    if !interrupted.is_empty() {
        mutate(|journal| {
            let operation = operation_mut(&mut journal.operations, operation_id)?;
            for step in &mut operation.steps {
                if interrupted.contains(&step.key) {
                    step.status = step
                        .status
                        .transition(StepEvent::Fail)
                        .map_err(|error| error.to_string())?;
                    step.failure = Some(ClassifiedFailure::message(
                        FailureClass::IntegrityUncertain,
                        step.staging.clone(),
                        "Rollback found a step interrupted before its filesystem effect was proven",
                    ));
                }
            }
            operation.updated_at_secs = crate::clock::now_secs();
            Ok(())
        })?;
    }

    Ok(record
        .steps
        .iter()
        .filter(|step| {
            interrupted.contains(&step.key)
                || (step.status == StepStatus::Failed
                    && (step.staging.is_some()
                        || step.checkpoint.is_some()
                        || step.placement.is_some()
                        || step.replacement.is_some()))
        })
        .map(|step| RepairItem {
            path: step
                .staging
                .clone()
                .unwrap_or_else(|| effect_path(step).to_path_buf()),
            action:
                "Inspect incomplete staging; it was preserved because it may contain the only copy"
                    .to_string(),
            automatic: false,
        })
        .collect())
}
