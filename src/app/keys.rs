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
pub(super) const TYPE_AHEAD_IDLE: f64 = 1.5;

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

fn consume_scroll_command(
    input: &mut egui::InputState,
    modifiers: egui::Modifiers,
    include_arrows: bool,
) -> Option<ScrollCommand> {
    [
        (egui::Key::ArrowUp, ScrollCommand::LineUp, true),
        (egui::Key::ArrowDown, ScrollCommand::LineDown, true),
        (egui::Key::ArrowLeft, ScrollCommand::LineLeft, true),
        (egui::Key::ArrowRight, ScrollCommand::LineRight, true),
        (egui::Key::PageUp, ScrollCommand::PageUp, false),
        (egui::Key::PageDown, ScrollCommand::PageDown, false),
        (egui::Key::Home, ScrollCommand::Home, false),
        (egui::Key::End, ScrollCommand::End, false),
    ]
    .into_iter()
    .filter(|(_, _, arrow)| include_arrows || !arrow)
    .find_map(|(key, command, _)| input.consume_key(modifiers, key).then_some(command))
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

/// Whether this frame's presses hold a plain Backspace that edits a live
/// type-ahead search instead of going up a folder.
fn backspace_edits_type_ahead(presses: &[KeyPress], live_search: bool) -> bool {
    live_search
        && presses
            .iter()
            .any(|p| p.code == KeyCode::Backspace && !p.command && !p.shift)
}

/// Drops a plain `.` from this frame's presses while a type-ahead search is
/// live, so typing "a.txt" extends the search instead of running Repeat Last.
fn yield_period_to_type_ahead(presses: &mut Vec<KeyPress>, live_search: bool) {
    if live_search {
        presses.retain(|p| p.code != KeyCode::Period || p.command);
    }
}

/// The search left after Backspace removes its last character, or `None`
/// once nothing is left to search for.
fn type_ahead_after_backspace(buffer: &str) -> Option<String> {
    let mut rest = buffer.to_string();
    rest.pop();
    (!rest.is_empty()).then_some(rest)
}

/// Escape that no surface owns ends a live type-ahead search, so the next
/// key starts a fresh one instead of extending the stale buffer.
fn escape_ends_type_ahead(route: EscapeRoute) -> bool {
    route == EscapeRoute::None
}

fn prioritized_modal(
    open: impl Fn(ModalSurface) -> bool,
    first_pending: Option<ModalSurface>,
) -> Option<ModalSurface> {
    top_open_modal(open).or(first_pending)
}

impl App {
    pub(crate) fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.handle_diff_scroll_keys(ctx) {
            self.ui.type_ahead = None;
            self.ui.chord = None;
            return;
        }
        if self.handle_text_preview_scroll_keys(ctx) {
            self.ui.type_ahead = None;
            self.ui.chord = None;
            return;
        }
        let contract = self.ui_contract(ctx);
        if matches!(
            contract.keyboard,
            KeyboardRoute::TextInput(_) | KeyboardRoute::Modal(_)
        ) {
            self.ui.type_ahead = None;
            self.ui.chord = None;
            return;
        }
        let (mut presses, escape, now) = ctx.input_mut(|i| {
            (
                Self::collect_presses(i),
                i.key_pressed(egui::Key::Escape),
                i.time,
            )
        });
        if escape && escape_ends_type_ahead(contract.escape) {
            self.ui.type_ahead = None;
            self.ui.chord = None;
        }
        let live_search = self
            .ui
            .type_ahead
            .as_ref()
            .is_some_and(|(_, last)| now - last <= TYPE_AHEAD_IDLE);
        if backspace_edits_type_ahead(&presses, live_search) {
            presses.retain(|p| p.code != KeyCode::Backspace || p.command || p.shift);
            self.type_ahead_backspace(ctx, now);
        }
        yield_period_to_type_ahead(&mut presses, live_search);
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
        if super::update::switch_into_covered_pane(&self.ui.tools, self.ws.active, command) {
            if let Some(diff) = &mut self.ui.tools.diff {
                diff.keyboard_focused = true;
                return;
            }
            self.push_key_feedback(ctx, super::update::SWITCH_INTO_COVERED_PANE);
            return;
        }
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

    fn handle_diff_scroll_keys(&mut self, ctx: &egui::Context) -> bool {
        let Some(diff) = &mut self.ui.tools.diff else {
            return false;
        };
        if !diff.keyboard_focused {
            return false;
        }

        let mut exit = false;
        let command = ctx.input_mut(|input| {
            if input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                || input.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
            {
                exit = true;
                return None;
            }
            consume_scroll_command(input, egui::Modifiers::NONE, true)
        });
        if exit {
            diff.keyboard_focused = false;
        } else if let Some(command) = command {
            diff.scroll_offset = apply_scroll_command(diff.scroll_offset, command, 396.0);
        }
        true
    }

    fn handle_text_preview_scroll_keys(&mut self, ctx: &egui::Context) -> bool {
        if !matches!(
            self.ws.active_panel().preview,
            Some(crate::panel::PreviewContent::Text { .. })
        ) {
            return false;
        }
        let command =
            ctx.input_mut(|input| consume_scroll_command(input, egui::Modifiers::ALT, false));
        let Some(command) = command else {
            return false;
        };
        let panel_side = match self.ws.active {
            crate::workspace::ActivePanel::Left => "left",
            crate::workspace::ActivePanel::Right => "right",
        };
        super::render::request_text_preview_scroll(ctx, panel_side, command);
        true
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
                self.ui.type_ahead_miss = !self.ws.active_panel().type_ahead(&buffer);
            }
            return;
        }
        self.ui.type_ahead_jumped = false;
        self.ui.type_ahead_miss = false;
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
            self.ui.type_ahead_miss = !self.ws.active_panel().type_ahead(&buffer);
        } else {
            // Wake up to jump once the count window passes without a motion.
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(COUNT_WINDOW));
        }
    }

    /// Backspace inside a live search deletes its last character and jumps
    /// again, rather than leaving the folder the user is searching in.
    fn type_ahead_backspace(&mut self, ctx: &egui::Context, now: f64) {
        let Some((buffer, _)) = &self.ui.type_ahead else {
            return;
        };
        let Some(rest) = type_ahead_after_backspace(buffer) else {
            self.ui.type_ahead = None;
            return;
        };
        self.ui.type_ahead = Some((rest.clone(), now));
        self.ui.type_ahead_jumped = false;
        self.ui.type_ahead_miss = false;
        if type_ahead_jumps(&rest) {
            self.ui.type_ahead_miss = !self.ws.active_panel().type_ahead(&rest);
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(COUNT_WINDOW));
        }
    }

    /// Snapshot the pressed keys we care about as toolkit-independent values.
    fn collect_presses(i: &mut egui::InputState) -> Vec<KeyPress> {
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
        let presses: Vec<_> = BINDINGS
            .iter()
            .filter(|(key, _)| i.key_pressed(*key))
            .map(|&(_, code)| KeyPress {
                code,
                command: i.modifiers.command,
                shift: i.modifiers.shift,
            })
            .collect();
        for (key, code) in [
            (egui::Key::Tab, KeyCode::Tab),
            (egui::Key::Enter, KeyCode::Enter),
            (egui::Key::Space, KeyCode::Space),
        ] {
            if presses
                .iter()
                .any(|press| press.code == code && crate::command::map_key(*press).is_some())
            {
                i.consume_key(i.modifiers, key);
            }
        }
        presses
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
            presses = ui.input_mut(App::collect_presses);
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
    fn workspace_tab_is_mapped_and_consumed_once() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut result = None;
        crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
            result = Some(ui.input_mut(|input| {
                let presses = App::collect_presses(input);
                (presses, input.key_pressed(egui::Key::Tab))
            }));
        }));
        let (presses, still_pressed) = result.unwrap();
        assert_eq!(map_keys(&presses), [Command::SwitchPanel]);
        assert!(!still_pressed, "egui focus traversal must not also see Tab");
    }

    #[test]
    fn workspace_activation_keys_do_not_also_click_focused_widgets() {
        for (key, expected) in [
            (egui::Key::Space, Command::ToggleSelect),
            (egui::Key::Enter, Command::Activate),
        ] {
            let ctx = egui::Context::default();
            let input = egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            };
            let mut result = None;
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                result = Some(ui.input_mut(|input| {
                    let presses = App::collect_presses(input);
                    (map_keys(&presses), input.key_pressed(key))
                }));
            }));
            let (commands, still_pressed) = result.unwrap();
            assert_eq!(commands, [expected]);
            assert!(!still_pressed, "focused widgets must not also see {key:?}");
        }
    }

    #[test]
    fn option_paging_is_reserved_for_preview_scroll_without_claiming_arrows() {
        let command_for = |key| {
            let ctx = egui::Context::default();
            let modifiers = egui::Modifiers::ALT;
            let input = egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            };
            let mut command = None;
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                command = ui
                    .input_mut(|input| consume_scroll_command(input, egui::Modifiers::ALT, false));
            }));
            command
        };
        assert_eq!(
            command_for(egui::Key::PageDown),
            Some(ScrollCommand::PageDown)
        );
        assert_eq!(command_for(egui::Key::Home), Some(ScrollCommand::Home));
        assert_eq!(command_for(egui::Key::ArrowDown), None);
    }

    fn press(code: KeyCode, command: bool) -> KeyPress {
        KeyPress {
            code,
            command,
            shift: false,
        }
    }

    #[test]
    fn period_extends_a_live_search_instead_of_repeating() {
        let mut presses = vec![press(KeyCode::Period, false)];
        yield_period_to_type_ahead(&mut presses, false);
        // No search running: `.` still repeats the last command.
        assert_eq!(presses.len(), 1);
        yield_period_to_type_ahead(&mut presses, true);
        assert!(presses.is_empty());
        let mut other = vec![press(KeyCode::Down, false)];
        yield_period_to_type_ahead(&mut other, true);
        assert_eq!(other.len(), 1);
    }

    #[test]
    fn backspace_edits_a_live_search_instead_of_going_up() {
        let backspace = [press(KeyCode::Backspace, false)];
        assert!(backspace_edits_type_ahead(&backspace, true));
        // No search running: Backspace still goes up a folder.
        assert!(!backspace_edits_type_ahead(&backspace, false));
        // Cmd+Backspace is a different binding.
        assert!(!backspace_edits_type_ahead(
            &[press(KeyCode::Backspace, true)],
            true
        ));
        assert!(!backspace_edits_type_ahead(
            &[press(KeyCode::Down, false)],
            true
        ));
        assert_eq!(type_ahead_after_backspace("src"), Some("sr".to_string()));
        assert_eq!(type_ahead_after_backspace("é"), None);
        assert_eq!(type_ahead_after_backspace(""), None);
    }

    #[test]
    fn unowned_escape_ends_the_search() {
        assert!(escape_ends_type_ahead(EscapeRoute::None));
        assert!(!escape_ends_type_ahead(EscapeRoute::PaneTool));
        assert!(!escape_ends_type_ahead(EscapeRoute::FocusMode));
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
