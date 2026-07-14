//! Input adapter: translates egui key events into toolkit-independent
//! [`KeyPress`]es, maps them to [`Command`]s (`crate::command`) and feeds
//! them to the workspace. No file-manager logic lives here.

use super::*;
use crate::command::{Command, KeyCode, KeyPress, map_keys};

impl App {
    pub(crate) fn handle_keys(&mut self, ctx: &egui::Context) {
        // A text field (e.g. the filter box) owns the keyboard:
        // typing there must not trigger navigation/file-op hotkeys.
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        // Dialogs own the keyboard too: they handle Enter/Esc themselves,
        // and hotkeys must not fire underneath a modal window.
        if self.ws.pending_op.is_some()
            || self.renaming.is_some()
            || self.mask_input.is_some()
            || self.run_command.is_some()
            || self.path_input.is_some()
            || self.recent_input.is_some()
            || self.palette_input.is_some()
            || self.batch_rename.is_some()
            || self.sync.is_some()
            || self.duplicates.is_some()
            || self.diff.is_some()
            || self.treemap.is_some()
            || self.find.is_some()
            || self.saved_search_open
        {
            self.type_ahead = None;
            self.chord = None;
            return;
        }
        let presses = ctx.input(Self::collect_presses);
        if self.ws.active_transfer.is_some() {
            // A transfer's progress window is effectively modal, but Copy/Move
            // stay live so a second transfer can be queued behind it instead
            // of the hotkey being dropped on the floor.
            for cmd in map_keys(&presses) {
                if matches!(cmd, Command::RequestCopy | Command::RequestMove) {
                    self.ws.execute(cmd);
                }
            }
            self.type_ahead = None;
            self.chord = None;
            return;
        }
        for cmd in map_keys(&presses) {
            self.ws.execute(cmd);
        }
        let claimed = self.handle_chords(ctx);
        self.handle_type_ahead(ctx, claimed);
    }

    /// Vim-style chords, modifier-free: `g g` jumps to the top, `s s`
    /// reverses the sort, and `j`/`k` move the cursor down/up, picking up a
    /// leading numeric count already buffered by type-ahead (`5j` moves 5
    /// rows). `j`/`k` only claim the motion while that buffer is empty or
    /// purely numeric; mid-search (e.g. typing "backjack") they fall through
    /// to type-ahead as ordinary characters instead of hijacking it.
    ///
    /// Returns the character (if any) claimed this frame, so
    /// [`Self::handle_type_ahead`] can leave it out of its own buffer.
    fn handle_chords(&mut self, ctx: &egui::Context) -> char {
        const CHORD_IDLE: f64 = 1.0;
        let now = ctx.input(|i| i.time);
        if let Some((_, started)) = self.chord
            && now - started > CHORD_IDLE
        {
            self.chord = None;
        }

        let (g, s, j, k, plain) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::G),
                i.key_pressed(egui::Key::S),
                i.key_pressed(egui::Key::J),
                i.key_pressed(egui::Key::K),
                !i.modifiers.command && !i.modifiers.shift && !i.modifiers.alt && !i.modifiers.ctrl,
            )
        });
        if !plain {
            return '\0';
        }

        if let Some((leader, _)) = self.chord {
            self.chord = None;
            return match (leader, g, s) {
                ('g', true, _) => {
                    self.ws.execute(Command::CursorHome);
                    'g'
                }
                ('s', _, true) => {
                    self.ws.execute(Command::ReverseSort);
                    's'
                }
                // Unrecognized second key: the chord just cancels.
                _ => '\0',
            };
        }
        if g {
            self.chord = Some(('g', now));
            return 'g';
        }
        if s {
            self.chord = Some(('s', now));
            return 's';
        }

        if j || k {
            let count_mode = self
                .type_ahead
                .as_ref()
                .map(|(buf, _)| buf.chars().all(|c| c.is_ascii_digit()))
                .unwrap_or(true);
            if count_mode {
                let count: i32 = self
                    .type_ahead
                    .as_ref()
                    .and_then(|(buf, _)| buf.parse().ok())
                    .filter(|n| *n > 0)
                    .unwrap_or(1);
                self.type_ahead = None;
                self.ws
                    .execute(Command::CursorMove(if j { count } else { -count }));
                return if j { 'j' } else { 'k' };
            }
        }
        '\0'
    }

    /// Type-to-jump: printable characters build a short-lived buffer that
    /// moves the cursor to the first matching name. Expires after ~1.5s idle.
    /// Command-modified keys and the mapped hotkeys never reach here as text;
    /// `claimed` (from [`Self::handle_chords`]) is also excluded, so a bare
    /// `g`/`s`/`j`/`k` used as a chord doesn't also start/extend a search.
    fn handle_type_ahead(&mut self, ctx: &egui::Context, claimed: char) {
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
        let typed: String = typed.chars().filter(|&c| c != claimed).collect();

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
            (egui::Key::C, KeyCode::C),
            (egui::Key::D, KeyCode::D),
            (egui::Key::E, KeyCode::E),
            (egui::Key::F, KeyCode::F),
            (egui::Key::G, KeyCode::G),
            (egui::Key::H, KeyCode::H),
            (egui::Key::I, KeyCode::I),
            (egui::Key::K, KeyCode::K),
            (egui::Key::L, KeyCode::L),
            (egui::Key::M, KeyCode::M),
            (egui::Key::N, KeyCode::N),
            (egui::Key::P, KeyCode::P),
            (egui::Key::R, KeyCode::R),
            (egui::Key::S, KeyCode::S),
            (egui::Key::U, KeyCode::U),
            (egui::Key::V, KeyCode::V),
            (egui::Key::Z, KeyCode::Z),
            (egui::Key::OpenBracket, KeyCode::BracketLeft),
            (egui::Key::CloseBracket, KeyCode::BracketRight),
            // Quick-jump slots: Cmd+1..9 jump, Cmd+Shift+1..9 assign. A bare
            // digit is left to type-ahead (map_key returns None without Cmd).
            (egui::Key::Num1, KeyCode::Digit(1)),
            (egui::Key::Num2, KeyCode::Digit(2)),
            (egui::Key::Num3, KeyCode::Digit(3)),
            (egui::Key::Num4, KeyCode::Digit(4)),
            (egui::Key::Num5, KeyCode::Digit(5)),
            (egui::Key::Num6, KeyCode::Digit(6)),
            (egui::Key::Num7, KeyCode::Digit(7)),
            (egui::Key::Num8, KeyCode::Digit(8)),
            (egui::Key::Num9, KeyCode::Digit(9)),
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
