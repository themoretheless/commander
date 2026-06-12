//! Input adapter: translates egui key events into toolkit-independent
//! [`KeyPress`]es, maps them to [`Command`]s (`crate::command`) and feeds
//! them to the workspace. No file-manager logic lives here.

use super::*;
use crate::command::{map_keys, KeyCode, KeyPress};

impl App {
    pub(crate) fn handle_keys(&mut self, ctx: &egui::Context) {
        // A text field (e.g. the filter box) owns the keyboard:
        // typing there must not trigger navigation/file-op hotkeys.
        if ctx.wants_keyboard_input() {
            return;
        }
        // Dialogs own the keyboard too: they handle Enter/Esc themselves,
        // and hotkeys must not fire underneath a modal window.
        if self.ws.pending_op.is_some() || self.ws.active_transfer.is_some() {
            return;
        }
        let presses = ctx.input(Self::collect_presses);
        for cmd in map_keys(&presses) {
            self.ws.execute(cmd);
        }
    }

    /// Snapshot the pressed keys we care about as toolkit-independent values.
    fn collect_presses(i: &egui::InputState) -> Vec<KeyPress> {
        const BINDINGS: &[(egui::Key, KeyCode)] = &[
            (egui::Key::Tab, KeyCode::Tab),
            (egui::Key::ArrowUp, KeyCode::Up),
            (egui::Key::ArrowDown, KeyCode::Down),
            (egui::Key::Enter, KeyCode::Enter),
            (egui::Key::Backspace, KeyCode::Backspace),
            (egui::Key::Space, KeyCode::Space),
            (egui::Key::F3, KeyCode::F3),
            (egui::Key::F5, KeyCode::F5),
            (egui::Key::F6, KeyCode::F6),
            (egui::Key::F7, KeyCode::F7),
            (egui::Key::F8, KeyCode::F8),
            (egui::Key::Delete, KeyCode::Delete),
            (egui::Key::A, KeyCode::A),
            (egui::Key::H, KeyCode::H),
        ];
        BINDINGS
            .iter()
            .filter(|(key, _)| i.key_pressed(*key))
            .map(|&(_, code)| KeyPress {
                code,
                command: i.modifiers.command,
            })
            .collect()
    }
}
