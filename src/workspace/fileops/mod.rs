//! Coherent file-operation facades extracted from [`crate::workspace::Workspace`].
//!
//! Each submodule owns one mutating command family. [`Workspace`] keeps thin
//! public wrappers so UI and `execute` call sites stay stable. Delete, transfer
//! queue, space probe, undo, gather, and recovery stay outside this tree.

mod batch_rename;
mod drop;
mod link;
mod mkdir;
mod pending;
mod rename;

pub use batch_rename::{
    apply_batch_rename_in, apply_rename_order_with_folding, batch_rename_context, dir_names,
};
#[cfg(test)]
pub use batch_rename::{apply_rename_order, apply_rename_order_using};
#[cfg(test)]
pub use drop::drop_dragged;
pub use drop::{
    DragSession, cancel_drag, drag_session, drop_dragged_as, transfer_selection_into_cursor_folder,
};
pub use link::{LinkReport, create_hardlinks, create_symlinks};
pub use mkdir::create_dir;
pub use pending::confirm_pending_op;
pub use rename::{
    RenameExecutionError, commit_rename, latch_rename_execution_error, rename_path_no_clobber,
    rename_siblings,
};
