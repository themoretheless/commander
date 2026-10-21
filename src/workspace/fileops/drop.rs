//! Drag-and-drop completion into the transfer confirmation pipeline.
//!
//! Consumes [`crate::panel::DragState`] on each panel so drop/cancel share one
//! session API with the rest of the workspace.

use std::path::PathBuf;

use super::super::{ActivePanel, FileEntry, TransferKind, Workspace};
use crate::ui_request::UiRequest;

/// Handle a completed drag (mouse released). The dragged entries are routed
/// through the same Move engine as F6 instead of a raw rename, so conflicts
/// are confirmed, cross-volume moves work, self/descendant drops are rejected
/// and errors surface. A clean, conflict-free drop runs immediately; a
/// conflicting one opens the confirmation dialog.
#[cfg(test)]
pub fn drop_dragged(workspace: &mut Workspace, notify: impl Fn() + Send + 'static) {
    drop_dragged_as(workspace, TransferKind::Move, notify);
}

/// Complete a drag using its announced effect. Option-drag copies; the default
/// and keyboard equivalent move. Both share identical preflight.
pub fn drop_dragged_as(
    workspace: &mut Workspace,
    kind: TransferKind,
    notify: impl Fn() + Send + 'static,
) {
    if let Some(reason) = drop_refusal(workspace) {
        // Say why a real drop (released over a target) did nothing; F5/F6
        // queue instead, so the user needs to know the difference.
        let dropped_on_target = drag_session(workspace).is_some_and(|s| s.target.is_some());
        cancel_drag(workspace);
        if dropped_on_target {
            workspace.emit_ui_request(UiRequest::Notice {
                message: reason,
                error: false,
            });
        }
        return;
    }
    let Some((paths, target)) = take_drop_plan(workspace) else {
        return;
    };
    // Bind the dragged rows lexically, like the listing they came from: a
    // dragged link is the link (not its target), and a broken link still
    // moves.
    let entries: Vec<FileEntry> = paths
        .iter()
        .filter_map(|path| FileEntry::observe_lexical(path.clone()))
        .collect();
    if entries.is_empty() {
        workspace.emit_ui_request(UiRequest::Notice {
            message: "Nothing to drop: the dragged items no longer exist".to_string(),
            error: true,
        });
        return;
    }
    workspace.stage_transfer(kind, entries, target, true, notify);
}

/// Why a drop cannot start now, or `None` when it can.
fn drop_refusal(workspace: &Workspace) -> Option<String> {
    if workspace.has_unfinished_transfer_work() {
        Some(
            "Drop cancelled while transfers are queued or running; use Copy (F5) or Move (F6) to queue it"
                .to_string(),
        )
    } else if workspace.pending_op.is_some() {
        Some("Drop cancelled: finish the open confirmation first".to_string())
    } else {
        workspace.mutation_block_reason("dropping files")
    }
}

/// Keyboard equivalent of dropping the active selection onto the folder under
/// the cursor. The synthesized drag plan deliberately enters the normal drop
/// pipeline, preserving every safety check and confirmation.
pub fn transfer_selection_into_cursor_folder(
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
    if !target.is_dir {
        return None;
    }
    let paths = panel
        .picked_entries_excluding(&target.path)
        .ok()?
        .into_iter()
        .map(|entry| entry.path)
        .collect::<Vec<_>>();
    (!paths.is_empty()).then(|| (paths, target.path.clone()))
}

/// A drag in flight, read from both panels' [`crate::panel::DragState`].
pub struct DragSession<'a> {
    pub source: ActivePanel,
    pub entries: &'a [PathBuf],
    /// A target hovered in the source panel itself (drag onto its own
    /// subdirectory) takes priority over the other panel's folder.
    pub target: Option<&'a PathBuf>,
}

/// The panel whose drag holds entries. Left wins if both somehow do.
pub const fn drag_source(workspace: &Workspace) -> Option<ActivePanel> {
    if !workspace.left.drag.is_empty() {
        Some(ActivePanel::Left)
    } else if !workspace.right.drag.is_empty() {
        Some(ActivePanel::Right)
    } else {
        None
    }
}

pub fn drag_session(workspace: &Workspace) -> Option<DragSession<'_>> {
    let source = drag_source(workspace)?;
    let (from, other) = match source {
        ActivePanel::Left => (&workspace.left.drag, &workspace.right.drag),
        ActivePanel::Right => (&workspace.right.drag, &workspace.left.drag),
    };
    Some(DragSession {
        source,
        entries: from.entries(),
        target: from.drop_target().or_else(|| other.drop_target()),
    })
}

/// Consume the drag session: its entries and resolved target. With no
/// explicit target the drag is consumed as a cancellation.
pub fn take_drop_plan(workspace: &mut Workspace) -> Option<(Vec<PathBuf>, PathBuf)> {
    let (source, other) = match drag_source(workspace)? {
        ActivePanel::Left => (&mut workspace.left.drag, &mut workspace.right.drag),
        ActivePanel::Right => (&mut workspace.right.drag, &mut workspace.left.drag),
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

pub fn cancel_drag(workspace: &mut Workspace) {
    let _ = workspace.left.drag.take();
    let _ = workspace.right.drag.take();
}
