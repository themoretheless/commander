//! Confirm a staged [`PendingOp`](crate::workspace::PendingOp).

use super::super::{DeleteOrigin, PendingOp, Workspace};

/// Confirm the pending op. Both transfers and deletes report completion
/// asynchronously through their controller poll methods.
pub(crate) fn confirm_pending_op(
    workspace: &mut Workspace,
    notify: impl Fn() + Send + 'static,
) -> bool {
    if workspace.mutation_commits_blocked() {
        return false;
    }
    match &workspace.pending_op {
        Some(PendingOp::Delete { .. }) => {
            if workspace.has_unfinished_transfer_work() || workspace.deletes.is_active() {
                return false;
            }
            if let Some(PendingOp::Delete { targets, .. }) = workspace.pending_op.take() {
                // Trash undo restores through version_store, so user deletes
                // always run Versioned regardless of the transfer profile.
                return workspace.deletes.start(
                    targets,
                    DeleteOrigin::Confirmation,
                    crate::operation::DurabilityProfile::Versioned,
                    workspace.version_retention,
                    notify,
                );
            }
            false
        }
        Some(PendingOp::Transfer(_)) => {
            workspace.start_transfer(notify);
            workspace.pending_op.is_none()
        }
        None => false,
    }
}
