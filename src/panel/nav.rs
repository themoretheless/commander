use std::path::{Path, PathBuf};

use super::PanelState;
use super::listing_job::PendingFocus;
use super::visit::record_visit;

impl PanelState {
    pub fn navigate_to(&mut self, path: PathBuf) {
        // Remember the outgoing directory's view before leaving it, then
        // restore the incoming one's if we've seen it before this session.
        self.stash_view_settings();
        // The jump trail truncates any forward tail and collapses a repeat of
        // the current directory, so every navigation entry point records here.
        self.history.push(path.clone());
        // A stale bookmark/omnibar path that no longer exists is not a visit
        // worth offering again in Recent; Back prunes it from the trail.
        if !is_missing_directory(&path) {
            record_visit(&path);
        }
        self.load_remembered_path(path);
    }

    pub fn go_up(&mut self) {
        // Remember the directory we are leaving so the cursor can land on it
        // in the parent (classic dual-pane behaviour).
        let child = self
            .current_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string());
        if let Some(parent) = self.current_path.parent().map(|p| p.to_path_buf()) {
            self.navigate_to(parent);
            if let Some(name) = child {
                // Prefer the child-name landing over any remembered parent focus.
                if self.can_async_list() && self.listing_job.is_awaiting() {
                    self.listing_job.set_focus(PendingFocus::NamedChild(name));
                } else if let Some(idx) = self.filtered_position(|e| e.name == name) {
                    self.set_cursor(idx + 1);
                    self.set_scroll_to_cursor(true);
                }
            }
        }
    }

    pub const fn can_go_back(&self) -> bool {
        self.history.can_back()
    }

    pub const fn can_go_forward(&self) -> bool {
        self.history.can_forward()
    }

    pub fn go_back(&mut self) {
        // Walk the existing trail without recording a new jump. Directories
        // can disappear after being visited, so prune dead entries on sight.
        self.stash_view_settings();
        // Pruning can leave a repeat of the current directory adjacent to it
        // (`[A, B, A]` with `B` deleted); skip it so Back/Forward moves.
        let current = self.current_path.clone();
        if let Some(path) = self
            .history
            .back_pruning(|p| p != current && p.is_dir())
            .map(|p| p.to_path_buf())
        {
            record_visit(&path);
            self.load_remembered_path(path);
        }
    }

    pub fn go_forward(&mut self) {
        self.stash_view_settings();
        // Pruning can leave a repeat of the current directory adjacent to it
        // (`[A, B, A]` with `B` deleted); skip it so Back/Forward moves.
        let current = self.current_path.clone();
        if let Some(path) = self
            .history
            .forward_pruning(|p| p != current && p.is_dir())
            .map(|p| p.to_path_buf())
        {
            record_visit(&path);
            self.load_remembered_path(path);
        }
    }
}

/// Whether `path` is known not to be a directory: it is gone or names a
/// non-directory. Unreadable (denied) locations still count as visits.
fn is_missing_directory(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => !meta.is_dir(),
        Err(err) => err.kind() == std::io::ErrorKind::NotFound,
    }
}
