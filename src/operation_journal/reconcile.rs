//! Reconciliation of replacements and placements interrupted mid-step.

use super::*;

pub(super) fn reconcile_prepared_replacements(operation_id: &OperationId) -> Result<(), String> {
    let record = operation(operation_id)?;
    for step in &record.steps {
        let Some(replacement) = &step.replacement else {
            continue;
        };
        let destination = PathIdentity::observe_deep(&replacement.original.path)
            .map_err(|error| format!("Could not inspect interrupted overwrite: {error}"))?;
        let backup = PathIdentity::observe_deep(&replacement.path)
            .map_err(|error| format!("Could not inspect interrupted overwrite backup: {error}"))?;
        if destination.exists && replacement.replacement.same_version(&destination) {
            // Placement already landed; close the crash window before
            // `mark_completed` rather than failing closed for review.
            complete_interrupted_overwrite_placement(
                operation_id,
                &step.key,
                replacement,
                &destination,
                &backup,
            )?;
            continue;
        }
        if backup.exists {
            if !replacement.original.same_version(&backup) {
                return Err(format!(
                    "Interrupted overwrite backup changed and requires review: {}",
                    replacement.path.display()
                ));
            }
            if destination.exists {
                return Err(format!(
                    "Interrupted overwrite destination was repopulated; original remains at {}",
                    replacement.path.display()
                ));
            }
            crate::fs_at::rename_noreplace(&replacement.path, &replacement.original.path).map_err(
                |error| {
                    format!(
                        "Could not restore interrupted overwrite from {}: {error}",
                        replacement.path.display()
                    )
                },
            )?;
            crate::fs_util::sync_parent_namespace(&replacement.original.path)
                .map_err(|error| format!("Could not sync restored overwrite: {error}"))?;
        } else if !replacement.original.same_binding(&destination) {
            return Err(format!(
                "Interrupted overwrite lost its proven original binding: {}",
                replacement.original.path.display()
            ));
        }

        mutate(|journal| {
            let operation = operation_mut(&mut journal.operations, operation_id)?;
            let journal_step = operation
                .steps
                .iter_mut()
                .find(|journal_step| journal_step.key == step.key)
                .ok_or_else(|| format!("Unknown operation step {}", step.key.0))?;
            if journal_step.replacement.as_ref() != Some(replacement) {
                return Err(format!(
                    "Overwrite proof changed while reconciling step {}",
                    step.key.0
                ));
            }
            journal_step.replacement = None;
            operation.updated_at_secs = crate::clock::now_secs();
            Ok(())
        })?;
    }
    Ok(())
}

/// Promote a proven overwrite placement that crashed before `mark_completed`.
///
/// Durable `ReplacementPlaced` (or live destination matching the staged
/// replacement after `OriginalBackedUp`) is enough to write the terminal
/// effect proof, clean the backup, and leave the step completed.
fn complete_interrupted_overwrite_placement(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    replacement: &ReplacementBackup,
    destination: &PathIdentity,
    backup: &PathIdentity,
) -> Result<(), String> {
    match replacement.phase {
        ReplacementPhase::Prepared => {
            return Err(format!(
                "Overwrite placement completed before its terminal proof; original is preserved at {} and requires review",
                replacement.path.display()
            ));
        }
        ReplacementPhase::OriginalBackedUp => {
            if !backup.exists {
                return Err(format!(
                    "Overwrite placement landed without its proven backup; inspect {}",
                    replacement.original.path.display()
                ));
            }
            if !replacement.original.same_version(backup) {
                return Err(format!(
                    "Interrupted overwrite backup changed and requires review: {}",
                    replacement.path.display()
                ));
            }
        }
        ReplacementPhase::ReplacementPlaced => {
            if backup.exists && !replacement.original.same_version(backup) {
                return Err(format!(
                    "Interrupted overwrite backup changed and requires review: {}",
                    replacement.path.display()
                ));
            }
        }
    }

    let destination_after = destination.clone();
    let landing = replacement.original.path.clone();
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
        let step = step_mut(&mut operation.steps, key)?;
        if step.replacement.as_ref() != Some(replacement) {
            return Err(format!(
                "Overwrite proof changed while completing step {}",
                key.0
            ));
        }
        if step.status == StepStatus::Completed {
            return check_duplicate_completion(step, &landing, &destination_after, None);
        }
        step.status = step
            .status
            .transition(StepEvent::Complete)
            .map_err(|error| error.to_string())?;
        step.landing = Some(landing);
        step.destination_after = Some(destination_after);
        step.checkpoint = None;
        if step.fast_path.is_none() {
            step.fast_path = Some(crate::transfer_tuning::FastPath::Resumed);
        }
        step.failure = None;
        operation.updated_at_secs = crate::clock::now_secs();
        Ok(())
    })?;

    release_overwrite_backup(operation_id, key, replacement, backup)
}

/// Remove a completed overwrite's backup of the original (only if it is still
/// that original), then drop the step's replacement record. Foreign data at
/// the backup path is preserved and reported.
pub(super) fn release_overwrite_backup(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    replacement: &ReplacementBackup,
    backup: &PathIdentity,
) -> Result<(), String> {
    if backup.exists {
        if !replacement.original.same_version(backup) {
            return Err(
                "Overwrite backup was replaced before cleanup; foreign data was preserved"
                    .to_string(),
            );
        }
        remove_expected_path(&replacement.path, &replacement.original)?;
    }
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
        let step = step_mut(&mut operation.steps, key)?;
        if step.status != StepStatus::Completed || step.replacement.as_ref() != Some(replacement) {
            return Err(format!(
                "Overwrite proof for step {} changed before backup cleanup",
                key.0
            ));
        }
        step.replacement = None;
        step.staging = None;
        operation.updated_at_secs = crate::clock::now_secs();
        Ok(())
    })
}

/// Promote a proven non-overwrite placement that crashed before `mark_completed`.
///
/// Durable `Placed` (or live destination matching the sealed staged identity
/// after `Prepared`) is enough to write the terminal effect proof and leave
/// the step completed without re-copying.
pub(super) fn reconcile_interrupted_placements(operation_id: &OperationId) -> Result<(), String> {
    let record = operation(operation_id)?;
    for step in &record.steps {
        let Some(placement) = &step.placement else {
            continue;
        };
        let landing = step
            .landing
            .as_deref()
            .ok_or_else(|| format!("Placement step {} has no landing", step.key.0))?;
        let destination = PathIdentity::observe_deep(landing)
            .map_err(|error| format!("Could not inspect interrupted placement: {error}"))?;
        let staging = step
            .staging
            .as_ref()
            .map(|path| {
                PathIdentity::observe_deep(path)
                    .map_err(|error| format!("Could not inspect interrupted staging: {error}"))
            })
            .transpose()?;
        let staging_gone = staging.as_ref().is_none_or(|identity| !identity.exists);
        if destination.exists && placement.staged.same_version(&destination) {
            complete_interrupted_placement(
                operation_id,
                &step.key,
                placement,
                landing,
                &destination,
            )?;
            continue;
        }
        let retryable_failure = step.status == StepStatus::Failed
            && step
                .failure
                .as_ref()
                .is_none_or(|failure| failure.class != FailureClass::IntegrityUncertain);
        if retryable_failure && placement.phase == PlacementPhase::Prepared && staging_gone {
            // An ordinary placement failure already undid its staging (deleted
            // the copy or moved a renamed source back); nothing was placed, so
            // the step is retried from its source.
            clear_failed_placement(operation_id, &step.key, placement)?;
            continue;
        }
        match placement.phase {
            PlacementPhase::Prepared if !destination.exists && !staging_gone => {
                let staging_path = step.staging.as_deref().ok_or_else(|| {
                    format!("Placement step {} lost its staging path", step.key.0)
                })?;
                let staging = staging.as_ref().expect("present staging identity");
                if !placement.staged.same_version(staging) {
                    return Err(format!(
                        "Interrupted placement staging changed and requires review: {}",
                        staging_path.display()
                    ));
                }
                crate::fs_at::rename_noreplace(staging_path, landing).map_err(|error| {
                    format!(
                        "Could not finish interrupted placement from {} to {}: {error}",
                        staging_path.display(),
                        landing.display()
                    )
                })?;
                crate::fs_util::sync_parent_namespace(landing).map_err(|error| {
                    format!(
                        "Could not sync interrupted placement at {}: {error}",
                        landing.display()
                    )
                })?;
                let placed = PathIdentity::observe_deep(landing).map_err(|error| {
                    format!("Could not prove finished interrupted placement: {error}")
                })?;
                if !placement.staged.same_version(&placed) {
                    return Err(format!(
                        "Interrupted placement at {} is not the sealed staged object",
                        landing.display()
                    ));
                }
                complete_interrupted_placement(
                    operation_id,
                    &step.key,
                    placement,
                    landing,
                    &placed,
                )?;
            }
            PlacementPhase::Prepared if !staging_gone => {
                return Err(format!(
                    "Interrupted placement for {} cannot finish: the destination is now occupied by another object; the staged copy is kept at {}",
                    landing.display(),
                    step.staging
                        .as_deref()
                        .unwrap_or_else(|| Path::new("?"))
                        .display()
                ));
            }
            PlacementPhase::Prepared | PlacementPhase::Placed => {
                return Err(format!(
                    "Interrupted placement for {} lost its proven staged object and requires review",
                    landing.display()
                ));
            }
        }
    }
    Ok(())
}

fn clear_failed_placement(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    placement: &PlacementProof,
) -> Result<(), String> {
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
        let step = step_mut(&mut operation.steps, key)?;
        if step.status != StepStatus::Failed || step.placement.as_ref() != Some(placement) {
            return Err(format!(
                "Placement proof changed while retrying failed step {}",
                key.0
            ));
        }
        step.placement = None;
        step.staging = None;
        step.checkpoint = None;
        operation.updated_at_secs = crate::clock::now_secs();
        Ok(())
    })
}

fn complete_interrupted_placement(
    operation_id: &OperationId,
    key: &IdempotencyKey,
    placement: &PlacementProof,
    landing: &Path,
    destination: &PathIdentity,
) -> Result<(), String> {
    match placement.phase {
        PlacementPhase::Prepared | PlacementPhase::Placed => {}
    }

    let destination_after = destination.clone();
    mutate(|journal| {
        let operation = operation_mut(&mut journal.operations, operation_id)?;
        let step = step_mut(&mut operation.steps, key)?;
        if step.placement.as_ref() != Some(placement) {
            return Err(format!(
                "Placement proof changed while completing step {}",
                key.0
            ));
        }
        if step.status == StepStatus::Completed {
            return check_duplicate_completion(step, landing, &destination_after, None);
        }
        step.status = step
            .status
            .transition(StepEvent::Complete)
            .map_err(|error| error.to_string())?;
        step.landing = Some(landing.to_path_buf());
        step.destination_after = Some(destination_after);
        step.checkpoint = None;
        if step.fast_path.is_none() {
            step.fast_path = Some(crate::transfer_tuning::FastPath::Resumed);
        }
        step.failure = None;
        step.placement = None;
        step.staging = None;
        operation.updated_at_secs = crate::clock::now_secs();
        Ok(())
    })
}
