//! Schema migration and structural validation of a loaded journal.

use super::*;

pub(super) fn migrate_and_validate(mut journal: Journal) -> Result<Journal, String> {
    let source_schema = journal.schema;
    for operation in &mut journal.operations {
        for step in &mut operation.steps {
            if step.rollback.is_none()
                && let Some(quarantine) = step.rollback_quarantine.take()
            {
                step.rollback = Some(RollbackReceipt {
                    quarantine,
                    phase: RollbackPhase::Prepared,
                    version: None,
                    version_identity: None,
                    restored_destination: None,
                });
                operation.status = OperationStatus::NeedsReview;
            }
        }
        if source_schema < JOURNAL_SCHEMA {
            let unproved_checkpoint = operation.steps.iter().any(|step| {
                step.checkpoint
                    .as_ref()
                    .is_some_and(|checkpoint| checkpoint.content_digest.is_none())
            });
            let unproved_container = operation.rollback_cleanup.is_some()
                && operation.rollback_cleanup_identity.is_none();
            if unproved_checkpoint || unproved_container {
                operation.status = OperationStatus::NeedsReview;
            }
        }
    }
    if source_schema < JOURNAL_SCHEMA {
        journal.schema = JOURNAL_SCHEMA;
    }
    validate_journal(&journal)?;
    Ok(journal)
}

pub(super) fn validate_journal(journal: &Journal) -> Result<(), String> {
    if journal.schema != JOURNAL_SCHEMA {
        return Err(format!(
            "Operation journal schema {} was not migrated to {JOURNAL_SCHEMA}",
            journal.schema
        ));
    }
    if journal.operations.len() > MAX_OPERATIONS {
        return Err(format!(
            "Operation journal contains {} records; the safe limit is {MAX_OPERATIONS}",
            journal.operations.len()
        ));
    }

    let mut operation_ids = HashSet::new();
    for operation in &journal.operations {
        if !operation_ids.insert(&operation.id) {
            return Err(format!(
                "Operation journal contains duplicate operation id {}",
                operation.id.0
            ));
        }
        if operation.rollback_cleanup.is_some() != operation.rollback_cleanup_identity.is_some()
            && operation.status != OperationStatus::NeedsReview
        {
            return Err(format!(
                "Operation {} has an unproved rollback container",
                operation.id.0
            ));
        }
        if let (Some(path), Some(identity)) = (
            operation.rollback_cleanup.as_ref(),
            operation.rollback_cleanup_identity.as_ref(),
        ) && identity.path != *path
        {
            return Err(format!(
                "Operation {} rollback container proof is bound to another path",
                operation.id.0
            ));
        }
        if operation.rollback_cleanup_quarantine.is_some() && operation.rollback_cleanup.is_none() {
            return Err(format!(
                "Operation {} has a container quarantine without a container",
                operation.id.0
            ));
        }

        let mut keys = HashSet::new();
        for step in &operation.steps {
            if !keys.insert(&step.key) {
                return Err(format!(
                    "Operation {} contains duplicate step key {}",
                    operation.id.0, step.key.0
                ));
            }
            validate_step(operation, step)?;
        }

        if operation.status == OperationStatus::Completed
            && (operation.rollback_cleanup_quarantine.is_some()
                || operation.steps.iter().any(|step| {
                    !matches!(step.status, StepStatus::Completed | StepStatus::Skipped)
                        || step.replacement.is_some()
                        || step.rollback.is_some()
                        || step.rollback_quarantine.is_some()
                }))
        {
            return Err(format!(
                "Completed operation {} has unsettled or unreconciled steps",
                operation.id.0
            ));
        }
        if operation.status == OperationStatus::RolledBack
            && (operation.rollback_cleanup_quarantine.is_some()
                || operation.steps.iter().any(|step| {
                    matches!(step.status, StepStatus::Running | StepStatus::Completed)
                        || step
                            .rollback
                            .as_ref()
                            .is_some_and(|receipt| receipt.phase != RollbackPhase::Complete)
                        || step.rollback_quarantine.is_some()
                }))
        {
            return Err(format!(
                "Rolled-back operation {} still has an applied or in-flight effect",
                operation.id.0
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_step(
    operation: &OperationRecord,
    step: &OperationStep,
) -> Result<(), String> {
    let invalid_binding = |identity: &Option<PathIdentity>, path: &Path| {
        identity
            .as_ref()
            .is_some_and(|identity| identity.path != path)
    };
    if invalid_binding(&step.source_before, &step.source)
        || invalid_binding(&step.destination_before, &step.destination)
        || step.landing_before.as_ref().is_some_and(|identity| {
            step.landing
                .as_ref()
                .is_none_or(|landing| identity.path != *landing)
        })
        || step
            .destination_after
            .as_ref()
            .is_some_and(|identity| identity.path != effect_path(step))
    {
        return Err(format!(
            "Operation {} step {} contains a path identity bound to another path",
            operation.id.0, step.key.0
        ));
    }
    let mut reachable_roots = vec![(
        step.source.clone(),
        step.source_before.as_ref().is_some_and(|identity| {
            identity.kind == Some(crate::path_identity::PathKind::Directory)
        }),
    )];
    for proof in &step.source_followed {
        if !reachable_roots.iter().any(|(root, descendants)| {
            proof.link == *root || (*descendants && proof.link.starts_with(root))
        }) {
            return Err(format!(
                "Operation {} step {} contains an unreachable followed proof",
                operation.id.0, step.key.0
            ));
        }
        if proof.target.kind == Some(crate::path_identity::PathKind::Directory) {
            reachable_roots.push((proof.target.path.clone(), true));
        }
    }
    if step.source_proof_complete
        && (step.source_before.is_none() || step.source_logical_bytes.is_none())
    {
        return Err(format!(
            "Operation {} step {} contains an incomplete source proof",
            operation.id.0, step.key.0
        ));
    }
    if !step.source_proof_complete
        && (!step.source_followed.is_empty() || step.source_logical_bytes.is_some())
    {
        return Err(format!(
            "Operation {} step {} contains unbound source proof fields",
            operation.id.0, step.key.0
        ));
    }
    if operation.symlink_policy != crate::filesystem_policy::SymlinkPolicy::Follow
        && !step.source_followed.is_empty()
    {
        return Err(format!(
            "Operation {} step {} contains followed proofs under another symlink policy",
            operation.id.0, step.key.0
        ));
    }

    if matches!(step.status, StepStatus::Completed | StepStatus::RolledBack)
        && (step.landing.is_none() || step.destination_after.is_none())
    {
        return Err(format!(
            "Operation {} step {} is completed without a terminal proof",
            operation.id.0, step.key.0
        ));
    }
    if !matches!(step.status, StepStatus::Completed | StepStatus::RolledBack)
        && step.destination_after.is_some()
    {
        return Err(format!(
            "Operation {} step {} has a terminal proof in a non-completed state",
            operation.id.0, step.key.0
        ));
    }
    if let Some(checkpoint) = &step.checkpoint {
        if step.staging.as_ref() != Some(&checkpoint.staging)
            || checkpoint.partial.path != checkpoint.staging
            || checkpoint.source.path != step.source
        {
            return Err(format!(
                "Operation {} step {} has a checkpoint bound to another path",
                operation.id.0, step.key.0
            ));
        }
        if checkpoint.content_digest.is_none() && operation.status != OperationStatus::NeedsReview {
            return Err(format!(
                "Operation {} step {} has a legacy checkpoint without content proof",
                operation.id.0, step.key.0
            ));
        }
    }
    if let Some(replacement) = &step.replacement
        && (replacement.path == effect_path(step)
            || replacement.original.path != effect_path(step)
            || replacement.replacement.path
                != step.staging.as_deref().unwrap_or_else(|| effect_path(step)))
    {
        return Err(format!(
            "Operation {} step {} has an invalid overwrite proof",
            operation.id.0, step.key.0
        ));
    }
    if let Some(placement) = &step.placement {
        if step.replacement.is_some() {
            return Err(format!(
                "Operation {} step {} has both overwrite and placement proofs",
                operation.id.0, step.key.0
            ));
        }
        let staging_ok = step
            .staging
            .as_ref()
            .is_some_and(|path| placement.staged.path == *path);
        if !staging_ok && !(placement.phase == PlacementPhase::Placed && step.landing.is_some()) {
            return Err(format!(
                "Operation {} step {} has an invalid placement proof",
                operation.id.0, step.key.0
            ));
        }
        if matches!(step.status, StepStatus::Completed | StepStatus::RolledBack) {
            return Err(format!(
                "Operation {} step {} still has an unreconciled placement proof",
                operation.id.0, step.key.0
            ));
        }
    }
    if step.rollback_quarantine.is_some() {
        return Err(format!(
            "Operation {} step {} contains an unmigrated rollback quarantine",
            operation.id.0, step.key.0
        ));
    }
    if let Some(receipt) = &step.rollback {
        if step.status != StepStatus::Completed && step.status != StepStatus::RolledBack {
            return Err(format!(
                "Operation {} step {} has rollback state outside an applied effect",
                operation.id.0, step.key.0
            ));
        }
        if receipt.quarantine == effect_path(step)
            || receipt.quarantine == step.source
            || receipt.version.as_ref().is_some_and(|version| {
                version.key != step.key || version.original != effect_path(step)
            })
            || receipt.version_identity.as_ref().is_some_and(|identity| {
                receipt
                    .version
                    .as_ref()
                    .is_none_or(|version| identity.path != version.stored)
            })
            || receipt
                .restored_destination
                .as_ref()
                .is_some_and(|identity| identity.path != effect_path(step))
        {
            return Err(format!(
                "Operation {} step {} has an invalid rollback receipt",
                operation.id.0, step.key.0
            ));
        }
        let replaces_existing = step
            .destination_before
            .as_ref()
            .is_some_and(|identity| identity.exists);
        if replaces_existing
            && receipt.phase >= RollbackPhase::EffectDetached
            && (receipt.version.is_none() || receipt.version_identity.is_none())
        {
            return Err(format!(
                "Operation {} step {} detached an overwrite without a durable version receipt",
                operation.id.0, step.key.0
            ));
        }
        if (replaces_existing
            && receipt.phase >= RollbackPhase::DestinationRestored
            && receipt.restored_destination.is_none())
            || (!replaces_existing && receipt.restored_destination.is_some())
        {
            return Err(format!(
                "Operation {} step {} claims an invalid destination restore",
                operation.id.0, step.key.0
            ));
        }
        if receipt.phase == RollbackPhase::Complete && step.status != StepStatus::RolledBack {
            return Err(format!(
                "Operation {} step {} has a terminal rollback receipt before rollback completion",
                operation.id.0, step.key.0
            ));
        }
    }
    Ok(())
}

pub(super) fn prune_terminal_history(journal: &mut Journal) -> Result<(), String> {
    let remove_count = journal.operations.len().saturating_sub(MAX_OPERATIONS);
    if remove_count == 0 {
        return Ok(());
    }
    let mut removable = journal
        .operations
        .iter()
        .filter(|operation| !operation.status.recoverable())
        .count();
    if removable < remove_count {
        for operation in &mut journal.operations {
            if removable >= remove_count {
                break;
            }
            if recoverable_operation_is_effect_free(operation) {
                operation.status = OperationStatus::RolledBack;
                removable += 1;
            }
        }
    }
    if removable < remove_count {
        return Err(format!(
            "Operation journal is full with recoverable work; refusing to discard recovery records (limit {MAX_OPERATIONS})"
        ));
    }
    let mut remaining = remove_count;
    journal.operations.retain(|operation| {
        if remaining > 0 && !operation.status.recoverable() {
            remaining -= 1;
            false
        } else {
            true
        }
    });
    Ok(())
}
