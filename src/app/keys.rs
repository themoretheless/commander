//! Input adapter: translates egui key events into toolkit-independent
//! [`KeyPress`]es, maps them to [`Command`]s (`crate::command`) and feeds
//! them to the workspace. No file-manager logic lives here.

use super::*;
use crate::accessibility::{
    EscapeRoute, KeyboardRoute, ModalSurface, UiContract, UiContractState, consume_escape_route,
    resolve_ui_contract, text_input_state, top_open_modal,
};
use crate::command::{Command, KeyCode, KeyPress, map_keys};

/// Seconds of idle after which the type-ahead buffer expires.
const TYPE_AHEAD_IDLE: f64 = 1.5;

/// Seconds a purely numeric buffer waits for a `j`/`k` before it is taken
/// as a name ("2024") and jumps.
const COUNT_WINDOW: f64 = 0.6;

/// The plain chord keys pressed this frame.
#[derive(Clone, Copy, Default)]
struct ChordKeys {
    g: bool,
    s: bool,
    j: bool,
    k: bool,
}

/// What one frame of chord handling does.
#[derive(Debug, PartialEq)]
struct ChordStep {
    /// Leader left pending for the next key.
    pending: Option<char>,
    /// Command to run this frame.
    command: Option<Command>,
    /// Key character consumed, kept out of type-ahead (`'\0'` for none).
    claimed: char,
    /// The numeric type-ahead buffer was used as a count.
    clear_type_ahead: bool,
    /// A pending leader cancelled without completing a chord or a motion:
    /// it was the first letter of a search, so it goes back to type-ahead.
    replay: Option<char>,
}

/// Resolve chord keys given the pending leader and the live (unexpired)
/// type-ahead buffer. A second key that completes no chord cancels the
/// chord and is handled as a fresh key, so `g j` still moves down. Any other
/// key replays the leader into type-ahead, so typing "src" searches "src".
/// A new `g`/`s` chord only starts outside a type-ahead search, so those
/// letters stay ordinary characters mid-search.
fn resolve_chord(pending: Option<char>, keys: ChordKeys, type_ahead: Option<&str>) -> ChordStep {
    let mut step = ChordStep {
        pending: None,
        command: None,
        claimed: '\0',
        clear_type_ahead: false,
        replay: None,
    };
    match (pending, keys.g, keys.s) {
        (Some('g'), true, _) => {
            step.command = Some(Command::CursorHome);
            step.claimed = 'g';
            return step;
        }
        (Some('s'), _, true) => {
            step.command = Some(Command::ReverseSort);
            step.claimed = 's';
            return step;
        }
        _ => {}
    }
    if let Some(leader) = pending
        && !keys.j
        && !keys.k
    {
        // Not a chord: the leader and this key are both search text (a `g`
        // or `s` here is a letter, not a new leader).
        step.replay = Some(leader);
        return step;
    }
    if type_ahead.is_none() {
        if keys.g {
            step.pending = Some('g');
            step.claimed = 'g';
            return step;
        }
        if keys.s {
            step.pending = Some('s');
            step.claimed = 's';
            return step;
        }
    }
    if keys.j || keys.k {
        let count_mode = type_ahead.is_none_or(|buf| buf.chars().all(|c| c.is_ascii_digit()));
        if count_mode {
            let count: i32 = type_ahead
                .and_then(|buf| buf.parse().ok())
                .filter(|n| *n > 0)
                .unwrap_or(1);
            step.command = Some(Command::CursorMove(if keys.j { count } else { -count }));
            step.claimed = if keys.j { 'j' } else { 'k' };
            step.clear_type_ahead = true;
        }
    }
    step
}

/// The frame's typed text minus one occurrence of each character already
/// consumed as a key (a chord letter, or the space/period of a command), so
/// a second, genuinely typed occurrence in the same frame survives.
fn strip_consumed(typed: &str, consumed: &[char]) -> String {
    let mut out: String = typed.to_string();
    for &c in consumed {
        if let Some(pos) = out.find(c) {
            out.remove(pos);
        }
    }
    out
}

/// Whether the type-ahead buffer should move the cursor. A purely numeric
/// buffer is a pending count for `j`/`k` (`5j`), so it must not jump first.
/// The text a frame adds to type-ahead: the typed text minus consumed keys,
/// led by a replayed chord leader. A leader is only replayed when the frame
/// really typed something, so a cancelling arrow or Escape drops it.
fn frame_type_ahead_text(typed: &str, consumed: &[char], replay: Option<char>) -> String {
    let rest = strip_consumed(typed, consumed);
    match replay {
        Some(leader) if !rest.is_empty() => format!("{leader}{rest}"),
        _ => rest,
    }
}

fn type_ahead_jumps(buffer: &str) -> bool {
    !buffer.chars().all(|c| c.is_ascii_digit())
}

/// Whether a numeric buffer, idle for `age` seconds with no `j`/`k`, should
/// now jump as a name. `jumped` is set once it has, so later frames (and
/// cursor moves the user makes meanwhile) are left alone.
fn numeric_jump_due(buffer: &str, age: f64, jumped: bool) -> bool {
    !jumped && !buffer.is_empty() && !type_ahead_jumps(buffer) && age >= COUNT_WINDOW
}

/// Whether a key press with these modifiers may route to a binding. Bindings
/// only distinguish Cmd and Shift, so Alt (and Ctrl where it is not the
/// command key, i.e. macOS) would otherwise trigger the unmodified command.
fn binding_modifiers(m: egui::Modifiers) -> bool {
    !m.alt && (m.command || !m.ctrl)
}

/// The egui-drawn fallback context menu closes on Escape itself, so that
/// press must not also close a pane tool or preview, or leave focus mode,
/// underneath it. Modals and text fields keep their own Escape.
const fn escape_route_under_menu(route: EscapeRoute, menu_open: bool) -> EscapeRoute {
    match route {
        EscapeRoute::PaneTool | EscapeRoute::ActivePreview | EscapeRoute::FocusMode
            if menu_open =>
        {
            EscapeRoute::None
        }
        route => route,
    }
}

fn prioritized_modal(
    open: impl Fn(ModalSurface) -> bool,
    first_pending: Option<ModalSurface>,
) -> Option<ModalSurface> {
    top_open_modal(open).or(first_pending)
}

impl App {
    pub(crate) fn handle_keys(&mut self, ctx: &egui::Context) {
        let contract = self.ui_contract(ctx);
        if matches!(
            contract.keyboard,
            KeyboardRoute::TextInput(_) | KeyboardRoute::Modal(_)
        ) {
            self.ui.type_ahead = None;
            self.ui.chord = None;
            return;
        }
        let presses = ctx.input(Self::collect_presses);
        // Space/Period also arrive as text; once they ran a command they must
        // not extend type-ahead too.
        let mut consumed: Vec<char> = presses
            .iter()
            .filter(|p| crate::command::map_key(**p).is_some())
            .filter_map(|p| match p.code {
                KeyCode::Space => Some(' '),
                KeyCode::Period => Some('.'),
                _ => None,
            })
            .collect();
        for cmd in map_keys(&presses) {
            self.execute_key_command(ctx, cmd);
            if self.ws.has_any_pending_ui_modal() {
                break;
            }
        }
        if self.ws.has_any_pending_ui_modal() {
            self.ui.type_ahead = None;
            self.ui.chord = None;
            return;
        }
        let (claimed, replay) = self.handle_chords(ctx);
        if claimed != '\0' {
            consumed.push(claimed);
        }
        self.handle_type_ahead(ctx, &consumed, replay);
    }

    pub(crate) fn ui_contract(&self, ctx: &egui::Context) -> UiContract {
        let first_pending = self.ws.first_pending_ui_modal().map(UiModal::surface);
        let modal = prioritized_modal(|surface| self.is_modal_surface_open(surface), first_pending);
        let active_left = self.ws.active == ActivePanel::Left;
        let active_preview_open = if active_left {
            self.ws.left.preview.is_some()
        } else {
            self.ws.right.preview.is_some()
        };
        resolve_ui_contract(UiContractState {
            text_input: text_input_state(ctx),
            modal,
            pane_tool_open: self.ui.tools.any_open(),
            active_preview_open,
            focus_mode: self.ui.focus_mode,
        })
    }

    pub(crate) fn capture_escape_request(&mut self, ctx: &egui::Context) {
        self.ui.escape_request = if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            escape_route_under_menu(self.ui_contract(ctx).escape, self.fallback_menu.is_some())
        } else {
            EscapeRoute::None
        };
    }

    pub(crate) fn take_escape_request(&mut self, owner: EscapeRoute) -> bool {
        consume_escape_route(&mut self.ui.escape_request, owner)
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
        self.toast(ctx, crate::toasts::ToastKind::Info, message, false);
    }

    /// Vim-style chords, modifier-free: `g g` jumps to the top, `s s`
    /// reverses the sort, and `j`/`k` move the cursor down/up, picking up a
    /// leading numeric count already buffered by type-ahead (`5j` moves 5
    /// rows). `j`/`k` only claim the motion while that buffer is empty or
    /// purely numeric; mid-search (e.g. typing "backjack") they fall through
    /// to type-ahead as ordinary characters instead of hijacking it.
    ///
    /// Returns the character (if any) claimed this frame, so
    /// [`Self::handle_type_ahead`] can leave it out of its own buffer, and a
    /// cancelled leader it should put back in front of the typed text.
    fn handle_chords(&mut self, ctx: &egui::Context) -> (char, Option<char>) {
        const CHORD_IDLE: f64 = 1.0;
        let now = ctx.input(|i| i.time);
        if let Some((_, started)) = self.ui.chord
            && now - started > CHORD_IDLE
        {
            self.ui.chord = None;
        }

        let (any_press, keys, plain) = ctx.input(|i| {
            (
                i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Key { pressed: true, .. })),
                ChordKeys {
                    g: i.key_pressed(egui::Key::G),
                    s: i.key_pressed(egui::Key::S),
                    j: i.key_pressed(egui::Key::J),
                    k: i.key_pressed(egui::Key::K),
                },
                !i.modifiers.command && !i.modifiers.shift && !i.modifiers.alt && !i.modifiers.ctrl,
            )
        });
        // Frames without a key press (a key release, a mouse move) keep a
        // pending leader instead of cancelling it.
        if !any_press {
            return ('\0', None);
        }
        if !plain {
            // A modified key (Shift+R typing "R") cancels the chord; the
            // leader was the start of a search.
            return ('\0', self.ui.chord.take().map(|(leader, _)| leader));
        }

        let type_ahead = self
            .ui
            .type_ahead
            .as_ref()
            .filter(|(_, last)| now - last <= TYPE_AHEAD_IDLE)
            .map(|(buf, _)| buf.as_str());
        let step = resolve_chord(self.ui.chord.map(|(leader, _)| leader), keys, type_ahead);
        self.ui.chord = step.pending.map(|leader| (leader, now));
        if step.clear_type_ahead {
            self.ui.type_ahead = None;
        }
        if let Some(command) = step.command {
            self.ws.execute(command);
        }
        (step.claimed, step.replay)
    }

    /// Type-to-jump: printable characters build a short-lived buffer that
    /// moves the cursor to the first matching name. Expires after ~1.5s idle.
    /// Command-modified keys and the mapped hotkeys never reach here as text;
    /// `consumed` (the chord key from [`Self::handle_chords`], and the
    /// space/period that ran a command) is also excluded, so a bare
    /// `g`/`s`/`j`/`k` used as a chord doesn't also start/extend a search.
    /// A purely numeric buffer is a pending `j`/`k` count: it jumps only
    /// once [`COUNT_WINDOW`] passes without a motion, so "2024" still finds a
    /// folder of that name. `replay` is a cancelled chord leader that starts the typed text.
    fn handle_type_ahead(&mut self, ctx: &egui::Context, consumed: &[char], replay: Option<char>) {
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
        let typed = frame_type_ahead_text(&typed, consumed, replay);

        // Expire a stale buffer.
        if let Some((_, last)) = &self.ui.type_ahead
            && now - last > TYPE_AHEAD_IDLE
        {
            self.ui.type_ahead = None;
        }
        if typed.is_empty() {
            // A numeric buffer that no `j`/`k` claimed is a name after all.
            if let Some((buffer, last)) = &self.ui.type_ahead
                && numeric_jump_due(buffer, now - last, self.ui.type_ahead_jumped)
            {
                let buffer = buffer.clone();
                self.ui.type_ahead_jumped = true;
                self.ws.active_panel().type_ahead(&buffer);
            }
            return;
        }
        self.ui.type_ahead_jumped = false;
        let buffer = match &mut self.ui.type_ahead {
            Some((b, t)) => {
                b.push_str(&typed);
                *t = now;
                b.clone()
            }
            None => {
                self.ui.type_ahead = Some((typed.clone(), now));
                typed
            }
        };
        if type_ahead_jumps(&buffer) {
            self.ws.active_panel().type_ahead(&buffer);
        } else {
            // Wake up to jump once the count window passes without a motion.
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(COUNT_WINDOW));
        }
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
            (egui::Key::Period, KeyCode::Period),
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
            (egui::Key::J, KeyCode::J),
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
        if !binding_modifiers(i.modifiers) {
            return Vec::new();
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_modal_keeps_priority_over_a_deferred_request() {
        let mut queue = crate::ui_request::UiRequestQueue::default();
        queue.emit(UiRequest::RunCommand);
        queue.emit(UiRequest::Palette);
        let modal = prioritized_modal(
            |surface| surface == ModalSurface::Palette,
            queue.first_pending_modal().map(UiModal::surface),
        );

        assert_eq!(modal, Some(ModalSurface::Palette));
    }

    #[test]
    fn first_pending_modal_reserves_escape_in_fifo_order() {
        let mut queue = crate::ui_request::UiRequestQueue::default();
        queue.emit(UiRequest::RunCommand);
        queue.emit(UiRequest::Palette);
        let modal = prioritized_modal(|_| false, queue.first_pending_modal().map(UiModal::surface));

        assert_eq!(modal, Some(ModalSurface::RunCommand));
        assert_eq!(
            resolve_ui_contract(UiContractState {
                modal,
                ..Default::default()
            })
            .escape,
            EscapeRoute::Modal(ModalSurface::RunCommand)
        );
    }

    fn keys(spec: &str) -> ChordKeys {
        ChordKeys {
            g: spec.contains('g'),
            s: spec.contains('s'),
            j: spec.contains('j'),
            k: spec.contains('k'),
        }
    }

    #[test]
    fn gg_and_ss_complete_their_chords() {
        let first = resolve_chord(None, keys("g"), None);
        assert_eq!(first.pending, Some('g'));
        assert_eq!(first.claimed, 'g');
        let second = resolve_chord(first.pending, keys("g"), None);
        assert_eq!(second.command, Some(Command::CursorHome));
        assert_eq!(second.pending, None);
        let sort = resolve_chord(Some('s'), keys("s"), None);
        assert_eq!(sort.command, Some(Command::ReverseSort));
    }

    #[test]
    fn an_aborted_chord_handles_the_second_key_as_a_motion() {
        let step = resolve_chord(Some('g'), keys("j"), None);
        assert_eq!(step.pending, None);
        assert_eq!(step.command, Some(Command::CursorMove(1)));
        assert_eq!(step.claimed, 'j');
    }

    #[test]
    fn g_and_s_stay_ordinary_letters_mid_search() {
        for key in ["g", "s"] {
            let step = resolve_chord(None, keys(key), Some("sr"));
            assert_eq!(step.pending, None);
            assert_eq!(step.command, None);
            assert_eq!(step.claimed, '\0');
        }
        // `j` mid-search is a letter too, but a numeric buffer is a count.
        assert_eq!(resolve_chord(None, keys("j"), Some("ba")).command, None);
        let counted = resolve_chord(None, keys("k"), Some("5"));
        assert_eq!(counted.command, Some(Command::CursorMove(-5)));
        assert!(counted.clear_type_ahead);
    }

    #[test]
    fn a_cancelled_leader_starts_the_search() {
        // "src" with an empty buffer: `s` waits as a leader, `r` cancels it.
        let first = resolve_chord(None, keys("s"), None);
        assert_eq!(first.pending, Some('s'));
        let second = resolve_chord(first.pending, keys(""), None);
        assert_eq!(second.pending, None);
        assert_eq!(second.command, None);
        assert_eq!(second.claimed, '\0');
        assert_eq!(second.replay, Some('s'));
        assert_eq!(frame_type_ahead_text("r", &[], second.replay), "sr");
        // A different leader is a letter too: "gs" searches "gs".
        let other = resolve_chord(Some('g'), keys("s"), None);
        assert_eq!(other.pending, None);
        assert_eq!(other.claimed, '\0');
        assert_eq!(other.replay, Some('g'));
        assert_eq!(frame_type_ahead_text("s", &[], other.replay), "gs");
        // A key that types nothing (an arrow) drops the leader.
        assert_eq!(frame_type_ahead_text("", &[], Some('s')), "");
        // `g j` stays a motion, with nothing replayed.
        assert_eq!(resolve_chord(Some('g'), keys("j"), None).replay, None);
    }

    #[test]
    fn consumed_keys_strip_only_one_occurrence_each() {
        assert_eq!(strip_consumed("gag", &['g']), "ag");
        assert_eq!(strip_consumed("a b", &[' ']), "ab");
        assert_eq!(strip_consumed(".", &['.']), "");
        assert_eq!(strip_consumed("x", &[]), "x");
    }

    #[test]
    fn a_numeric_buffer_waits_for_its_motion_instead_of_jumping() {
        assert!(!type_ahead_jumps("5"));
        assert!(!type_ahead_jumps("12"));
        assert!(type_ahead_jumps("5a"));
        assert!(type_ahead_jumps("src"));
    }

    #[test]
    fn a_numeric_buffer_jumps_as_a_name_once_no_motion_follows() {
        assert!(!numeric_jump_due("2024", COUNT_WINDOW / 2.0, false));
        assert!(numeric_jump_due("2024", COUNT_WINDOW, false));
        assert!(!numeric_jump_due("2024", COUNT_WINDOW * 2.0, true));
        // Text buffers already jumped as they were typed.
        assert!(!numeric_jump_due("src", COUNT_WINDOW, false));
        assert!(!numeric_jump_due("", COUNT_WINDOW, false));
    }

    fn presses_for(key: egui::Key, modifiers: egui::Modifiers) -> Vec<KeyPress> {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        let mut presses = Vec::new();
        crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
            presses = ui.input(App::collect_presses);
        }));
        presses
    }

    #[test]
    fn alt_and_mac_ctrl_never_route_to_unmodified_bindings() {
        assert!(presses_for(egui::Key::Backspace, egui::Modifiers::ALT).is_empty());
        assert!(presses_for(egui::Key::Delete, egui::Modifiers::ALT).is_empty());
        let mac_ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert!(presses_for(egui::Key::Backspace, mac_ctrl).is_empty());
        assert_eq!(
            presses_for(egui::Key::Backspace, egui::Modifiers::NONE).len(),
            1
        );
        let command = presses_for(egui::Key::A, egui::Modifiers::COMMAND);
        assert_eq!(command.len(), 1);
        assert!(command[0].command);
    }

    #[test]
    fn escape_on_the_fallback_menu_only_closes_the_menu() {
        for route in [
            EscapeRoute::PaneTool,
            EscapeRoute::ActivePreview,
            EscapeRoute::FocusMode,
        ] {
            assert_eq!(escape_route_under_menu(route, true), EscapeRoute::None);
            assert_eq!(escape_route_under_menu(route, false), route);
        }
        let modal = EscapeRoute::Modal(ModalSurface::Palette);
        assert_eq!(escape_route_under_menu(modal, true), modal);
    }
}
