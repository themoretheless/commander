//! Single-file rename (F2), including case-only staging.

use std::path::{Path, PathBuf};

use super::super::Workspace;
use super::batch_rename::dir_names;

#[derive(Debug)]
pub(crate) struct RenameExecutionError {
    pub(crate) message: String,
    pub(crate) integrity_uncertain: bool,
    pub(crate) paths: Vec<PathBuf>,
}

impl RenameExecutionError {
    pub(crate) fn unchanged(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            integrity_uncertain: false,
            paths: Vec::new(),
        }
    }

    pub(crate) fn uncertain(message: impl Into<String>, paths: Vec<PathBuf>) -> Self {
        Self {
            message: message.into(),
            integrity_uncertain: true,
            paths,
        }
    }
}

impl std::fmt::Display for RenameExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

/// Names beside `old`, excluding `old` itself. Captured by the rename UI at
/// open time; commit performs the same check again against the live disk.
pub(crate) fn rename_siblings(old: &Path) -> Vec<String> {
    let old_name = old.file_name().map(|n| n.to_string_lossy().to_string());
    old.parent()
        .map(dir_names)
        .unwrap_or_default()
        .into_iter()
        .filter(|name| Some(name) != old_name.as_ref())
        .collect()
}

/// Rename one path without replacing an unrelated destination. Case-only
/// changes stage through a temporary name and roll back if the second move
/// fails, preserving the recovery path in a composite error if rollback
/// itself also fails.
pub(crate) fn rename_path_no_clobber(from: &Path, to: &Path) -> Result<(), RenameExecutionError> {
    if from == to {
        return Ok(());
    }
    let dest_meta = to.symlink_metadata().ok();
    let same_file = match &dest_meta {
        Some(dest) => from.symlink_metadata().ok().is_some_and(|source| {
            use std::os::unix::fs::MetadataExt;
            source.ino() == dest.ino() && source.dev() == dest.dev()
        }),
        None => false,
    };
    if dest_meta.is_some() && !same_file {
        return Err(RenameExecutionError::unchanged("Name already in use"));
    }
    if !same_file {
        return crate::native_copy::rename_noreplace(from, to)
            .map_err(|error| RenameExecutionError::unchanged(error.to_string()));
    }

    let parent = to
        .parent()
        .ok_or_else(|| RenameExecutionError::unchanged("Path has no parent"))?;
    let tmp = crate::fs_util::first_available(|i| parent.join(format!(".cmdr-rename.{i}")));
    crate::native_copy::rename_noreplace(from, &tmp)
        .map_err(|error| RenameExecutionError::unchanged(error.to_string()))?;
    match crate::native_copy::rename_noreplace(&tmp, to) {
        Ok(()) => Ok(()),
        Err(rename_error) => match crate::native_copy::rename_noreplace(&tmp, from) {
            Ok(()) => Err(RenameExecutionError::unchanged(rename_error.to_string())),
            Err(rollback_error) => Err(RenameExecutionError::uncertain(
                format!(
                    "{rename_error}; rollback failed: {rollback_error}; file preserved at {}",
                    tmp.display()
                ),
                vec![from.to_path_buf(), to.to_path_buf(), tmp],
            )),
        },
    }
}

pub(crate) fn latch_rename_execution_error(
    workspace: &mut Workspace,
    error: &RenameExecutionError,
) {
    if !error.integrity_uncertain || workspace.safe_state.is_some() {
        return;
    }
    let reason = format!(
        "Rename rollback did not restore the original namespace: {}",
        error.message
    );
    let failure = crate::operation::ClassifiedFailure::message(
        crate::operation::FailureClass::IntegrityUncertain,
        error.paths.first().cloned(),
        reason.clone(),
    );
    workspace.safe_state = Some(crate::operation::SafeState {
        operation_id: crate::operation::OperationId::new(),
        reason,
        paths: error.paths.clone(),
        failures: vec![failure],
    });
}

/// Rename `old` to `new_name` in the same directory. A no-op (unchanged
/// name) succeeds silently. Successful changes are recorded for undo/redo.
pub(crate) fn commit_rename(
    workspace: &mut Workspace,
    old: &Path,
    new_name: &str,
) -> Result<(), String> {
    if let Some(reason) = workspace.mutation_block_reason("rename") {
        return Err(reason);
    }
    let new_name = new_name.trim();
    let old_name = old
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if new_name == old_name {
        return Ok(()); // nothing to do
    }
    let siblings = rename_siblings(old);
    crate::pathname::validate_new_name(new_name, &siblings).map_err(|error| error.to_string())?;
    let dest = old
        .parent()
        .map(|p| p.join(new_name))
        .ok_or("Path has no parent")?;
    if let Err(error) = rename_path_no_clobber(old, &dest) {
        latch_rename_execution_error(workspace, &error);
        return Err(error.to_string());
    }
    workspace
        .undo
        .record(crate::undo::Action::Rename {
            from: old.to_path_buf(),
            to: dest,
        })
        .map_err(|error| workspace.history_invariant_error(error))?;
    workspace.left.refresh();
    workspace.right.refresh();
    Ok(())
}
