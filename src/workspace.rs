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
    /// Bytes the operation needs (recursive total of the entries).
    pub need_bytes: u64,
    /// Free bytes on the target volume (None if it could not be read).
    pub free_bytes: Option<u64>,
    /// Source and target are on the same volume (a move is then instant).
    pub same_volume: bool,
}

impl PendingTransfer {
    /// Classify how this operation consumes target space: a same-volume move is
    /// an instant rename, a same-volume Native copy is an APFS clone (both need
    /// ~0), while a cross-volume transfer or a buffered same-volume copy writes
    /// the full size.
    fn op_class(&self) -> crate::fs_util::OpClass {
        use crate::fs_util::OpClass;
        match self.kind {
            TransferKind::Move => OpClass::Move {
                same_volume: self.same_volume,
            },
            TransferKind::Copy => {
                if self.same_volume && self.method == CopyMethod::Native {
                    OpClass::Clone
                } else {
                    OpClass::Copy
                }
            }
        }
    }

    /// Space verdict driving the will-it-fit guard. (No overwrite reclaim or
    /// safety reserve is applied yet; both are supported by the pure core.)
    pub fn space_verdict(&self) -> crate::fs_util::SpaceVerdict {
        crate::fs_util::space_verdict(self.need_bytes, self.free_bytes, self.op_class(), 0, 0)
    }

    /// True when the operation needs no meaningful extra space on the target
    /// (a same-volume move, or a same-volume clone).
    pub fn needs_no_space(&self) -> bool {
        matches!(
            self.op_class(),
            crate::fs_util::OpClass::Move { same_volume: true } | crate::fs_util::OpClass::Clone
        )
    }

    /// True when the operation cannot fit on the target volume.
    pub fn overflows(&self) -> bool {
        matches!(
            self.space_verdict(),
            crate::fs_util::SpaceVerdict::WontFit { .. }
        )
    }
}

/// One transfer waiting in (or running from) the queue: the fully-built spec
/// plus the undo action to record if it finishes cleanly (a user Move) or
/// `None` for copies and undo/redo-driven transfers.
pub struct QueuedJob {
    spec: TransferSpec,
    undo: Option<crate::undo::Action>,
}

/// Pending file operation awaiting user confirmation.
pub enum PendingOp {
    Transfer(PendingTransfer),
    Delete {
        entries: Vec<FileEntry>,
        flat: FlatList,
    },
}

/// How a delete-to-Trash turned out, so the UI can confirm it and flag any
/// entries that could not be removed instead of failing silently.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct DeleteOutcome {
    pub trashed: usize,
    pub failed: usize,
}

/// How a shelf drain turned out: how many copies started, and how many items
/// could not be read and so were kept on the shelf rather than discarded.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct ShelfDrainOutcome {
    pub started: usize,
    pub unavailable: usize,
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
    /// Set by [`Command::BeginRunBar`]; the UI opens the run-command bar.
    pub run_command_request: bool,
    /// Set by [`Command::BeginGoToPath`]; the UI opens the path input.
    pub path_request: bool,
    /// Set by [`Command::BeginRecent`]; the UI opens the recent switcher.
    pub recent_request: bool,
    /// Set by [`Command::Undo`]; the UI runs the undo with a notify callback.
    pub undo_request: bool,
    /// Set by [`Command::BeginPalette`]; the UI opens the command palette.
    pub palette_request: bool,
    /// Set by [`Command::BeginBatchRename`]; the UI opens the batch-rename
    /// studio for the active panel's selection.
    pub batch_rename_request: bool,
    /// Set by [`Command::BeginSync`]; the UI opens the synchronise sheet.
    pub sync_request: bool,
    /// Set by [`Command::FindDuplicates`]; the UI opens the duplicates sheet.
    pub duplicates_request: bool,
    /// Set by [`Command::DiffFiles`]; the UI opens the diff sheet.
    pub diff_request: bool,
    /// Set by [`Command::DiskTreemap`]; the UI opens the treemap sheet.
    pub treemap_request: bool,
    /// Set by [`Command::BeginFind`]; the UI opens the recursive find sheet.
    pub find_request: bool,
    /// Set by [`Command::OpenSavedSearch`]; the UI opens the smart-folder picker.
    pub saved_search_request: bool,
    /// Set by the Copy* commands; the UI formats the selection and copies it.
    pub clipboard_request: Option<crate::clipboard::PathStyle>,
    /// Set by [`Command::Redo`]; the UI replays the next redoable action.
    pub redo_request: bool,
    /// Set by [`Command::ShelfDrain`]; the UI drains the shelf with a notify.
    pub drain_request: bool,
    /// Set by [`Command::CycleDensity`]; the UI cycles its list density.
    pub cycle_density_request: bool,
    /// The drop stack: paths gathered across folders to copy in one go.
    pub shelf: crate::shelf::Shelf,
    /// Persisted directory bookmarks (favorites + quick-jump slots 1..9).
    pub bookmarks: crate::bookmarks::Bookmarks,
    /// A stashed selection for set-algebra combinations (union/intersect/...).
    pub selection_stash: std::collections::HashSet<PathBuf>,
    /// Pending transfer pipeline. Every copy/move enqueues here; the worker
    /// runs one job at a time (concurrency cap 1 for now) and `poll_transfer`
    /// drains the next when the active one finishes. Replaces the old ad-hoc
    /// single-slot sync follow-up.
    queue: crate::opqueue::Queue<QueuedJob>,
    /// The queue job currently spawned as `active_transfer`, so it can be
    /// marked done/failed when the worker finishes.
    running_job: Option<crate::opqueue::JobId>,
    /// Undo/redo history of reversible operations (moves, batch renames).
    pub stack: crate::undo::UndoStack,
    /// The action the in-flight transfer will record on a clean finish (a user
    /// Move). `None` for copies and for undo/redo-driven transfers, which must
    /// not record fresh history.
    pending_undo_action: Option<crate::undo::Action>,
    /// Opens a file in an external application. Injected so tests don't
    /// launch real programs; the UI also routes double-clicks through it.
    pub opener: Box<dyn Fn(&Path)>,
}

/// `(from, to)` pairs for a Move: each entry goes from its current path to
/// `target/name`. Recorded as an [`undo::Action::Move`] so the move is
/// reversible. Pure/testable.
pub fn move_pairs(entries: &[FileEntry], target: &Path) -> Vec<(PathBuf, PathBuf)> {
    entries
        .iter()
        .map(|e| (e.path.clone(), target.join(&e.name)))
        .collect()
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

/// Paths among `entries` whose lowercased name appears in `names`.
/// Pure set logic so "select files also present in the other panel" can be
/// unit-tested without a panel or filesystem. The complement of the
/// [`CompareCriterion::Unique`] set: name-matched regardless of size/mtime.
pub fn matching_name_paths<'a>(
    entries: impl Iterator<Item = &'a FileEntry>,
    names: &std::collections::HashSet<String>,
) -> std::collections::HashSet<PathBuf> {
    entries
        .filter(|e| names.contains(&e.name_lower))
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

/// Human explanation for compare-mode row tinting. Identical rows intentionally
/// return `None` so quiet rows stay quiet.
pub fn compare_hint(entry: &FileEntry, other: &CompareMap) -> Option<String> {
    match other.get(&entry.name_lower) {
        None => Some("Only in this panel".to_string()),
        Some(&(size, mtime)) => {
            let size_differs = size != entry.size;
            let time_differs = mtime != entry.modified;
            match (size_differs, time_differs) {
                (false, false) => None,
                (true, true) => Some("Same name, different size and modified time".to_string()),
                (true, false) => Some("Same name, different size".to_string()),
                (false, true) => Some("Same name, different modified time".to_string()),
            }
        }
    }
}

/// Compute (need bytes, free bytes on target, same-volume) for a transfer,
/// used to drive the will-it-fit guard in the confirmation dialog.
fn fit_stats(
    entries: &[FileEntry],
    target: &Path,
    _kind: TransferKind,
) -> (u64, Option<u64>, bool) {
    let need = transfer::total_bytes(entries);
    let free = crate::fs_util::free_space(target);
    // "Same volume" must hold for EVERY source, not just the first: a mixed
    // selection straddling volumes cannot take the no-extra-space move path, so
    // one cross-volume entry makes the whole batch cross-volume for the guard.
    let same = !entries.is_empty()
        && entries.iter().all(|e| {
            e.path
                .parent()
                .is_some_and(|src| crate::fs_util::same_volume(src, target))
        });
    (need, free, same)
}

/// Resolve a typed path for go-to-path (Cmd+L): trim, expand a leading `~`
/// to `home`, and require the result to be an existing directory.
pub fn resolve_dir_input(input: &str, home: &Path) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Path is empty".into());
    }
    let expanded: PathBuf = if trimmed == "~" {
        home.to_path_buf()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    if !expanded.exists() {
        return Err("Path does not exist".into());
    }
    if !expanded.is_dir() {
        return Err("Not a folder".into());
    }
    Ok(expanded)
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
            run_command_request: false,
            path_request: false,
            recent_request: false,
            undo_request: false,
            palette_request: false,
            batch_rename_request: false,
            sync_request: false,
            duplicates_request: false,
            diff_request: false,
            treemap_request: false,
            find_request: false,
            saved_search_request: false,
            clipboard_request: None,
            redo_request: false,
            drain_request: false,
            cycle_density_request: false,
            shelf: crate::shelf::Shelf::default(),
            bookmarks: crate::bookmarks::load(),
            selection_stash: std::collections::HashSet::new(),
            queue: crate::opqueue::Queue::new(),
            running_job: None,
            stack: crate::undo::UndoStack::default(),
            pending_undo_action: None,
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

    /// Add to the active panel's selection every visible entry whose name also
    /// exists in the inactive panel (by lowercased name). Builds on top of any
    /// existing selection so it composes with mask/manual picks.
    pub fn select_same_named(&mut self) {
        let names: std::collections::HashSet<String> = self
            .inactive_panel()
            .filtered_entries()
            .iter()
            .map(|e| e.name_lower.clone())
            .collect();
        let picks = {
            let active = self.active_panel_ref();
            matching_name_paths(active.filtered_entries().into_iter(), &names)
        };
        self.active_panel().extend_selection(picks);
    }

    /// Replace the active panel's selection with the entries a cross-pane
    /// relation (vs the inactive panel) picks: only-here, differing, or
    /// identical. Distinct from [`select_same_named`](Self::select_same_named),
    /// which adds all same-named entries regardless of content.
    fn select_by_relation(&mut self, pick: fn(&crate::sync::PaneRelation) -> &Vec<usize>) {
        let rel = crate::sync::pane_relation(
            &self.active_panel_ref().entries,
            &self.inactive_panel().entries,
        );
        let paths: Vec<PathBuf> = pick(&rel)
            .iter()
            .filter_map(|&i| {
                self.active_panel_ref()
                    .entries
                    .get(i)
                    .map(|e| e.path.clone())
            })
            .collect();
        self.active_panel().selected = paths.into_iter().collect();
    }

    /// Copy the active panel's current selection into the stash.
    pub fn stash_selection(&mut self) {
        self.selection_stash = self.active_panel_ref().selected.clone();
    }

    /// Replace the active panel's selection with `op(current, stash)`, dropping
    /// any paths no longer present in the panel so the result stays valid.
    fn combine_with_stash(
        &mut self,
        op: fn(
            &std::collections::HashSet<PathBuf>,
            &std::collections::HashSet<PathBuf>,
        ) -> std::collections::HashSet<PathBuf>,
    ) {
        let stash = self.selection_stash.clone();
        let panel = self.active_panel();
        let present: std::collections::HashSet<PathBuf> =
            panel.entries.iter().map(|e| e.path.clone()).collect();
        let combined = op(&panel.selected, &stash);
        panel.selected = combined.intersection(&present).cloned().collect();
    }

    pub fn stash_union(&mut self) {
        self.combine_with_stash(crate::selset::union);
    }
    pub fn stash_intersect(&mut self) {
        self.combine_with_stash(crate::selset::intersect);
    }
    pub fn stash_subtract(&mut self) {
        self.combine_with_stash(crate::selset::difference);
    }
    pub fn stash_symmetric_diff(&mut self) {
        self.combine_with_stash(crate::selset::symmetric_difference);
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
            Command::JumpBack => {
                self.active_panel().go_back();
            }
            Command::JumpForward => {
                self.active_panel().go_forward();
            }
            Command::JumpSlot(n) => {
                if let Some(path) = self.bookmarks.by_slot(n).map(|b| b.path.clone())
                    && path.is_dir()
                {
                    self.active_panel().navigate_to(path);
                }
            }
            Command::AssignSlot(n) => {
                let dir = self.active_panel_ref().current_path.clone();
                self.bookmark_dir(dir.clone());
                self.bookmarks.assign_slot(&dir, n);
                crate::bookmarks::save(&self.bookmarks);
            }
            Command::BookmarkCurrentDir => {
                // Toggle: bookmark the active directory, or un-bookmark it if it
                // is already a favorite.
                let dir = self.active_panel_ref().current_path.clone();
                if self.bookmarks.contains(&dir) {
                    self.bookmarks.remove(&dir);
                } else {
                    self.bookmark_dir(dir);
                }
                crate::bookmarks::save(&self.bookmarks);
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
            Command::BeginBatchRename => self.batch_rename_request = true,
            Command::BeginSync => self.sync_request = true,
            Command::FindDuplicates => self.duplicates_request = true,
            Command::DiffFiles => self.diff_request = true,
            Command::DiskTreemap => self.treemap_request = true,
            Command::BeginFind => self.find_request = true,
            Command::OpenSavedSearch => self.saved_search_request = true,
            Command::CopyPath => {
                self.clipboard_request = Some(crate::clipboard::PathStyle::FullPath)
            }
            Command::CopyName => {
                self.clipboard_request = Some(crate::clipboard::PathStyle::NameOnly)
            }
            Command::CopyParentPath => {
                self.clipboard_request = Some(crate::clipboard::PathStyle::ParentPath)
            }
            Command::CopyFileUrl => {
                self.clipboard_request = Some(crate::clipboard::PathStyle::FileUrl)
            }
            Command::CopyShellPath => {
                self.clipboard_request = Some(crate::clipboard::PathStyle::ShellEscaped)
            }
            Command::CopyRelativePath => {
                self.clipboard_request = Some(crate::clipboard::PathStyle::RelativeToOther)
            }
            Command::CycleDensity => self.cycle_density_request = true,
            Command::ShelfAdd => {
                let paths: Vec<PathBuf> = self
                    .active_panel_ref()
                    .selected_or_cursor()
                    .into_iter()
                    .map(|e| e.path)
                    .collect();
                self.shelf.add_all(paths);
            }
            Command::ShelfDrain => {
                if !self.shelf.is_empty() {
                    self.drain_request = true;
                }
            }
            Command::BeginSelectMask => self.mask_request = true,
            Command::BeginRunBar => self.run_command_request = true,
            Command::BeginGoToPath => self.path_request = true,
            Command::BeginRecent => self.recent_request = true,
            Command::BeginPalette => self.palette_request = true,
            Command::Undo => {
                if self.stack.can_undo() {
                    self.undo_request = true;
                }
            }
            Command::Redo => {
                if self.stack.can_redo() {
                    self.redo_request = true;
                }
            }
            Command::ToggleInfo => self.toggle_info(),
            Command::SelectAll => self.active_panel().select_all(),
            Command::InvertSelection => self.active_panel().invert_selection(),
            Command::SelectSameNamed => self.select_same_named(),
            Command::SelectOnlyHere => self.select_by_relation(|r| &r.only_here),
            Command::SelectDiffering => self.select_by_relation(|r| &r.differing),
            Command::SelectIdentical => self.select_by_relation(|r| &r.identical),
            Command::StashSelection => self.stash_selection(),
            Command::StashUnion => self.stash_union(),
            Command::StashIntersect => self.stash_intersect(),
            Command::StashSubtract => self.stash_subtract(),
            Command::StashSymmetricDiff => self.stash_symmetric_diff(),
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
        let (need_bytes, free_bytes, same_volume) = fit_stats(&entries, &target, kind);
        self.pending_op = Some(PendingOp::Transfer(PendingTransfer {
            kind,
            entries,
            target,
            conflicts,
            policy: OverwritePolicy::Ask,
            method: CopyMethod::Native,
            flat,
            need_bytes,
            free_bytes,
            same_volume,
        }));
    }

    /// Rich conflict list for the pending Copy/Move: source entries whose name
    /// already exists in the destination folder, with both sides' size/mtime.
    pub fn pending_conflicts(&self) -> Vec<crate::conflict::Conflict> {
        let Some(PendingOp::Transfer(tr)) = &self.pending_op else {
            return Vec::new();
        };
        // The destination listing is whichever loaded panel shows the target.
        let dest: &[FileEntry] = if self.left.current_path == tr.target {
            &self.left.entries
        } else if self.right.current_path == tr.target {
            &self.right.entries
        } else {
            &[]
        };
        crate::conflict::detect(&tr.entries, dest)
    }

    /// Apply a relation policy to the pending Copy: filter its entries to the
    /// resolution's keep-set and set the matching overwrite policy. Returns
    /// `true` if anything remains to transfer.
    pub fn resolve_pending_conflicts(&mut self, policy: crate::conflict::RelationPolicy) -> bool {
        let conflicts = self.pending_conflicts();
        let Some(PendingOp::Transfer(tr)) = &mut self.pending_op else {
            return false;
        };
        let res = crate::conflict::resolve(&tr.entries, &conflicts, policy);
        let keep: std::collections::HashSet<PathBuf> = res.keep.into_iter().collect();
        tr.entries.retain(|e| keep.contains(&e.path));
        tr.policy = match res.decision {
            crate::conflict::Decision::Overwrite => OverwritePolicy::OverwriteAll,
            crate::conflict::Decision::KeepBoth => OverwritePolicy::KeepBoth,
            crate::conflict::Decision::Skip => OverwritePolicy::SkipAll,
        };
        !tr.entries.is_empty()
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
        // A Move is undoable, promoted onto the history stack when it finishes
        // cleanly (see `poll_transfer`); a Copy records no history.
        let undo = if t.kind == TransferKind::Move {
            Some(crate::undo::Action::Move {
                pairs: move_pairs(&t.entries, &t.target),
            })
        } else {
            None
        };
        let spec = TransferSpec {
            kind: t.kind,
            entries: t.entries,
            target: t.target,
            policy: t.policy,
            method: t.method,
        };
        self.enqueue_only(spec, undo);
        self.pump_queue(notify);
    }

    /// Append a transfer to the queue without starting it.
    fn enqueue_only(&mut self, spec: TransferSpec, undo: Option<crate::undo::Action>) {
        let kind = match spec.kind {
            TransferKind::Copy => crate::opqueue::JobKind::Copy,
            TransferKind::Move => crate::opqueue::JobKind::Move,
        };
        self.queue.enqueue(kind, QueuedJob { spec, undo });
    }

    /// Start the next queued job if no transfer is active (concurrency cap 1).
    /// The single place that spawns the worker, so the running job, its undo
    /// action and `active_transfer` always move together.
    fn pump_queue(&mut self, notify: impl Fn() + Send + 'static) {
        if self.active_transfer.is_some() {
            return;
        }
        let Some(id) = self.queue.dequeue_next() else {
            return;
        };
        // Clone the spec/undo out of the (now Running) job to launch it.
        // `job.spec` is the opqueue payload (a QueuedJob); its `.spec` is the
        // TransferSpec and `.undo` the recorded action.
        let Some(job) = self.queue.get(id) else {
            return;
        };
        let spec = job.spec.spec.clone();
        self.pending_undo_action = job.spec.undo.clone();
        self.running_job = Some(id);
        let total = transfer::total_bytes(&spec.entries);
        let progress = Arc::new(Mutex::new(TransferProgress::new(total, spec.entries.len())));
        self.active_transfer = Some(progress.clone());
        transfer::spawn_transfer(spec, progress, notify);
    }

    /// Number of transfers waiting behind the active one (for a queued-count
    /// indicator).
    pub fn queued_count(&self) -> usize {
        self.queue
            .jobs()
            .iter()
            .filter(|j| j.state == crate::opqueue::JobState::Pending)
            .count()
    }

    /// Cancel active transfer.
    pub fn cancel_transfer(&mut self) {
        if let Some(ref state) = self.active_transfer {
            // A poisoned progress mutex (worker thread panicked) must not panic
            // the UI thread in turn; recover the guard and flag cancellation.
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            // Ignore a cancel that races in after the worker already finished
            // cleanly: flagging it would demote a completed Move to "not clean"
            // in poll_transfer and silently drop its undo entry.
            if !s.finished {
                s.cancelled = true;
            }
        }
    }

    /// Auto-close finished transfers. A transfer that finished with errors
    /// stays open so the user can read the error list (dismissed via OK).
    /// Returns `true` when a clean Move just finished, so the UI can raise the
    /// undo toast.
    pub fn poll_transfer(&mut self, notify: impl Fn() + Send + 'static) -> bool {
        let (close, clean, had_errors) = self
            .active_transfer
            .as_ref()
            .map(|s| {
                // Recover from a poisoned lock rather than panicking the UI.
                let s = s.lock().unwrap_or_else(|e| e.into_inner());
                // Close only once the worker has set `finished` (it now does so
                // even on cancel, after its cleanup), so we never tear the
                // shared state out from under a still-running cleanup pass. A
                // finished run with errors stays open so the user can read them.
                let clean = s.finished && s.errors.is_empty() && !s.cancelled;
                let errs = !s.errors.is_empty();
                (
                    s.finished && (s.cancelled || s.errors.is_empty()),
                    clean,
                    errs,
                )
            })
            .unwrap_or((false, false, false));

        if !close {
            return false;
        }
        self.active_transfer = None;
        // Retire the finished job from the queue so a free slot opens up.
        if let Some(id) = self.running_job.take() {
            if had_errors {
                self.queue.fail(id);
            } else {
                self.queue.complete(id);
            }
            self.queue.clear_finished();
        }
        self.left.refresh();
        self.right.refresh();
        // Record the move on the history stack on a clean run; this must read
        // the finished job's undo action BEFORE pumping the next job (which
        // overwrites `pending_undo_action`).
        let raised = if clean {
            match self.pending_undo_action.take() {
                Some(action) => {
                    self.stack.push(action);
                    true
                }
                None => false,
            }
        } else {
            self.pending_undo_action = None;
            false
        };
        // Start the next queued transfer, if any.
        self.pump_queue(notify);
        raised
    }

    /// Undo the most recent reversible action (Cmd+Z): execute its inverse and
    /// move it onto the redo stack.
    pub fn perform_undo(&mut self, notify: impl Fn() + Send + 'static) {
        if let Some(inverse) = self.stack.undo() {
            self.execute_action(inverse, notify);
        }
    }

    /// Redo the most recently undone action (Cmd+Shift+Z): re-apply it and move
    /// it back onto the undo stack.
    pub fn perform_redo(&mut self, notify: impl Fn() + Send + 'static) {
        if let Some(action) = self.stack.redo() {
            self.execute_action(action, notify);
        }
    }

    /// Execute `action` forward against the filesystem. Used by undo (with an
    /// inverted action) and redo (with the original). It records no new history
    /// of its own: the stack was already shuffled by `undo`/`redo`.
    fn execute_action(&mut self, action: crate::undo::Action, notify: impl Fn() + Send + 'static) {
        match action {
            crate::undo::Action::Move { pairs } => {
                // Each pair is (from, to): move the file at `from` into dir(to).
                let Some(dest_dir) = pairs
                    .first()
                    .and_then(|(_, to)| to.parent())
                    .map(Path::to_path_buf)
                else {
                    return;
                };
                let sources: Vec<PathBuf> = pairs.into_iter().map(|(from, _)| from).collect();
                self.start_move_silent(sources, dest_dir, notify);
            }
            crate::undo::Action::BatchRename { dir, pairs } => {
                // Undo/redo replays recorded pairs without re-planning, so order
                // them against the live directory (the inverse of a swap or a
                // case-only rename is itself a swap/case-only and needs staging).
                let existing = Self::dir_names(&dir);
                let _ = Self::apply_rename_order(&dir, &pairs, &existing);
                self.left.refresh();
                self.right.refresh();
            }
        }
    }

    /// Move the files at `sources` into `dest_dir` without recording undo
    /// history (the caller already updated the stack). One source dir, one
    /// dest dir, matching how user moves are shaped.
    fn start_move_silent(
        &mut self,
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
        notify: impl Fn() + Send + 'static,
    ) {
        let entries: Vec<FileEntry> = sources
            .iter()
            .filter_map(|p| {
                let meta = std::fs::metadata(p).ok()?;
                FileEntry::from_meta(p.clone(), &meta)
            })
            .collect();
        if entries.is_empty() {
            return;
        }
        let spec = TransferSpec {
            kind: TransferKind::Move,
            entries,
            target: dest_dir,
            policy: OverwritePolicy::Ask,
            method: CopyMethod::Native,
        };
        // Undo-driven: this move records no new history (undo=None).
        self.enqueue_only(spec, None);
        self.pump_queue(notify);
    }

    /// Add `dir` to the bookmarks if it is not already present, naming it after
    /// its final path component (the volume root falls back to its display
    /// path). A no-op when already bookmarked, so a slot assign keeps the name.
    fn bookmark_dir(&mut self, dir: PathBuf) {
        if self.bookmarks.contains(&dir) {
            return;
        }
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| dir.display().to_string());
        self.bookmarks.add(name, dir);
    }

    /// Every name currently in `dir` (best-effort), so a rename batch can be
    /// ordered against the live directory at apply/replay time.
    fn dir_names(dir: &Path) -> std::collections::HashSet<String> {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect()
    }

    /// Apply a `(from, to)` rename map in `dir` by computing a mid-batch-safe
    /// order (swaps, rotations and case-only renames are staged through a temp;
    /// see [`crate::rename_order`]) and executing it, rolling back on an OS
    /// failure. Returns how many entries were renamed, or a user-facing error
    /// (an unresolvable conflict, or the first OS error).
    fn apply_rename_order(
        dir: &Path,
        map: &[(String, String)],
        existing: &std::collections::HashSet<String>,
    ) -> Result<usize, String> {
        use crate::rename_order::{RenameOrder, apply_steps, safe_rename_order};
        match safe_rename_order(map, existing) {
            RenameOrder::Conflict(why) => Err(why),
            RenameOrder::Steps(steps) => apply_steps(&steps, |from, to| {
                std::fs::rename(dir.join(from), dir.join(to))
            })
            .map_err(|e: std::io::Error| e.to_string()),
        }
    }

    /// Move every entry to the Trash, counting successes and failures so the
    /// caller can confirm the outcome (and flag any that could not be removed)
    /// rather than failing silently.
    pub fn exec_delete(entries: &[FileEntry]) -> DeleteOutcome {
        let mut outcome = DeleteOutcome::default();
        for entry in entries {
            if trash::delete(&entry.path).is_ok() {
                outcome.trashed += 1;
            } else {
                outcome.failed += 1;
            }
        }
        outcome
    }

    /// Confirm the pending op. Returns `Some` only for a Delete (the synchronous
    /// op), so the caller can raise a result toast; a Transfer reports its own
    /// outcome asynchronously through [`poll_transfer`](Self::poll_transfer).
    pub fn confirm_pending_op(
        &mut self,
        notify: impl Fn() + Send + 'static,
    ) -> Option<DeleteOutcome> {
        match &self.pending_op {
            Some(PendingOp::Delete { .. }) => {
                if let Some(PendingOp::Delete { entries, .. }) = self.pending_op.take() {
                    let outcome = Self::exec_delete(&entries);
                    self.left.refresh();
                    self.right.refresh();
                    return Some(outcome);
                }
                None
            }
            Some(PendingOp::Transfer(_)) => {
                self.start_transfer(notify);
                None
            }
            None => None,
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
        // Probe the destination on disk (no-follow), not just the in-memory
        // sibling list: a hidden file, a case-fold match, or a file created
        // since the last refresh would otherwise be silently clobbered by the
        // atomic rename.
        if dest.symlink_metadata().is_ok() {
            return Err("Name already in use".into());
        }
        std::fs::rename(old, &dest).map_err(|e| e.to_string())?;
        self.active_panel().refresh();
        Ok(())
    }

    // ── Batch rename ────────────────────────────────────────────────────

    /// Names the batch-rename studio operates on: the active panel's selection,
    /// falling back to the entry under the cursor. Order drives the counter.
    pub fn batch_rename_targets(&self) -> Vec<String> {
        self.active_panel_ref()
            .selected_or_cursor()
            .into_iter()
            .map(|e| e.name)
            .collect()
    }

    /// Every name currently in the active directory, so the planner can catch
    /// collisions with siblings that are not part of the rename.
    pub fn active_dir_names(&self) -> std::collections::HashSet<String> {
        self.active_panel_ref()
            .entries
            .iter()
            .map(|e| e.name.clone())
            .collect()
    }

    /// Apply a batch rename to the active panel. Genuine swaps, rotations and
    /// case-only renames are now allowed: they are ordered safely (staging
    /// through a temp where needed) by [`Self::apply_rename_order`]. Only
    /// invalid target names and unresolvable conflicts (a target landing on an
    /// untouched sibling, or two rows clashing) are refused. Returns the number
    /// of entries renamed, or a user-facing error.
    pub fn apply_batch_rename(
        &mut self,
        rule: &crate::rename::RenameRule,
    ) -> Result<usize, String> {
        let names = self.batch_rename_targets();
        if names.is_empty() {
            return Err("Nothing selected to rename".into());
        }
        let existing = self.active_dir_names();
        let plans = crate::rename::plan_batch_rename(&names, &existing, rule);
        if plans
            .iter()
            .any(|p| p.status == crate::rename::PlanStatus::Invalid)
        {
            return Err("Fix the invalid names first".into());
        }
        // Every row whose name actually changes (Ok or a resolvable collision).
        let changes: Vec<(String, String)> = plans
            .iter()
            .filter(|p| p.to != p.from)
            .map(|p| (p.from.clone(), p.to.clone()))
            .collect();
        if changes.is_empty() {
            return Ok(0);
        }
        let dir = self.active_panel_ref().current_path.clone();

        let done = match Self::apply_rename_order(&dir, &changes, &existing) {
            Ok(done) => done,
            Err(e) => {
                self.active_panel().refresh();
                return Err(e);
            }
        };
        // Record the batch as one undoable unit (Cmd+Z reverts the whole run).
        if done > 0 {
            self.stack.push(crate::undo::Action::BatchRename {
                dir,
                pairs: changes,
            });
        }
        self.active_panel().selected.clear();
        self.active_panel().refresh();
        Ok(done)
    }

    // ── Duplicates ──────────────────────────────────────────────────────

    /// Find duplicate files in the active panel's directory (files only,
    /// non-recursive for now). Size-prefilters so only files whose size
    /// collides are content-hashed, groups by (size, hash), then confirms each
    /// group is byte-identical (a 64-bit hash match is not proof) before
    /// reporting it, so the dialog never offers to trash a false positive.
    pub fn find_duplicates(&self) -> Vec<crate::dedup::DupGroup> {
        use std::collections::HashMap;
        let files: Vec<&FileEntry> = self
            .active_panel_ref()
            .entries
            .iter()
            .filter(|e| !e.is_dir)
            .collect();
        let mut size_counts: HashMap<u64, usize> = HashMap::new();
        for f in &files {
            *size_counts.entry(f.size).or_insert(0) += 1;
        }
        let mut keys: Vec<crate::dedup::FileKey> = Vec::new();
        for f in &files {
            if size_counts.get(&f.size).copied().unwrap_or(0) < 2 {
                continue; // a unique size cannot have a duplicate
            }
            if let Some(hash) = crate::fs_util::content_hash(&f.path) {
                keys.push(crate::dedup::FileKey {
                    path: f.path.clone(),
                    size: f.size,
                    hash,
                    modified: f.modified,
                });
            }
        }
        crate::dedup::group_duplicates(&keys)
            .into_iter()
            .flat_map(Self::confirm_dup_group)
            .collect()
    }

    /// Split a hash-matched group into byte-identical clusters, keeping only
    /// those with two or more members. Guards against 64-bit hash collisions
    /// being trashed as duplicates.
    fn confirm_dup_group(group: crate::dedup::DupGroup) -> Vec<crate::dedup::DupGroup> {
        let size = group.size;
        let mut clusters: Vec<Vec<crate::dedup::FileKey>> = Vec::new();
        for file in group.files {
            if let Some(c) = clusters
                .iter_mut()
                .find(|c| crate::fs_util::files_equal(&c[0].path, &file.path))
            {
                c.push(file);
            } else {
                clusters.push(vec![file]);
            }
        }
        clusters
            .into_iter()
            .filter(|c| c.len() >= 2)
            .map(|files| crate::dedup::DupGroup { size, files })
            .collect()
    }

    /// Move `paths` to the Trash and refresh both panels. Not yet undoable
    /// here (recoverable from the Trash). Returns how many were trashed.
    pub fn trash_paths(&mut self, paths: &[PathBuf]) -> usize {
        let mut n = 0;
        for p in paths {
            if trash::delete(p).is_ok() {
                n += 1;
            }
        }
        self.left.refresh();
        self.right.refresh();
        n
    }

    // ── Disk usage treemap ──────────────────────────────────────────────

    /// The active folder's direct children as (entry, bytes), sized by file
    /// length or the cached recursive directory size (0 if not computed yet),
    /// sorted largest first. Reads the existing dir-size cache; never walks.
    pub fn treemap_items(&self) -> Vec<(FileEntry, u64)> {
        let active = self.active_panel_ref();
        let sizes = active.dir_sizes.lock().ok();
        let mut items: Vec<(FileEntry, u64)> = active
            .entries
            .iter()
            .map(|e| {
                let bytes = if e.is_dir {
                    sizes
                        .as_ref()
                        .and_then(|s| s.get(&e.path).copied())
                        .unwrap_or(0)
                } else {
                    e.size
                };
                (e.clone(), bytes)
            })
            .collect();
        items.sort_by_key(|i| std::cmp::Reverse(i.1));
        items
    }

    // ── Find ────────────────────────────────────────────────────────────

    /// Recursively walk `root` and collect entries matching `query`, capped at
    /// `cap`. Synchronous for now (a deep tree may pause briefly); walk errors
    /// and unreadable entries are skipped.
    pub fn run_find(&self, query: &crate::query::Query, root: &Path, cap: usize) -> Vec<FileEntry> {
        let now = std::time::SystemTime::now();
        let mut out = Vec::new();
        for entry in jwalk::WalkDir::new(root)
            .skip_hidden(false)
            .into_iter()
            .flatten()
        {
            if out.len() >= cap {
                break;
            }
            let path = entry.path();
            if path == root {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if let Some(fe) = FileEntry::from_meta(path, &meta)
                && query.matches(&fe, now)
            {
                out.push(fe);
            }
        }
        out
    }

    /// Reveal `path` in the active panel: navigate to its parent folder and put
    /// the cursor on it (used by find results).
    pub fn reveal(&mut self, path: &Path) {
        let Some(parent) = path.parent().map(Path::to_path_buf) else {
            return;
        };
        let name = path.file_name().map(|n| n.to_string_lossy().to_string());
        let panel = self.active_panel();
        panel.navigate_to(parent);
        if let Some(name) = name
            && let Some(idx) = panel.filtered_entries().iter().position(|e| e.name == name)
        {
            panel.cursor = idx + 1;
            panel.scroll_to_cursor = true;
        }
    }

    // ── Diff ────────────────────────────────────────────────────────────

    /// Pick the file pair to diff: two selected files in the active panel (in
    /// the panel's order), or one active file paired with a same-named file in
    /// the other panel. Returns `None` when no sensible pair exists.
    pub fn diff_targets(&self) -> Option<(PathBuf, PathBuf)> {
        let active = self.active_panel_ref();
        let files: Vec<PathBuf> = active
            .selected_entries()
            .into_iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.path)
            .collect();
        if files.len() == 2 {
            return Some((files[0].clone(), files[1].clone()));
        }
        // One file (selected, else under the cursor) vs the same name opposite.
        let one = if files.len() == 1 {
            files.into_iter().next()
        } else if active.cursor > 0 {
            active
                .filtered_get(active.cursor - 1)
                .filter(|e| !e.is_dir)
                .map(|e| e.path.clone())
        } else {
            None
        }?;
        let name = one.file_name()?.to_string_lossy().to_lowercase();
        let other = self
            .inactive_panel()
            .entries
            .iter()
            .find(|e| !e.is_dir && e.name_lower == name)?;
        Some((other.path.clone(), one))
    }

    // ── Shelf (drop stack) ──────────────────────────────────────────────

    /// Copy every shelved item into the active panel's directory in one pass,
    /// dropping self-copies and keeping both on name collisions, then clear the
    /// shelf. Routes through the transfer engine. Items whose source can no
    /// longer be read are kept on the shelf (not silently discarded), and the
    /// outcome reports both how many copies started and how many were left.
    pub fn drain_shelf(&mut self, notify: impl Fn() + Send + 'static) -> ShelfDrainOutcome {
        if self.shelf.is_empty() || self.active_transfer.is_some() {
            return ShelfDrainOutcome::default();
        }
        let dest = self.active_panel_ref().current_path.clone();
        let existing: std::collections::HashSet<String> = self
            .active_panel_ref()
            .entries
            .iter()
            .map(|e| e.name.clone())
            .collect();
        let plan = crate::shelf::drain_plan(self.shelf.items(), &dest, &existing);

        // Build sources for the planned copies, keeping any whose source could
        // not be read so they stay on the shelf rather than vanishing.
        let mut entries: Vec<FileEntry> = Vec::new();
        let mut unavailable: Vec<PathBuf> = Vec::new();
        for (src, _) in &plan {
            match std::fs::metadata(src)
                .ok()
                .and_then(|m| FileEntry::from_meta(src.clone(), &m))
            {
                Some(fe) => entries.push(fe),
                None => unavailable.push(src.clone()),
            }
        }

        // Self-copies (excluded by the plan) and started copies leave the
        // shelf; only the unreadable items remain for the user to retry.
        self.shelf.clear();
        self.shelf.add_all(unavailable.iter().cloned());

        let outcome = ShelfDrainOutcome {
            started: entries.len(),
            unavailable: unavailable.len(),
        };
        if !entries.is_empty() {
            // KeepBoth so a drained file never clobbers an existing one.
            self.start_copy(entries, dest, OverwritePolicy::KeepBoth, notify);
        }
        outcome
    }

    // ── Directory sync ──────────────────────────────────────────────────

    /// Compute the synchronisation plan between the two panels for `policy`.
    pub fn build_sync_actions(
        &self,
        policy: crate::sync::SyncPolicy,
    ) -> Vec<crate::sync::SyncAction> {
        crate::sync::sync_diff(&self.left.entries, &self.right.entries, policy)
    }

    /// Resolve and start a synchronisation plan: copy each `ToRight` row's left
    /// file into the right directory and each `ToLeft` row's right file into the
    /// left directory. Both passes are enqueued on the transfer queue and run in
    /// order (the left-bound pass starts when the right-bound one finishes), so
    /// the engine stays single-transfer with no special follow-up handling.
    pub fn apply_sync(
        &mut self,
        actions: &[crate::sync::SyncAction],
        notify: impl Fn() + Send + 'static,
    ) {
        use crate::sync::SyncDirection;
        let lookup = |entries: &[FileEntry], name_lower: &str| {
            entries.iter().find(|e| e.name_lower == name_lower).cloned()
        };
        let mut to_right = Vec::new();
        let mut to_left = Vec::new();
        for a in actions {
            let nl = a.name.to_lowercase();
            match a.direction {
                SyncDirection::ToRight => {
                    if let Some(e) = lookup(&self.left.entries, &nl) {
                        to_right.push(e);
                    }
                }
                SyncDirection::ToLeft => {
                    if let Some(e) = lookup(&self.right.entries, &nl) {
                        to_left.push(e);
                    }
                }
                SyncDirection::Skip => {}
            }
        }
        let right_dir = self.right.current_path.clone();
        let left_dir = self.left.current_path.clone();

        // Enqueue both passes; the queue runs them in order (the second starts
        // when the first finishes), so a two-way sync needs no special casing.
        if !to_right.is_empty() {
            self.enqueue_copy(to_right, right_dir, OverwritePolicy::OverwriteAll);
        }
        if !to_left.is_empty() {
            self.enqueue_copy(to_left, left_dir, OverwritePolicy::OverwriteAll);
        }
        self.pump_queue(notify);
    }

    /// Queue a background Copy of `entries` into `target` under `policy` without
    /// starting it. Used by the sync sheet and shelf drain, which already
    /// served as the review step (no confirmation dialog). Copies record no
    /// undo history.
    fn enqueue_copy(&mut self, entries: Vec<FileEntry>, target: PathBuf, policy: OverwritePolicy) {
        if entries.is_empty() {
            return;
        }
        let spec = TransferSpec {
            kind: TransferKind::Copy,
            entries,
            target,
            policy,
            method: CopyMethod::Native,
        };
        self.enqueue_only(spec, None);
    }

    /// Queue a copy and start it if idle (single-copy callers, e.g. shelf drain).
    fn start_copy(
        &mut self,
        entries: Vec<FileEntry>,
        target: PathBuf,
        policy: OverwritePolicy,
        notify: impl Fn() + Send + 'static,
    ) {
        self.enqueue_copy(entries, target, policy);
        self.pump_queue(notify);
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
        let (need_bytes, free_bytes, same_volume) =
            fit_stats(&entries, &target, TransferKind::Move);
        self.pending_op = Some(PendingOp::Transfer(PendingTransfer {
            kind: TransferKind::Move,
            entries,
            target,
            conflicts,
            policy: OverwritePolicy::Ask,
            method: CopyMethod::Native,
            flat,
            need_bytes,
            free_bytes,
            same_volume,
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
        ws.poll_transfer(|| {});
    }

    /// Drive the queue to completion: wait out the active transfer and any jobs
    /// queued behind it, polling between each.
    fn drain_transfers(ws: &mut Workspace) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while ws.active_transfer.is_some() {
            wait_transfer(ws);
            assert!(
                std::time::Instant::now() < deadline,
                "queue drain timed out"
            );
        }
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
        assert_eq!(
            compare_hint(&mk(&same, t0), &other),
            None,
            "identical rows should stay visually quiet"
        );
        assert_eq!(
            compare_hint(&mk(&diff, t0), &other).as_deref(),
            Some("Same name, different size and modified time")
        );
        assert_eq!(
            compare_hint(&mk(&only, t0), &other).as_deref(),
            Some("Only in this panel")
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
    fn matching_name_paths_picks_name_matches_only() {
        let tmp = TempDir::new();
        let mk = |name: &str| {
            let p = tmp.file(name, "");
            let meta = std::fs::metadata(&p).unwrap();
            FileEntry::from_meta(p, &meta).unwrap()
        };
        let shared = mk("Shared.txt");
        let local = mk("local.txt");
        let entries = [shared.clone(), local.clone()];
        // Name set is lowercased, mirroring build_compare_map's key space.
        let names: std::collections::HashSet<String> =
            ["shared.txt".to_string()].into_iter().collect();
        let picks = matching_name_paths(entries.iter(), &names);
        assert!(picks.contains(&shared.path), "name match selected");
        assert!(!picks.contains(&local.path), "unmatched name skipped");
        assert_eq!(picks.len(), 1);
    }

    #[test]
    fn select_same_named_adds_common_names_keeping_prior_picks() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let shared = l.file("report.txt", "x");
        let only_here = l.file("draft.txt", "y");
        r.file("Report.TXT", "z"); // same name, different case -> still a match
        let mut ws = workspace(&l, &r);

        // A pre-existing manual pick must survive the union.
        ws.left.selected.insert(only_here.clone());
        ws.select_same_named();

        assert!(ws.left.selected.contains(&shared), "common name selected");
        assert!(ws.left.selected.contains(&only_here), "prior pick kept");
        assert_eq!(ws.left.selected.len(), 2, "no spurious selections");
    }

    #[test]
    fn stash_union_and_subtract_combine_with_current_selection() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let a = l.file("a.txt", "1");
        let b = l.file("b.txt", "2");
        let c = l.file("c.txt", "3");
        let mut ws = workspace(&l, &r);

        // Stash {a, b}, then change the selection to {c}.
        ws.left.selected = [a.clone(), b.clone()].into_iter().collect();
        ws.stash_selection();
        ws.left.selected = [c.clone()].into_iter().collect();

        // Union with the stash -> {a, b, c}.
        ws.stash_union();
        assert_eq!(ws.left.selected.len(), 3);
        assert!(ws.left.selected.contains(&a) && ws.left.selected.contains(&c));

        // Subtract the stash {a, b} from {a, b, c} -> {c}.
        ws.stash_subtract();
        assert_eq!(
            ws.left.selected,
            [c.clone()]
                .into_iter()
                .collect::<std::collections::HashSet<_>>()
        );
    }

    #[test]
    fn apply_batch_rename_renames_only_the_selection() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("a.txt", "1");
        l.file("b.txt", "2");
        l.file("keep.log", "3"); // not selected
        let mut ws = workspace(&l, &r);
        ws.left.selected.insert(l.path().join("a.txt"));
        ws.left.selected.insert(l.path().join("b.txt"));

        let rule = crate::rename::RenameRule {
            prefix: "x_".into(),
            ..Default::default()
        };
        let n = ws.apply_batch_rename(&rule).unwrap();
        assert_eq!(n, 2);
        assert!(l.path().join("x_a.txt").is_file());
        assert!(l.path().join("x_b.txt").is_file());
        assert!(!l.path().join("a.txt").exists());
        assert!(
            l.path().join("keep.log").is_file(),
            "non-selected untouched"
        );
        assert!(
            ws.left.selected.is_empty(),
            "selection cleared after rename"
        );
    }

    #[test]
    fn apply_batch_rename_refuses_a_colliding_plan() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("report_v1.txt", "a");
        l.file("report_v2.txt", "b");
        let mut ws = workspace(&l, &r);
        ws.left.selected.insert(l.path().join("report_v1.txt"));
        ws.left.selected.insert(l.path().join("report_v2.txt"));

        // "v1" -> "v2" maps report_v1 onto report_v2's name while report_v2
        // stays put: a duplicate/sibling collision, so the plan is rejected.
        let dup = crate::rename::RenameRule {
            find: "v1".into(),
            replace: "v2".into(),
            ..Default::default()
        };
        let err = ws.apply_batch_rename(&dup);
        assert!(err.is_err(), "colliding plan rejected: {err:?}");
        // Both files are left untouched on refusal.
        assert!(l.path().join("report_v1.txt").is_file());
        assert!(l.path().join("report_v2.txt").is_file());
    }

    #[test]
    fn apply_sync_mirror_copies_left_only_file_right() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("new.txt", "hello");
        let mut ws = workspace(&l, &r);
        let actions = ws.build_sync_actions(crate::sync::SyncPolicy::MirrorLeftToRight);
        ws.apply_sync(&actions, || {});
        wait_transfer(&mut ws);
        assert!(r.path().join("new.txt").is_file(), "left -> right copied");
    }

    #[test]
    fn two_way_sync_runs_both_passes() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("left.txt", "L");
        r.file("right.txt", "R");
        let mut ws = workspace(&l, &r);
        let actions = ws.build_sync_actions(crate::sync::SyncPolicy::TwoWay);
        ws.apply_sync(&actions, || {});

        // Both passes are enqueued; the first runs now, the second waits behind
        // it on the queue (no more ad-hoc follow-up handling).
        assert!(ws.active_transfer.is_some(), "first pass running");
        assert_eq!(ws.queued_count(), 1, "second pass queued behind the first");

        // Drive the queue to completion; poll_transfer drains the second pass.
        drain_transfers(&mut ws);
        assert!(r.path().join("left.txt").is_file(), "left -> right");
        assert!(l.path().join("right.txt").is_file(), "right -> left");
        assert_eq!(ws.queued_count(), 0, "queue fully drained");
    }

    #[test]
    fn second_transfer_queues_and_runs_after_the_first() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let a = l.file("a.txt", "AAA");
        let b = l.file("b.txt", "BBBB");
        let mut ws = workspace(&l, &r);
        let entry = |p: &std::path::Path| {
            let meta = std::fs::metadata(p).unwrap();
            FileEntry::from_meta(p.to_path_buf(), &meta).unwrap()
        };

        // Fire two copies into the right dir. The first starts; the second is
        // queued behind it instead of being dropped (active_transfer stays set
        // until poll_transfer closes it).
        ws.start_copy(
            vec![entry(&a)],
            r.path().to_path_buf(),
            OverwritePolicy::KeepBoth,
            || {},
        );
        ws.start_copy(
            vec![entry(&b)],
            r.path().to_path_buf(),
            OverwritePolicy::KeepBoth,
            || {},
        );
        assert!(ws.active_transfer.is_some(), "first transfer running");
        assert_eq!(ws.queued_count(), 1, "second transfer queued, not dropped");

        drain_transfers(&mut ws);
        assert!(ws.active_transfer.is_none());
        assert_eq!(ws.queued_count(), 0);
        assert!(r.path().join("a.txt").is_file(), "first copy landed");
        assert!(r.path().join("b.txt").is_file(), "queued copy ran after");
    }

    #[test]
    fn find_duplicates_groups_identical_files_in_active_dir() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("a.txt", "same content");
        l.file("b.txt", "same content"); // byte-identical dup of a
        l.file("c.txt", "unique bytes"); // same length, different bytes
        l.file("d.txt", "x"); // unique size
        let ws = workspace(&l, &r);

        let groups = ws.find_duplicates();
        assert_eq!(groups.len(), 1, "only a.txt/b.txt are byte-identical");
        assert_eq!(groups[0].files.len(), 2);
        let names: Vec<String> = groups[0]
            .files
            .iter()
            .filter_map(|f| f.path.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert!(names.contains(&"a.txt".to_string()));
        assert!(names.contains(&"b.txt".to_string()));
    }

    #[test]
    fn drain_shelf_copies_staged_files_into_active_dir() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let src = r.file("gathered.txt", "data"); // lives in the right folder
        let mut ws = workspace(&l, &r); // active panel is the left

        ws.shelf.add(src);
        assert_eq!(ws.shelf.len(), 1);
        ws.drain_shelf(|| {});
        wait_transfer(&mut ws);

        assert!(
            l.path().join("gathered.txt").is_file(),
            "drained into the active (left) folder"
        );
        assert!(ws.shelf.is_empty(), "shelf cleared after drain");
    }

    #[test]
    fn drain_shelf_keeps_unreadable_items_instead_of_dropping_them() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let src = r.file("ghost.txt", "x"); // staged from the right folder
        let mut ws = workspace(&l, &r); // active panel is the left

        ws.shelf.add(src.clone());
        std::fs::remove_file(&src).unwrap(); // source vanishes before the drain

        let outcome = ws.drain_shelf(|| {});
        assert_eq!(outcome.started, 0);
        assert_eq!(outcome.unavailable, 1);
        assert_eq!(ws.shelf.len(), 1, "unreadable item kept for retry");
        assert!(
            ws.active_transfer.is_none(),
            "nothing readable, no transfer"
        );
    }

    #[test]
    fn diff_targets_picks_two_selected_or_same_named() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let a = l.file("a.txt", "1");
        let b = l.file("b.txt", "2");
        r.file("a.txt", "9"); // same name on the other side
        let mut ws = workspace(&l, &r);

        // Two selected in the active panel -> that pair.
        ws.left.selected.insert(a.clone());
        ws.left.selected.insert(b.clone());
        let (x, y) = ws.diff_targets().unwrap();
        let names: Vec<String> = [&x, &y]
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        assert!(names.contains(&"a.txt".to_string()) && names.contains(&"b.txt".to_string()));

        // One selected -> pair with the same-named file in the other panel.
        ws.left.selected.clear();
        ws.left.selected.insert(a.clone());
        let (x, y) = ws.diff_targets().unwrap();
        assert_eq!(y, a, "active file is the second target");
        assert_eq!(x, r.path().join("a.txt"), "other-panel same name is first");
    }

    #[test]
    fn run_find_walks_recursively_and_filters() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("top.log", "x");
        l.file("sub/deep.log", "yy");
        l.file("sub/note.txt", "z");
        let ws = workspace(&l, &r);

        let query = crate::query::Query {
            predicates: vec![crate::query::Predicate::NameContains(".log".into())],
        };
        let found = ws.run_find(&query, l.path(), 100);
        let names: Vec<String> = found.iter().map(|e| e.name.clone()).collect();
        assert!(names.contains(&"top.log".to_string()), "top-level match");
        assert!(names.contains(&"deep.log".to_string()), "nested match");
        assert!(
            !names.contains(&"note.txt".to_string()),
            "non-match excluded"
        );
    }

    #[test]
    fn select_by_relation_picks_only_here_and_differing() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("only.txt", "x"); // only in the active (left) panel
        l.file("both.txt", "AAA"); // present both sides, different size -> differing
        r.file("both.txt", "BBBBB");
        let mut ws = workspace(&l, &r);
        ws.left.refresh();
        ws.right.refresh();

        ws.execute(Command::SelectOnlyHere);
        assert_eq!(
            ws.left.selected,
            [l.path().join("only.txt")].into_iter().collect()
        );

        ws.execute(Command::SelectDiffering);
        assert_eq!(
            ws.left.selected,
            [l.path().join("both.txt")].into_iter().collect()
        );
    }

    #[test]
    fn jump_slot_navigates_active_panel_to_the_bookmarked_dir() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let project = l.dir("project");
        let mut ws = workspace(&l, &r);
        // Bookmark `project` in slot 1 (set the store directly; the execute
        // path for AssignSlot persists to the real config, so it is not used
        // in tests).
        ws.bookmarks.add("project", project.clone());
        assert!(ws.bookmarks.assign_slot(&project, 1));

        // Active panel elsewhere, then Cmd+1 jumps it to the bookmark.
        ws.left.navigate_to(l.path().to_path_buf());
        assert_eq!(ws.active_panel_ref().current_path, l.path());
        ws.execute(Command::JumpSlot(1));
        assert_eq!(ws.active_panel_ref().current_path, project);

        // An empty slot is a no-op (no panic, no navigation).
        let before = ws.active_panel_ref().current_path.clone();
        ws.execute(Command::JumpSlot(7));
        assert_eq!(ws.active_panel_ref().current_path, before);
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
    fn move_pairs_maps_source_to_dest() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let a = l.file("a.txt", "x");
        let b = l.file("b.txt", "y");
        let meta_a = std::fs::metadata(&a).unwrap();
        let meta_b = std::fs::metadata(&b).unwrap();
        let entries = vec![
            FileEntry::from_meta(a.clone(), &meta_a).unwrap(),
            FileEntry::from_meta(b.clone(), &meta_b).unwrap(),
        ];
        let pairs = move_pairs(&entries, r.path());
        // (from, to): from the entry's current path to target/name.
        assert_eq!(pairs[0], (a, r.path().join("a.txt")));
        assert_eq!(pairs[1], (b, r.path().join("b.txt")));
    }

    #[test]
    fn move_then_undo_restores_the_source() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let f = l.file("doc.txt", "data");
        let mut ws = workspace(&l, &r);
        ws.left.cursor = 1;

        ws.execute(Command::RequestMove);
        ws.confirm_pending_op(|| {});
        wait_transfer(&mut ws);
        assert!(!f.exists(), "move removed the source");
        assert!(r.path().join("doc.txt").exists());
        assert!(ws.stack.can_undo(), "a clean move is undoable");

        ws.perform_undo(|| {});
        wait_transfer(&mut ws);
        assert!(f.exists(), "undo restored the source");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "data");
        assert!(
            !r.path().join("doc.txt").exists(),
            "undo emptied the target"
        );

        // Redo re-applies the move.
        assert!(ws.stack.can_redo(), "the undone move is redoable");
        ws.perform_redo(|| {});
        wait_transfer(&mut ws);
        assert!(!f.exists(), "redo re-moved the source away");
        assert!(
            r.path().join("doc.txt").exists(),
            "redo restored the target"
        );
    }

    #[test]
    fn batch_rename_is_undoable_and_redoable() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("a.txt", "1");
        l.file("b.txt", "2");
        let mut ws = workspace(&l, &r);
        ws.left.selected.insert(l.path().join("a.txt"));
        ws.left.selected.insert(l.path().join("b.txt"));

        let rule = crate::rename::RenameRule {
            prefix: "x_".into(),
            ..Default::default()
        };
        assert_eq!(ws.apply_batch_rename(&rule).unwrap(), 2);
        assert!(l.path().join("x_a.txt").is_file());
        assert!(ws.stack.can_undo());

        ws.perform_undo(|| {});
        assert!(l.path().join("a.txt").is_file(), "undo restored names");
        assert!(!l.path().join("x_a.txt").exists());

        ws.perform_redo(|| {});
        assert!(l.path().join("x_a.txt").is_file(), "redo re-applied names");
        assert!(!l.path().join("a.txt").exists());
    }

    #[test]
    fn pending_transfer_overflow_logic() {
        let mk = |kind, method, need, free, same| PendingTransfer {
            kind,
            entries: vec![],
            target: PathBuf::from("/t"),
            conflicts: vec![],
            policy: OverwritePolicy::Ask,
            method,
            flat: scan::spawn_scan(vec![]),
            need_bytes: need,
            free_bytes: free,
            same_volume: same,
        };
        use CopyMethod::{Buffered, Native};
        // Cross-volume copy needing more than free overflows.
        assert!(mk(TransferKind::Copy, Native, 100, Some(50), false).overflows());
        // Cross-volume copy that fits does not.
        assert!(!mk(TransferKind::Copy, Native, 40, Some(50), false).overflows());
        // Same-volume move never overflows (instant rename).
        assert!(!mk(TransferKind::Move, Native, 100, Some(50), true).overflows());
        // Cross-volume move behaves like copy.
        assert!(mk(TransferKind::Move, Native, 100, Some(50), false).overflows());
        // Unknown free space: don't block.
        assert!(!mk(TransferKind::Copy, Native, 100, None, false).overflows());

        // Same-volume Native copy is an APFS clone: ~0 extra space, so it fits
        // even when the size dwarfs free (the bug the preflight fixes).
        let clone = mk(TransferKind::Copy, Native, 1_000, Some(10), true);
        assert!(!clone.overflows());
        assert!(clone.needs_no_space());
        // A same-volume BUFFERED copy writes every byte, so it can overflow.
        assert!(mk(TransferKind::Copy, Buffered, 1_000, Some(10), true).overflows());
        assert!(!mk(TransferKind::Copy, Buffered, 1_000, Some(10), true).needs_no_space());
        // A same-volume move needs no space regardless of method.
        assert!(mk(TransferKind::Move, Native, 1_000, Some(10), true).needs_no_space());
    }

    #[test]
    fn resolve_dir_input_expands_tilde_and_validates() {
        let home = TempDir::new();
        home.dir("Documents");
        let file = home.file("note.txt", "x");

        assert_eq!(resolve_dir_input("~", home.path()).unwrap(), home.path());
        assert_eq!(
            resolve_dir_input("~/Documents", home.path()).unwrap(),
            home.path().join("Documents")
        );
        let abs = home.path().join("Documents");
        assert_eq!(
            resolve_dir_input(abs.to_str().unwrap(), home.path()).unwrap(),
            abs
        );
        assert!(resolve_dir_input("   ", home.path()).is_err());
        assert!(resolve_dir_input("/no/such/dir/xyz", home.path()).is_err());
        // A file is not a directory.
        assert!(resolve_dir_input(file.to_str().unwrap(), home.path()).is_err());
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
    fn commit_rename_refuses_to_clobber_a_file_only_on_disk() {
        let (l, r) = (TempDir::new(), TempDir::new());
        let a = l.file("a.txt", "1");
        let mut ws = workspace(&l, &r);
        // Create the destination on disk AFTER the panel was loaded, so it is
        // not in the in-memory sibling list, exercising the disk probe.
        l.file("b.txt", "2");
        let err = ws.commit_rename(&a, "b.txt");
        assert!(err.is_err(), "must refuse to overwrite an existing file");
        assert_eq!(
            std::fs::read_to_string(l.path().join("b.txt")).unwrap(),
            "2"
        );
        assert!(a.exists(), "source untouched on refusal");
    }

    #[test]
    fn apply_rename_order_refuses_to_clobber_unrelated_target() {
        let tmp = TempDir::new();
        tmp.file("a.txt", "1");
        tmp.file("b.txt", "2"); // not part of the batch
        let map = vec![("a.txt".to_string(), "b.txt".to_string())];
        let existing = Workspace::dir_names(tmp.path());
        let r = Workspace::apply_rename_order(tmp.path(), &map, &existing);
        assert!(r.is_err(), "renaming onto an untouched sibling is refused");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
            "1"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("b.txt")).unwrap(),
            "2"
        );
    }

    #[test]
    fn apply_rename_order_swaps_two_files() {
        let tmp = TempDir::new();
        tmp.file("a.txt", "A");
        tmp.file("b.txt", "B");
        let map = vec![
            ("a.txt".to_string(), "b.txt".to_string()),
            ("b.txt".to_string(), "a.txt".to_string()),
        ];
        let existing = Workspace::dir_names(tmp.path());
        let n = Workspace::apply_rename_order(tmp.path(), &map, &existing).unwrap();
        assert_eq!(n, 2);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
            "B"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("b.txt")).unwrap(),
            "A"
        );
    }

    #[test]
    fn apply_batch_rename_allows_case_only_rename() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("readme.md", "x");
        let mut ws = workspace(&l, &r);
        ws.left.selected.insert(l.path().join("readme.md"));
        // Upper-case the stem: readme.md -> README.md (a case-only change the
        // old studio refused on a case-insensitive volume).
        let rule = crate::rename::RenameRule {
            case: crate::rename::CaseMode::Upper,
            ..Default::default()
        };
        let n = ws.apply_batch_rename(&rule).unwrap();
        assert_eq!(n, 1);
        // The on-disk name now reads with the upper-cased stem.
        let names = Workspace::dir_names(l.path());
        assert!(names.contains("README.md"), "names: {names:?}");
    }

    #[test]
    fn apply_batch_rename_undo_restores_a_case_only_rename() {
        let (l, r) = (TempDir::new(), TempDir::new());
        l.file("readme.md", "x");
        let mut ws = workspace(&l, &r);
        ws.left.selected.insert(l.path().join("readme.md"));
        let rule = crate::rename::RenameRule {
            case: crate::rename::CaseMode::Upper,
            ..Default::default()
        };
        ws.apply_batch_rename(&rule).unwrap();
        assert!(Workspace::dir_names(l.path()).contains("README.md"));
        // Undo puts the lower-case name back (itself a case-only rename).
        ws.perform_undo(|| {});
        let names = Workspace::dir_names(l.path());
        assert!(names.contains("readme.md"), "after undo: {names:?}");
        assert!(!names.contains("README.md"));
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
