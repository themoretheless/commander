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
    /// Shift+Up: select the current row and move up (range selection).
    ExtendSelectUp,
    /// Shift+Down: select the current row and move down (range selection).
    ExtendSelectDown,
    /// Enter: open file / enter dir / go up on the ".." row.
    Activate,
    GoUp,
    /// Space: toggle selection and advance cursor.
    ToggleSelect,
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
    /// Begin bookmarks / favorites hotlist picker (designer Iteration 2).
    BeginBookmarks,
    /// Assign current dir to a bookmark slot / hotlist (designer Iteration 2).
    AssignCurrentToBookmark,
    /// Cmd+T: duplicate current tab on the active side (designer Iteration 1).
    NewTab,
    /// Cmd+W: close the current tab on the active side (never zero tabs) (designer Iteration 1).
    CloseTab,
    /// Ctrl+Tab: cycle to next tab on the active side.
    NextTab,
    /// Ctrl+Shift+Tab: cycle to previous tab on the active side.
    PrevTab,
    /// Open terminal at active tab dir (like mc / TotalCmd / VSCode integrated terminal; high value "most needed").
    OpenTerminal,
    /// Git diff for selected / cursor (basic from top 50).
    GitDiff,
    /// Git stage selected paths.
    GitStage,
    /// Git discard changes for selected (with confirm in future).
    GitDiscard,
    /// Save current tabs as a set (basic for top 50 saved tab sets).
    SaveTabSet,
    /// Toggle git status display (column customization).
    ToggleShowGit,
    /// Restore last saved tab set (UI stub).
    RestoreTabSet,
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
    ToggleHidden,
    /// Permissions / chmod viewer stub (idea #83).
    ShowPermissions,
    /// Browse/extract archive stub (zip/tar) (idea #84).
    BrowseArchive,
}

/// User-facing commands for the Cmd+K palette: (label, shortcut, command).
pub fn command_catalog() -> Vec<(&'static str, &'static str, Command)> {
    vec![
        ("Copy to other panel", "F5", Command::RequestCopy),
        ("Move to other panel", "F6", Command::RequestMove),
        ("New folder", "F7", Command::CreateDir),
        ("Delete (to Trash)", "F8", Command::RequestDelete),
        ("Rename", "F2", Command::BeginRename),
        ("Batch rename", "Cmd+Shift+R", Command::BeginBatchRename),
        ("Synchronize panels", "Cmd+Shift+S", Command::BeginSync),
        ("Find duplicates", "", Command::FindDuplicates),
        ("Diff files", "Cmd+D", Command::DiffFiles),
        ("Disk usage map", "Cmd+Shift+M", Command::DiskTreemap),
        ("Find files", "Cmd+F", Command::BeginFind),
        ("Open saved search", "", Command::OpenSavedSearch),
        ("Bookmarks / favorites", "", Command::BeginBookmarks),
        ("Assign current dir to bookmark", "", Command::AssignCurrentToBookmark),
        ("New tab", "Cmd+T", Command::NewTab),
        ("Close tab", "Cmd+W", Command::CloseTab),
        ("Next tab", "Ctrl+Tab", Command::NextTab),
        ("Previous tab", "Ctrl+Shift+Tab", Command::PrevTab),
        ("Open terminal here", "", Command::OpenTerminal),
        ("Git diff", "", Command::GitDiff),
        ("Git stage selected", "", Command::GitStage),
        ("Git discard selected", "", Command::GitDiscard),
        ("Save current tabs as set", "", Command::SaveTabSet),
        ("Toggle git status", "", Command::ToggleShowGit),
        ("Restore last tab set", "", Command::RestoreTabSet),
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
        ("Recent folders", "Cmd+P", Command::BeginRecent),
        ("Select all", "Cmd+A", Command::SelectAll),
        ("Invert selection", "", Command::InvertSelection),
        (
            "Select files also in other panel",
            "",
            Command::SelectSameNamed,
        ),
        ("Permissions (chmod)", "", Command::ShowPermissions),
        ("Browse archive (zip/tar)", "", Command::BrowseArchive),
        ("Select by mask", "Cmd+G", Command::BeginSelectMask),
        ("Toggle hidden files", "Cmd+H", Command::ToggleHidden),
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
        if let Some(ms) = crate::fuzzy::score(query, item.0) {
            let total = combined_score(ms.score, usage.uses.get(item.0), now);
            scored.push((total, i, to_match(item, ms.matched_ranges)));
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, m)| m).collect()
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
    T,
    U,
    V,
    W,
    Z,
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
        Tab if press.command && press.shift => Some(Command::PrevTab),
        Tab if press.command => Some(Command::NextTab),
        Tab => Some(Command::SwitchPanel),
        KeyCode::T if press.command => Some(Command::NewTab),
        KeyCode::W if press.command => Some(Command::CloseTab),
        KeyCode::T if press.command && press.shift => Some(Command::OpenTerminal),
        Up if press.shift => Some(Command::ExtendSelectUp),
        Down if press.shift => Some(Command::ExtendSelectDown),
        Up => Some(Command::CursorUp),
        Down => Some(Command::CursorDown),
        Home => Some(Command::CursorHome),
        End => Some(Command::CursorEnd),
        PageUp => Some(Command::CursorPageUp),
        PageDown => Some(Command::CursorPageDown),
        Enter => Some(Command::Activate),
        Backspace => Some(Command::GoUp),
        Space => Some(Command::ToggleSelect),
        F2 => Some(Command::BeginRename),
        R if press.command && press.shift => Some(Command::BeginBatchRename),
        R if press.command => Some(Command::BeginRename),
        S if press.command && press.shift => Some(Command::BeginSync),
        D if press.command && press.shift => Some(Command::CycleDensity),
        D if press.command => Some(Command::DiffFiles),
        M if press.command && press.shift => Some(Command::DiskTreemap),
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
    }
}
