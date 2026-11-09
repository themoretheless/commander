//! Rebuilding a transfer spec from a journaled operation.

use super::*;

pub fn build_resume_spec(operation_id: &OperationId) -> Result<TransferSpec, String> {
    reconcile_prepared_replacements(operation_id)?;
    reconcile_interrupted_placements(operation_id)?;
    let record = operation(operation_id)?;
    build_resume_spec_from(record)
}

pub(super) fn build_resume_spec_from(record: OperationRecord) -> Result<TransferSpec, String> {
    if record
        .steps
        .iter()
        .any(|step| step.status == StepStatus::RolledBack)
    {
        return Err(
            "Operation was partially rolled back; continue rollback or inspect it manually"
                .to_string(),
        );
    }
    for step in record
        .steps
        .iter()
        .filter(|step| step.status == StepStatus::Completed)
    {
        prove_completed_effect(step)?;
    }

    let mut entries = Vec::new();
    let mut expectations = Vec::new();
    let mut preflight_bytes = 0_u64;
    for step in record.steps.iter().filter(|step| {
        matches!(
            step.status,
            StepStatus::Planned | StepStatus::Running | StepStatus::Requeued | StepStatus::Failed
        )
    }) {
        if step
            .failure
            .as_ref()
            .is_some_and(|failure| failure.class == FailureClass::IntegrityUncertain)
        {
            return Err(format!(
                "Recovery step requires manual review: {}",
                step.source.display()
            ));
        }
        let resume = validated_checkpoint(step)?;
        let metadata = std::fs::symlink_metadata(&step.source)
            .map_err(|error| format!("Could not inspect {}: {error}", step.source.display()))?;
        let entry = FileEntry::from_meta(step.source.clone(), &metadata)
            .ok_or_else(|| format!("Invalid recovery source: {}", step.source.display()))?;
        let source = crate::scan::capture_transfer_source(&step.source, record.symlink_policy)
            .map_err(|failure| {
                format!(
                    "Could not re-stat recovery source {}: {}",
                    step.source.display(),
                    failure.message
                )
            })?;
        let source_before = step.source_before.as_ref().ok_or_else(|| {
            format!(
                "Recovery source was never captured: {}",
                step.source.display()
            )
        })?;
        let destination_before = step.destination_before.as_ref().ok_or_else(|| {
            format!(
                "Recovery destination was never captured: {}",
                step.destination.display()
            )
        })?;
        let destination = observe_with_expected_depth(&step.destination, destination_before)
            .map_err(|error| format!("Could not re-stat recovery destination: {error}"))?;
        if !source_before.same_binding(&source.lexical) {
            return Err(format!(
                "Recovery source changed since the operation: {}",
                step.source.display()
            ));
        }
        let expected_source = if step.source_proof_complete {
            let expected = crate::path_identity::TransferSourceIdentity {
                lexical: source_before.clone(),
                followed: step.source_followed.clone(),
                logical_bytes: step.source_logical_bytes.ok_or_else(|| {
                    format!(
                        "Recovery source proof has no logical size: {}",
                        step.source.display()
                    )
                })?,
            };
            if !expected.same_binding(&source) {
                return Err(format!(
                    "Recovery source bytes changed since the operation: {}",
                    step.source.display()
                ));
            }
            expected
        } else {
            if record.symlink_policy == crate::filesystem_policy::SymlinkPolicy::Follow
                && !source.followed.is_empty()
            {
                return Err(format!(
                    "Legacy recovery lacks followed-target proofs for {}",
                    step.source.display()
                ));
            }
            source
        };
        preflight_bytes = preflight_bytes
            .checked_add(expected_source.logical_bytes)
            .ok_or_else(|| "Recovery source size exceeds the supported range".to_string())?;
        if !destination_before.same_binding(&destination) {
            return Err(format!(
                "Recovery destination changed since the operation: {}",
                step.destination.display()
            ));
        }
        if let (Some(landing), Some(landing_before)) = (&step.landing, &step.landing_before) {
            let current = observe_with_expected_depth(landing, landing_before)
                .map_err(|error| format!("Could not re-stat recovery landing: {error}"))?;
            if !landing_before.same_binding(&current) {
                return Err(format!(
                    "Recovery landing changed since the operation: {}",
                    landing.display()
                ));
            }
        }
        entries.push(entry);
        expectations.push(TransferExpectation {
            key: Some(step.key.clone()),
            source: Ok(expected_source),
            destination: Ok(destination_before.clone()),
            landing: step.landing.clone(),
            landing_before: step.landing_before.clone(),
            resume,
        });
    }
    Ok(TransferSpec {
        operation_id: record.id,
        group_id: record.group_id,
        expectations,
        policy: record.policy,
        method: record.method,
        durability: record.durability,
        version_retention: record.version_retention,
        name_policy: record.name_policy,
        symlink_policy: record.symlink_policy,
        preflight_bytes: Some(preflight_bytes),
        post_success: record.post_success,
        rollback_cleanup: record.rollback_cleanup,
        rollback_cleanup_identity: record.rollback_cleanup_identity,
        ..TransferSpec::new(record.kind, entries, record.target)
    })
}

pub(super) fn observe_with_expected_depth(
    path: &Path,
    expected: &PathIdentity,
) -> std::io::Result<PathIdentity> {
    if expected.tree_fingerprint.is_some() {
        PathIdentity::observe_deep(path)
    } else {
        PathIdentity::observe(path)
    }
}

fn validated_checkpoint(step: &OperationStep) -> Result<Option<ResumeCheckpoint>, String> {
    let Some(staging) = &step.staging else {
        return Ok(None);
    };
    if !crate::fs_util::path_is_taken(staging) {
        return Ok(None);
    }
    let checkpoint = step.checkpoint.as_ref().ok_or_else(|| {
        format!(
            "Recovery staging has no verified checkpoint: {}",
            staging.display()
        )
    })?;
    if checkpoint.staging != *staging {
        return Err(format!(
            "Recovery checkpoint points at different staging: {}",
            staging.display()
        ));
    }
    checkpoint.verify(&step.source, staging).map_err(|reason| {
        format!(
            "Recovery checkpoint cannot be used ({reason}); inspect manually: {}",
            staging.display()
        )
    })?;
    Ok(Some(checkpoint.clone()))
}
