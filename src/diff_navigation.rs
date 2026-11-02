//! Contiguous change blocks over the retained edit script; no reads or edits.
use crate::textdiff::{DiffKind, DiffLine};
use std::ops::Range;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub rows: Range<usize>,
    pub left_start: usize,
    pub left_len: usize,
    pub right_start: usize,
    pub right_len: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    First,
    Previous,
    Next,
    Last,
}
impl Step {
    pub const ALL: [Self; 4] = [Self::First, Self::Previous, Self::Next, Self::Last];
    #[cfg(any(test, feature = "visual-qa"))]
    pub const fn key(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Previous => "prev",
            Self::Next => "next",
            Self::Last => "last",
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::First => "First difference",
            Self::Previous => "Previous difference",
            Self::Next => "Next difference",
            Self::Last => "Last difference",
        }
    }
}
#[derive(Default)]
pub struct Navigation {
    pub hunks: Vec<Hunk>,
    pub current: Option<usize>,
    reveal: bool,
    pub show_numbers: bool,
    pub source_numbers: Vec<(Option<usize>, Option<usize>)>,
}
impl Navigation {
    pub fn new(lines: &[DiffLine]) -> Self {
        let mut source_numbers = Vec::with_capacity(lines.len());
        let (mut minus, mut plus) = (1, 1);
        for line in lines {
            let a = (line.kind != DiffKind::Insert).then_some(minus);
            let b = (line.kind != DiffKind::Delete).then_some(plus);
            source_numbers.push((a, b));
            minus += usize::from(a.is_some());
            plus += usize::from(b.is_some());
        }
        let mut hunks = Vec::new();
        let (mut left, mut right, mut row) = (1, 1, 0);
        while row < lines.len() {
            if lines[row].kind == DiffKind::Equal {
                left += 1;
                right += 1;
                row += 1;
                continue;
            }
            let mut hunk = Hunk {
                rows: row..row,
                left_start: left,
                left_len: 0,
                right_start: right,
                right_len: 0,
            };
            while row < lines.len() && lines[row].kind != DiffKind::Equal {
                match lines[row].kind {
                    DiffKind::Delete => {
                        hunk.left_len += 1;
                        left += 1;
                    }
                    DiffKind::Insert => {
                        hunk.right_len += 1;
                        right += 1;
                    }
                    DiffKind::Equal => unreachable!(),
                }
                row += 1;
            }
            hunk.rows.end = row;
            hunks.push(hunk);
        }
        let current = (!hunks.is_empty()).then_some(0);
        Self {
            hunks,
            source_numbers,
            show_numbers: false,
            current,
            reveal: current.is_some(),
        }
    }
    pub fn selected(&self) -> Option<&Hunk> {
        self.current.and_then(|i| self.hunks.get(i))
    }
    pub fn enabled(&self, step: Step) -> bool {
        match (self.current, step) {
            (_, _) if self.hunks.is_empty() => false,
            (Some(i), Step::Previous) => i > 0,
            (Some(i), Step::Next) => i + 1 < self.hunks.len(),
            _ => true,
        }
    }
    pub fn go(&mut self, step: Step) {
        if !self.enabled(step) {
            return;
        }
        self.current = Some(match step {
            Step::First => 0,
            Step::Last => self.hunks.len() - 1,
            Step::Previous => self.current.unwrap_or(0).saturating_sub(1),
            Step::Next => (self.current.unwrap_or(0) + 1).min(self.hunks.len() - 1),
        });
        self.reveal = true;
    }
    pub fn reveal_current(&mut self) {
        self.reveal = self.current.is_some();
    }
    pub fn take_reveal(&mut self) -> Option<usize> {
        if std::mem::take(&mut self.reveal) {
            self.selected().map(|h| h.rows.start)
        } else {
            None
        }
    }
    pub fn position_label(&self) -> String {
        let Some(hunk) = self.selected() else {
            return "No differences".into();
        };
        let side = |start: usize, len: usize| {
            if len == 0 {
                format!("before line {start}")
            } else if len == 1 {
                format!("line {start}")
            } else {
                format!("lines {start}–{}", start + len - 1)
            }
        };
        format!(
            "Difference {} / {} · - file {} · + file {}",
            self.current.unwrap() + 1,
            self.hunks.len(),
            side(hunk.left_start, hunk.left_len),
            side(hunk.right_start, hunk.right_len)
        )
    }
}
/// Ctrl+U belongs to comparison navigation, never to an editing field.
/// A held key must not repeatedly reverse the operands.
pub fn consume_swap_shortcut(input: &mut egui::InputState) -> bool {
    let modifiers = input.events.iter().find_map(|event| match event {
        egui::Event::Key {
            key: egui::Key::U,
            pressed: true,
            repeat: false,
            modifiers,
            ..
        } if modifiers.ctrl && !modifiers.alt && !modifiers.shift && !modifiers.mac_cmd => {
            Some(*modifiers)
        }
        _ => None,
    });
    modifiers.is_some_and(|m| input.consume_key(m, egui::Key::U))
}

pub fn consume_shortcut(input: &mut egui::InputState) -> Option<Step> {
    for (key, step) in [
        (egui::Key::ArrowUp, Step::Previous),
        (egui::Key::ArrowDown, Step::Next),
        (egui::Key::Home, Step::First),
        (egui::Key::End, Step::Last),
    ] {
        let exact=input.events.iter().any(|event|matches!(event,egui::Event::Key {key:pressed,pressed:true,modifiers,..} if *pressed==key && *modifiers==egui::Modifiers::ALT));
        if exact && input.consume_key(egui::Modifiers::ALT, key) {
            return Some(step);
        }
    }
    None
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn swap_shortcut_requires_ctrl_without_other_modifiers_and_ignores_repeats() {
        for (modifiers, repeat, expected) in [
            (egui::Modifiers::CTRL, false, true),
            (
                egui::Modifiers {
                    ctrl: true,
                    command: true,
                    ..Default::default()
                },
                false,
                true,
            ),
            (egui::Modifiers::CTRL, true, false),
            (egui::Modifiers::NONE, false, false),
            (egui::Modifiers::COMMAND, false, false),
            (egui::Modifiers::CTRL | egui::Modifiers::SHIFT, false, false),
            (egui::Modifiers::CTRL | egui::Modifiers::ALT, false, false),
            (
                egui::Modifiers::CTRL | egui::Modifiers::MAC_CMD,
                false,
                false,
            ),
        ] {
            let ctx = egui::Context::default();
            // egui derives repeat from held-key state, overriding RawInput's flag.
            if repeat {
                crate::testutil::discard_egui_output(ctx.run_ui(
                    egui::RawInput {
                        events: vec![egui::Event::Key {
                            key: egui::Key::U,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers,
                        }],
                        ..Default::default()
                    },
                    |_| {},
                ));
            }
            let output = ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::U,
                        physical_key: None,
                        pressed: true,
                        repeat,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    assert_eq!(
                        ctx.input_mut(consume_swap_shortcut),
                        expected,
                        "{modifiers:?}, repeat={repeat}"
                    );
                    if expected {
                        assert!(!ctx.input_mut(consume_swap_shortcut));
                    }
                },
            );
            crate::testutil::discard_egui_output(output);
        }
    }

    #[test]
    fn source_numbers_follow_each_operand_through_unbalanced_changes() {
        let lines = crate::textdiff::diff_lines("a\nb\nc\nd\n", "a\nx\ny\nc\n").unwrap();
        let nav = Navigation::new(&lines);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        for (line, &(minus, plus)) in lines.iter().zip(&nav.source_numbers) {
            assert_eq!(minus.is_some(), line.kind != DiffKind::Insert);
            assert_eq!(plus.is_some(), line.kind != DiffKind::Delete);
            a.extend(minus);
            b.extend(plus);
        }
        assert_eq!(a, [1, 2, 3, 4]);
        assert_eq!(b, [1, 2, 3, 4]);
        assert!(!nav.show_numbers);
        assert!(Navigation::new(&[]).source_numbers.is_empty());
    }
    #[test]
    fn shortcuts_consume_only_exact_alt_navigation_keys() {
        let ctx = egui::Context::default();
        for (key, modifiers, expected) in [
            (egui::Key::ArrowDown, egui::Modifiers::ALT, Some(Step::Next)),
            (
                egui::Key::ArrowUp,
                egui::Modifiers::ALT,
                Some(Step::Previous),
            ),
            (egui::Key::Home, egui::Modifiers::ALT, Some(Step::First)),
            (egui::Key::End, egui::Modifiers::ALT, Some(Step::Last)),
            (egui::Key::ArrowDown, egui::Modifiers::NONE, None),
            (
                egui::Key::ArrowDown,
                egui::Modifiers {
                    alt: true,
                    shift: true,
                    ..Default::default()
                },
                None,
            ),
        ] {
            let mut actual = None;
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    }],
                    ..Default::default()
                },
                |ui| {
                    actual = ui.input_mut(consume_shortcut);
                    if expected.is_some() {
                        assert!(!ui.input(|input| input.key_pressed(key)));
                    }
                },
            ));
            assert_eq!(actual, expected);
        }
    }
    #[test]
    fn replacement_insertion_deletion_ranges_and_endings_keep_source_line_positions() {
        let lines = crate::textdiff::diff_lines(
            "same\nold\nkeep\ngone\ntail\n",
            "same\nnew\nkeep\ntail\nextra\n",
        )
        .unwrap();
        let nav = Navigation::new(&lines);
        assert_eq!(nav.hunks.len(), 3);
        assert_eq!(
            nav.hunks
                .iter()
                .map(|h| (h.left_start, h.left_len, h.right_start, h.right_len))
                .collect::<Vec<_>>(),
            [(2, 1, 2, 1), (4, 1, 4, 0), (6, 0, 5, 1)]
        );
        let endings =
            Navigation::new(&crate::textdiff::diff_lines("a\r\nlast", "a\nlast\n").unwrap());
        assert_eq!(endings.hunks.len(), 1);
        assert_eq!(endings.hunks[0].left_len, 2);
        assert_eq!(endings.hunks[0].right_len, 2);
        assert!(
            Navigation::new(&crate::textdiff::diff_lines("same\n", "same\n").unwrap())
                .hunks
                .is_empty()
        );
    }
    #[test]
    fn boundary_navigation_stops_and_repeated_first_last_reveal_after_manual_scroll() {
        let mut nav = Navigation::new(
            &crate::textdiff::diff_lines("a\nx\nb\ny\nc\n", "a\nX\nb\nY\nc\n").unwrap(),
        );
        assert_eq!(nav.current, Some(0));
        assert!(nav.take_reveal().is_some());
        assert_eq!(nav.take_reveal(), None);
        assert!(!nav.enabled(Step::Previous));
        nav.go(Step::Last);
        assert_eq!(nav.current, Some(1));
        let last = nav.take_reveal();
        assert!(last.is_some());
        nav.go(Step::Next);
        assert_eq!(nav.take_reveal(), None);
        nav.go(Step::Last);
        assert_eq!(nav.take_reveal(), last);
        nav.go(Step::Previous);
        assert_eq!(nav.current, Some(0));
        nav.go(Step::First);
        assert!(nav.take_reveal().is_some());
        assert!(nav.position_label().contains("Difference 1 / 2"));
        let mut empty = Navigation::default();
        for step in Step::ALL {
            assert!(!empty.enabled(step));
            empty.go(step);
        }
        assert!(empty.take_reveal().is_none());
    }
}
