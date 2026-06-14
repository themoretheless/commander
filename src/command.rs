//! Keyboard commands: a pure mapping from key presses to [`Command`]s.
//! No egui types here; the UI layer translates raw input into [`KeyPress`].

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
    /// Cmd+Z: undo the last clean move.
    Undo,
    /// Cmd+K: open the command palette.
    BeginPalette,
    SelectAll,
    /// Flip the selection across the visible rows of the active panel.
    InvertSelection,
    /// Select active-panel entries whose name also exists in the other panel.
    SelectSameNamed,
    ToggleHidden,
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
        ("Select by mask", "Cmd+G", Command::BeginSelectMask),
        ("Toggle hidden files", "Cmd+H", Command::ToggleHidden),
        ("Toggle preview", "F3", Command::TogglePreview),
        ("Equalize panels", "Cmd+E", Command::EqualizePanels),
        ("Swap panels", "Cmd+U", Command::SwapPanels),
        ("Undo last move", "Cmd+Z", Command::Undo),
    ]
}

/// Filter the command catalog by a case-insensitive substring over the label.
pub fn filter_commands(query: &str) -> Vec<(&'static str, &'static str, Command)> {
    let q = query.trim().to_lowercase();
    command_catalog()
        .into_iter()
        .filter(|(label, _, _)| q.is_empty() || label.to_lowercase().contains(&q))
        .collect()
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
    E,
    G,
    H,
    I,
    K,
    L,
    P,
    R,
    S,
    U,
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
        Tab => Some(Command::SwitchPanel),
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
        F3 => Some(Command::TogglePreview),
        F5 => Some(Command::RequestCopy),
        F6 => Some(Command::RequestMove),
        F7 => Some(Command::CreateDir),
        F8 | Delete => Some(Command::RequestDelete),
        A if press.command => Some(Command::SelectAll),
        E if press.command => Some(Command::EqualizePanels),
        U if press.command => Some(Command::SwapPanels),
        G if press.command => Some(Command::BeginSelectMask),
        I if press.command => Some(Command::ToggleInfo),
        L if press.command => Some(Command::BeginGoToPath),
        P if press.command => Some(Command::BeginRecent),
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
    fn undo_binds_to_cmd_z() {
        assert_eq!(map_key(cmd_press(KeyCode::Z)), Some(Command::Undo));
        assert_eq!(map_key(press(KeyCode::Z)), None);
    }

    #[test]
    fn palette_binds_to_cmd_k() {
        assert_eq!(map_key(cmd_press(KeyCode::K)), Some(Command::BeginPalette));
        assert_eq!(map_key(press(KeyCode::K)), None);
    }

    #[test]
    fn filter_commands_matches_label_substring() {
        assert_eq!(filter_commands("").len(), command_catalog().len());
        let mv = filter_commands("move");
        assert!(mv.iter().any(|(_, _, c)| *c == Command::RequestMove));
        let swap = filter_commands("SWAP");
        assert_eq!(swap.len(), 1);
        assert_eq!(swap[0].2, Command::SwapPanels);
        assert!(filter_commands("zzzzz").is_empty());
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
