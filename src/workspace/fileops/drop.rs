//! Drag-and-drop completion into the transfer confirmation pipeline.
//!
//! Consumes [`crate::panel::DragState`] on each panel so drop/cancel share one
//! session API with the rest of the workspace.

use std::path::PathBuf;

use super::super::{
    CopyMethod, FileEntry, OverwritePolicy, PendingOp, PendingTransfer, TransferKind,
    TransferSpaceState, Workspace, filesystem_preflight,
};
use crate::scan;

/// Handle a completed drag (mouse released). The dragged entries are routed
/// through the same Move engine as F6 instead of a raw rename, so conflicts
/// are confirmed, cross-volume moves work, self/descendant drops are rejected
/// and errors surface. A clean, conflict-free drop runs immediately; a
/// conflicting one opens the confirmation dialog.
pub(crate) fn drop_dragged(workspace: &mut Workspace, notify: impl Fn() + Send + 'static) {
    drop_dragged_as(workspace, TransferKind::Move, notify);
}

/// Complete a drag using its announced effect. Option-drag copies; the default
/// and keyboard equivalent move. Both share identical preflight.
pub(crate) fn drop_dragged_as(
    workspace: &mut Workspace,
    kind: TransferKind,
    notify: impl Fn() + Send + 'static,
) {
    if workspace.has_unfinished_transfer_work()
        || workspace.pending_op.is_some()
        || workspace.mutation_commits_blocked()
    {
        cancel_drag(workspace);
        return;
    }
    let Some((paths, target)) = take_drop_plan(workspace) else {
        return;
    };
    let entries: Vec<FileEntry> = paths
        .iter()
        .filter_map(|path| {
            let meta = std::fs::metadata(path).ok()?;
            FileEntry::from_meta(path.clone(), &meta)
        })
        .collect();
    if entries.is_empty() {
        return;
    }
    let conflicts = scan::find_conflicts(&entries, &target);
    let flat = scan::pending_flat_list();
    let policy = match workspace.name_policy.collision {
        crate::filesystem_policy::CollisionPolicy::Ask => OverwritePolicy::Ask,
        crate::filesystem_policy::CollisionPolicy::KeepBoth => OverwritePolicy::KeepBoth,
        crate::filesystem_policy::CollisionPolicy::Skip => OverwritePolicy::SkipAll,
    };
    let has_conflicts = !conflicts.is_empty() && policy == OverwritePolicy::Ask;
    let generation = workspace.start_space_probe(
        entries.clone(),
        target.clone(),
        flat.clone(),
        workspace.symlink_policy,
        notify,
    );
    let filesystem = filesystem_preflight(&entries, &target, workspace.name_policy);
    workspace.pending_op = Some(PendingOp::Transfer(PendingTransfer {
        kind,
        entries,
        expectations: Vec::new(),
        target,
        conflicts,
        policy,
        method: CopyMethod::Native,
        durability: workspace.durability_profile,
        version_retention: workspace.version_retention,
        name_policy: workspace.name_policy,
        symlink_policy: workspace.symlink_policy,
        filesystem,
        flat,
        space: TransferSpaceState::Pending { generation },
        start_when_ready: !has_conflicts,
    }));
}

/// Keyboard equivalent of dropping the active selection onto the folder under
/// the cursor. The synthesized drag plan deliberately enters the normal drop
/// pipeline, preserving every safety check and confirmation.
pub(crate) fn transfer_selection_into_cursor_folder(
    workspace: &mut Workspace,
    kind: TransferKind,
    notify: impl Fn() + Send + 'static,
) {
    if workspace.has_unfinished_transfer_work()
        || workspace.pending_op.is_some()
        || workspace.mutation_commits_blocked()
    {
        return;
    }
    let Some((paths, target)) = keyboard_drop_plan(workspace) else {
        return;
    };
    let panel = workspace.active_panel();
    panel.drag.set(paths);
    panel.drag.set_drop_target(target);
    drop_dragged_as(workspace, kind, notify);
}

fn keyboard_drop_plan(workspace: &Workspace) -> Option<(Vec<PathBuf>, PathBuf)> {
    let panel = workspace.active_panel_ref();
    let target = panel.cursor_entry()?;
    if !target.is_dir || panel.selection_is_empty() {
        return None;
    }
    let paths = panel
        .selected_entries()
        .into_iter()
        .map(|entry| entry.path)
        .filter(|path| path != &target.path)
        .collect::<Vec<_>>();
    (!paths.is_empty()).then(|| (paths, target.path.clone()))
}

/// Resolve which panel is the drag source and where the drop lands, consuming
/// the drag/drop state. A target hovered in the source panel itself (drag onto
/// its own subdirectory) takes priority over the other panel. With no explicit
/// target the drag is consumed as a cancellation.
pub(crate) fn take_drop_plan(workspace: &mut Workspace) -> Option<(Vec<PathBuf>, PathBuf)> {
    let (source, other) = if !workspace.left.drag.is_empty() {
        (&mut workspace.left.drag, &mut workspace.right.drag)
    } else if !workspace.right.drag.is_empty() {
        (&mut workspace.right.drag, &mut workspace.left.drag)
    } else {
        return None;
    };
    let target = source
        .take_drop_target()
        .or_else(|| other.take_drop_target());
    let paths = source.take_entries();
    other.clear_drop_target();
    if paths.is_empty() {
        return None;
    }
    Some((paths, target?))
}

pub(crate) fn cancel_drag(workspace: &mut Workspace) {
    let _ = workspace.left.drag.take();
    let _ = workspace.right.drag.take();
}
