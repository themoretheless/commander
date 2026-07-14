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
    /// Enter: open file / enter dir / go up on the ".." row.
    Activate,
    GoUp,
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
    /// Open the saved-search (smart folder) picker.
    OpenSavedSearch,
    /// Open named multi-root project collections.
    OpenProjectCollections,
    /// Cmd+Shift+C: copy the selection's full path(s) to the clipboard.
    CopyPath,
    /// Copy the selection's file name(s).
    CopyName,
    /// Copy the selection's parent directory path(s).
    CopyParentPath,
    /// Copy the selection as `file://` URL(s).
    CopyFileUrl,
    /// Copy the selection's path(s), shell-escaped.
    CopyShellPath,
    /// Copy the selection's path(s) relative to the other pane.
    CopyRelativePath,
    /// Cmd+Shift+A: add the active selection to the shelf (drop stack).
    ShelfAdd,
    /// Cmd+Shift+V: copy the whole shelf into the active panel's folder.
    ShelfDrain,
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
    /// Stash the current selection for later set-algebra combinations.
    StashSelection,
    /// Replace the selection with (selection ∪ stash).
    StashUnion,
    /// Replace the selection with (selection ∩ stash).
    StashIntersect,
    /// Replace the selection with (selection - stash).
    StashSubtract,
    /// Replace the selection with (selection symmetric-difference stash).
    StashSymmetricDiff,
    /// M: flip whether the cursor entry is in the mark set (distinct from
    /// `selected`; survives navigation and feeds the selection algebra).
    ToggleMark,
    /// Clear all marks in the active panel.
    ClearMarks,
    /// Replace the selection with (selection ∪ marked).
    MarkedUnion,
    /// Replace the selection with (selection ∩ marked).
    MarkedIntersect,
    /// Replace the selection with (selection - marked).
    MarkedSubtract,
    /// Replace the selection with (selection symmetric-difference marked).
    MarkedSymmetricDiff,
    /// Show/hide the transfer-queue panel (pause/resume/reorder/cancel the
    /// jobs waiting behind the active transfer).
    ToggleQueuePanel,
    /// Open the searchable history of completed moves/deletes/batch-renames.
    OpenReceipts,
    /// Open interrupted-operation recovery, rollback, and staging cleanup.
    OpenRecoveryCenter,
    ToggleHidden,
    /// Toggle whether folders are pinned to the top of the active listing.
    ToggleFoldersFirst,
    /// Toggle natural (`file2` < `file10`) vs plain A-Z name ordering.
    ToggleNaturalSort,
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
    /// Select zero-byte files in the filtered view.
    SelectEmptyFiles,
    /// Copy the active listing to the clipboard as plain text.
    CopyListingText,
    /// Copy the active listing to the clipboard as CSV.
    CopyListingCsv,
    /// Copy the active listing to the clipboard as a Markdown table.
    CopyListingMarkdown,
}

impl Command {
    pub fn mutates_filesystem(self) -> bool {
        matches!(
            self,
            Self::RequestCopy
                | Self::RequestMove
                | Self::CreateDir
                | Self::RequestDelete
                | Self::BeginRename
                | Self::BeginBatchRename
                | Self::BeginSync
                | Self::BeginRunBar
                | Self::GatherIntoFolder
                | Self::MoveIntoCursorFolder
                | Self::Undo
                | Self::Redo
                | Self::ShelfDrain
        )
    }
}

/// User-facing commands for the Cmd+K palette: (label, shortcut, command).
pub fn command_catalog() -> Vec<(&'static str, &'static str, Command)> {
    vec![
        ("Copy to other panel", "F5", Command::RequestCopy),
        ("Move to other panel", "F6", Command::RequestMove),
        (
            "Move selection into highlighted folder",
            "Cmd+Enter",
            Command::MoveIntoCursorFolder,
        ),
        ("New folder", "F7", Command::CreateDir),
        (
            "New folder with selection",
            "Cmd+Shift+N",
            Command::GatherIntoFolder,
        ),
        ("Delete (to Trash)", "F8", Command::RequestDelete),
        ("Rename", "F2", Command::BeginRename),
        ("Batch rename", "Cmd+Shift+R", Command::BeginBatchRename),
        ("Synchronize panels", "Cmd+Shift+S", Command::BeginSync),
        ("Find duplicates", "", Command::FindDuplicates),
        ("Diff files", "Cmd+D", Command::DiffFiles),
        ("Disk usage map", "Cmd+Shift+M", Command::DiskTreemap),
        ("Find files", "Cmd+F", Command::BeginFind),
        ("Open saved search", "", Command::OpenSavedSearch),
        ("Project collections", "", Command::OpenProjectCollections),
        ("Copy path", "Cmd+Shift+C", Command::CopyPath),
        ("Copy name", "", Command::CopyName),
        ("Copy parent path", "", Command::CopyParentPath),
        ("Copy as file URL", "", Command::CopyFileUrl),
        ("Copy shell-escaped path", "", Command::CopyShellPath),
        (
            "Copy path relative to other pane",
            "",
            Command::CopyRelativePath,
        ),
        ("Add to shelf", "Cmd+Shift+A", Command::ShelfAdd),
        ("Drain shelf here", "Cmd+Shift+V", Command::ShelfDrain),
        ("Get Info", "Cmd+I", Command::ToggleInfo),
        ("Go to path", "Cmd+L", Command::BeginGoToPath),
        ("Back", "Cmd+[", Command::JumpBack),
        ("Forward", "Cmd+]", Command::JumpForward),
        ("Bookmark this folder", "", Command::BookmarkCurrentDir),
        ("Recent folders", "Cmd+P", Command::BeginRecent),
        ("Select all", "Cmd+A", Command::SelectAll),
        ("Invert selection", "Cmd+Shift+I", Command::InvertSelection),
        (
            "Select files also in other panel",
            "",
            Command::SelectSameNamed,
        ),
        (
            "Select files only in this panel",
            "",
            Command::SelectOnlyHere,
        ),
        (
            "Select files differing from other panel",
            "",
            Command::SelectDiffering,
        ),
        (
            "Select files identical to other panel",
            "",
            Command::SelectIdentical,
        ),
        ("Stash selection", "", Command::StashSelection),
        ("Selection: union with stash", "", Command::StashUnion),
        (
            "Selection: intersect with stash",
            "",
            Command::StashIntersect,
        ),
        ("Selection: subtract stash", "", Command::StashSubtract),
        (
            "Selection: symmetric difference with stash",
            "",
            Command::StashSymmetricDiff,
        ),
        ("Toggle mark", "M", Command::ToggleMark),
        ("Clear marks", "", Command::ClearMarks),
        ("Selection: union with marked", "", Command::MarkedUnion),
        (
            "Selection: intersect with marked",
            "",
            Command::MarkedIntersect,
        ),
        ("Selection: subtract marked", "", Command::MarkedSubtract),
        (
            "Selection: symmetric difference with marked",
            "",
            Command::MarkedSymmetricDiff,
        ),
        ("Transfer queue", "", Command::ToggleQueuePanel),
        ("Operation history", "", Command::OpenReceipts),
        ("Recovery center", "", Command::OpenRecoveryCenter),
        ("Select clutter files", "", Command::SelectJunk),
        ("Select 10 largest files", "", Command::SelectLargest),
        (
            "Select files like cursor (same extension)",
            "",
            Command::SelectLikeCursor,
        ),
        ("Select empty files", "", Command::SelectEmptyFiles),
        ("Copy listing as text", "", Command::CopyListingText),
        ("Copy listing as CSV", "", Command::CopyListingCsv),
        ("Copy listing as Markdown", "", Command::CopyListingMarkdown),
        ("Select by mask", "Cmd+G", Command::BeginSelectMask),
        ("Run command on selection", "", Command::BeginRunBar),
        ("Toggle hidden files", "Cmd+H", Command::ToggleHidden),
        (
            "Sort: folders first (toggle)",
            "",
            Command::ToggleFoldersFirst,
        ),
        (
            "Sort: natural order (toggle)",
            "",
            Command::ToggleNaturalSort,
        ),
        ("Sort by extension", "", Command::SortByExtension),
        ("Sort by kind", "", Command::SortByKind),
        ("Reverse sort order", "", Command::ReverseSort),
        ("Cycle density", "Cmd+Shift+D", Command::CycleDensity),
        ("Toggle preview", "F3", Command::TogglePreview),
        ("Equalize panels", "Cmd+E", Command::EqualizePanels),
        ("Swap panels", "Cmd+U", Command::SwapPanels),
        ("Undo", "Cmd+Z", Command::Undo),
        ("Redo", "Cmd+Shift+Z", Command::Redo),
    ]
}

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
    let catalog = command_catalog();
    let to_match =
        |(label, shortcut, command): (&'static str, &'static str, Command), matched| CommandMatch {
            label,
            shortcut,
            command,
            matched,
        };

    if query.trim().is_empty() {
        let mut indexed: Vec<(usize, (&'static str, &'static str, Command))> =
            catalog.into_iter().enumerate().collect();
        indexed.sort_by(|(ia, a), (ib, b)| {
            let ua = usage.uses.get(a.0);
            let ub = usage.uses.get(b.0);
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
    for (i, item) in catalog.into_iter().enumerate() {
        let mut best: Option<(i32, Vec<(usize, usize)>)> = None;
        if let Some(ms) = crate::fuzzy::score(query, item.0) {
            best = Some((ms.score, ms.matched_ranges));
        }
        if let Some(score) = metadata_score(query, item.1, item.2)
            && best.as_ref().is_none_or(|(current, _)| score > *current)
        {
            best = Some((score, Vec::new()));
        }
        if let Some((score, matched)) = best {
            let total = combined_score(score, usage.uses.get(item.0), now);
            scored.push((total, i, to_match(item, matched)));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, m)| m).collect()
}

fn metadata_score(query: &str, shortcut: &str, command: Command) -> Option<i32> {
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
        if compact_shortcut.contains(&compact_q) {
            return Some(65);
        }
    }

    let aliases = command_aliases(command);
    let tokens: Vec<&str> = q.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    let all_tokens_match = tokens
        .iter()
        .all(|token| aliases.iter().any(|alias| alias.contains(token)));
    all_tokens_match.then_some(42 + tokens.len() as i32)
}

fn shortcut_search_text(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn command_aliases(command: Command) -> &'static [&'static str] {
    match command {
        Command::RequestCopy => &["file copy duplicate transfer send"],
        Command::RequestMove => &["file move transfer relocate send"],
        Command::MoveIntoCursorFolder => {
            &["file move selection drop highlighted cursor folder keyboard"]
        }
        Command::CreateDir => &["file new folder directory mkdir create"],
        Command::GatherIntoFolder => {
            &["file new folder with selection gather group move into subfolder"]
        }
        Command::RequestDelete => &["file delete remove trash"],
        Command::BeginRename => &["file rename edit name"],
        Command::BeginBatchRename => &["file batch rename bulk multi rename studio"],
        Command::BeginSync => &["panels sync synchronize mirror compare"],
        Command::FindDuplicates => &["file duplicate duplicates dedupe identical"],
        Command::DiffFiles => &["view diff compare text"],
        Command::DiskTreemap => &["view disk usage map treemap size"],
        Command::BeginFind => &["file find recursive search"],
        Command::OpenSavedSearch => &["file saved search smart folder"],
        Command::OpenProjectCollections => &["project collection workspace roots virtual view"],
        Command::CopyPath => &["clipboard copy path"],
        Command::CopyName => &["clipboard copy name filename"],
        Command::CopyParentPath => &["clipboard copy parent folder path"],
        Command::CopyFileUrl => &["clipboard copy url file url"],
        Command::CopyShellPath => &["clipboard copy shell escaped path"],
        Command::CopyRelativePath => &["clipboard copy relative path other pane"],
        Command::ShelfAdd => &["shelf stack add stage collect"],
        Command::ShelfDrain => &["shelf stack drain paste copy here"],
        Command::ToggleInfo => &["view info get info properties metadata"],
        Command::BeginGoToPath => &["navigation go path location jump"],
        Command::JumpBack => &["navigation back history previous jump return"],
        Command::JumpForward => &["navigation forward history next jump"],
        Command::JumpSlot(_) => &["navigation bookmark favorite slot jump go"],
        Command::AssignSlot(_) => &["navigation bookmark favorite slot assign set"],
        Command::BookmarkCurrentDir => &["navigation bookmark favorite add pin folder this"],
        Command::BeginRecent => &["navigation recent folders history projects"],
        Command::SelectAll => &["selection select all mark all"],
        Command::InvertSelection => &["selection invert reverse flip"],
        Command::SelectSameNamed => &["selection same name matching files compare"],
        Command::SelectOnlyHere => &["selection only here unique missing other panel compare diff"],
        Command::SelectDiffering => {
            &["selection differing changed modified other panel compare diff"]
        }
        Command::SelectIdentical => {
            &["selection identical same equal other panel compare diff dedupe"]
        }
        Command::BeginSelectMask => &["selection mask glob pattern wildcard"],
        Command::BeginRunBar => &["run command shell open with terminal execute tool launcher"],
        Command::ToggleHidden => &["view hidden show hidden dotfiles invisible"],
        Command::ToggleFoldersFirst => &["sort folders first dirs top order grouping directories"],
        Command::ToggleNaturalSort => {
            &["sort natural numeric ascii alphabetical order names file10"]
        }
        Command::SortByExtension => &["sort extension type suffix group ext"],
        Command::SortByKind => &["sort kind category group images docs code archives"],
        Command::SelectJunk => {
            &["selection junk clutter ds_store thumbs desktop.ini cleanup cruft"]
        }
        Command::ReverseSort => &["sort reverse flip order ascending descending invert"],
        Command::SelectLargest => &["selection largest biggest top size files heavy"],
        Command::SelectLikeCursor => &["selection same extension like cursor type matching"],
        Command::SelectEmptyFiles => &["selection empty zero byte blank files cleanup"],
        Command::CopyListingText => &["clipboard copy listing export text list folder contents"],
        Command::CopyListingCsv => &["clipboard copy listing export csv spreadsheet folder"],
        Command::CopyListingMarkdown => &["clipboard copy listing export markdown table folder"],
        Command::CycleDensity => &["view density rows compact comfortable spacious"],
        Command::TogglePreview => &["view preview quick look viewer inspect"],
        Command::EqualizePanels => &["panels equalize same folder mirror"],
        Command::SwapPanels => &["panels swap exchange switch sides"],
        Command::Undo => &["history undo revert rollback"],
        Command::Redo => &["history redo repeat"],
        _ => &["command"],
    }
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
    BracketLeft,
    BracketRight,
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

pub fn map_key(press: KeyPress) -> Option<Command> {
    use KeyCode::*;
    match press.code {
        Tab => Some(Command::SwitchPanel),
        Up if press.shift => Some(Command::ExtendSelectUp),
        Down if press.shift => Some(Command::ExtendSelectDown),
        Up => Some(Command::CursorUp),
        Down => Some(Command::CursorDown),
        Home => Some(Command::CursorHome),
        End => Some(Command::CursorEnd),
        PageUp => Some(Command::CursorPageUp),
        PageDown => Some(Command::CursorPageDown),
        Enter if press.command => Some(Command::MoveIntoCursorFolder),
        Enter => Some(Command::Activate),
        Backspace => Some(Command::GoUp),
        BracketLeft if press.command => Some(Command::JumpBack),
        BracketRight if press.command => Some(Command::JumpForward),
        // Cmd+Shift+1..9 assigns the active dir to a slot; Cmd+1..9 jumps to it.
        // A bare digit falls through (None) so type-ahead can use it.
        Digit(n) if press.command && press.shift => Some(Command::AssignSlot(n)),
        Digit(n) if press.command => Some(Command::JumpSlot(n)),
        Space => Some(Command::ToggleSelect),
        F2 => Some(Command::BeginRename),
        R if press.command && press.shift => Some(Command::BeginBatchRename),
        R if press.command => Some(Command::BeginRename),
        S if press.command && press.shift => Some(Command::BeginSync),
        D if press.command && press.shift => Some(Command::CycleDensity),
        D if press.command => Some(Command::DiffFiles),
        M if press.command && press.shift => Some(Command::DiskTreemap),
        M => Some(Command::ToggleMark),
        N if press.command && press.shift => Some(Command::GatherIntoFolder),
        F if press.command => Some(Command::BeginFind),
        C if press.command && press.shift => Some(Command::CopyPath),
        F3 => Some(Command::TogglePreview),
        F5 => Some(Command::RequestCopy),
        F6 => Some(Command::RequestMove),
        F7 => Some(Command::CreateDir),
        F8 | Delete => Some(Command::RequestDelete),
        A if press.command && press.shift => Some(Command::ShelfAdd),
        A if press.command => Some(Command::SelectAll),
        V if press.command && press.shift => Some(Command::ShelfDrain),
        E if press.command => Some(Command::EqualizePanels),
        U if press.command => Some(Command::SwapPanels),
        G if press.command => Some(Command::BeginSelectMask),
        I if press.command && press.shift => Some(Command::InvertSelection),
        I if press.command => Some(Command::ToggleInfo),
        L if press.command => Some(Command::BeginGoToPath),
        P if press.command => Some(Command::BeginRecent),
        Z if press.command && press.shift => Some(Command::Redo),
        Z if press.command => Some(Command::Undo),
        K if press.command => Some(Command::BeginPalette),
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
        assert_eq!(all.len(), command_catalog().len());
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
    fn shelf_binds_to_cmd_shift_a_and_cmd_shift_v() {
        let cmd_shift = |code| KeyPress {
            code,
            command: true,
            shift: true,
        };
        assert_eq!(map_key(cmd_shift(KeyCode::A)), Some(Command::ShelfAdd));
        assert_eq!(map_key(cmd_shift(KeyCode::V)), Some(Command::ShelfDrain));
        // Cmd+A without shift stays Select all; plain V types text.
        assert_eq!(map_key(cmd_press(KeyCode::A)), Some(Command::SelectAll));
        assert_eq!(map_key(press(KeyCode::V)), None);
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
    fn cmd_shift_m_opens_treemap() {
        let cmd_shift_m = KeyPress {
            code: KeyCode::M,
            command: true,
            shift: true,
        };
        assert_eq!(map_key(cmd_shift_m), Some(Command::DiskTreemap));
        // Bare M (no modifiers) toggles the mark on the cursor entry.
        assert_eq!(map_key(press(KeyCode::M)), Some(Command::ToggleMark));
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
    }
}
