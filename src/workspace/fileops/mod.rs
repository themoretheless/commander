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

#[cfg(test)]
pub(crate) use batch_rename::apply_rename_order_using;
pub(crate) use batch_rename::{
    apply_batch_rename_in, apply_rename_order, batch_rename_context, dir_names,
};
pub(crate) use link::{LinkReport, create_hardlinks, create_symlinks};
pub(crate) use drop::{
    cancel_drag, drop_dragged, drop_dragged_as, transfer_selection_into_cursor_folder,
};
pub(crate) use mkdir::create_dir;
pub(crate) use pending::confirm_pending_op;
pub(crate) use rename::{
    commit_rename, latch_rename_execution_error, rename_path_no_clobber, rename_siblings,
};
