//! UI-agnostic application core: two panels, the active-panel marker,
//! pending operations and the running transfer. All file-manager behaviour
//! lives here so it can be driven and tested without a GUI. The UI layer
//! only renders this state and forwards [`Command`]s / notify callbacks.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::command::Command;
use crate::panel::{self, FileEntry, PanelState, PreviewContent};
use crate::scan::{self, FlatList};
use crate::transfer::{
    self, CopyMethod, OverwritePolicy, TransferKind, TransferProgress, TransferSpec,
    TransferState,
};

#[derive(PartialEq, Clone, Copy)]
pub enum ActivePanel {
    Left,
    Right,
}

/// A copy/move awaiting user confirmation in the dialog.
pub struct PendingTransfer {
    pub kind: TransferKind,
    pub entries: Vec<FileEntry>,
    pub target: PathBuf,
    pub conflicts: Vec<String>,
    pub policy: OverwritePolicy,
    pub method: CopyMethod,
    pub flat: FlatList,
}

/// Pending file operation awaiting user confirmation.
pub enum PendingOp {
    Transfer(PendingTransfer),
    Delete {
        entries: Vec<FileEntry>,
        flat: FlatList,
    },
}

pub struct Workspace {
    pub left: PanelState,
    pub right: PanelState,
    pub active: ActivePanel,
    pub pending_op: Option<PendingOp>,
    pub active_transfer: Option<TransferState>,
    /// Opens a file in an external application. Injected so tests don't
    /// launch real programs; the UI also routes double-clicks through it.
    pub opener: Box<dyn Fn(&Path)>,
}

impl Workspace {
    pub fn new(left: PathBuf, right: PathBuf) -> Self {
        Self::with_opener(left, right, Box::new(|p| {
            let _ = open::that(p);
        }))
    }

    pub fn with_opener(left: PathBuf, right: PathBuf, opener: Box<dyn Fn(&Path)>) -> Self {
        Workspace {
            left: PanelState::new(left),
            right: PanelState::new(right),
            active: ActivePanel::Left,
            pending_op: None,
            active_transfer: None,
            opener,
        }
    }

    pub fn active_panel(&mut self) -> &mut PanelState {
        match self.active {
            ActivePanel::Left => &mut self.left,
            ActivePanel::Right => &mut self.right,
        }
    }

    pub fn active_panel_ref(&self) -> &PanelState {
        match self.active {
            ActivePanel::Left => &self.left,
            ActivePanel::Right => &self.right,
        }
    }

    pub fn inactive_panel(&self) -> &PanelState {
        match self.active {
            ActivePanel::Left => &self.right,
            ActivePanel::Right => &self.left,
        }
    }

    pub fn inactive_panel_mut(&mut self) -> &mut PanelState {
        match self.active {
            ActivePanel::Left => &mut self.right,
            ActivePanel::Right => &mut self.left,
        }
    }

    // ── Command dispatch ────────────────────────────────────────────────

    pub fn execute(&mut self, cmd: Command) {
        match cmd {
            Command::SwitchPanel => {
                self.active = match self.active {
                    ActivePanel::Left => ActivePanel::Right,
                    ActivePanel::Right => ActivePanel::Left,
                };
            }
            Command::CursorUp => {
                let panel = self.active_panel();
                if panel.cursor > 0 {
                    panel.cursor -= 1;
                    panel.scroll_to_cursor = true;
                }
            }
            Command::CursorDown => {
                let panel = self.active_panel();
                let max = panel.filtered_entries().len();
                if panel.cursor < max {
                    panel.cursor += 1;
                    panel.scroll_to_cursor = true;
                }
            }
            Command::Activate => {
                // Cursor 0 is the ".." row, real files start at cursor 1.
                if self.active_panel_ref().cursor == 0 {
                    self.active_panel().go_up();
                } else if let Some(entry) = {
                    let panel = self.active_panel_ref();
                    panel.filtered_entries().get(panel.cursor - 1).cloned().cloned()
                } {
                    if entry.is_dir {
                        self.active_panel().navigate_to(entry.path);
                    } else {
                        (self.opener)(&entry.path);
                    }
                }
            }
            Command::GoUp => {
                self.active_panel().go_up();
            }
            Command::ToggleSelect => {
                let panel = self.active_panel();
                if panel.cursor > 0 {
                    let path = panel
                        .filtered_entries()
                        .get(panel.cursor - 1)
                        .map(|e| e.path.clone());
                    if let Some(path) = path {
                        panel.toggle_select(path);
                    }
                }
                let max = panel.filtered_entries().len();
                if panel.cursor < max {
                    panel.cursor += 1;
                }
            }
            Command::TogglePreview => {
                if self.inactive_panel().preview.is_some() {
                    self.inactive_panel_mut().preview = None;
                } else {
                    let preview = {
                        let panel = self.active_panel_ref();
                        panel
                            .filtered_entries()
                            .get(panel.cursor.saturating_sub(1))
                            .and_then(|e| panel::make_preview(e))
                    };
                    self.inactive_panel_mut().preview = preview;
                }
            }
            Command::RequestCopy => self.request_copy(),
            Command::RequestMove => self.request_move(),
            Command::CreateDir => self.create_dir(),
            Command::RequestDelete => self.request_delete(),
            Command::SelectAll => self.active_panel().select_all(),
            Command::ToggleHidden => {
                let panel = self.active_panel();
                panel.show_hidden = !panel.show_hidden;
                panel.refresh();
            }
        }
    }

    // ── File operations ─────────────────────────────────────────────────

    pub fn request_copy(&mut self) {
        self.request_transfer(TransferKind::Copy);
    }

    pub fn request_move(&mut self) {
        self.request_transfer(TransferKind::Move);
    }

    fn request_transfer(&mut self, kind: TransferKind) {
        let target = self.inactive_panel().current_path.clone();
        let entries = self.active_panel_ref().selected_or_cursor();
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

    pub fn request_delete(&mut self) {
        let entries = self.active_panel_ref().selected_or_cursor();
        if !entries.is_empty() {
            let flat = scan::spawn_scan(entries.clone());
            self.pending_op = Some(PendingOp::Delete { entries, flat });
        }
    }

    /// Start background copy/move with progress tracking.
    /// `notify` is invoked when visible progress changes (UI passes a
    /// repaint request).
    pub fn start_transfer(&mut self, notify: impl Fn() + Send + 'static) {
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
        transfer::spawn_transfer(spec, progress, notify);
    }

    /// Cancel active transfer.
    pub fn cancel_transfer(&mut self) {
        if let Some(ref state) = self.active_transfer {
            let mut s = state.lock().unwrap();
            s.cancelled = true;
        }
    }

    /// Auto-close finished transfers. A transfer that finished with errors
    /// stays open so the user can read the error list (dismissed via OK).
    pub fn poll_transfer(&mut self) {
        let close = self
            .active_transfer
            .as_ref()
            .map(|s| {
                let s = s.lock().unwrap();
                s.cancelled || (s.finished && s.errors.is_empty())
            })
            .unwrap_or(false);

        if close {
            self.active_transfer = None;
            self.left.refresh();
            self.right.refresh();
        }
    }

    pub fn exec_delete(entries: &[FileEntry]) {
        for entry in entries {
            let _ = trash::delete(&entry.path);
        }
    }

    pub fn confirm_pending_op(&mut self, notify: impl Fn() + Send + 'static) {
        match &self.pending_op {
            Some(PendingOp::Delete { .. }) => {
                if let Some(PendingOp::Delete { entries, .. }) = self.pending_op.take() {
                    Self::exec_delete(&entries);
                    self.left.refresh();
                    self.right.refresh();
                }
            }
            Some(PendingOp::Transfer(_)) => {
                self.start_transfer(notify);
            }
            None => {}
        }
    }

    pub fn set_pending_policy(&mut self, new_policy: OverwritePolicy) {
        if let Some(PendingOp::Transfer(t)) = &mut self.pending_op {
            t.policy = new_policy;
        }
    }

    pub fn create_dir(&mut self) {
        let base = self.active_panel_ref().current_path.clone();
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

    // ── Preview ─────────────────────────────────────────────────────────

    /// Keep the preview (shown in the opposite panel) in sync with the
    /// cursor. The preview is cached by path: while the cursor stays on
    /// the same file nothing touches the filesystem.
    pub fn sync_preview(&mut self) {
        let (source, target) = if self.right.preview.is_some() {
            (&self.left, &mut self.right)
        } else if self.left.preview.is_some() {
            (&self.right, &mut self.left)
        } else {
            return;
        };

        let entries = source.filtered_entries();
        let Some(entry) = entries.get(source.cursor.saturating_sub(1)) else {
            return;
        };

        let shown = match &target.preview {
            Some(PreviewContent::Image(p)) => Some(p.as_path()),
            Some(PreviewContent::Text { path, .. }) => Some(path.as_path()),
            None => None,
        };
        if shown == Some(entry.path.as_path()) {
            return;
        }
        target.preview = panel::make_preview(entry);
    }

    // ── Drag and drop ───────────────────────────────────────────────────

    /// Move dragged entries into the hovered directory (mouse released).
    pub fn drop_dragged(&mut self) {
        Self::drop_into(&mut self.left, &mut self.right);
        Self::drop_into(&mut self.right, &mut self.left);
        self.left.drop_target = None;
        self.right.drop_target = None;
    }

    /// Drop `source`'s dragged entries. A target hovered in the source panel
    /// itself (drag onto own subdirectory) takes priority over the other panel.
    fn drop_into(source: &mut PanelState, other: &mut PanelState) {
        if source.drag_entries.is_empty() {
            return;
        }
        let target = source
            .drop_target
            .take()
            .or_else(|| other.drop_target.take())
            .unwrap_or_else(|| other.current_path.clone());
        for src in &source.drag_entries {
            if let Some(name) = src.file_name() {
                let _ = std::fs::rename(src, target.join(name));
            }
        }
        source.drag_entries.clear();
        source.refresh();
        other.refresh();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn workspace(left: &TempDir, right: &TempDir) -> Workspace {
        let mut ws = Workspace::with_opener(
            left.path().to_path_buf(),
            right.path().to_path_buf(),
            Box::new(|_| {}),
        );
        ws.left.refresh();
        ws.right.refresh();
        ws
    }

    fn wait_transfer(ws: &mut Workspace) {
        let state = ws.active_transfer.clone().expect("transfer should be running");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !state.lock().unwrap().finished {
            assert!(std::time::Instant::now() < deadline, "transfer timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        ws.poll_transfer();
    }

    #[test]
    fn switch_panel_toggles_active() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let mut ws = workspace(&l, &r);
        assert!(ws.active == ActivePanel::Left);
        ws.execute(Command::SwitchPanel);
        assert!(ws.active == ActivePanel::Right);
        ws.execute(Command::SwitchPanel);
        assert!(ws.active == ActivePanel::Left);
    }

    #[test]
    fn cursor_moves_are_clamped() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("a.txt", "x");
        l.file("b.txt", "x");
        let mut ws = workspace(&l, &r);

        ws.execute(Command::CursorUp);
        assert_eq!(ws.left.cursor, 0, "cursor must not go below 0");

        for _ in 0..10 {
            ws.execute(Command::CursorDown);
        }
        assert_eq!(ws.left.cursor, 2, "cursor must stop at the last entry");
    }

    #[test]
    fn activate_dir_navigates_into_it() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let sub = l.dir("sub");
        l.file("sub/inner.txt", "x");
        let mut ws = workspace(&l, &r);

        ws.left.cursor = 1; // dirs sort first, so "sub" is the first row
        ws.execute(Command::Activate);
        assert_eq!(ws.left.current_path, sub);
        assert_eq!(ws.left.entries.len(), 1);
    }

    #[test]
    fn activate_file_calls_opener() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("a.txt", "x");
        let opened = Arc::new(AtomicUsize::new(0));
        let opened2 = opened.clone();
        let mut ws = Workspace::with_opener(
            l.path().to_path_buf(),
            r.path().to_path_buf(),
            Box::new(move |_| {
                opened2.fetch_add(1, Ordering::Relaxed);
            }),
        );
        ws.left.refresh();

        ws.left.cursor = 1;
        ws.execute(Command::Activate);
        assert_eq!(opened.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn copy_flow_end_to_end() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("a.txt", "hello");
        let mut ws = workspace(&l, &r);

        ws.left.cursor = 1;
        ws.execute(Command::RequestCopy);
        assert!(matches!(ws.pending_op, Some(PendingOp::Transfer(_))));

        ws.confirm_pending_op(|| {});
        wait_transfer(&mut ws);

        assert!(ws.active_transfer.is_none(), "clean transfer auto-closes");
        let copied = std::fs::read_to_string(r.path().join("a.txt")).unwrap();
        assert_eq!(copied, "hello");
        assert!(l.path().join("a.txt").exists(), "copy must keep the source");
    }

    #[test]
    fn move_flow_removes_source() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("a.txt", "hello");
        let mut ws = workspace(&l, &r);

        ws.left.cursor = 1;
        ws.execute(Command::RequestMove);
        ws.confirm_pending_op(|| {});
        wait_transfer(&mut ws);

        assert!(r.path().join("a.txt").exists());
        assert!(!l.path().join("a.txt").exists(), "move must delete the source");
    }

    #[test]
    fn request_delete_builds_pending_op() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("a.txt", "x");
        let mut ws = workspace(&l, &r);

        ws.left.cursor = 1;
        ws.execute(Command::RequestDelete);
        match &ws.pending_op {
            Some(PendingOp::Delete { entries, .. }) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].name, "a.txt");
            }
            _ => panic!("expected a pending delete"),
        }
    }

    #[test]
    fn create_dir_picks_first_free_name() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let mut ws = workspace(&l, &r);

        ws.create_dir();
        assert!(l.path().join("New Folder").is_dir());
        ws.create_dir();
        assert!(l.path().join("New Folder 1").is_dir());
    }

    #[test]
    fn drop_prefers_source_panel_target() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let file = l.file("a.txt", "x");
        let sub = l.dir("sub");
        let mut ws = workspace(&l, &r);

        // Dragging within the left panel onto its own subdirectory:
        // the right panel must not steal the drop.
        ws.left.drag_entries = vec![file.clone()];
        ws.left.drop_target = Some(sub.clone());
        ws.drop_dragged();

        assert!(sub.join("a.txt").exists(), "file lands in the hovered subdir");
        assert!(!r.path().join("a.txt").exists());
        assert!(ws.left.drag_entries.is_empty());
    }

    #[test]
    fn preview_follows_cursor_and_is_cached_by_path() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("a.txt", "alpha");
        l.file("b.txt", "beta");
        let mut ws = workspace(&l, &r);

        ws.left.cursor = 1; // a.txt
        ws.execute(Command::TogglePreview);
        match &ws.right.preview {
            Some(PreviewContent::Text { content, .. }) => assert_eq!(content, "alpha"),
            _ => panic!("expected text preview for a.txt"),
        }

        // Preview follows the cursor.
        ws.left.cursor = 2; // b.txt
        ws.sync_preview();
        match &ws.right.preview {
            Some(PreviewContent::Text { content, .. }) => assert_eq!(content, "beta"),
            _ => panic!("expected text preview for b.txt"),
        }

        // Cached by path: the file is gone, but the cursor didn't move,
        // so sync must NOT re-read the filesystem (a re-read would drop
        // the preview).
        std::fs::remove_file(l.path().join("b.txt")).unwrap();
        ws.sync_preview();
        match &ws.right.preview {
            Some(PreviewContent::Text { content, .. }) => assert_eq!(content, "beta"),
            _ => panic!("preview must survive while the cursor is unchanged"),
        }
    }

    #[test]
    fn drop_falls_back_to_other_panel_path() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let file = l.file("a.txt", "x");
        let mut ws = workspace(&l, &r);

        ws.left.drag_entries = vec![file];
        ws.drop_dragged();

        assert!(r.path().join("a.txt").exists());
    }
}
