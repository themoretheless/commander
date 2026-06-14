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
    /// Cmd+E: point the inactive panel at the active panel's directory.
    EqualizePanels,
    /// Cmd+U: swap the left and right panels.
    SwapPanels,
    SelectAll,
    ToggleHidden,
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
    H,
    R,
    U,
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
        R if press.command => Some(Command::BeginRename),
        F3 => Some(Command::TogglePreview),
        F5 => Some(Command::RequestCopy),
        F6 => Some(Command::RequestMove),
        F7 => Some(Command::CreateDir),
        F8 | Delete => Some(Command::RequestDelete),
        A if press.command => Some(Command::SelectAll),
        E if press.command => Some(Command::EqualizePanels),
        U if press.command => Some(Command::SwapPanels),
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
        // Plain E/U type text (type-ahead), not panel commands.
        assert_eq!(map_key(press(KeyCode::E)), None);
        assert_eq!(map_key(press(KeyCode::U)), None);
    }

    #[test]
    fn rename_binds_to_f2_and_cmd_r() {
        assert_eq!(map_key(press(KeyCode::F2)), Some(Command::BeginRename));
        assert_eq!(map_key(cmd_press(KeyCode::R)), Some(Command::BeginRename));
        // Plain R types text; it must not trigger rename.
        assert_eq!(map_key(press(KeyCode::R)), None);
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
