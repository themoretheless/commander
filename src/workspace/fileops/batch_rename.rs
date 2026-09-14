//! Batch rename planning helpers and apply path.

use std::path::Path;

use super::super::{ActivePanel, BatchRenameContext, Workspace};
use super::rename::{RenameExecutionError, latch_rename_execution_error};

/// Every name currently in `dir` (best-effort), so a rename batch can be
/// ordered against the live directory at apply/replay time.
pub(crate) fn dir_names(dir: &Path) -> std::collections::HashSet<String> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect()
}

/// Apply a `(from, to)` rename map in `dir` by computing a mid-batch-safe
/// order (swaps, rotations and case-only renames are staged through a temp;
/// see [`crate::rename_order`]) and executing it, rolling back on an OS
/// failure. Returns how many entries were renamed, or a user-facing error
/// (an unresolvable conflict, or the first OS error).
pub(crate) fn apply_rename_order(
    dir: &Path,
    map: &[(String, String)],
    existing: &std::collections::HashSet<String>,
) -> Result<usize, RenameExecutionError> {
    apply_rename_order_using(dir, map, existing, |from, to| {
        crate::native_copy::rename_noreplace(&dir.join(from), &dir.join(to))
    })
}

pub(crate) fn apply_rename_order_using<E: std::fmt::Display>(
    dir: &Path,
    map: &[(String, String)],
    existing: &std::collections::HashSet<String>,
    rename: impl FnMut(&str, &str) -> Result<(), E>,
) -> Result<usize, RenameExecutionError> {
    use crate::rename_order::{RenameOrder, apply_steps, safe_rename_order};
    match safe_rename_order(map, existing) {
        RenameOrder::Conflict(why) => Err(RenameExecutionError::unchanged(why)),
        RenameOrder::Steps(steps) => apply_steps(&steps, rename).map_err(|error| {
            let message = error.to_string();
            if error.rollback_failures.is_empty() {
                RenameExecutionError::unchanged(message)
            } else {
                let mut paths = error
                    .rollback_failures
                    .iter()
                    .flat_map(|failure| [dir.join(&failure.from), dir.join(&failure.to)])
                    .collect::<Vec<_>>();
                paths.sort();
                paths.dedup();
                RenameExecutionError::uncertain(message, paths)
            }
        }),
    }
}

pub(crate) fn batch_rename_context(workspace: &Workspace) -> Option<BatchRenameContext> {
    let panel = workspace.active_panel_ref();
    let targets: Vec<String> = panel
        .selected_or_cursor()
        .ok()?
        .into_iter()
        .map(|e| e.name)
        .collect();
    if targets.is_empty() {
        return None;
    }
    Some(BatchRenameContext {
        panel: workspace.active,
        dir: panel.current_path.clone(),
        targets,
        existing: panel.entries().iter().map(|e| e.name.clone()).collect(),
    })
}

/// Apply a batch rename to the exact context captured when its dialog
/// opened. The live directory is re-read at commit time so newly-created
/// siblings still participate in collision checks.
pub(crate) fn apply_batch_rename_in(
    workspace: &mut Workspace,
    context: &BatchRenameContext,
    rule: &crate::rename::RenameRule,
) -> Result<usize, String> {
    if let Some(reason) = workspace.mutation_block_reason("batch rename") {
        return Err(reason);
    }
    if context.targets.is_empty() {
        return Err("Nothing selected to rename".into());
    }
    if let Some(error) = crate::rename::regex_error(rule) {
        return Err(format!("Invalid regex: {error}"));
    }
    let existing = dir_names(&context.dir);
    let plans = crate::rename::plan_batch_rename(&context.targets, &existing, rule);
    if plans
        .iter()
        .any(|p| p.status == crate::rename::PlanStatus::Invalid)
    {
        return Err("Fix the invalid names first".into());
    }
    // Every row whose name actually changes (Ok or a resolvable collision).
    let changes: Vec<(String, String)> = plans
        .iter()
        .filter(|p| p.to != p.from)
        .map(|p| (p.from.clone(), p.to.clone()))
        .collect();
    if changes.is_empty() {
        return Ok(0);
    }
    let dir = context.dir.clone();

    let done = match apply_rename_order(&dir, &changes, &existing) {
        Ok(done) => done,
        Err(error) => {
            latch_rename_execution_error(workspace, &error);
            let panel = match context.panel {
                ActivePanel::Left => &mut workspace.left,
                ActivePanel::Right => &mut workspace.right,
            };
            if panel.current_path == context.dir {
                panel.refresh();
            }
            return Err(error.to_string());
        }
    };
    // Record the batch as one undoable unit (Cmd+Z reverts the whole run).
    if done > 0 {
        workspace
            .undo
            .record(crate::undo::Action::BatchRename {
                dir,
                pairs: changes,
            })
            .map_err(|error| workspace.history_invariant_error(error))?;
    }
    let panel = match context.panel {
        ActivePanel::Left => &mut workspace.left,
        ActivePanel::Right => &mut workspace.right,
    };
    if panel.current_path == context.dir {
        panel.clear_selection();
        panel.refresh();
    }
    Ok(done)
}
