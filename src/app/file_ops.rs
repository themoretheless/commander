//! Glue between the UI and the transfer engine: building pending
//! operations, confirming them and tracking the active transfer.

use super::*;
use std::sync::{Arc, Mutex};

use crate::scan;
use crate::transfer::{self, TransferSpec};

impl App {
    pub(crate) fn request_copy(&mut self) {
        self.request_transfer(TransferKind::Copy);
    }

    pub(crate) fn request_move(&mut self) {
        self.request_transfer(TransferKind::Move);
    }

    fn request_transfer(&mut self, kind: TransferKind) {
        let target = self.inactive_panel().current_path.clone();
        let entries = self.active_panel().selected_or_cursor();
        if entries.is_empty() {
            return;
        }
        let flat = scan::spawn_scan(entries.clone());
        let conflicts = scan::find_conflicts(&entries, &target);
        self.pending_op = Some(PendingOp::Transfer(PendingTransfer {
            kind,
            entries,
            target,
            conflicts,
            policy: OverwritePolicy::Ask,
            method: CopyMethod::Native,
            flat,
        }));
    }

    pub(crate) fn request_delete(&mut self) {
        let entries = self.active_panel().selected_or_cursor();
        if !entries.is_empty() {
            let flat = scan::spawn_scan(entries.clone());
            self.pending_op = Some(PendingOp::Delete { entries, flat });
        }
    }

    /// Start background copy/move with progress tracking.
    pub(crate) fn start_transfer(&mut self, ctx: &egui::Context) {
        let Some(PendingOp::Transfer(t)) = self.pending_op.take() else {
            return;
        };

        let total = transfer::total_bytes(&t.entries);
        let progress = Arc::new(Mutex::new(TransferProgress::new(total, t.entries.len())));
        self.active_transfer = Some(progress.clone());

        let spec = TransferSpec {
            kind: t.kind,
            entries: t.entries,
            target: t.target,
            conflicts: t.conflicts,
            policy: t.policy,
            method: t.method,
        };
        let ctx = ctx.clone();
        transfer::spawn_transfer(spec, progress, move || ctx.request_repaint());
    }

    /// Cancel active transfer.
    pub(crate) fn cancel_transfer(&mut self) {
        if let Some(ref state) = self.active_transfer {
            let mut s = state.lock().unwrap();
            s.cancelled = true;
        }
    }

    /// Auto-close finished transfers. A transfer that finished with errors
    /// stays open so the user can read the error list (dismissed via OK).
    pub(crate) fn poll_transfer(&mut self) {
        let close = self.active_transfer.as_ref().map(|s| {
            let s = s.lock().unwrap();
            s.cancelled || (s.finished && s.errors.is_empty())
        }).unwrap_or(false);

        if close {
            self.active_transfer = None;
            self.left.refresh();
            self.right.refresh();
        }
    }

    pub(crate) fn exec_delete(entries: &[crate::panel::FileEntry]) {
        for entry in entries {
            let _ = trash::delete(&entry.path);
        }
    }

    pub(crate) fn confirm_pending_op(&mut self, ctx: &egui::Context) {
        match &self.pending_op {
            Some(PendingOp::Delete { .. }) => {
                if let Some(PendingOp::Delete { entries, .. }) = self.pending_op.take() {
                    Self::exec_delete(&entries);
                    self.left.refresh();
                    self.right.refresh();
                }
            }
            Some(PendingOp::Transfer(_)) => {
                self.start_transfer(ctx);
            }
            None => {}
        }
    }

    pub(crate) fn set_pending_policy(&mut self, new_policy: OverwritePolicy) {
        if let Some(PendingOp::Transfer(t)) = &mut self.pending_op {
            t.policy = new_policy;
        }
    }

    pub(crate) fn create_dir(&mut self) {
        let base = self.active_panel().current_path.clone();
        let path = crate::fs_util::first_available(|i| {
            if i == 0 {
                base.join("New Folder")
            } else {
                base.join(format!("New Folder {}", i))
            }
        });
        let _ = std::fs::create_dir(&path);
        self.left.refresh();
        self.right.refresh();
    }
}
