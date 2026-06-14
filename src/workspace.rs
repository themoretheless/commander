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
    self, CopyMethod, OverwritePolicy, TransferKind, TransferProgress, TransferSpec, TransferState,
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
    /// Set by [`Command::BeginRename`]; the UI picks this up to open the
    /// inline rename editor seeded with this path, then clears it.
    pub rename_target: Option<PathBuf>,
    /// Set by [`Command::BeginSelectMask`]; the UI opens the mask input.
    pub mask_request: bool,
    /// Opens a file in an external application. Injected so tests don't
    /// launch real programs; the UI also routes double-clicks through it.
    pub opener: Box<dyn Fn(&Path)>,
}

/// How an entry relates to the same-named entry in the other panel.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CompareStatus {
    /// Same name, size and mtime as the other panel's entry.
    Identical,
    /// Same name but a different size or mtime.
    Differs,
    /// No entry of this name in the other panel.
    Unique,
}

/// Other-panel entries indexed by lowercase name → (size, mtime), for folder
/// comparison. Built once per frame from a panel's loaded entries.
pub type CompareMap = std::collections::HashMap<String, (u64, Option<std::time::SystemTime>)>;

/// Index a panel's entries for comparison against the other panel.
pub fn build_compare_map(entries: &[FileEntry]) -> CompareMap {
    entries
        .iter()
        .map(|e| (e.name_lower.clone(), (e.size, e.modified)))
        .collect()
}

/// Which entries to select when turning a folder comparison into a selection.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CompareCriterion {
    /// Present in the other panel but newer here (by mtime).
    Newer,
    /// Present in the other panel but differing in size or mtime.
    Differing,
    /// Absent from the other panel.
    Unique,
}

/// Collect the paths of `entries` matching `criterion` against the other
/// panel's [`CompareMap`]. Pure, so it can feed the selection set directly.
pub fn select_by_compare<'a>(
    entries: impl Iterator<Item = &'a FileEntry>,
    other: &CompareMap,
    criterion: CompareCriterion,
) -> std::collections::HashSet<PathBuf> {
    entries
        .filter(|e| match other.get(&e.name_lower) {
            None => criterion == CompareCriterion::Unique,
            Some(&(size, mtime)) => match criterion {
                CompareCriterion::Unique => false,
                CompareCriterion::Differing => size != e.size || mtime != e.modified,
                CompareCriterion::Newer => match (e.modified, mtime) {
                    (Some(a), Some(b)) => a > b,
                    _ => false,
                },
            },
        })
        .map(|e| e.path.clone())
        .collect()
}

/// Classify `entry` against the other panel's [`CompareMap`].
pub fn classify_entry(entry: &FileEntry, other: &CompareMap) -> CompareStatus {
    match other.get(&entry.name_lower) {
        None => CompareStatus::Unique,
        Some(&(size, mtime)) => {
            if size == entry.size && mtime == entry.modified {
                CompareStatus::Identical
            } else {
                CompareStatus::Differs
            }
        }
    }
}

/// Validate a proposed file name against its siblings (UI-independent so it
/// can drive live feedback while typing). `siblings` must exclude the entry
/// being renamed.
pub fn validate_new_name(name: &str, siblings: &[String]) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("Name cannot be empty".into());
    }
    if n.contains('/') {
        return Err("Name cannot contain '/'".into());
    }
    if n == "." || n == ".." {
        return Err("Invalid name".into());
    }
    if siblings.iter().any(|s| s == n) {
        return Err("Name already in use".into());
    }
    Ok(())
}

impl Workspace {
    pub fn new(left: PathBuf, right: PathBuf) -> Self {
        Self::with_opener(
            left,
            right,
            Box::new(|p| {
                let _ = open::that(p);
            }),
        )
    }

    pub fn with_opener(left: PathBuf, right: PathBuf, opener: Box<dyn Fn(&Path)>) -> Self {
        Workspace {
            left: PanelState::new(left),
            right: PanelState::new(right),
            active: ActivePanel::Left,
            pending_op: None,
            active_transfer: None,
            rename_target: None,
            mask_request: false,
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
                let max = panel.filtered_count();
                if panel.cursor < max {
                    panel.cursor += 1;
                    panel.scroll_to_cursor = true;
                }
            }
            Command::CursorHome => {
                let panel = self.active_panel();
                panel.cursor = 0;
                panel.scroll_to_cursor = true;
            }
            Command::CursorEnd => {
                let panel = self.active_panel();
                panel.cursor = panel.filtered_count();
                panel.scroll_to_cursor = true;
            }
            Command::CursorPageUp => {
                let panel = self.active_panel();
                let page = panel.page_rows.max(1);
                panel.cursor = panel.cursor.saturating_sub(page);
                panel.scroll_to_cursor = true;
            }
            Command::CursorPageDown => {
                let panel = self.active_panel();
                let page = panel.page_rows.max(1);
                let max = panel.filtered_count();
                panel.cursor = (panel.cursor + page).min(max);
                panel.scroll_to_cursor = true;
            }
            Command::ExtendSelectDown => {
                let panel = self.active_panel();
                panel.select_cursor();
                let max = panel.filtered_count();
                if panel.cursor < max {
                    panel.cursor += 1;
                }
                panel.select_cursor();
                panel.scroll_to_cursor = true;
            }
            Command::ExtendSelectUp => {
                let panel = self.active_panel();
                panel.select_cursor();
                if panel.cursor > 1 {
                    panel.cursor -= 1;
                }
                panel.select_cursor();
                panel.scroll_to_cursor = true;
            }
            Command::Activate => {
                // Cursor 0 is the ".." row, real files start at cursor 1.
                if self.active_panel_ref().cursor == 0 {
                    self.active_panel().go_up();
                } else if let Some(entry) = {
                    let panel = self.active_panel_ref();
                    panel.filtered_get(panel.cursor - 1).cloned()
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
                    let path = panel.filtered_get(panel.cursor - 1).map(|e| e.path.clone());
                    if let Some(path) = path {
                        panel.toggle_select(path);
                    }
                }
                let max = panel.filtered_count();
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
                            .filtered_get(panel.cursor.saturating_sub(1))
                            .and_then(panel::make_preview)
                    };
                    self.inactive_panel_mut().preview = preview;
                }
            }
            Command::RequestCopy => self.request_copy(),
            Command::RequestMove => self.request_move(),
            Command::CreateDir => self.create_dir(),
            Command::RequestDelete => self.request_delete(),
            Command::BeginRename => {
                let panel = self.active_panel_ref();
                if panel.cursor > 0 {
                    self.rename_target =
                        panel.filtered_get(panel.cursor - 1).map(|e| e.path.clone());
                }
            }
            Command::EqualizePanels => {
                let target = self.active_panel_ref().current_path.clone();
                self.inactive_panel_mut().navigate_to(target);
            }
            Command::SwapPanels => {
                std::mem::swap(&mut self.left, &mut self.right);
                self.active = match self.active {
                    ActivePanel::Left => ActivePanel::Right,
                    ActivePanel::Right => ActivePanel::Left,
                };
            }
            Command::BeginSelectMask => self.mask_request = true,
            Command::ToggleInfo => self.toggle_info(),
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
        // Only one transfer at a time: never replace a live transfer's handle
        // (that would orphan the running thread and its progress window).
        if self.active_transfer.is_some() {
            return;
        }
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

    /// Rename `old` to `new_name` in the same directory. Validates against the
    /// active panel's siblings; a no-op (unchanged name) succeeds silently.
    /// On success the panel is refreshed and the cursor follows the file by
    /// path. Returns a user-facing message on failure.
    pub fn commit_rename(&mut self, old: &Path, new_name: &str) -> Result<(), String> {
        let new_name = new_name.trim();
        let old_name = old
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if new_name == old_name {
            return Ok(()); // nothing to do
        }
        let siblings: Vec<String> = self
            .active_panel_ref()
            .entries
            .iter()
            .map(|e| e.name.clone())
            .filter(|n| n != &old_name)
            .collect();
        validate_new_name(new_name, &siblings)?;
        let dest = old
            .parent()
            .map(|p| p.join(new_name))
            .ok_or("Path has no parent")?;
        std::fs::rename(old, &dest).map_err(|e| e.to_string())?;
        self.active_panel().refresh();
        Ok(())
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

        let Some(entry) = source.filtered_get(source.cursor.saturating_sub(1)) else {
            return;
        };

        let shown = match &target.preview {
            Some(PreviewContent::Image(p)) => Some(p.as_path()),
            Some(PreviewContent::Text { path, .. }) => Some(path.as_path()),
            // The Get-Info card is a deliberate snapshot; don't auto-follow it.
            Some(PreviewContent::Info(_)) => return,
            None => None,
        };
        if shown == Some(entry.path.as_path()) {
            return;
        }
        target.preview = panel::make_preview(entry);
    }

    /// Toggle the Get-Info inspector for the cursor entry, shown in the
    /// opposite panel (like preview).
    pub fn toggle_info(&mut self) {
        if matches!(self.inactive_panel().preview, Some(PreviewContent::Info(_))) {
            self.inactive_panel_mut().preview = None;
            return;
        }
        let panel = self.active_panel_ref();
        if panel.cursor == 0 {
            return;
        }
        let Some(entry) = panel.filtered_get(panel.cursor - 1).cloned() else {
            return;
        };
        let dir_size = if entry.is_dir {
            panel
                .dir_sizes
                .lock()
                .ok()
                .and_then(|m| m.get(&entry.path).copied())
        } else {
            Some(entry.size)
        };
        let children = if entry.is_dir {
            panel
                .dir_counts
                .lock()
                .ok()
                .and_then(|m| m.get(&entry.path).copied())
        } else {
            None
        };
        let card = panel::make_info(&entry, dir_size, children);
        self.inactive_panel_mut().preview = Some(PreviewContent::Info(card));
    }

    // ── Drag and drop ───────────────────────────────────────────────────

    /// Handle a completed drag (mouse released). The dragged entries are
    /// routed through the same Move engine as F6 instead of a raw rename, so
    /// conflicts are confirmed, cross-volume moves work, self/descendant drops
    /// are rejected and errors surface. A clean, conflict-free drop runs
    /// immediately; a conflicting one opens the confirmation dialog.
    pub fn drop_dragged(&mut self, notify: impl Fn() + Send + 'static) {
        // Ignore drops while a transfer or another dialog is in flight, so we
        // never stack a second operation over the first.
        if self.active_transfer.is_some() || self.pending_op.is_some() {
            return;
        }
        let Some((paths, target)) = self.take_drop_plan() else {
            return;
        };
        let entries: Vec<FileEntry> = paths
            .iter()
            .filter_map(|p| {
                let meta = std::fs::metadata(p).ok()?;
                FileEntry::from_meta(p.clone(), &meta)
            })
            .collect();
        if entries.is_empty() {
            return;
        }
        let conflicts = scan::find_conflicts(&entries, &target);
        let flat = scan::spawn_scan(entries.clone());
        let has_conflicts = !conflicts.is_empty();
        self.pending_op = Some(PendingOp::Transfer(PendingTransfer {
            kind: TransferKind::Move,
            entries,
            target,
            conflicts,
            policy: OverwritePolicy::Ask,
            method: CopyMethod::Native,
            flat,
        }));
        // No conflicts: run the move straight away. Conflicts: leave the
        // pending op for the confirmation dialog to resolve.
        if !has_conflicts {
            self.start_transfer(notify);
        }
    }

    /// Resolve which panel is the drag source and where the drop lands,
    /// consuming the drag/drop state. A target hovered in the source panel
    /// itself (drag onto its own subdirectory) takes priority over the other
    /// panel; otherwise the other panel's current directory is the target.
    fn take_drop_plan(&mut self) -> Option<(Vec<PathBuf>, PathBuf)> {
        let (source, other) = if !self.left.drag_entries.is_empty() {
            (&mut self.left, &mut self.right)
        } else if !self.right.drag_entries.is_empty() {
            (&mut self.right, &mut self.left)
        } else {
            return None;
        };
        let target = source
            .drop_target
            .take()
            .or_else(|| other.drop_target.take())
            .unwrap_or_else(|| other.current_path.clone());
        let paths = std::mem::take(&mut source.drag_entries);
        source.drop_target = None;
        other.drop_target = None;
        if paths.is_empty() {
            return None;
        }
        Some((paths, target))
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
        let state = ws
            .active_transfer
            .clone()
            .expect("transfer should be running");
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
    fn equalize_points_inactive_panel_at_active_dir() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let mut ws = workspace(&l, &r);
        assert_ne!(ws.left.current_path, ws.right.current_path);

        // Active is Left; equalize sends Right to Left's directory.
        ws.execute(Command::EqualizePanels);
        assert_eq!(ws.right.current_path, l.path());
        assert_eq!(ws.left.current_path, l.path());
    }

    #[test]
    fn swap_exchanges_panels_and_keeps_focus_on_content() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("a.txt", "x");
        let mut ws = workspace(&l, &r);
        ws.left.cursor = 1;

        ws.execute(Command::SwapPanels);

        // Left's content (and cursor) is now on the right, and focus follows.
        assert_eq!(ws.right.current_path, l.path());
        assert_eq!(ws.left.current_path, r.path());
        assert_eq!(ws.right.cursor, 1);
        assert!(ws.active == ActivePanel::Right);
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
    fn home_end_and_page_navigation() {
        let (l, r) = (TempDir::new(), TempDir::new());
        for n in 0..20 {
            l.file(&format!("f{n:02}.txt"), "x");
        }
        let mut ws = workspace(&l, &r);
        ws.left.page_rows = 5;

        ws.execute(Command::CursorEnd);
        assert_eq!(ws.left.cursor, 20, "End jumps to the last row");

        ws.execute(Command::CursorHome);
        assert_eq!(ws.left.cursor, 0, "Home jumps to the top");

        ws.execute(Command::CursorPageDown);
        assert_eq!(ws.left.cursor, 5, "PageDown moves by one page");

        ws.execute(Command::CursorPageUp);
        assert_eq!(ws.left.cursor, 0, "PageUp moves back, clamped at 0");
    }

    #[test]
    fn shift_arrows_build_a_contiguous_selection() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("a.txt", "x");
        l.file("b.txt", "x");
        l.file("c.txt", "x");
        let mut ws = workspace(&l, &r);

        ws.left.cursor = 1; // a.txt
        ws.execute(Command::ExtendSelectDown); // select a, move to b, select b
        ws.execute(Command::ExtendSelectDown); // select b, move to c, select c

        assert_eq!(ws.left.cursor, 3);
        assert_eq!(ws.left.selected.len(), 3, "a, b and c are selected");
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
        assert!(
            !l.path().join("a.txt").exists(),
            "move must delete the source"
        );
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
    fn classify_entry_against_other_panel() {
        use std::time::{Duration, UNIX_EPOCH};
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        let t1 = UNIX_EPOCH + Duration::from_secs(2000);

        let (l, r) = (TempDir::new(), TempDir::new());
        let same = l.file("same.txt", "abc");
        let diff = l.file("diff.txt", "abc");
        let only = l.file("only.txt", "abc");
        let mk = |p: &std::path::Path, time| {
            let meta = std::fs::metadata(p).unwrap();
            let mut e = FileEntry::from_meta(p.to_path_buf(), &meta).unwrap();
            e.modified = Some(time);
            e
        };
        let _ = &r;

        // Other panel has "same" (identical) and "diff" (different size/mtime).
        let mut other = CompareMap::new();
        other.insert("same.txt".to_string(), (3, Some(t0)));
        other.insert("diff.txt".to_string(), (999, Some(t1)));

        assert_eq!(
            classify_entry(&mk(&same, t0), &other),
            CompareStatus::Identical
        );
        assert_eq!(
            classify_entry(&mk(&diff, t0), &other),
            CompareStatus::Differs
        );
        assert_eq!(
            classify_entry(&mk(&only, t0), &other),
            CompareStatus::Unique
        );
    }

    #[test]
    fn select_by_compare_picks_newer_differing_unique() {
        use std::time::{Duration, UNIX_EPOCH};
        let older = UNIX_EPOCH + Duration::from_secs(1000);
        let newer = UNIX_EPOCH + Duration::from_secs(2000);

        let tmp = TempDir::new();
        let mk = |name: &str, size: u64, time| {
            let p = tmp.file(name, "");
            let meta = std::fs::metadata(&p).unwrap();
            let mut e = FileEntry::from_meta(p, &meta).unwrap();
            e.size = size;
            e.modified = Some(time);
            e
        };
        let a = mk("a.txt", 10, newer); // exists in other, newer here
        let b = mk("b.txt", 99, older); // exists in other, differs (size)
        let c = mk("c.txt", 10, older); // unique here
        let entries = [a.clone(), b.clone(), c.clone()];

        let mut other = CompareMap::new();
        other.insert("a.txt".to_string(), (10, Some(older)));
        other.insert("b.txt".to_string(), (10, Some(older)));

        let newer_sel = select_by_compare(entries.iter(), &other, CompareCriterion::Newer);
        assert!(newer_sel.contains(&a.path) && newer_sel.len() == 1);

        let diff_sel = select_by_compare(entries.iter(), &other, CompareCriterion::Differing);
        assert!(diff_sel.contains(&b.path) && diff_sel.contains(&a.path) && diff_sel.len() == 2);

        let uniq_sel = select_by_compare(entries.iter(), &other, CompareCriterion::Unique);
        assert!(uniq_sel.contains(&c.path) && uniq_sel.len() == 1);
    }

    #[test]
    fn build_compare_map_indexes_by_lowercase_name() {
        let tmp = TempDir::new();
        let f = tmp.file("Photo.JPG", "xy");
        let meta = std::fs::metadata(&f).unwrap();
        let e = FileEntry::from_meta(f, &meta).unwrap();
        let map = build_compare_map(std::slice::from_ref(&e));
        assert!(map.contains_key("photo.jpg"));
        assert_eq!(map["photo.jpg"].0, 2);
    }

    #[test]
    fn validate_new_name_rules() {
        let siblings = vec!["taken.txt".to_string()];
        assert!(validate_new_name("fresh.txt", &siblings).is_ok());
        assert!(validate_new_name("  ", &siblings).is_err());
        assert!(validate_new_name("a/b", &siblings).is_err());
        assert!(validate_new_name("..", &siblings).is_err());
        assert!(validate_new_name("taken.txt", &siblings).is_err());
    }

    #[test]
    fn begin_rename_targets_the_cursor_entry() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let f = l.file("a.txt", "x");
        let mut ws = workspace(&l, &r);
        ws.left.cursor = 1;

        ws.execute(Command::BeginRename);
        assert_eq!(ws.rename_target.as_deref(), Some(f.as_path()));
    }

    #[test]
    fn commit_rename_moves_the_file_and_follows_cursor() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let f = l.file("old.txt", "data");
        let mut ws = workspace(&l, &r);
        ws.left.cursor = 1;

        ws.commit_rename(&f, "new.txt").unwrap();

        assert!(!f.exists());
        let renamed = l.path().join("new.txt");
        assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "data");
        // Cursor follows the renamed file by path.
        assert_eq!(
            ws.left.filtered_get(ws.left.cursor - 1).unwrap().name,
            "new.txt"
        );
    }

    #[test]
    fn commit_rename_rejects_a_collision_without_touching_disk() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let f = l.file("a.txt", "A");
        l.file("b.txt", "B");
        let mut ws = workspace(&l, &r);

        let err = ws.commit_rename(&f, "b.txt");
        assert!(err.is_err());
        assert!(f.exists(), "source untouched on collision");
        assert_eq!(
            std::fs::read_to_string(l.path().join("b.txt")).unwrap(),
            "B"
        );
    }

    #[test]
    fn commit_rename_noop_on_unchanged_name() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let f = l.file("a.txt", "A");
        let mut ws = workspace(&l, &r);
        assert!(ws.commit_rename(&f, "a.txt").is_ok());
        assert!(f.exists());
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
        ws.drop_dragged(|| {});
        wait_transfer(&mut ws);

        assert!(
            sub.join("a.txt").exists(),
            "file lands in the hovered subdir"
        );
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
        ws.drop_dragged(|| {});
        wait_transfer(&mut ws);

        assert!(r.path().join("a.txt").exists());
        assert!(!l.path().join("a.txt").exists(), "drop is a move");
    }

    #[test]
    fn drop_with_conflict_opens_dialog_instead_of_moving() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let file = l.file("a.txt", "new");
        r.file("a.txt", "old");
        let mut ws = workspace(&l, &r);

        ws.left.drag_entries = vec![file];
        ws.drop_dragged(|| {});

        // A conflicting drop must NOT move immediately; it stages a
        // confirmation instead, leaving both sides intact.
        assert!(ws.active_transfer.is_none());
        assert!(matches!(ws.pending_op, Some(PendingOp::Transfer(_))));
        assert!(l.path().join("a.txt").exists());
        assert_eq!(
            std::fs::read_to_string(r.path().join("a.txt")).unwrap(),
            "old"
        );
    }
}
