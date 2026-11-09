//! Recovery inventory and orphaned staging discovery.

use super::*;

pub fn recovery_inventory(extra: &[PathBuf]) -> Result<RecoveryInventory, String> {
    let journal = load()?;
    let mut roots = extra
        .iter()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let mut referenced = std::collections::HashSet::new();
    let mut retained = Vec::new();
    for operation in &journal.operations {
        if let Some(quarantine) = &operation.rollback_cleanup_quarantine {
            referenced.insert(quarantine.clone());
        }
        for step in &operation.steps {
            if let Some(staging) = &step.staging {
                referenced.insert(staging.clone());
            }
            if let Some(replacement) = &step.replacement {
                referenced.insert(replacement.path.clone());
            }
            if let Some(receipt) = &step.rollback
                && receipt.phase != RollbackPhase::Complete
            {
                referenced.insert(receipt.quarantine.clone());
            }
        }
        if !operation_needs_recovery(operation) {
            let paths = operation.steps.iter().flat_map(|step| {
                [
                    step.staging.as_ref(),
                    step.replacement
                        .as_ref()
                        .map(|replacement| &replacement.path),
                    step.rollback.as_ref().map(|receipt| &receipt.quarantine),
                ]
                .into_iter()
                .flatten()
            });
            for path in paths {
                if let Ok(identity) = PathIdentity::observe(path)
                    && identity.exists
                {
                    retained.push(RetainedStaging {
                        path: path.clone(),
                        identity,
                        operation_id: operation.id.clone(),
                    });
                }
            }
        }
        if !operation_needs_recovery(operation) {
            continue;
        }
        roots.insert(operation.target.clone());
        if let Some(quarantine) = &operation.rollback_cleanup_quarantine
            && let Some(parent) = quarantine.parent()
        {
            roots.insert(parent.to_path_buf());
        }
        for step in &operation.steps {
            for path in [
                step.landing.as_ref(),
                step.staging.as_ref(),
                step.replacement
                    .as_ref()
                    .map(|replacement| &replacement.path),
                step.rollback
                    .as_ref()
                    .filter(|receipt| receipt.phase != RollbackPhase::Complete)
                    .map(|receipt| &receipt.quarantine),
            ]
            .into_iter()
            .flatten()
            {
                if let Some(parent) = path.parent() {
                    roots.insert(parent.to_path_buf());
                }
            }
        }
    }
    let mut roots = roots.into_iter().collect::<Vec<_>>();
    roots.sort();
    let mut operations = journal
        .operations
        .into_iter()
        .filter(operation_needs_recovery)
        .collect::<Vec<_>>();
    operations.sort_by_key(|operation| std::cmp::Reverse(operation.updated_at_secs));
    retained.sort_by(|a, b| a.path.cmp(&b.path));
    retained.dedup_by(|a, b| a.path == b.path);
    Ok(RecoveryInventory {
        operations,
        orphans: discover_orphan_staging_with(&roots, &referenced),
        retained,
    })
}

fn operation_needs_recovery(operation: &OperationRecord) -> bool {
    operation.status.recoverable()
        || operation.rollback_cleanup_quarantine.is_some()
        || operation.steps.iter().any(|step| {
            step.replacement.is_some()
                || step
                    .rollback
                    .as_ref()
                    .is_some_and(|receipt| receipt.phase != RollbackPhase::Complete)
        })
}

pub(super) fn discover_orphan_staging_with(
    roots: &[PathBuf],
    referenced: &std::collections::HashSet<PathBuf>,
) -> Vec<OrphanStaging> {
    let mut orphans = Vec::new();
    for root in roots {
        crate::io_budget::background_checkpoint(|| false);
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if is_commander_temporary_name(&name)
                && !referenced.contains(&path)
                && let Ok(identity) = PathIdentity::observe_deep(&path)
            {
                orphans.push(OrphanStaging { path, identity });
            }
        }
    }
    orphans.sort_by(|left, right| left.path.cmp(&right.path));
    orphans
}

/// Match only names Commander generates: `.{name}.cmdr-tmp.{n}` and
/// `.{name}.cmdr-quarantine.{n}`. A user file that merely contains a marker
/// is never offered for deletion.
pub(super) fn is_commander_temporary_name(name: &str) -> bool {
    name.starts_with('.')
        && [".cmdr-tmp.", ".cmdr-quarantine."].iter().any(|marker| {
            name.rfind(marker).is_some_and(|at| {
                let index = &name[at + marker.len()..];
                at > 0 && !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
}

pub fn clean_orphan(orphan: &OrphanStaging) -> Result<(), String> {
    let current = PathIdentity::observe_deep(&orphan.path)
        .map_err(|error| format!("Could not recheck orphan staging: {error}"))?;
    if !orphan.identity.same_binding(&current) {
        return Err("Orphan staging changed after discovery".to_string());
    }
    remove_expected_path(&orphan.path, &orphan.identity)
}
