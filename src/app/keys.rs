//! Input adapter: translates egui key events into toolkit-independent
//! [`KeyPress`]es, maps them to [`Command`]s (`crate::command`) and feeds
//! them to the workspace. No file-manager logic lives here.

use super::*;
use crate::accessibility::{
    EscapeRoute, KeyboardRoute, ModalSurface, UiContract, UiContractState, consume_escape_route,
    resolve_ui_contract, text_input_state, top_open_modal,
};
use crate::command::{Command, KeyCode, KeyPress, map_keys};

fn keyboard_command_allowed(route: KeyboardRoute, command: Command) -> bool {
    match route {
        KeyboardRoute::Workspace => true,
        KeyboardRoute::Transfer => {
            matches!(command, Command::RequestCopy | Command::RequestMove)
        }
        KeyboardRoute::TextInput(_) | KeyboardRoute::Modal(_) => false,
    }
}

impl App {
    pub(crate) fn handle_keys(&mut self, ctx: &egui::Context) {
        let contract = self.ui_contract(ctx);
        if matches!(
            contract.keyboard,
            KeyboardRoute::TextInput(_) | KeyboardRoute::Modal(_)
        ) {
            self.type_ahead = None;
            self.chord = None;
            return;
        }
        let presses = ctx.input(Self::collect_presses);
        if contract.keyboard == KeyboardRoute::Transfer {
            // A transfer's progress window is effectively modal, but Copy/Move
            // stay live so a second transfer can be queued behind it instead
            // of the hotkey being dropped on the floor. Every other mapped key
            // gets direct feedback instead of failing silently.
            for cmd in map_keys(&presses) {
                if keyboard_command_allowed(contract.keyboard, cmd) {
                    self.execute_key_command(ctx, cmd);
                } else {
                    self.push_key_feedback(ctx, "Wait for the active transfer to finish");
                }
            }
            self.type_ahead = None;
            self.chord = None;
            return;
        }
        for cmd in map_keys(&presses) {
            self.execute_key_command(ctx, cmd);
        }
        let claimed = self.handle_chords(ctx);
        self.handle_type_ahead(ctx, claimed);
    }

    pub(crate) fn ui_contract(&self, ctx: &egui::Context) -> UiContract {
        let modal = top_open_modal(|surface| match surface {
            ModalSurface::Transfer => self.ws.active_transfer.is_some(),
            ModalSurface::SafeState => self.ws.safe_state.is_some(),
            ModalSurface::Recovery => self.recovery.open || self.ws.recovery_request,
            ModalSurface::History => {
                self.history_preview.is_some() || self.ws.undo_request || self.ws.redo_request
            }
            ModalSurface::Confirmation => self.ws.pending_op.is_some(),
            ModalSurface::Rename => self.renaming.is_some() || self.ws.rename_target.is_some(),
            ModalSurface::BatchRename => {
                self.batch_rename.is_some() || self.ws.batch_rename_request
            }
            ModalSurface::Sync => self.sync.is_some() || self.ws.sync_request,
            ModalSurface::Duplicates => self.duplicates.is_some() || self.ws.duplicates_request,
            ModalSurface::Diff => self.diff.is_some() || self.ws.diff_request,
            ModalSurface::Treemap => self.treemap.is_some() || self.ws.treemap_request,
            ModalSurface::Find => self.find.is_some() || self.ws.find_request,
            ModalSurface::Archive => self.archive.is_some() || self.ws.archive_request.is_some(),
            ModalSurface::SavedSearch => self.saved_search_open || self.ws.saved_search_request,
            ModalSurface::Collections => {
                self.collections_dialog.is_some() || self.ws.collections_request
            }
            ModalSurface::Mask => self.mask_input.is_some() || self.ws.mask_request,
            ModalSurface::Path => self.path_input.is_some() || self.ws.path_request,
            ModalSurface::Recent => self.recent_input.is_some() || self.ws.recent_request,
            ModalSurface::RunCommand => self.run_command.is_some() || self.ws.run_command_request,
            ModalSurface::Palette => self.palette_input.is_some() || self.ws.palette_request,
        });
        let active_left = self.ws.active == ActivePanel::Left;
        let active_preview_open = if active_left {
            self.ws.left.preview.is_some()
        } else {
            self.ws.right.preview.is_some()
        };
        resolve_ui_contract(UiContractState {
            text_input: text_input_state(ctx),
            modal,
            active_preview_open,
            focus_mode: self.focus_mode,
        })
    }

    pub(crate) fn capture_escape_request(&mut self, ctx: &egui::Context) {
        self.escape_request = if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.ui_contract(ctx).escape
        } else {
            EscapeRoute::None
        };
    }

    pub(crate) fn take_escape_request(&mut self, owner: EscapeRoute) -> bool {
        consume_escape_route(&mut self.escape_request, owner)
    }

    pub(crate) fn take_modal_escape(&mut self, owner: ModalSurface) -> bool {
        self.take_escape_request(EscapeRoute::Modal(owner))
    }

    fn execute_key_command(&mut self, ctx: &egui::Context, command: Command) {
        if !crate::command::requires_context(command) {
            self.ws.execute(command);
            return;
        }

        let context = self.ws.command_context();
        let availability = crate::command::availability(command, &context);
        if availability.enabled {
            self.ws.execute(command);
        } else if let Some(reason) = availability.reason {
            self.push_key_feedback(ctx, reason);
        }
    }

    fn push_key_feedback(&mut self, ctx: &egui::Context, message: &'static str) {
        self.toasts.push(crate::toasts::Toast::new(
            message,
            crate::toasts::ToastKind::Info,
            false,
            ctx.input(|input| input.time),
        ));
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

#[cfg(test)]
#[path = "../ui_contract.rs"]
mod ui_contract;
