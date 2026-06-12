//! Keyboard handling: keys are first mapped to [`Command`]s, then each
//! command is executed against the app state. Mapping stays pure and
//! the dispatch logic lives in one place.

use super::*;

#[derive(Clone, Copy)]
enum Command {
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

impl App {
    pub(crate) fn handle_keys(&mut self, ctx: &egui::Context) {
        // A text field (e.g. the filter box) owns the keyboard:
        // typing there must not trigger navigation/file-op hotkeys.
        if ctx.wants_keyboard_input() {
            return;
        }
        let commands = ctx.input(Self::map_keys);
        for cmd in commands {
            self.execute(cmd);
        }
    }

    /// Pure mapping from pressed keys to commands.
    fn map_keys(i: &egui::InputState) -> Vec<Command> {
        use egui::Key;
        let mut out = Vec::new();
        if i.key_pressed(Key::Tab) {
            out.push(Command::SwitchPanel);
        }
        if i.key_pressed(Key::ArrowUp) {
            out.push(Command::CursorUp);
        }
        if i.key_pressed(Key::ArrowDown) {
            out.push(Command::CursorDown);
        }
        if i.key_pressed(Key::Enter) {
            out.push(Command::Activate);
        }
        if i.key_pressed(Key::Backspace) {
            out.push(Command::GoUp);
        }
        if i.key_pressed(Key::Space) {
            out.push(Command::ToggleSelect);
        }
        if i.key_pressed(Key::F3) {
            out.push(Command::TogglePreview);
        }
        if i.key_pressed(Key::F5) {
            out.push(Command::RequestCopy);
        }
        if i.key_pressed(Key::F6) {
            out.push(Command::RequestMove);
        }
        if i.key_pressed(Key::F7) {
            out.push(Command::CreateDir);
        }
        if i.key_pressed(Key::F8) || i.key_pressed(Key::Delete) {
            out.push(Command::RequestDelete);
        }
        if i.modifiers.command && i.key_pressed(Key::A) {
            out.push(Command::SelectAll);
        }
        if i.modifiers.command && i.key_pressed(Key::H) {
            out.push(Command::ToggleHidden);
        }
        out
    }

    fn execute(&mut self, cmd: Command) {
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
                let panel = self.active_panel();
                if panel.cursor == 0 {
                    panel.go_up();
                } else if let Some(entry) =
                    panel.filtered_entries().get(panel.cursor - 1).cloned()
                {
                    if entry.is_dir {
                        let path = entry.path.clone();
                        panel.navigate_to(path);
                    } else {
                        let _ = open::that(&entry.path);
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
                        let panel = match self.active {
                            ActivePanel::Left => &self.left,
                            ActivePanel::Right => &self.right,
                        };
                        panel
                            .filtered_entries()
                            .get(panel.cursor.saturating_sub(1))
                            .and_then(|e| Self::make_preview(e))
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
}
