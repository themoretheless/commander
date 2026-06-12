//! Keyboard commands: a pure mapping from key presses to [`Command`]s.
//! No egui types here; the UI layer translates raw input into [`KeyPress`].

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Command {
    SwitchPanel,
    CursorUp,
    CursorDown,
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
    SelectAll,
    ToggleHidden,
}

/// The keys the file manager reacts to (UI-toolkit independent).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum KeyCode {
    Tab,
    Up,
    Down,
    Enter,
    Backspace,
    Space,
    F3,
    F5,
    F6,
    F7,
    F8,
    Delete,
    A,
    H,
}

/// A single key press with the Cmd-modifier state.
#[derive(Clone, Copy, Debug)]
pub struct KeyPress {
    pub code: KeyCode,
    /// Cmd (macOS command key) held.
    pub command: bool,
}

pub fn map_key(press: KeyPress) -> Option<Command> {
    use KeyCode::*;
    match (press.code, press.command) {
        (Tab, _) => Some(Command::SwitchPanel),
        (Up, _) => Some(Command::CursorUp),
        (Down, _) => Some(Command::CursorDown),
        (Enter, _) => Some(Command::Activate),
        (Backspace, _) => Some(Command::GoUp),
        (Space, _) => Some(Command::ToggleSelect),
        (F3, _) => Some(Command::TogglePreview),
        (F5, _) => Some(Command::RequestCopy),
        (F6, _) => Some(Command::RequestMove),
        (F7, _) => Some(Command::CreateDir),
        (F8, _) | (Delete, _) => Some(Command::RequestDelete),
        (A, true) => Some(Command::SelectAll),
        (H, true) => Some(Command::ToggleHidden),
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
        KeyPress { code, command: false }
    }

    fn cmd_press(code: KeyCode) -> KeyPress {
        KeyPress { code, command: true }
    }

    #[test]
    fn function_keys_map_to_file_ops() {
        assert_eq!(map_key(press(KeyCode::F5)), Some(Command::RequestCopy));
        assert_eq!(map_key(press(KeyCode::F6)), Some(Command::RequestMove));
        assert_eq!(map_key(press(KeyCode::F7)), Some(Command::CreateDir));
        assert_eq!(map_key(press(KeyCode::F8)), Some(Command::RequestDelete));
        assert_eq!(map_key(press(KeyCode::Delete)), Some(Command::RequestDelete));
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
}
