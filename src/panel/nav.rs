use std::path::{Path, PathBuf};
use std::time::Duration;

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
        // The visit is recorded once the listing resolves (off the UI thread
        // for async listings): a stale bookmark/omnibar path that turns out
        // to be gone is not offered again in Recent; Back prunes it.
        self.pending_visit = Some(path.clone());
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
                self.focus_child(name);
            }
        }
    }

    /// Put the cursor on the entry called `name` right after a navigation,
    /// preferring it over any remembered focus. While the listing is still
    /// loading in the background, the landing is deferred until it publishes.
    pub fn focus_child(&mut self, name: String) {
        if self.can_async_list() && self.listing_job.is_awaiting() {
            self.listing_job.set_focus(PendingFocus::NamedChild(name));
        } else if let Some(idx) = self.filtered_position(|e| e.name == name) {
            self.set_cursor(idx + 1);
            self.set_scroll_to_cursor(true);
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
            .back_pruning(|p| p != current && history_dir_alive(p))
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
            .forward_pruning(|p| p != current && history_dir_alive(p))
            .map(|p| p.to_path_buf())
        {
            record_visit(&path);
            self.load_remembered_path(path);
        }
    }
}

/// How long Back/Forward waits on one history entry's existence check.
const HISTORY_PROBE_TIMEOUT: Duration = Duration::from_millis(200);

/// Whether a history entry is still a directory worth stepping to. The stat
/// runs on a helper thread so an entry on a hung network mount cannot freeze
/// the UI: one that has not answered in time counts as alive, and the async
/// listing then reports it gone or denied like any other navigation. A hung
/// probe leaks its thread until the mount answers.
fn history_dir_alive(path: &Path) -> bool {
    probe_bounded(path, HISTORY_PROBE_TIMEOUT)
}

fn probe_bounded(path: &Path, timeout: Duration) -> bool {
    let (sender, receiver) = std::sync::mpsc::channel();
    let probe_path = path.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("commander-history-probe".to_string())
        .spawn(move || {
            let _ = sender.send(probe_path.is_dir());
        });
    if spawned.is_err() {
        return path.is_dir();
    }
    receiver.recv_timeout(timeout).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_probe_answers_for_live_and_missing_dirs() {
        let tmp = crate::testutil::TempDir::new();
        let file = tmp.file("plain.txt", "x");
        let timeout = Duration::from_secs(10);
        assert!(probe_bounded(tmp.path(), timeout));
        assert!(!probe_bounded(&tmp.path().join("missing"), timeout));
        assert!(!probe_bounded(&file, timeout));
    }
}
