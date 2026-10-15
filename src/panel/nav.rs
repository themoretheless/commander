use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
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
        } else if let Some(idx) = self.named_child_position(&name) {
            self.set_cursor(idx + 1);
            self.set_scroll_to_cursor(true);
        }
    }

    /// Filtered position of the child called `name`. An exact match wins;
    /// otherwise a case-insensitive one, so a path typed as `…/downloads` on
    /// a case-insensitive volume still lands on `Downloads` after Go Up.
    pub(super) fn named_child_position(&self, name: &str) -> Option<usize> {
        self.filtered_position(|e| e.name == name).or_else(|| {
            let lower = name.to_lowercase();
            self.filtered_position(|e| e.name_lower == lower)
        })
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
/// Hung history probes remembered at once; past this, entries are assumed
/// alive without probing.
const MAX_HUNG_HISTORY_PROBES: usize = 64;

/// Whether a history entry is still a directory worth stepping to. The stat
/// runs on a helper thread so an entry on a hung network mount cannot freeze
/// the UI: one that has not answered in time counts as alive, and the async
/// listing then reports it gone or denied like any other navigation.
fn history_dir_alive(path: &Path) -> bool {
    probe_bounded(path, HISTORY_PROBE_TIMEOUT, Path::is_dir)
}

/// Paths with a history probe still running. A path stays here until its
/// probe thread returns, so pressing Back/Forward again over an entry on a
/// hung mount answers at once instead of leaking another thread.
fn probes_in_flight() -> &'static Mutex<HashSet<PathBuf>> {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(Mutex::default)
}

fn probe_bounded(
    path: &Path,
    timeout: Duration,
    probe: impl FnOnce(&Path) -> bool + Send + 'static,
) -> bool {
    {
        let mut in_flight = crate::lock_util::recover(probes_in_flight());
        if in_flight.contains(path) || in_flight.len() >= MAX_HUNG_HISTORY_PROBES {
            return true;
        }
        in_flight.insert(path.to_path_buf());
    }
    // Unmarks the path when the probe closure is dropped: after the probe
    // returns on its thread, or at once if no thread could be spawned.
    struct InFlight(PathBuf);
    impl Drop for InFlight {
        fn drop(&mut self) {
            crate::lock_util::recover(probes_in_flight()).remove(&self.0);
        }
    }
    let marker = InFlight(path.to_path_buf());
    // Timed out (or unspawnable): counts as alive. A hung path stays marked
    // until its thread returns, so it is not probed again meanwhile.
    crate::mount_guard::run_bounded(move || probe(&marker.0), timeout).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_probe_answers_for_live_and_missing_dirs() {
        let tmp = crate::testutil::TempDir::new();
        let file = tmp.file("plain.txt", "x");
        let timeout = Duration::from_secs(10);
        assert!(probe_bounded(tmp.path(), timeout, Path::is_dir));
        assert!(!probe_bounded(
            &tmp.path().join("missing"),
            timeout,
            Path::is_dir
        ));
        assert!(!probe_bounded(&file, timeout, Path::is_dir));
    }

    #[test]
    fn a_hung_history_probe_answers_alive_and_is_not_respawned() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let path = PathBuf::from("/hung-history-probe-test/entry");
        let calls = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicBool::new(false));
        let hanging_probe = || {
            let calls = Arc::clone(&calls);
            let released = Arc::clone(&released);
            move |_: &Path| {
                calls.fetch_add(1, Ordering::SeqCst);
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                while !released.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(2));
                }
                false
            }
        };
        let short = Duration::from_millis(20);

        assert!(
            probe_bounded(&path, short, hanging_probe()),
            "timeout = alive"
        );
        // While the first probe hangs, presses answer at once without
        // spawning another thread.
        assert!(probe_bounded(&path, short, hanging_probe()));
        assert!(probe_bounded(&path, short, hanging_probe()));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Once the mount answers, the path is probed normally again.
        released.store(true, Ordering::SeqCst);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while crate::lock_util::recover(probes_in_flight()).contains(&path) {
            assert!(std::time::Instant::now() < deadline, "probe never returned");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!probe_bounded(
            &path,
            Duration::from_secs(10),
            hanging_probe()
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
