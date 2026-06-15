//! Input adapter: translates egui key events into toolkit-independent
//! [`KeyPress`]es, maps them to [`Command`]s (`crate::command`) and feeds
//! them to the workspace. No file-manager logic lives here.

use super::*;
use crate::command::{KeyCode, KeyPress, map_keys};

impl App {
    pub(crate) fn handle_keys(&mut self, ctx: &egui::Context) {
        // A text field (e.g. the filter box) owns the keyboard:
        // typing there must not trigger navigation/file-op hotkeys.
        if ctx.wants_keyboard_input() {
            return;
        }
        // Dialogs own the keyboard too: they handle Enter/Esc themselves,
        // and hotkeys must not fire underneath a modal window.
        if self.ws.pending_op.is_some()
            || self.ws.active_transfer.is_some()
            || self.renaming.is_some()
            || self.mask_input.is_some()
            || self.path_input.is_some()
            || self.recent_input.is_some()
            || self.palette_input.is_some()
            || self.batch_rename.is_some()
            || self.sync.is_some()
            || self.duplicates.is_some()
            || self.diff.is_some()
            || self.treemap.is_some()
            || self.find.is_some()
        {
            self.type_ahead = None;
            return;
        }
        let presses = ctx.input(Self::collect_presses);
        for cmd in map_keys(&presses) {
            self.ws.execute(cmd);
        }
        self.handle_type_ahead(ctx);
    }

    /// Type-to-jump: printable characters build a short-lived buffer that
    /// moves the cursor to the first matching name. Expires after ~1.5s idle.
    /// Command-modified keys and the mapped hotkeys never reach here as text.
    fn handle_type_ahead(&mut self, ctx: &egui::Context) {
        const IDLE: f64 = 1.5;
        let (typed, now) = ctx.input(|i| {
            let typed: String = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect();
            (typed, i.time)
        });

        // Expire a stale buffer.
        if let Some((_, last)) = &self.type_ahead
            && now - last > IDLE
        {
            self.type_ahead = None;
        }
        if typed.is_empty() {
            return;
        }
        let buffer = match &mut self.type_ahead {
            Some((b, t)) => {
                b.push_str(&typed);
                *t = now;
                b.clone()
            }
            None => {
                self.type_ahead = Some((typed.clone(), now));
                typed
            }
        };
        self.ws.active_panel().type_ahead(&buffer);
    }

    /// Snapshot the pressed keys we care about as toolkit-independent values.
    fn collect_presses(i: &egui::InputState) -> Vec<KeyPress> {
        const BINDINGS: &[(egui::Key, KeyCode)] = &[
            (egui::Key::Tab, KeyCode::Tab),
            (egui::Key::ArrowUp, KeyCode::Up),
            (egui::Key::ArrowDown, KeyCode::Down),
            (egui::Key::Home, KeyCode::Home),
            (egui::Key::End, KeyCode::End),
            (egui::Key::PageUp, KeyCode::PageUp),
            (egui::Key::PageDown, KeyCode::PageDown),
            (egui::Key::Enter, KeyCode::Enter),
            (egui::Key::Backspace, KeyCode::Backspace),
            (egui::Key::Space, KeyCode::Space),
            (egui::Key::F2, KeyCode::F2),
            (egui::Key::F3, KeyCode::F3),
            (egui::Key::F5, KeyCode::F5),
            (egui::Key::F6, KeyCode::F6),
            (egui::Key::F7, KeyCode::F7),
            (egui::Key::F8, KeyCode::F8),
            (egui::Key::Delete, KeyCode::Delete),
            (egui::Key::A, KeyCode::A),
            (egui::Key::D, KeyCode::D),
            (egui::Key::E, KeyCode::E),
            (egui::Key::F, KeyCode::F),
            (egui::Key::G, KeyCode::G),
            (egui::Key::H, KeyCode::H),
            (egui::Key::I, KeyCode::I),
            (egui::Key::K, KeyCode::K),
            (egui::Key::L, KeyCode::L),
            (egui::Key::M, KeyCode::M),
            (egui::Key::P, KeyCode::P),
            (egui::Key::R, KeyCode::R),
            (egui::Key::S, KeyCode::S),
            (egui::Key::U, KeyCode::U),
            (egui::Key::V, KeyCode::V),
            (egui::Key::Z, KeyCode::Z),
        ];
        BINDINGS
            .iter()
            .filter(|(key, _)| i.key_pressed(*key))
            .map(|&(_, code)| KeyPress {
                code,
                command: i.modifiers.command,
                shift: i.modifiers.shift,
            })
            .collect()
    }
}
