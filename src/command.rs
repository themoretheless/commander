//! Keyboard commands: a pure mapping from key presses to [`Command`]s.
//! No egui types here; the UI layer translates raw input into [`KeyPress`].

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Command {
    SwitchPanel,
    CursorUp,
    CursorDown,
    /// Jump to the first row.
    CursorHome,
    /// Jump to the last row.
    CursorEnd,
    CursorFirstFile,
    CursorLastFile,
    /// Move up by one visible page.
    CursorPageUp,
    /// Move down by one visible page.
    CursorPageDown,
    /// Move the cursor by `n` rows (negative = up, positive = down),
    /// clamped to the filtered view. Driven by the vim-style `j`/`k` chords,
    /// with an optional leading count (e.g. `5j`).
    CursorMove(i32),
    /// Shift+Up: select the current row and move up (range selection).
    ExtendSelectUp,
    /// Shift+Down: select the current row and move down (range selection).
    ExtendSelectDown,
    /// Shift+Home: select every row from the cursor up to the first entry.
    ExtendSelectHome,
    /// Shift+End: select every row from the cursor down to the last entry.
    ExtendSelectEnd,
    /// Shift+PageUp: select the rows from the cursor one page up.
    ExtendSelectPageUp,
    /// Shift+PageDown: select the rows from the cursor one page down.
    ExtendSelectPageDown,
    /// Enter: open file / enter dir / go up on the ".." row.
    Activate,
    GoUp,
    GoHome,
    GoRoot,
    /// Cmd+[: walk back through this panel's directory history.
    JumpBack,
    /// Cmd+]: walk forward again after a [`JumpBack`](Command::JumpBack).
    JumpForward,
    /// Cmd+1..9: jump the active panel to the bookmark in that quick-jump slot.
    JumpSlot(u8),
    /// Cmd+Shift+1..9: bind the active directory to that quick-jump slot.
    AssignSlot(u8),
    /// Bookmark the active directory (palette command).
    BookmarkCurrentDir,
    /// Space: toggle selection and advance cursor.
    ToggleSelect,
    /// Cmd+Enter: move the selection into the directory under the cursor.
    MoveIntoCursorFolder,
    /// Cmd+Shift+Enter: copy the selection into the directory under the cursor.
    CopyIntoCursorFolder,
    /// F3: open/close preview in the other panel.
    TogglePreview,
    RequestCopy,
    RequestMove,
    CreateDir,
    RequestDelete,
    /// F2 / Cmd+R: rename the entry under the cursor.
    BeginRename,
    /// Cmd+Shift+R: open the batch-rename studio for the selection.
    BeginBatchRename,
    /// Cmd+Shift+S: open the directory-synchronisation sheet.
    BeginSync,
    /// Cmd+E: point the inactive panel at the active panel's directory.
    EqualizePanels,
    /// Cmd+U: swap the left and right panels.
    SwapPanels,
    /// Cmd+G: open the select-by-mask input.
    BeginSelectMask,
    /// Open the run-command / open-with bar for the current selection.
    BeginRunBar,
    /// Cmd+Shift+N: move the selection into a new subfolder.
    GatherIntoFolder,
    /// Cmd+I: toggle the Get-Info inspector for the cursor entry.
    ToggleInfo,
    /// Cmd+L: open the go-to-path input.
    BeginGoToPath,
    /// Cmd+P: open the recent-directories quick switcher.
    BeginRecent,
    /// Cmd+Z: undo the last reversible operation.
    Undo,
    /// Cmd+Shift+Z: redo the last undone operation.
    Redo,
    /// Cmd+K: open the command palette.
    BeginPalette,
    SelectAll,
    /// Cmd+Shift+D: cycle the list density (Compact / Comfortable / Spacious).
    CycleDensity,
    /// Open the duplicate finder for the active panel's folder.
    FindDuplicates,
    /// Cmd+D: diff the selected file pair (read-only unified view).
    DiffFiles,
    /// Cmd+Shift+M: open the disk-usage treemap for the active folder.
    DiskTreemap,
    /// Cmd+F: open the recursive find sheet.
    BeginFind,
    /// Cmd+Shift+F: move keyboard focus into the active pane's filter box.
    FocusFilter,
    /// Open the saved-search (smart folder) picker.
    OpenSavedSearch,
    /// Open named multi-root project collections.
    OpenProjectCollections,
    /// Cmd+Shift+C: copy the selection's full path(s) to the clipboard.
    CopyPath,
    /// Copy the selection's file name(s).
    CopyName,
    CopyNameWithoutExtension,
    /// Copy the selection's parent directory path(s).
    CopyParentPath,
    /// Copy the selection as `file://` URL(s).
    CopyFileUrl,
    /// Copy the selection's path(s), shell-escaped.
    CopyShellPath,
    /// Copy the selection's path(s) relative to the other pane.
    CopyRelativePath,
    /// Flip the selection across the visible rows of the active panel.
    InvertSelection,
    /// Select active-panel entries whose name also exists in the other panel.
    SelectSameNamed,
    /// Select active-panel entries that have no same-named entry in the other.
    SelectOnlyHere,
    /// Select entries present in both panels but differing in size/mtime.
    SelectDiffering,
    /// Select entries present in both panels with identical size and mtime.
    SelectIdentical,
    /// Clear the selection in this folder and every other folder visited.
    ClearSelection,
    SaveSelection,
    RestoreSelection,
    /// Show/hide the transfer-queue panel (pause/resume/reorder/cancel the
    /// jobs waiting behind the active transfer).
    ToggleQueuePanel,
    /// Turn panel comparison (difference highlighting) on or off.
    ToggleCompare,
    /// Switch between the light and dark theme.
    ToggleTheme,
    ShowListView,
    ShowIconsView,
    NewTab,
    CloseTab,
    ProtectTab,
    UnprotectTab,
    ReopenClosedTab,
    EditPermissions,
    NextTab,
    PreviousTab,
    FirstTab,
    LastTab,
    /// Activate a zero-based folder-tab position in the focused pane.
    GoToTab(usize),
    OpenFolderTab,
    OpenFolderBackgroundTab,
    /// Navigate the other pane to the highlighted folder without moving focus.
    OpenFolderOtherPane,
    CopyTabsToOther,
    MoveTabLeft,
    MoveTabRight,
    CloseDuplicateTabs,
    CloseOtherTabs,
    CloseTabsLeft,
    CloseTabsRight,
    KeyboardHelp,
    /// Show or hide the size bars behind file sizes.
    ToggleSizeBars,
    /// Show or hide the folder sidebar.
    ToggleFolderSidebar,
    /// Open the searchable history of completed moves/deletes/batch-renames.
    OpenReceipts,
    /// Open interrupted-operation recovery, rollback, and staging cleanup.
    OpenRecoveryCenter,
    ToggleHidden,
    /// Toggle whether folders are pinned to the top of the active listing.
    ToggleFoldersFirst,
    ToggleFoldersByName,
    /// Toggle natural (`file2` < `file10`) vs plain A-Z name ordering.
    ToggleNaturalSort,
    ToggleCaseSensitiveSort,
    /// Sort the active panel by name (again: flip the order).
    SortByName,
    SortByStem,
    /// Sort the active panel by size (again: flip the order).
    SortBySize,
    /// Sort the active panel by modification time (again: flip the order).
    SortByModified,
    /// Sort the active panel by file extension.
    SortByExtension,
    /// Sort the active panel by coarse kind (folder/image/doc/...).
    SortByKind,
    /// Select the well-known clutter files (`.DS_Store`, `Thumbs.db`, ...).
    SelectJunk,
    /// Reverse the active panel's current sort order.
    ReverseSort,
    /// Select the largest files in the active panel's filtered view.
    SelectLargest,
    /// Select files sharing the cursor file's extension.
    SelectLikeCursor,
    DeselectLikeCursor,
    SelectSameStem,
    DeselectSameStem,
    SelectVisibleFiles,
    SelectVisibleFolders,
    InvertVisibleFiles,
    InvertVisibleFolders,
    /// Select zero-byte files in the filtered view.
    SelectEmptyFiles,
    /// Copy the active listing to the clipboard as plain text.
    CopyListingText,
    /// Copy the active listing to the clipboard as CSV.
    CopyListingCsv,
    /// Copy the active listing to the clipboard as a Markdown table.
    CopyListingMarkdown,
    /// Verify content hashes for the selection (user-facing checksum).
    VerifyChecksum,
    /// Read the cursor file as hexadecimal bytes without editing it.
    ViewHex,
    /// Create symlinks in the other panel pointing at the selection.
    CreateSymlink,
    /// Create hard links in the other panel for selected regular files.
    CreateHardlink,
    /// Toggle synchronized dual-pane navigation lock.
    ToggleNavLock,
    /// Repeat the last workspace command (dot-repeat).
    RepeatLast,
    /// Browse the archive under the cursor (ZIP / TAR / tar.gz).
    BrowseArchive,
    /// Mark the active panel root as Trusted (research J003).
    SetTrustTrusted,
    /// Mark the active panel root as Restricted (research J003).
    SetTrustRestricted,
    /// Mark the active panel root as Untrusted (research J003).
    SetTrustUntrusted,
    /// Persist the current dual-pane roots/filters as the default workspace profile (J008).
    SaveWorkspaceProfile,
    /// Apply the saved default workspace profile to both panels (J008).
    ApplyWorkspaceProfile,
}

impl Command {
    /// Whether the command changes the filesystem: exactly the commands gated
    /// by [`CommandPredicate::MutationsAllowed`], so the safe-state block and
    /// the availability policy cannot drift apart.
    pub const fn mutates_filesystem(self) -> bool {
        let gates = predicates(self);
        let mut index = 0;
        while index < gates.len() {
            if matches!(gates[index], CommandPredicate::MutationsAllowed) {
                return true;
            }
            index += 1;
        }
        false
    }
}

/// UI-independent facts used to decide whether a command can do useful work.
/// Keeping this snapshot free of egui and filesystem access lets every command
/// surface share one policy and makes the edge cases cheap to test.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommandContext {
    pub visible_entries: usize,
    pub tab_count: usize,
    pub active_tab_protected: bool,
    pub other_tab_count: usize,
    pub active_tab_index: usize,
    pub duplicate_tabs: usize,
    pub closed_tabs: usize,
    pub closable_tabs_left: usize,
    pub closable_tabs_right: usize,
    pub visible_files: usize,
    pub other_entries: usize,
    pub picked_entries: usize,
    pub selected_entries: usize,
    pub can_save_selection: bool,
    pub can_restore_selection: bool,
    pub listing_entries: usize,
    pub cursor_entry: bool,
    /// The cursor row is already selected, so Space would deselect it.
    pub cursor_selected: bool,
    pub cursor_is_dir: bool,
    pub cursor_is_file: bool,
    pub cursor_has_extension: bool,
    pub can_go_up: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub can_diff: bool,
    pub can_transfer_into_cursor_folder: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub preview_open: bool,
    pub info_open: bool,
    pub safe_state: bool,
    pub active_mutation: bool,
    pub pending_operation: bool,
    pub active_transfer: bool,
    pub transfer_queue_busy: bool,
    pub active_read_only: bool,
    pub inactive_read_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandAvailability {
    pub enabled: bool,
    pub reason: Option<&'static str>,
}

impl CommandAvailability {
    const fn enabled() -> Self {
        Self {
            enabled: true,
            reason: None,
        }
    }

    const fn disabled(reason: &'static str) -> Self {
        Self {
            enabled: false,
            reason: Some(reason),
        }
    }
}

const SAFE_STATE_REASON: &str = "Review the interrupted operation first";
const ACTIVE_MUTATION_REASON: &str = "Wait for the current file operation to finish";
const PENDING_REASON: &str = "Finish or cancel the current confirmation";
const QUEUE_REASON: &str = "Wait for the transfer queue to finish";
const PICK_REASON: &str = "Select or highlight at least one item";
const CURSOR_REASON: &str = "Highlight an item first";
const VISIBLE_REASON: &str = "No visible items in this panel";

/// A reusable, typed condition for command availability. Commands compose
/// these small predicates instead of embedding UI-specific boolean formulas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandPredicate {
    MutationsAllowed,
    ConfirmationClosed,
    TransferQueueIdle,
    HasPickedEntry,
    HasCursorEntry,
    CursorIsDirectory,
    CursorIsFile,
    HasTransferSource,
    CanUndo,
    CanRedo,
    CanDiff,
    PreviewTarget,
    InfoTarget,
    HasVisibleEntry,
    HasVisibleFile,
    HasVisibleFolder,
    HasOtherEntry,
    HasListingEntry,
    CursorHasExtension,
    CanGoBack,
    CanGoForward,
    CanGoUp,
    CanActivate,
    ActiveWritable,
    InactiveWritable,
    AnyPaneWritable,
    CanAddTab,
    CanCopyTabsToOther,
    CanCloseTab,
    ActiveTabUnprotected,
    ActiveTabProtected,
    CanMoveTabLeft,
    CanMoveTabRight,
    HasDuplicateTabs,
    HasClosedTab,
    HasClosableTabLeft,
    HasClosableTabRight,
    HasClosableOtherTab,
    UnixPermissions,
    CanSaveSelection,
    CanRestoreSelection,
}

impl CommandPredicate {
    fn failure(self, context: &CommandContext) -> Option<&'static str> {
        let (met, reason) = match self {
            Self::MutationsAllowed => {
                if context.safe_state {
                    (false, SAFE_STATE_REASON)
                } else {
                    (!context.active_mutation, ACTIVE_MUTATION_REASON)
                }
            }
            Self::CanAddTab => (
                context.tab_count < crate::workspace::tabs::MAX_TABS,
                "This pane already has eight tabs",
            ),
            Self::CanCopyTabsToOther => (
                context.tab_count > 0
                    && context.other_tab_count > 0
                    && context.tab_count.saturating_add(context.other_tab_count)
                        <= crate::workspace::tabs::MAX_TABS,
                "The other pane needs room for the whole tab group (eight tabs maximum)",
            ),
            Self::CanMoveTabLeft => (context.active_tab_index > 0, "This tab is already first"),
            Self::CanMoveTabRight => (
                context.active_tab_index + 1 < context.tab_count,
                "This tab is already last",
            ),
            Self::CanSaveSelection => (
                context.can_save_selection,
                "Wait for a complete folder listing",
            ),
            Self::CanRestoreSelection => (
                context.can_restore_selection,
                "No saved selection for this folder, or its listing is incomplete",
            ),
            Self::UnixPermissions => (cfg!(unix), "POSIX permission editing requires a Unix host"),
            Self::HasClosableTabLeft => (
                context.closable_tabs_left > 0,
                "No unprotected folder tabs to the left",
            ),
            Self::HasClosableTabRight => (
                context.closable_tabs_right > 0,
                "No unprotected folder tabs to the right",
            ),
            Self::HasClosableOtherTab => (
                context.closable_tabs_left > 0 || context.closable_tabs_right > 0,
                "No other unprotected folder tabs",
            ),
            Self::HasClosedTab => (
                context.closed_tabs > 0,
                "No recently closed folder tabs in this pane",
            ),
            Self::HasDuplicateTabs => (
                context.duplicate_tabs > 0,
                "No duplicate folder tabs in this pane",
            ),
            Self::CanCloseTab => (context.tab_count > 1, "Keep at least one tab in each pane"),
            Self::ActiveTabProtected => (context.active_tab_protected, "This tab is not protected"),
            Self::ActiveTabUnprotected => (
                !context.active_tab_protected,
                "Unprotect this tab before closing it",
            ),
            Self::ConfirmationClosed => (!context.pending_operation, PENDING_REASON),
            Self::TransferQueueIdle => (!context.transfer_queue_busy, QUEUE_REASON),
            Self::HasPickedEntry => (context.picked_entries > 0, PICK_REASON),
            Self::HasCursorEntry => (context.cursor_entry, CURSOR_REASON),
            Self::CursorIsDirectory => (context.cursor_is_dir, "Highlight a destination folder"),
            Self::CursorIsFile => (
                context.cursor_is_file,
                "Highlight a ZIP, TAR or tar.gz file",
            ),
            Self::HasTransferSource => (
                context.can_transfer_into_cursor_folder,
                "Choose a source outside the destination folder",
            ),
            Self::CanUndo => (context.can_undo, "Nothing to undo"),
            Self::CanRedo => (context.can_redo, "Nothing to redo"),
            Self::CanDiff => (context.can_diff, "Select two files or a matching file pair"),
            Self::PreviewTarget => (
                context.preview_open || context.cursor_is_file,
                "Highlight a file to preview",
            ),
            Self::InfoTarget => (context.info_open || context.cursor_entry, CURSOR_REASON),
            Self::HasVisibleEntry => (context.visible_entries > 0, VISIBLE_REASON),
            Self::HasVisibleFile => (context.visible_files > 0, "No visible files in this panel"),
            Self::HasVisibleFolder => (
                context.visible_entries > context.visible_files,
                "No visible folders in this panel",
            ),
            Self::HasOtherEntry => (context.other_entries > 0, "The other panel is empty"),
            Self::HasListingEntry => (context.listing_entries > 0, VISIBLE_REASON),
            Self::CursorHasExtension => (
                context.cursor_has_extension,
                "Highlight a file with an extension",
            ),
            Self::CanGoBack => (context.can_go_back, "No earlier folder in history"),
            Self::CanGoForward => (context.can_go_forward, "No later folder in history"),
            Self::CanGoUp => (context.can_go_up, "Already at the filesystem root"),
            Self::CanActivate => (context.cursor_entry || context.can_go_up, "No item to open"),
            Self::ActiveWritable => (!context.active_read_only, "The active panel is read-only"),
            Self::InactiveWritable => (
                !context.inactive_read_only,
                "The destination panel is read-only",
            ),
            Self::AnyPaneWritable => (
                !context.active_read_only || !context.inactive_read_only,
                "Both panels are read-only",
            ),
        };
        (!met).then_some(reason)
    }
}

/// Return the reusable policy fragments attached to a command.
pub const fn predicates(command: Command) -> &'static [CommandPredicate] {
    use Command::*;
    use CommandPredicate as P;

    const COPY: &[P] = &[
        P::MutationsAllowed,
        P::ConfirmationClosed,
        P::HasPickedEntry,
        P::InactiveWritable,
    ];
    const MOVE: &[P] = &[
        P::MutationsAllowed,
        P::ConfirmationClosed,
        P::HasPickedEntry,
        P::ActiveWritable,
        P::InactiveWritable,
    ];
    const DELETE: &[P] = &[
        P::MutationsAllowed,
        P::ConfirmationClosed,
        P::TransferQueueIdle,
        P::HasPickedEntry,
        P::ActiveWritable,
    ];
    const TRANSFER_INTO: &[P] = &[
        P::MutationsAllowed,
        P::ConfirmationClosed,
        P::TransferQueueIdle,
        P::HasPickedEntry,
        P::CursorIsDirectory,
        P::HasTransferSource,
        P::ActiveWritable,
    ];
    const MUTATE_ACTIVE: &[P] = &[P::MutationsAllowed, P::ActiveWritable];
    const SYNC: &[P] = &[P::MutationsAllowed, P::AnyPaneWritable];
    const RENAME: &[P] = &[P::MutationsAllowed, P::HasCursorEntry, P::ActiveWritable];
    const MUTATE_SELECTION: &[P] = &[P::MutationsAllowed, P::HasPickedEntry, P::ActiveWritable];
    const RUN_SELECTION: &[P] = &[P::MutationsAllowed, P::HasPickedEntry];
    const UNDO: &[P] = &[P::MutationsAllowed, P::TransferQueueIdle, P::CanUndo];
    const REDO: &[P] = &[P::MutationsAllowed, P::TransferQueueIdle, P::CanRedo];
    const DIFF: &[P] = &[P::CanDiff];
    const PREVIEW: &[P] = &[P::PreviewTarget];
    const INFO: &[P] = &[P::InfoTarget];
    const PICKED: &[P] = &[P::HasPickedEntry];
    const VISIBLE: &[P] = &[P::HasVisibleEntry];
    const COMPARE: &[P] = &[P::HasVisibleEntry, P::HasOtherEntry];
    const VISIBLE_FILES: &[P] = &[P::HasVisibleFile];
    const EXTENSION: &[P] = &[P::CursorHasExtension];
    const LISTING: &[P] = &[P::HasListingEntry];
    const CURSOR: &[P] = &[P::HasCursorEntry];
    // Browse archive opens the highlighted entry, not the selection.
    const CURSOR_FILE: &[P] = &[P::HasCursorEntry, P::CursorIsFile];
    const BACK: &[P] = &[P::CanGoBack];
    const FORWARD: &[P] = &[P::CanGoForward];
    const UP: &[P] = &[P::CanGoUp];
    const ACTIVATE: &[P] = &[P::CanActivate];

    match command {
        SaveSelection => &[P::ConfirmationClosed, P::CanSaveSelection],
        RestoreSelection => &[P::ConfirmationClosed, P::CanRestoreSelection],
        ProtectTab => &[P::ConfirmationClosed, P::ActiveTabUnprotected],
        UnprotectTab => &[P::ConfirmationClosed, P::ActiveTabProtected],
        GoToTab(_) => &[P::ConfirmationClosed],
        FirstTab => &[P::ConfirmationClosed, P::CanMoveTabLeft],
        LastTab => &[P::ConfirmationClosed, P::CanMoveTabRight],
        MoveTabLeft => &[P::ConfirmationClosed, P::CanMoveTabLeft],
        MoveTabRight => &[P::ConfirmationClosed, P::CanMoveTabRight],
        CloseOtherTabs => &[P::ConfirmationClosed, P::HasClosableOtherTab],
        CloseTabsLeft => &[P::ConfirmationClosed, P::HasClosableTabLeft],
        CloseTabsRight => &[P::ConfirmationClosed, P::HasClosableTabRight],
        CloseDuplicateTabs => &[P::ConfirmationClosed, P::HasDuplicateTabs],
        EditPermissions => &[
            P::MutationsAllowed,
            P::ConfirmationClosed,
            P::TransferQueueIdle,
            P::HasPickedEntry,
            P::ActiveWritable,
            P::UnixPermissions,
        ],
        ReopenClosedTab => &[P::ConfirmationClosed, P::CanAddTab, P::HasClosedTab],
        NewTab => &[P::ConfirmationClosed, P::CanAddTab],
        CopyTabsToOther => &[P::ConfirmationClosed, P::CanCopyTabsToOther],
        OpenFolderTab | OpenFolderBackgroundTab => {
            &[P::ConfirmationClosed, P::CanAddTab, P::CursorIsDirectory]
        }
        OpenFolderOtherPane => &[P::ConfirmationClosed, P::CursorIsDirectory],
        CloseTab => &[
            P::ConfirmationClosed,
            P::CanCloseTab,
            P::ActiveTabUnprotected,
        ],
        NextTab | PreviousTab => &[P::ConfirmationClosed, P::CanCloseTab],
        RequestCopy => COPY,
        RequestMove => MOVE,
        RequestDelete => DELETE,
        MoveIntoCursorFolder | CopyIntoCursorFolder => TRANSFER_INTO,
        CreateDir => MUTATE_ACTIVE,
        BeginSync => SYNC,
        BeginRename => RENAME,
        BeginBatchRename | GatherIntoFolder => MUTATE_SELECTION,
        CreateSymlink | CreateHardlink => COPY,
        BeginRunBar => RUN_SELECTION,
        VerifyChecksum => PICKED,
        BrowseArchive | ViewHex => CURSOR_FILE,
        Undo => UNDO,
        Redo => REDO,
        DiffFiles => DIFF,
        TogglePreview => PREVIEW,
        ToggleInfo => INFO,
        CopyPath
        | CopyName
        | CopyNameWithoutExtension
        | CopyParentPath
        | CopyFileUrl
        | CopyShellPath
        | CopyRelativePath => PICKED,
        SelectAll | InvertSelection | SelectOnlyHere | BeginSelectMask | SelectVisibleFolders => {
            VISIBLE
        }
        SelectSameNamed | SelectDiffering | SelectIdentical => COMPARE,
        SelectJunk | SelectLargest | SelectEmptyFiles | SelectVisibleFiles => VISIBLE_FILES,
        CursorFirstFile | CursorLastFile => VISIBLE_FILES,
        InvertVisibleFiles => VISIBLE_FILES,
        InvertVisibleFolders => &[P::HasVisibleFolder],
        SelectLikeCursor | DeselectLikeCursor => EXTENSION,
        SelectSameStem | DeselectSameStem => CURSOR_FILE,
        CopyListingText | CopyListingCsv | CopyListingMarkdown => LISTING,
        ToggleSelect | ExtendSelectUp | ExtendSelectDown | ExtendSelectHome | ExtendSelectEnd
        | ExtendSelectPageUp | ExtendSelectPageDown => CURSOR,
        JumpBack => BACK,
        JumpForward => FORWARD,
        GoUp => UP,
        GoHome | GoRoot => &[P::ConfirmationClosed],
        Activate => ACTIVATE,
        _ => &[],
    }
}

/// Return whether availability for `command` depends on workspace state.
/// Unconditional commands can bypass the comparatively expensive context
/// snapshot on keyboard hot paths.
pub const fn requires_context(command: Command) -> bool {
    !predicates(command).is_empty()
}

/// Return whether `command` is meaningful in `context`, with a concise reason
/// suitable for disabled controls. This describes capability, not execution;
/// operation methods still repeat their safety checks at commit time.
pub fn availability(command: Command, context: &CommandContext) -> CommandAvailability {
    if let Some(reason) = predicates(command)
        .iter()
        .find_map(|predicate| predicate.failure(context))
    {
        return CommandAvailability::disabled(reason);
    }
    if let Command::GoToTab(index) = command {
        if index >= context.tab_count || index >= crate::workspace::tabs::MAX_TABS {
            return CommandAvailability::disabled("This folder tab is not open");
        }
        if index == context.active_tab_index {
            return CommandAvailability::disabled("This folder tab is already active");
        }
    }
    CommandAvailability::enabled()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShortcutHint {
    pub key: &'static str,
    pub label: &'static str,
    pub command: Command,
}

/// Keyboard help for the focused pane. Unavailable actions are omitted and
/// stateful actions describe what the next key press will actually do.
pub fn contextual_shortcuts(context: &CommandContext) -> Vec<ShortcutHint> {
    let mut candidates = vec![
        ShortcutHint {
            key: "Tab",
            label: "Switch",
            command: Command::SwitchPanel,
        },
        ShortcutHint {
            key: "Enter",
            label: "Open",
            command: Command::Activate,
        },
        ShortcutHint {
            key: "Space",
            label: if context.cursor_selected {
                "Deselect"
            } else {
                "Select"
            },
            command: Command::ToggleSelect,
        },
        ShortcutHint {
            key: "F3",
            label: if context.preview_open {
                "Close preview"
            } else {
                "Preview"
            },
            command: Command::TogglePreview,
        },
        ShortcutHint {
            key: "F5",
            label: "Copy",
            command: Command::RequestCopy,
        },
        ShortcutHint {
            key: "F6",
            label: "Move",
            command: Command::RequestMove,
        },
        ShortcutHint {
            key: "Cmd+Enter",
            label: "Move in",
            command: Command::MoveIntoCursorFolder,
        },
        ShortcutHint {
            key: "Cmd+Z",
            label: "Undo",
            command: Command::Undo,
        },
        ShortcutHint {
            key: "Backspace",
            label: "Up",
            command: Command::GoUp,
        },
        ShortcutHint {
            key: "F7",
            label: "New folder",
            command: Command::CreateDir,
        },
        ShortcutHint {
            key: "F8",
            label: "Delete",
            command: Command::RequestDelete,
        },
    ];
    candidates.retain(|hint| {
        (!context.active_transfer
            || matches!(hint.command, Command::RequestCopy | Command::RequestMove))
            && availability(hint.command, context).enabled
    });
    candidates
}

/// User-facing commands for the Cmd+K palette: (label, shortcut, command).
/// One command-palette row: what it reads as, its shortcut hint, the search
/// keywords that find it, and the command it runs. A palette command is
/// declared once, here; availability policy lives in [`predicates`].
#[derive(Clone, Copy, Debug)]
pub struct CommandEntry {
    pub label: &'static str,
    pub shortcut: &'static str,
    pub keywords: &'static str,
    pub command: Command,
}

const fn entry(
    label: &'static str,
    shortcut: &'static str,
    keywords: &'static str,
    command: Command,
) -> CommandEntry {
    CommandEntry {
        label,
        shortcut,
        keywords,
        command,
    }
}

/// The palette catalog in declared order.
#[rustfmt::skip]
pub const CATALOG: &[CommandEntry] = &[
    entry("Copy to other panel", "F5", "file copy duplicate transfer send", Command::RequestCopy),
    entry("Move to other panel", "F6", "file move transfer relocate send", Command::RequestMove),
    entry("Move selection into highlighted folder", "Cmd+Enter", "file move selection drop highlighted cursor folder keyboard", Command::MoveIntoCursorFolder),
    entry("Copy selection into highlighted folder", "Cmd+Shift+Enter", "file copy selection drop highlighted cursor folder keyboard", Command::CopyIntoCursorFolder),
    entry("New folder", "F7", "file new folder directory mkdir create", Command::CreateDir),
    entry("New folder with selection", "Cmd+Shift+N", "file new folder with selection gather group move into subfolder", Command::GatherIntoFolder),
    entry("Delete (to Trash)", "F8", "file delete remove trash", Command::RequestDelete),
    entry("Rename", "F2", "file rename edit name", Command::BeginRename),
    entry("Batch rename", "Cmd+Shift+R", "file batch rename bulk multi rename studio", Command::BeginBatchRename),
    entry("Synchronize panels", "Cmd+Shift+S", "panels sync synchronize mirror compare", Command::BeginSync),
    entry("Find duplicates", "", "file duplicate duplicates dedupe identical", Command::FindDuplicates),
    entry("Diff files", "Cmd+D", "view diff compare text", Command::DiffFiles),
    entry("Disk usage map", "Cmd+Shift+M", "view disk usage map treemap size", Command::DiskTreemap),
    entry("Find files", "Cmd+F", "file find recursive search", Command::BeginFind),
    entry("Filter this pane", "Cmd+Shift+F", "filter pane narrow quick search focus", Command::FocusFilter),
    entry("Open saved search", "", "file saved search smart folder", Command::OpenSavedSearch),
    entry("Project collections", "", "project collection workspace roots virtual view", Command::OpenProjectCollections),
    entry("Copy path", "Cmd+Shift+C", "clipboard copy path", Command::CopyPath),
    entry("Copy name", "", "clipboard copy name filename", Command::CopyName),
    entry("Copy name without extension", "", "clipboard copy filename stem basename no suffix extension", Command::CopyNameWithoutExtension),
    entry("Copy parent path", "", "clipboard copy parent folder path", Command::CopyParentPath),
    entry("Copy as file URL", "", "clipboard copy url file url", Command::CopyFileUrl),
    entry("Copy shell-escaped path", "", "clipboard copy shell escaped path", Command::CopyShellPath),
    entry("Copy path relative to other pane", "", "clipboard copy relative path other pane", Command::CopyRelativePath),
    entry("Get Info", "Cmd+I", "view info get info properties metadata", Command::ToggleInfo),
    entry("Go to path", "Cmd+L", "navigation go path location jump", Command::BeginGoToPath),
    entry("Back", "Cmd+[", "navigation back history previous jump return", Command::JumpBack),
    entry("Forward", "Cmd+]", "navigation forward history next jump", Command::JumpForward),
    entry("Bookmark this folder", "", "navigation bookmark favorite add pin folder this", Command::BookmarkCurrentDir),
    entry("Recent folders", "Cmd+P", "navigation recent folders history projects", Command::BeginRecent),
    entry("Select all", "Cmd+A", "selection select all mark all", Command::SelectAll),
    entry("Invert selection", "Cmd+Shift+I", "selection invert reverse flip", Command::InvertSelection),
    entry("Select files also in other panel", "", "selection same name matching files compare", Command::SelectSameNamed),
    entry("Select files only in this panel", "", "selection only here unique missing other panel compare diff", Command::SelectOnlyHere),
    entry("Select files differing from other panel", "", "selection differing changed modified other panel compare diff", Command::SelectDiffering),
    entry("Select files identical to other panel", "", "selection identical same equal other panel compare diff dedupe", Command::SelectIdentical),
    entry("Save selection in memory", "", "selection save remember store buffer marks", Command::SaveSelection),
    entry("Restore saved selection", "", "selection restore recall buffer marks", Command::RestoreSelection),
    entry("Clear selection in all folders", "", "selection clear deselect none reset all folders", Command::ClearSelection),
    entry("Operations panel", "Cmd+J", "activity operations queue progress history errors recovery", Command::ToggleQueuePanel),
    entry("Operation history", "", "history receipts journal log operations undo", Command::OpenReceipts),
    entry("Recovery center", "", "recovery restore journal interrupted resume rollback staging", Command::OpenRecoveryCenter),
    entry("Select clutter files", "", "selection junk clutter ds_store thumbs desktop.ini cleanup cruft", Command::SelectJunk),
    entry("Select 10 largest files", "", "selection largest biggest top size files heavy", Command::SelectLargest),
    entry("Select files like cursor (same extension)", "", "selection same extension like cursor type matching", Command::SelectLikeCursor),
    entry("Select files with cursor's name (ignore extension)", "", "selection same stem basename name without extension", Command::SelectSameStem),
    entry("Deselect files with cursor's name (ignore extension)", "", "selection unselect subtract same stem basename name without extension", Command::DeselectSameStem),
    entry("Deselect files like cursor (same extension)", "", "selection unselect subtract remove same extension type", Command::DeselectLikeCursor),
    entry("Invert visible file selection", "", "selection invert files only toggle flip keep folders", Command::InvertVisibleFiles),
    entry("Invert visible folder selection", "", "selection invert folders directories only toggle flip keep files", Command::InvertVisibleFolders),
    entry("Add visible files to selection", "", "selection select all files only add", Command::SelectVisibleFiles),
    entry("Add visible folders to selection", "", "selection select all folders directories only add", Command::SelectVisibleFolders),
    entry("Select empty files", "", "selection empty zero byte blank files cleanup", Command::SelectEmptyFiles),
    entry("Copy listing as text", "", "clipboard copy listing export text list folder contents", Command::CopyListingText),
    entry("Copy listing as CSV", "", "clipboard copy listing export csv spreadsheet folder", Command::CopyListingCsv),
    entry("Copy listing as Markdown", "", "clipboard copy listing export markdown table folder", Command::CopyListingMarkdown),
    entry("Verify checksum", "", "file checksum hash verify blake sha256 sha-256 expected content integrity digests", Command::VerifyChecksum),
    entry("Change permissions", "", "file permissions chmod mode access read write execute owner group others", Command::EditPermissions),
    entry("Create symlink in other panel", "", "file symlink soft link alias create other panel", Command::CreateSymlink),
    entry("Create hard link in other panel", "", "file hardlink hard link create other panel", Command::CreateHardlink),
    entry("Toggle navigation lock", "", "panels navigation lock sync mirror lockstep dual", Command::ToggleNavLock),
    entry("Repeat last command", ".", "repeat last command again dot redo", Command::RepeatLast),
    entry("View as hex", "", "binary bytes hexadecimal inspect offset viewer", Command::ViewHex),
    entry("Browse archive", "", "archive zip tar gz browse extract inspect members", Command::BrowseArchive),
    entry("Trust: mark Trusted", "", "trust trusted root allow run command providers archive", Command::SetTrustTrusted),
    entry("Trust: mark Restricted", "", "trust restricted root gate providers archive inspect", Command::SetTrustRestricted),
    entry("Trust: mark Untrusted", "", "trust untrusted root block run command providers", Command::SetTrustUntrusted),
    entry("Save workspace profile", "", "workspace profile save roots filters layout remember", Command::SaveWorkspaceProfile),
    entry("Apply workspace profile", "", "workspace profile apply load roots filters restore", Command::ApplyWorkspaceProfile),
    entry("Select by mask", "Cmd+G", "selection mask glob pattern wildcard", Command::BeginSelectMask),
    entry("Run command on selection", "", "run command shell open with terminal execute tool launcher", Command::BeginRunBar),
    entry("Toggle hidden files", "Cmd+H", "view hidden show hidden dotfiles invisible", Command::ToggleHidden),
    entry("Sort: folders first (toggle)", "", "sort folders first dirs top order grouping directories", Command::ToggleFoldersFirst),
    entry("Sort: case-sensitive names (toggle)", "", "sort case sensitive uppercase lowercase Unicode names ordering", Command::ToggleCaseSensitiveSort),
    entry("Sort: folders by name first (toggle)", "", "sort folders directories name first alphabetical independent files", Command::ToggleFoldersByName),
    entry("Sort: natural order (toggle)", "", "sort natural numeric ascii alphabetical order names file10", Command::ToggleNaturalSort),
    entry("Sort by name without extension", "", "sort stem basename name no extension suffix alphabetical order", Command::SortByStem),
    entry("Sort by name", "", "sort name alphabetical filename order a-z", Command::SortByName),
    entry("Sort by size", "", "sort size bytes largest smallest big", Command::SortBySize),
    entry("Sort by modified", "", "sort modified date time newest oldest recent mtime", Command::SortByModified),
    entry("Sort by extension", "", "sort extension type suffix group ext", Command::SortByExtension),
    entry("Sort by kind", "", "sort kind category group images docs code archives", Command::SortByKind),
    entry("Reverse sort order", "", "sort reverse flip order ascending descending invert", Command::ReverseSort),
    entry("Compare panels (toggle)", "", "compare panels differences diff highlight mirror", Command::ToggleCompare),
    entry("New folder tab", "Cmd+T", "view tabs new duplicate pane folder", Command::NewTab),
    entry("Open highlighted folder in new tab", "", "view tabs open folder", Command::OpenFolderTab),
    entry("Open highlighted folder in background tab", "", "view tabs open folder background keep focus", Command::OpenFolderBackgroundTab),
    entry("Open highlighted folder in other pane", "", "navigation directory opposite target browse folder keep focus", Command::OpenFolderOtherPane),
    entry("Copy all folder tabs to other pane", "", "view tabs copy all opposite panel group background", Command::CopyTabsToOther),
    entry("Reopen closed folder tab", "Cmd+Shift+T", "view tabs reopen restore recently closed undo", Command::ReopenClosedTab),
    entry("Protect folder tab from closing", "", "view tabs protect lock keep prevent close", Command::ProtectTab),
    entry("Unprotect folder tab", "", "view tabs unprotect unlock allow close", Command::UnprotectTab),
    entry("Close folder tab", "Cmd+Shift+W", "view tabs close pane", Command::CloseTab),
    entry("Go to folder tab 1", "", "view tabs position index jump 1", Command::GoToTab(0)),
    entry("Go to folder tab 2", "", "view tabs position index jump 2", Command::GoToTab(1)),
    entry("Go to folder tab 3", "", "view tabs position index jump 3", Command::GoToTab(2)),
    entry("Go to folder tab 4", "", "view tabs position index jump 4", Command::GoToTab(3)),
    entry("Go to folder tab 5", "", "view tabs position index jump 5", Command::GoToTab(4)),
    entry("Go to folder tab 6", "", "view tabs position index jump 6", Command::GoToTab(5)),
    entry("Go to folder tab 7", "", "view tabs position index jump 7", Command::GoToTab(6)),
    entry("Go to folder tab 8", "", "view tabs position index jump 8", Command::GoToTab(7)),
    entry("First folder tab", "", "view tabs first beginning jump", Command::FirstTab),
    entry("Last folder tab", "", "view tabs last end jump", Command::LastTab),
    entry("Next folder tab", "Cmd+Shift+]", "view tabs next pane", Command::NextTab),
    entry("Previous folder tab", "Cmd+Shift+[", "view tabs previous pane", Command::PreviousTab),
    entry("Move folder tab left", "", "view tabs reorder move left", Command::MoveTabLeft),
    entry("Move folder tab right", "", "view tabs reorder move right", Command::MoveTabRight),
    entry("Close other folder tabs", "", "view tabs close others keep current protected", Command::CloseOtherTabs),
    entry("Close folder tabs to the left", "", "view tabs close left group protected", Command::CloseTabsLeft),
    entry("Close folder tabs to the right", "", "view tabs close right group protected", Command::CloseTabsRight),
    entry("Close duplicate folder tabs", "", "view tabs deduplicate cleanup duplicate close", Command::CloseDuplicateTabs),
    entry("View as list", "", "view list table browser", Command::ShowListView),
    entry("View as icons", "", "view icons grid thumbnails browser", Command::ShowIconsView),
    entry("Switch light/dark theme", "", "view theme dark light mode appearance colors", Command::ToggleTheme),
    entry("Size bars (toggle)", "", "view size bars graph usage proportion", Command::ToggleSizeBars),
    entry("Folder sidebar (toggle)", "", "view folder sidebar tree navigation outline", Command::ToggleFolderSidebar),
    entry("Cycle density", "Cmd+Shift+D", "view density rows compact comfortable spacious", Command::CycleDensity),
    entry("Keyboard shortcuts", "F1", "help keyboard shortcuts reference", Command::KeyboardHelp),
    entry("Toggle preview", "F3", "view preview quick look viewer inspect", Command::TogglePreview),
    entry("Go to first visible file", "", "navigation cursor first file skip folders start", Command::CursorFirstFile),
    entry("Go to last visible file", "", "navigation cursor last file skip folders end", Command::CursorLastFile),
    entry("Go to home folder", "Cmd+Shift+H", "navigation home user directory personal folder", Command::GoHome),
    entry("Go to disk root", "", "navigation root filesystem drive volume top directory", Command::GoRoot),
    entry("Equalize panels", "Cmd+E", "panels equalize same folder mirror", Command::EqualizePanels),
    entry("Swap panels", "Cmd+U", "panels swap exchange switch sides", Command::SwapPanels),
    entry("Undo", "Cmd+Z", "history undo revert rollback", Command::Undo),
    entry("Redo", "Cmd+Shift+Z", "history redo repeat", Command::Redo),
];

/// A command-palette row: the catalog entry plus the matched character ranges
/// over its label (half-open char indices), so the UI can highlight them.
pub struct CommandMatch {
    pub label: &'static str,
    pub shortcut: &'static str,
    pub command: Command,
    pub matched: Vec<(usize, usize)>,
}

/// How often and how recently a palette command has been run, keyed by label.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Usage {
    pub count: u32,
    /// Monotonic tick of the most recent use (higher == more recent).
    pub last: u64,
}

/// Per-command usage history, persisted so the palette can rank by recency and
/// frequency on top of the fuzzy match.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct UsageStats {
    pub uses: HashMap<String, Usage>,
}

impl UsageStats {
    /// Record a run of `label` at monotonic time `now`.
    pub fn record(&mut self, label: &str, now: u64) {
        let u = self.uses.entry(label.to_string()).or_default();
        u.count += 1;
        u.last = now;
    }
}

/// A modest usage bonus added to the fuzzy score: frequency (up to +12) plus
/// recency (up to +12, decaying over the last ~12 runs). Kept small so a
/// clearly-better fuzzy match still wins, but ties go to the habitual command.
pub fn combined_score(fuzzy: i32, usage: Option<&Usage>, now: u64) -> i32 {
    let Some(u) = usage else {
        return fuzzy;
    };
    let freq = u.count.min(12) as i32;
    let recency = if u.last == 0 {
        0
    } else {
        let age = now.saturating_sub(u.last).min(12) as i32;
        12 - age
    };
    fuzzy + freq + recency
}

/// Rank the command catalog against `query`, blending the fuzzy score with
/// usage. An empty query lists the catalog most-recently-used first (then by
/// count, then declared order). Non-empty keeps fuzzy matches, ordered by the
/// combined score (stable on ties via declared order).
pub fn rank(query: &str, usage: &UsageStats, now: u64) -> Vec<CommandMatch> {
    let to_match = |item: &CommandEntry, matched| CommandMatch {
        label: item.label,
        shortcut: item.shortcut,
        command: item.command,
        matched,
    };

    if query.trim().is_empty() {
        let mut indexed: Vec<(usize, &CommandEntry)> = CATALOG.iter().enumerate().collect();
        indexed.sort_by(|(ia, a), (ib, b)| {
            let ua = usage.uses.get(a.label);
            let ub = usage.uses.get(b.label);
            let (la, ca) = ua.map_or((0, 0), |u| (u.last, u.count));
            let (lb, cb) = ub.map_or((0, 0), |u| (u.last, u.count));
            lb.cmp(&la).then(cb.cmp(&ca)).then(ia.cmp(ib))
        });
        return indexed
            .into_iter()
            .map(|(_, item)| to_match(item, Vec::new()))
            .collect();
    }

    let mut scored: Vec<(i32, usize, CommandMatch)> = Vec::new();
    for (i, item) in CATALOG.iter().enumerate() {
        let mut best: Option<(i32, Vec<(usize, usize)>)> = None;
        if let Some(ms) = crate::fuzzy::score(query, item.label) {
            best = Some((ms.score, ms.matched_ranges));
        }
        if let Some(score) = metadata_score(query, item)
            && best.as_ref().is_none_or(|(current, _)| score > *current)
        {
            // A keyword or shortcut hit only lifts the score: any label
            // ranges the fuzzy pass found stay highlighted.
            let matched = best.map(|(_, ranges)| ranges).unwrap_or_default();
            best = Some((score, matched));
        }
        if let Some((score, matched)) = best {
            let total = combined_score(score, usage.uses.get(item.label), now);
            scored.push((total, i, to_match(item, matched)));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, m)| m).collect()
}

fn metadata_score(query: &str, item: &CommandEntry) -> Option<i32> {
    let shortcut = item.shortcut;
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }

    let compact_q = shortcut_search_text(&q);
    if !shortcut.is_empty() && !compact_q.is_empty() {
        let compact_shortcut = shortcut_search_text(shortcut);
        if compact_shortcut == compact_q {
            return Some(90);
        }
        if shortcut_prefix_matches(shortcut, &compact_q) {
            return Some(65);
        }
    }

    let tokens: Vec<&str> = q.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    // Each query word must start some keyword word, so `re` finds "recent"
    // but not the "re" inside "directory".
    let all_tokens_match = tokens.iter().all(|token| {
        item.keywords
            .split_whitespace()
            .any(|word| word.starts_with(token))
    });
    all_tokens_match.then_some(42 + tokens.len() as i32)
}

/// Whether compact query `compact_q` is a partial shortcut: a prefix of the
/// compact shortcut that ends on a key boundary (`cmd`, `cmdshift`) or runs
/// into the final key. A stray letter such as `s` or `sh` inside
/// `Cmd+Shift+R` is not a shortcut search, so one-character queries never
/// match here.
fn shortcut_prefix_matches(shortcut: &str, compact_q: &str) -> bool {
    if compact_q.chars().count() < 2 {
        return false;
    }
    let keys: Vec<String> = shortcut
        .split(|c: char| c == '+' || c.is_whitespace())
        .filter(|key| !key.is_empty())
        .map(shortcut_search_text)
        .collect();
    let Some((last, modifiers)) = keys.split_last() else {
        return false;
    };
    let mut boundary = String::new();
    for key in modifiers {
        boundary.push_str(key);
        if boundary == compact_q {
            return true;
        }
    }
    // Every modifier typed in full, then the start of the final key.
    compact_q
        .strip_prefix(boundary.as_str())
        .is_some_and(|rest| !rest.is_empty() && last.starts_with(rest))
}

/// Shortcut text with separators removed (`Cmd+Shift+R` and `cmd shift r`
/// both become `cmdshiftr`). Punctuation keys are kept, so `Cmd+[` and
/// `Cmd+]` stay distinct from a bare `cmd` and `.` remains searchable.
fn shortcut_search_text(input: &str) -> String {
    input
        .chars()
        .filter(|&c| c != '+' && !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Rank without usage (pure fuzzy order); the empty-history baseline used in
/// tests.
#[cfg(test)]
pub fn filter_commands(query: &str) -> Vec<CommandMatch> {
    rank(query, &UsageStats::default(), 0)
}

/// The keys the file manager reacts to (UI-toolkit independent).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum KeyCode {
    Tab,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    Backspace,
    Space,
    F1,
    F2,
    F3,
    F5,
    F6,
    F7,
    F8,
    Delete,
    A,
    C,
    E,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    P,
    D,
    F,
    R,
    S,
    U,
    V,
    N,
    Z,
    T,
    W,
    BracketLeft,
    BracketRight,
    /// Period / `.` for vim-style repeat-last-command.
    Period,
    /// Number-row digits 1..9 (0 is intentionally excluded; slots are 1..9).
    Digit(u8),
}

/// A single key press with modifier state.
#[derive(Clone, Copy, Debug)]
pub struct KeyPress {
    pub code: KeyCode,
    /// Cmd (macOS command key) held.
    pub command: bool,
    /// Shift held.
    pub shift: bool,
}

pub const fn map_key(press: KeyPress) -> Option<Command> {
    use KeyCode::*;
    match press.code {
        Tab => Some(Command::SwitchPanel),
        Up if press.shift => Some(Command::ExtendSelectUp),
        Down if press.shift => Some(Command::ExtendSelectDown),
        Home if press.shift => Some(Command::ExtendSelectHome),
        End if press.shift => Some(Command::ExtendSelectEnd),
        PageUp if press.shift => Some(Command::ExtendSelectPageUp),
        PageDown if press.shift => Some(Command::ExtendSelectPageDown),
        Up => Some(Command::CursorUp),
        Down => Some(Command::CursorDown),
        Home => Some(Command::CursorHome),
        End => Some(Command::CursorEnd),
        PageUp => Some(Command::CursorPageUp),
        PageDown => Some(Command::CursorPageDown),
        Enter if press.command && press.shift => Some(Command::CopyIntoCursorFolder),
        Enter if press.command => Some(Command::MoveIntoCursorFolder),
        Enter => Some(Command::Activate),
        Backspace => Some(Command::GoUp),
        T if press.command && press.shift => Some(Command::ReopenClosedTab),
        T if press.command => Some(Command::NewTab),
        W if press.command && press.shift => Some(Command::CloseTab),
        BracketLeft if press.command && press.shift => Some(Command::PreviousTab),
        BracketRight if press.command && press.shift => Some(Command::NextTab),
        BracketLeft if press.command => Some(Command::JumpBack),
        BracketRight if press.command => Some(Command::JumpForward),
        // Cmd+Shift+1..9 assigns the active dir to a slot; Cmd+1..9 jumps to it.
        // A bare digit falls through (None) so type-ahead can use it.
        Digit(n) if press.command && press.shift => Some(Command::AssignSlot(n)),
        Digit(n) if press.command => Some(Command::JumpSlot(n)),
        Space => Some(Command::ToggleSelect),
        // Only a bare `.` repeats; Cmd+. is a system/cancel chord elsewhere.
        Period if !press.command => Some(Command::RepeatLast),
        F1 => Some(Command::KeyboardHelp),
        F2 => Some(Command::BeginRename),
        R if press.command && press.shift => Some(Command::BeginBatchRename),
        R if press.command => Some(Command::BeginRename),
        S if press.command && press.shift => Some(Command::BeginSync),
        D if press.command && press.shift => Some(Command::CycleDensity),
        D if press.command => Some(Command::DiffFiles),
        M if press.command && press.shift => Some(Command::DiskTreemap),
        N if press.command && press.shift => Some(Command::GatherIntoFolder),
        F if press.command && press.shift => Some(Command::FocusFilter),
        F if press.command => Some(Command::BeginFind),
        C if press.command && press.shift => Some(Command::CopyPath),
        F3 => Some(Command::TogglePreview),
        F5 => Some(Command::RequestCopy),
        F6 => Some(Command::RequestMove),
        F7 => Some(Command::CreateDir),
        F8 | Delete => Some(Command::RequestDelete),
        A if press.command => Some(Command::SelectAll),
        E if press.command => Some(Command::EqualizePanels),
        U if press.command => Some(Command::SwapPanels),
        G if press.command => Some(Command::BeginSelectMask),
        I if press.command && press.shift => Some(Command::InvertSelection),
        I if press.command => Some(Command::ToggleInfo),
        L if press.command => Some(Command::BeginGoToPath),
        P if press.command => Some(Command::BeginRecent),
        Z if press.command && press.shift => Some(Command::Redo),
        Z if press.command => Some(Command::Undo),
        J if press.command => Some(Command::ToggleQueuePanel),
        K if press.command => Some(Command::BeginPalette),
        H if press.command && press.shift => Some(Command::GoHome),
        H if press.command => Some(Command::ToggleHidden),
        _ => None,
    }
}

pub fn map_keys(presses: &[KeyPress]) -> Vec<Command> {
    presses.iter().copied().filter_map(map_key).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyPress {
        KeyPress {
            code,
            command: false,
            shift: false,
        }
    }

    fn cmd_press(code: KeyCode) -> KeyPress {
        KeyPress {
            code,
            command: true,
            shift: false,
        }
    }

    fn shift_press(code: KeyCode) -> KeyPress {
        KeyPress {
            code,
            command: false,
            shift: true,
        }
    }

    #[test]
    fn availability_explains_missing_command_context() {
        let context = CommandContext::default();

        assert_eq!(
            availability(Command::RequestCopy, &context).reason,
            Some(PICK_REASON)
        );
        assert_eq!(
            availability(Command::DiffFiles, &context).reason,
            Some("Select two files or a matching file pair")
        );
        assert_eq!(
            availability(Command::Undo, &context).reason,
            Some("Nothing to undo")
        );
        assert_eq!(
            availability(Command::SelectAll, &context).reason,
            Some(VISIBLE_REASON)
        );
    }

    #[test]
    fn availability_prioritizes_safety_and_busy_reasons() {
        let mut context = CommandContext {
            picked_entries: 1,
            selected_entries: 1,
            cursor_entry: true,
            cursor_is_dir: true,
            can_transfer_into_cursor_folder: true,
            ..CommandContext::default()
        };
        assert!(availability(Command::RequestCopy, &context).enabled);

        context.pending_operation = true;
        assert_eq!(
            availability(Command::RequestCopy, &context).reason,
            Some(PENDING_REASON)
        );
        context.safe_state = true;
        assert_eq!(
            availability(Command::RequestCopy, &context).reason,
            Some(SAFE_STATE_REASON)
        );

        context.safe_state = false;
        context.pending_operation = false;
        context.active_mutation = true;
        assert_eq!(
            availability(Command::RequestCopy, &context).reason,
            Some(ACTIVE_MUTATION_REASON)
        );

        context.active_mutation = false;
        context.active_transfer = true;
        context.transfer_queue_busy = true;
        assert!(
            availability(Command::RequestCopy, &context).enabled,
            "copy may queue behind an active transfer"
        );
        assert_eq!(
            availability(Command::RequestDelete, &context).reason,
            Some(QUEUE_REASON)
        );
    }

    #[test]
    fn availability_keeps_close_actions_reachable() {
        let context = CommandContext {
            preview_open: true,
            info_open: true,
            ..CommandContext::default()
        };

        assert!(availability(Command::TogglePreview, &context).enabled);
        assert!(availability(Command::ToggleInfo, &context).enabled);
    }

    #[test]
    fn commands_compose_typed_predicates_in_priority_order() {
        assert_eq!(
            predicates(Command::RequestDelete),
            &[
                CommandPredicate::MutationsAllowed,
                CommandPredicate::ConfirmationClosed,
                CommandPredicate::TransferQueueIdle,
                CommandPredicate::HasPickedEntry,
                CommandPredicate::ActiveWritable,
            ]
        );
        assert_eq!(
            predicates(Command::SelectDiffering),
            &[
                CommandPredicate::HasVisibleEntry,
                CommandPredicate::HasOtherEntry,
            ]
        );
        assert!(!requires_context(Command::CursorDown));
        assert!(requires_context(Command::RequestDelete));
    }

    #[test]
    fn capability_predicates_gate_only_the_required_panes() {
        let mut context = CommandContext {
            picked_entries: 1,
            cursor_entry: true,
            ..CommandContext::default()
        };
        context.inactive_read_only = true;
        assert_eq!(
            availability(Command::RequestCopy, &context).reason,
            Some("The destination panel is read-only")
        );
        assert!(availability(Command::RequestDelete, &context).enabled);

        context.inactive_read_only = false;
        context.active_read_only = true;
        assert!(availability(Command::RequestCopy, &context).enabled);
        assert_eq!(
            availability(Command::RequestMove, &context).reason,
            Some("The active panel is read-only")
        );
        assert_eq!(
            availability(Command::RequestDelete, &context).reason,
            Some("The active panel is read-only")
        );

        context.inactive_read_only = true;
        assert_eq!(
            availability(Command::BeginSync, &context).reason,
            Some("Both panels are read-only")
        );
    }

    #[test]
    fn contextual_shortcuts_only_describe_available_next_actions() {
        let mut context = CommandContext {
            cursor_entry: true,
            cursor_is_file: true,
            picked_entries: 1,
            can_go_up: true,
            ..CommandContext::default()
        };
        let hints = contextual_shortcuts(&context);
        assert!(hints.iter().any(|hint| hint.label == "Preview"));
        assert!(
            hints
                .iter()
                .any(|hint| hint.command == Command::RequestCopy)
        );
        assert!(
            hints
                .iter()
                .any(|hint| hint.command == Command::RequestDelete)
        );
        assert!(!hints.iter().any(|hint| hint.command == Command::Undo));

        context.preview_open = true;
        context.cursor_entry = false;
        context.cursor_is_file = false;
        context.picked_entries = 0;
        let hints = contextual_shortcuts(&context);
        assert!(hints.iter().any(|hint| hint.label == "Close preview"));
        assert!(
            !hints
                .iter()
                .any(|hint| hint.command == Command::RequestCopy)
        );
        assert!(
            !hints
                .iter()
                .any(|hint| hint.command == Command::RequestDelete)
        );

        context.active_transfer = true;
        context.picked_entries = 1;
        let hints = contextual_shortcuts(&context);
        assert!(
            hints
                .iter()
                .all(|hint| matches!(hint.command, Command::RequestCopy | Command::RequestMove))
        );
    }

    #[test]
    fn space_hint_names_the_toggle_it_would_perform() {
        let space_label = |cursor_selected| {
            let context = CommandContext {
                cursor_entry: true,
                cursor_is_file: true,
                cursor_selected,
                picked_entries: 1,
                ..CommandContext::default()
            };
            contextual_shortcuts(&context)
                .into_iter()
                .find(|hint| hint.command == Command::ToggleSelect)
                .map(|hint| hint.label)
        };
        assert_eq!(space_label(false), Some("Select"));
        assert_eq!(space_label(true), Some("Deselect"));
    }

    #[test]
    fn every_catalog_availability_has_a_consistent_reason() {
        let context = CommandContext::default();
        for &CommandEntry { label, command, .. } in CATALOG {
            let availability = availability(command, &context);
            assert_eq!(
                availability.enabled,
                availability.reason.is_none(),
                "inconsistent availability for {label}"
            );
        }
    }

    #[test]
    fn cmd_digits_are_quick_jump_slots() {
        assert_eq!(
            map_key(cmd_press(KeyCode::Digit(1))),
            Some(Command::JumpSlot(1))
        );
        let cmd_shift_3 = KeyPress {
            code: KeyCode::Digit(3),
            command: true,
            shift: true,
        };
        assert_eq!(map_key(cmd_shift_3), Some(Command::AssignSlot(3)));
        // A bare digit is not a command, so type-ahead can use it.
        assert_eq!(map_key(press(KeyCode::Digit(1))), None);
    }

    #[test]
    fn cmd_brackets_walk_history() {
        assert_eq!(
            map_key(cmd_press(KeyCode::BracketLeft)),
            Some(Command::JumpBack)
        );
        assert_eq!(
            map_key(cmd_press(KeyCode::BracketRight)),
            Some(Command::JumpForward)
        );
        // Without Cmd the brackets are not history navigation.
        assert_eq!(map_key(press(KeyCode::BracketLeft)), None);
    }

    #[test]
    fn function_keys_map_to_file_ops() {
        assert_eq!(map_key(press(KeyCode::F5)), Some(Command::RequestCopy));
        assert_eq!(map_key(press(KeyCode::F6)), Some(Command::RequestMove));
        assert_eq!(map_key(press(KeyCode::F7)), Some(Command::CreateDir));
        assert_eq!(map_key(press(KeyCode::F8)), Some(Command::RequestDelete));
        assert_eq!(
            map_key(press(KeyCode::Delete)),
            Some(Command::RequestDelete)
        );
    }

    #[test]
    fn letter_keys_require_command_modifier() {
        assert_eq!(map_key(press(KeyCode::A)), None);
        assert_eq!(map_key(press(KeyCode::H)), None);
        assert_eq!(map_key(cmd_press(KeyCode::A)), Some(Command::SelectAll));
        assert_eq!(map_key(cmd_press(KeyCode::H)), Some(Command::ToggleHidden));
    }

    #[test]
    fn cmd_i_is_info_but_cmd_shift_i_inverts_selection() {
        assert_eq!(map_key(cmd_press(KeyCode::I)), Some(Command::ToggleInfo));
        let cmd_shift_i = KeyPress {
            code: KeyCode::I,
            command: true,
            shift: true,
        };
        assert_eq!(map_key(cmd_shift_i), Some(Command::InvertSelection));
    }

    #[test]
    fn map_keys_preserves_order_and_drops_unmapped() {
        let cmds = map_keys(&[
            press(KeyCode::Tab),
            press(KeyCode::A), // no modifier — dropped
            press(KeyCode::Down),
        ]);
        assert_eq!(cmds, vec![Command::SwitchPanel, Command::CursorDown]);
    }

    #[test]
    fn navigation_keys_map() {
        assert_eq!(map_key(press(KeyCode::Home)), Some(Command::CursorHome));
        assert_eq!(map_key(press(KeyCode::End)), Some(Command::CursorEnd));
        assert_eq!(map_key(press(KeyCode::PageUp)), Some(Command::CursorPageUp));
        assert_eq!(
            map_key(press(KeyCode::PageDown)),
            Some(Command::CursorPageDown)
        );
    }

    #[test]
    fn command_enter_maps_to_keyboard_drop_while_enter_still_activates() {
        assert_eq!(
            map_key(cmd_press(KeyCode::Enter)),
            Some(Command::MoveIntoCursorFolder)
        );
        assert_eq!(map_key(press(KeyCode::Enter)), Some(Command::Activate));
        assert_eq!(
            map_key(KeyPress {
                code: KeyCode::Enter,
                command: true,
                shift: true,
            }),
            Some(Command::CopyIntoCursorFolder)
        );
    }

    #[test]
    fn panel_sync_binds_to_cmd_e_and_cmd_u() {
        assert_eq!(
            map_key(cmd_press(KeyCode::E)),
            Some(Command::EqualizePanels)
        );
        assert_eq!(map_key(cmd_press(KeyCode::U)), Some(Command::SwapPanels));
        assert_eq!(
            map_key(cmd_press(KeyCode::G)),
            Some(Command::BeginSelectMask)
        );
        // Plain E/U/G type text (type-ahead), not panel commands.
        assert_eq!(map_key(press(KeyCode::E)), None);
        assert_eq!(map_key(press(KeyCode::U)), None);
        assert_eq!(map_key(press(KeyCode::G)), None);
    }

    #[test]
    fn undo_binds_to_cmd_z_and_redo_to_cmd_shift_z() {
        assert_eq!(map_key(cmd_press(KeyCode::Z)), Some(Command::Undo));
        let cmd_shift_z = KeyPress {
            code: KeyCode::Z,
            command: true,
            shift: true,
        };
        assert_eq!(map_key(cmd_shift_z), Some(Command::Redo));
        assert_eq!(map_key(press(KeyCode::Z)), None);
    }

    #[test]
    fn palette_binds_to_cmd_k() {
        assert_eq!(map_key(cmd_press(KeyCode::K)), Some(Command::BeginPalette));
        assert_eq!(map_key(press(KeyCode::K)), None);
    }

    #[test]
    fn filter_commands_ranks_by_fuzzy_relevance() {
        // Empty query (no usage) returns the whole catalog in declared order.
        let all = filter_commands("");
        assert_eq!(all.len(), CATALOG.len());
        assert_eq!(all[0].command, Command::RequestCopy);

        // The best fuzzy match leads the list.
        assert_eq!(filter_commands("move")[0].command, Command::RequestMove);
        assert_eq!(filter_commands("SWAP")[0].command, Command::SwapPanels);

        // A non-subsequence query matches nothing.
        assert!(filter_commands("zzzzz").is_empty());
    }

    #[test]
    fn filter_commands_matches_shortcuts_and_aliases() {
        assert_eq!(filter_commands("cmd h")[0].command, Command::ToggleHidden);
        assert_eq!(filter_commands("mkdir")[0].command, Command::CreateDir);
        assert_eq!(
            filter_commands("view hidden")[0].command,
            Command::ToggleHidden
        );
        assert_eq!(
            filter_commands("cmd shift d")[0].command,
            Command::CycleDensity
        );
    }

    fn catalog_entry(command: Command) -> &'static CommandEntry {
        CATALOG
            .iter()
            .find(|entry| entry.command == command)
            .unwrap()
    }

    #[test]
    fn stray_letters_do_not_match_inside_shortcuts() {
        let batch = catalog_entry(Command::BeginBatchRename); // Cmd+Shift+R
        assert_eq!(metadata_score("sh", batch), None);
        assert_eq!(
            metadata_score("md", catalog_entry(Command::DiffFiles)),
            None
        );
        // Whole modifiers, or modifiers plus the start of the key, still do.
        assert_eq!(metadata_score("cmd shift", batch), Some(65));
        assert_eq!(metadata_score("cmdshiftr", batch), Some(90));
        assert_eq!(
            metadata_score("f5", catalog_entry(Command::RequestCopy)),
            Some(90)
        );
    }

    #[test]
    fn keywords_match_by_word_prefix() {
        let new_folder = catalog_entry(Command::CreateDir);
        // "re" sits inside "directory" and "create" but starts no keyword.
        assert_eq!(metadata_score("re", new_folder), None);
        assert_eq!(metadata_score("mk", new_folder), Some(43));
        assert_eq!(
            metadata_score("receipts", catalog_entry(Command::OpenReceipts)),
            Some(43)
        );
        assert_eq!(
            filter_commands("journal interrupted")[0].command,
            Command::OpenRecoveryCenter
        );
    }

    #[test]
    fn keyword_wins_keep_the_label_highlight() {
        // "hid" scores higher as a keyword than as a fuzzy label match, but
        // the label's matched letters stay highlighted.
        let row = filter_commands("hid")
            .into_iter()
            .find(|m| m.command == Command::ToggleHidden)
            .unwrap();
        assert!(!row.matched.is_empty());
    }

    #[test]
    fn browse_archive_needs_a_highlighted_file() {
        let mut context = CommandContext {
            picked_entries: 1,
            selected_entries: 1,
            ..CommandContext::default()
        };
        assert_eq!(
            availability(Command::BrowseArchive, &context).reason,
            Some(CURSOR_REASON)
        );
        context.cursor_entry = true;
        context.cursor_is_dir = true;
        assert_eq!(
            availability(Command::BrowseArchive, &context).reason,
            Some("Highlight a ZIP, TAR or tar.gz file")
        );
        context.cursor_is_dir = false;
        context.cursor_is_file = true;
        assert!(availability(Command::BrowseArchive, &context).enabled);
    }

    #[test]
    fn shortcut_punctuation_is_part_of_the_match() {
        // A bare modifier is not an exact match for Back/Forward.
        let back = CATALOG
            .iter()
            .find(|entry| entry.command == Command::JumpBack)
            .unwrap();
        assert!(metadata_score("cmd", back).unwrap() < 90);
        assert_eq!(metadata_score("cmd+[", back), Some(90));
        let repeat = CATALOG
            .iter()
            .find(|entry| entry.command == Command::RepeatLast)
            .unwrap();
        assert_eq!(metadata_score(".", repeat), Some(90));
        assert_eq!(filter_commands("cmd [")[0].command, Command::JumpBack);
        assert_eq!(filter_commands("cmd ]")[0].command, Command::JumpForward);
    }

    #[test]
    fn modified_period_does_not_repeat_last_command() {
        assert_eq!(map_key(press(KeyCode::Period)), Some(Command::RepeatLast));
        let cmd_period = KeyPress {
            code: KeyCode::Period,
            command: true,
            shift: false,
        };
        assert_eq!(map_key(cmd_period), None);
    }

    #[test]
    fn combined_score_lets_habit_break_close_ties_not_clear_wins() {
        let frequent_recent = Usage {
            count: 5,
            last: 100,
        };
        let now = 100;
        // A habitual command with a weaker fuzzy score beats an unused one that
        // is only slightly better (gap within the usage bonus).
        assert!(combined_score(8, Some(&frequent_recent), now) > combined_score(18, None, now));
        // But a clearly-better fuzzy match still wins.
        assert!(combined_score(8, Some(&frequent_recent), now) < combined_score(40, None, now));
        // No usage -> the fuzzy score is unchanged.
        assert_eq!(combined_score(25, None, now), 25);
    }

    #[test]
    fn empty_query_lists_most_recently_used_first() {
        let mut usage = UsageStats::default();
        usage.record("Swap panels", 1);
        usage.record("Find files", 2); // more recent
        let ranked = rank("", &usage, 2);
        // The two used commands lead, most-recent first.
        assert_eq!(ranked[0].label, "Find files");
        assert_eq!(ranked[1].label, "Swap panels");
        // Determinism.
        let again = rank("", &usage, 2);
        let a: Vec<&str> = ranked.iter().map(|m| m.label).collect();
        let b: Vec<&str> = again.iter().map(|m| m.label).collect();
        assert_eq!(a, b);
    }

    #[test]
    fn unused_commands_fall_back_to_fuzzy_order() {
        let usage = UsageStats::default();
        let with_rank: Vec<Command> = rank("move", &usage, 0).iter().map(|m| m.command).collect();
        let with_fuzzy: Vec<Command> = filter_commands("move").iter().map(|m| m.command).collect();
        assert_eq!(with_rank, with_fuzzy);
    }

    #[test]
    fn rename_binds_to_f2_and_cmd_r() {
        assert_eq!(map_key(press(KeyCode::F2)), Some(Command::BeginRename));
        assert_eq!(map_key(cmd_press(KeyCode::R)), Some(Command::BeginRename));
        // Plain R types text; it must not trigger rename.
        assert_eq!(map_key(press(KeyCode::R)), None);
    }

    #[test]
    fn cmd_shift_r_is_batch_rename_distinct_from_cmd_r() {
        let cmd_shift_r = KeyPress {
            code: KeyCode::R,
            command: true,
            shift: true,
        };
        assert_eq!(map_key(cmd_shift_r), Some(Command::BeginBatchRename));
        // Cmd+R without shift stays single rename.
        assert_eq!(map_key(cmd_press(KeyCode::R)), Some(Command::BeginRename));
    }

    #[test]
    fn cmd_shift_c_copies_path() {
        let cmd_shift_c = KeyPress {
            code: KeyCode::C,
            command: true,
            shift: true,
        };
        assert_eq!(map_key(cmd_shift_c), Some(Command::CopyPath));
        assert_eq!(map_key(press(KeyCode::C)), None);
    }

    #[test]
    fn cmd_f_opens_find() {
        assert_eq!(map_key(cmd_press(KeyCode::F)), Some(Command::BeginFind));
        assert_eq!(map_key(press(KeyCode::F)), None);
    }

    #[test]
    fn cmd_shift_f_focuses_the_pane_filter() {
        let cmd_shift_f = KeyPress {
            code: KeyCode::F,
            command: true,
            shift: true,
        };
        assert_eq!(map_key(cmd_shift_f), Some(Command::FocusFilter));
        assert_eq!(
            filter_commands("filter pane")[0].command,
            Command::FocusFilter
        );
    }

    #[test]
    fn cmd_shift_m_opens_treemap() {
        let cmd_shift_m = KeyPress {
            code: KeyCode::M,
            command: true,
            shift: true,
        };
        assert_eq!(map_key(cmd_shift_m), Some(Command::DiskTreemap));
        // Bare M (no modifiers) is free for type-ahead.
        assert_eq!(map_key(press(KeyCode::M)), None);
    }

    #[test]
    fn cmd_d_diffs_and_cmd_shift_d_cycles_density() {
        let cmd_shift_d = KeyPress {
            code: KeyCode::D,
            command: true,
            shift: true,
        };
        assert_eq!(map_key(cmd_shift_d), Some(Command::CycleDensity));
        // Cmd+D (no shift) is the diff; plain D types text.
        assert_eq!(map_key(cmd_press(KeyCode::D)), Some(Command::DiffFiles));
        assert_eq!(map_key(press(KeyCode::D)), None);
    }

    #[test]
    fn cmd_shift_s_is_synchronize() {
        let cmd_shift_s = KeyPress {
            code: KeyCode::S,
            command: true,
            shift: true,
        };
        assert_eq!(map_key(cmd_shift_s), Some(Command::BeginSync));
        // Plain S and Cmd+S type / are free; they must not sync.
        assert_eq!(map_key(press(KeyCode::S)), None);
        assert_eq!(map_key(cmd_press(KeyCode::S)), None);
    }

    #[test]
    fn shift_arrows_extend_selection() {
        assert_eq!(map_key(press(KeyCode::Up)), Some(Command::CursorUp));
        assert_eq!(
            map_key(shift_press(KeyCode::Up)),
            Some(Command::ExtendSelectUp)
        );
        assert_eq!(
            map_key(shift_press(KeyCode::Down)),
            Some(Command::ExtendSelectDown)
        );
        for (code, extend, plain) in [
            (
                KeyCode::Home,
                Command::ExtendSelectHome,
                Command::CursorHome,
            ),
            (KeyCode::End, Command::ExtendSelectEnd, Command::CursorEnd),
            (
                KeyCode::PageUp,
                Command::ExtendSelectPageUp,
                Command::CursorPageUp,
            ),
            (
                KeyCode::PageDown,
                Command::ExtendSelectPageDown,
                Command::CursorPageDown,
            ),
        ] {
            assert_eq!(map_key(shift_press(code)), Some(extend));
            assert_eq!(map_key(press(code)), Some(plain));
        }
    }

    #[test]
    fn mutating_commands_are_exactly_the_mutation_gated_ones() {
        let mutating = [
            Command::EditPermissions,
            Command::RequestCopy,
            Command::RequestMove,
            Command::CreateDir,
            Command::RequestDelete,
            Command::BeginRename,
            Command::BeginBatchRename,
            Command::BeginSync,
            Command::BeginRunBar,
            Command::GatherIntoFolder,
            Command::MoveIntoCursorFolder,
            Command::CopyIntoCursorFolder,
            Command::CreateSymlink,
            Command::CreateHardlink,
            Command::Undo,
            Command::Redo,
        ];
        for command in mutating {
            assert!(command.mutates_filesystem(), "{command:?}");
        }
        for entry in CATALOG {
            assert_eq!(
                entry.command.mutates_filesystem(),
                mutating.contains(&entry.command),
                "{}",
                entry.label
            );
        }
        for command in [
            Command::CursorDown,
            Command::Activate,
            Command::JumpSlot(1),
            Command::AssignSlot(1),
            Command::BeginPalette,
        ] {
            assert!(!command.mutates_filesystem(), "{command:?}");
        }
    }

    #[test]
    fn shifted_new_tab_shortcut_restores_instead_of_creating_a_tab() {
        assert_eq!(
            map_key(KeyPress {
                code: KeyCode::T,
                command: true,
                shift: false
            }),
            Some(Command::NewTab)
        );
        assert_eq!(
            map_key(KeyPress {
                code: KeyCode::T,
                command: true,
                shift: true
            }),
            Some(Command::ReopenClosedTab)
        );
    }

    #[test]
    fn catalog_labels_and_commands_are_unique() {
        let mut labels = std::collections::HashSet::new();
        let mut commands = Vec::new();
        for entry in CATALOG {
            // Usage stats are keyed by label, so a duplicate would merge counts.
            assert!(
                labels.insert(entry.label),
                "duplicate label {}",
                entry.label
            );
            assert!(
                !commands.contains(&entry.command),
                "{:?} is listed twice",
                entry.command
            );
            commands.push(entry.command);
        }
    }

    /// Parses a catalog shortcut hint such as `Cmd+Shift+Enter` into the
    /// key press it describes.
    fn parse_shortcut(shortcut: &str) -> KeyPress {
        let mut press = press(KeyCode::Tab);
        let keys: Vec<&str> = if shortcut == "." {
            vec!["."]
        } else {
            shortcut.split('+').collect()
        };
        let (key, modifiers) = keys.split_last().unwrap();
        for modifier in modifiers {
            match *modifier {
                "Cmd" => press.command = true,
                "Shift" => press.shift = true,
                other => panic!("unknown modifier {other} in {shortcut}"),
            }
        }
        press.code = match *key {
            "F1" => KeyCode::F1,
            "F2" => KeyCode::F2,
            "F3" => KeyCode::F3,
            "F5" => KeyCode::F5,
            "F6" => KeyCode::F6,
            "F7" => KeyCode::F7,
            "F8" => KeyCode::F8,
            "Enter" => KeyCode::Enter,
            "[" => KeyCode::BracketLeft,
            "]" => KeyCode::BracketRight,
            "." => KeyCode::Period,
            "A" => KeyCode::A,
            "C" => KeyCode::C,
            "D" => KeyCode::D,
            "E" => KeyCode::E,
            "F" => KeyCode::F,
            "G" => KeyCode::G,
            "H" => KeyCode::H,
            "I" => KeyCode::I,
            "J" => KeyCode::J,
            "K" => KeyCode::K,
            "L" => KeyCode::L,
            "M" => KeyCode::M,
            "N" => KeyCode::N,
            "P" => KeyCode::P,
            "R" => KeyCode::R,
            "S" => KeyCode::S,
            "T" => KeyCode::T,
            "W" => KeyCode::W,
            "U" => KeyCode::U,
            "Z" => KeyCode::Z,
            other => panic!("unknown key {other} in {shortcut}"),
        };
        press
    }

    #[test]
    fn catalog_shortcut_hints_match_the_key_bindings() {
        for entry in CATALOG.iter().filter(|entry| !entry.shortcut.is_empty()) {
            assert_eq!(
                map_key(parse_shortcut(entry.shortcut)),
                Some(entry.command),
                "{} advertises {}",
                entry.label,
                entry.shortcut
            );
        }
    }

    #[test]
    fn activate_needs_a_cursor_entry_or_a_parent_folder() {
        let mut context = CommandContext::default();
        assert_eq!(
            availability(Command::Activate, &context).reason,
            Some("No item to open")
        );
        // An empty folder can still be left with Enter on "..".
        context.can_go_up = true;
        assert!(availability(Command::Activate, &context).enabled);
        context.can_go_up = false;
        context.cursor_entry = true;
        assert!(availability(Command::Activate, &context).enabled);
    }

    #[test]
    fn sync_needs_only_one_writable_pane() {
        let mut context = CommandContext {
            active_read_only: true,
            ..CommandContext::default()
        };
        assert!(availability(Command::BeginSync, &context).enabled);
        context.active_read_only = false;
        context.inactive_read_only = true;
        assert!(availability(Command::BeginSync, &context).enabled);
    }

    #[test]
    fn transfer_into_folder_explains_each_missing_target() {
        let mut context = CommandContext {
            picked_entries: 1,
            selected_entries: 1,
            cursor_entry: true,
            cursor_is_file: true,
            ..CommandContext::default()
        };
        for command in [Command::MoveIntoCursorFolder, Command::CopyIntoCursorFolder] {
            assert_eq!(
                availability(command, &context).reason,
                Some("Highlight a destination folder")
            );
        }
        context.cursor_is_file = false;
        context.cursor_is_dir = true;
        for command in [Command::MoveIntoCursorFolder, Command::CopyIntoCursorFolder] {
            assert_eq!(
                availability(command, &context).reason,
                Some("Choose a source outside the destination folder")
            );
        }
        context.can_transfer_into_cursor_folder = true;
        assert!(availability(Command::MoveIntoCursorFolder, &context).enabled);
        context.picked_entries = 0;
        assert_eq!(
            availability(Command::CopyIntoCursorFolder, &context).reason,
            Some(PICK_REASON)
        );
    }

    #[test]
    fn combined_score_caps_frequency_and_recency() {
        let heavy = Usage {
            count: 50,
            last: 100,
        };
        // Both bonuses top out at 12, however often the command ran.
        assert_eq!(combined_score(0, Some(&heavy), 100), 24);
        assert_eq!(
            combined_score(0, Some(&heavy), 100),
            combined_score(
                0,
                Some(&Usage {
                    count: 12,
                    last: 100
                }),
                100
            )
        );
        // Recency decays to nothing after 12 ticks and stays there.
        assert_eq!(combined_score(0, Some(&heavy), 112), 12);
        assert_eq!(combined_score(0, Some(&heavy), 10_000), 12);
        assert_eq!(combined_score(0, Some(&heavy), 105), 19);
        // A never-run tick (0) earns no recency.
        let never = Usage { count: 3, last: 0 };
        assert_eq!(combined_score(5, Some(&never), 1), 8);
    }
}
