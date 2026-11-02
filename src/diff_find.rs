//! Cached row search over the immutable displayed edit script, including folded rows.
use crate::textdiff::DiffLine;
const LIMIT: usize = 4096;
pub fn query_id() -> egui::Id {
    egui::Id::new("diff-find-input")
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Focus,
    GoToLine,
    Reload,
    Next,
    Previous,
}
pub fn consume_shortcut(input: &mut egui::InputState) -> Option<Shortcut> {
    let found = input.events.iter().find_map(|event| {
        let egui::Event::Key {
            key,
            pressed: true,
            modifiers: m,
            ..
        } = event
        else {
            return None;
        };
        let action = match *key {
            egui::Key::F
                if !m.alt && !m.shift && ((m.ctrl && !m.mac_cmd) || (m.command && !m.ctrl)) =>
            {
                Shortcut::Focus
            }
            egui::Key::G
                if !m.alt && !m.shift && ((m.ctrl && !m.mac_cmd) || (m.command && !m.ctrl)) =>
            {
                Shortcut::GoToLine
            }
            egui::Key::R
                if !m.alt && !m.shift && ((m.ctrl && !m.mac_cmd) || (m.command && !m.ctrl)) =>
            {
                Shortcut::Reload
            }
            egui::Key::F7 if *m == egui::Modifiers::NONE => Shortcut::Focus,
            egui::Key::F3 if *m == egui::Modifiers::NONE => Shortcut::Next,
            egui::Key::F3 if *m == egui::Modifiers::SHIFT => Shortcut::Previous,
            _ => return None,
        };
        Some((*key, *m, action))
    });
    found.and_then(|(key, modifiers, action)| input.consume_key(modifiers, key).then_some(action))
}
#[derive(Default)]
pub struct Find {
    pub query: String,
    pub case: bool,
    pub regex: bool,
    pub words: bool,
    key: Option<(String, bool, bool, bool)>,
    pub rows: Vec<usize>,
    pub current: Option<usize>,
    pub error: Option<String>,
    pub limited: bool,
    pub focused: bool,
    pub pending_reveal: Option<usize>,
}
impl Find {
    pub fn refresh(&mut self, lines: &[DiffLine]) -> bool {
        let key = (self.query.clone(), self.case, self.regex, self.words);
        if self.key.as_ref() == Some(&key) {
            return false;
        }
        self.key = Some(key);
        self.rows.clear();
        self.current = None;
        self.error = None;
        self.limited = false;
        if self.query.is_empty() {
            self.focused = false;
            return true;
        }
        if self.query.chars().count() > 256 {
            self.focused = false;
            self.error = Some("Find is limited to 256 characters.".into());
            return true;
        }
        match crate::preview_find::compile_search_pattern(
            &self.query,
            self.case,
            self.regex,
            self.words,
        ) {
            Err(error) => self.error = Some(error.to_string()),
            Ok(pattern) => {
                for (i, line) in lines.iter().enumerate() {
                    if pattern.is_match(&line.text) {
                        if self.rows.len() == LIMIT {
                            self.limited = true;
                            break;
                        }
                        self.rows.push(i);
                    }
                }
            }
        }
        self.current = (!self.rows.is_empty()).then_some(0);
        self.focused = self.current.is_some();
        true
    }
    pub fn selected(&self) -> Option<usize> {
        self.current.and_then(|i| self.rows.get(i)).copied()
    }
    pub fn step(&mut self, backwards: bool) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }
        let n = self.rows.len();
        let i = self.current.unwrap_or(0);
        self.current = Some(if backwards {
            (i + n - 1) % n
        } else {
            (i + 1) % n
        });
        self.focused = true;
        self.selected()
    }
    pub fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        lines: &[DiffLine],
    ) -> (Option<usize>, Vec<egui::Id>) {
        let mut reveal = self.pending_reveal.take();
        let mut ids = Vec::new();
        ui.horizontal_wrapped(|ui| {
            // Consume before TextEdit: single-line Enter otherwise surrenders focus.
            let enter = ui.ctx().memory(|m| m.has_focus(query_id()))
                && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .id(query_id())
                    .hint_text("Find in diff")
                    .char_limit(256)
                    .desired_width(170.0),
            );

            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx()
                .data_mut(|data| data.insert_temp(egui::Id::new("diff-find-query"), response.rect));
            ui.checkbox(&mut self.case, "Match case");
            ui.checkbox(&mut self.words, "Whole words");
            ui.checkbox(&mut self.regex, "Regex");
            if self.refresh(lines) {
                reveal = self.selected();
            }
            for (back, label, key) in [
                (true, "Previous match", "prev"),
                (false, "Next match", "next"),
            ] {
                let response = ui.add_enabled(!self.rows.is_empty(), egui::Button::new(label));
                ids.push(response.id);
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|data| {
                    data.insert_temp(egui::Id::new(("diff-find", key)), response.rect)
                });
                #[cfg(not(any(test, feature = "visual-qa")))]
                let _ = key;
                if response.clicked() || (enter && !back) {
                    reveal = self.step(back);
                }
            }
            if ui.button("Clear find").clicked() {
                self.query.clear();
                self.refresh(lines);
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        } else if !self.query.is_empty() {
            ui.small(format!(
                "Matching row {} / {}{} · includes hidden context",
                self.current.map_or(0, |i| i + 1),
                self.rows.len(),
                if self.limited {
                    " (first 4096; more exist)"
                } else {
                    ""
                }
            ));
        }
        (reveal, ids)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn lines(text: &str) -> Vec<DiffLine> {
        text.lines()
            .map(|s| DiffLine {
                kind: crate::textdiff::DiffKind::Equal,
                text: s.into(),
            })
            .collect()
    }
    #[test]
    fn search_shortcuts_consume_only_exact_keys_and_modifiers() {
        let ctx = egui::Context::default();
        for (key, modifiers, expected) in [
            (
                egui::Key::F,
                egui::Modifiers::COMMAND,
                Some(Shortcut::Focus),
            ),
            (egui::Key::F, egui::Modifiers::CTRL, Some(Shortcut::Focus)),
            (
                egui::Key::G,
                egui::Modifiers::COMMAND,
                Some(Shortcut::GoToLine),
            ),
            (
                egui::Key::G,
                egui::Modifiers::CTRL,
                Some(Shortcut::GoToLine),
            ),
            (egui::Key::G, egui::Modifiers::ALT, None),
            (
                egui::Key::R,
                egui::Modifiers::COMMAND,
                Some(Shortcut::Reload),
            ),
            (egui::Key::R, egui::Modifiers::CTRL, Some(Shortcut::Reload)),
            (egui::Key::R, egui::Modifiers::NONE, None),
            (egui::Key::R, egui::Modifiers::ALT, None),
            (egui::Key::F7, egui::Modifiers::NONE, Some(Shortcut::Focus)),
            (egui::Key::F3, egui::Modifiers::NONE, Some(Shortcut::Next)),
            (
                egui::Key::F3,
                egui::Modifiers::SHIFT,
                Some(Shortcut::Previous),
            ),
            (egui::Key::F, egui::Modifiers::NONE, None),
            (egui::Key::F3, egui::Modifiers::CTRL, None),
            (egui::Key::F7, egui::Modifiers::ALT, None),
            (
                egui::Key::F,
                egui::Modifiers {
                    alt: true,
                    command: true,
                    ..Default::default()
                },
                None,
            ),
        ] {
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
                    assert_eq!(ui.input_mut(consume_shortcut), expected);
                    assert_eq!(ui.input(|i| i.key_pressed(key)), expected.is_none());
                },
            ));
        }
    }
    #[test]
    fn focused_query_enter_advances_and_pending_keyboard_reveal_is_one_shot() {
        let ctx = egui::Context::default();
        let rows = lines("hit\nother\nhit");
        let mut f = Find {
            query: "hit".into(),
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
            assert_eq!(f.toolbar(ui, &rows).0, Some(0));
        }));
        ctx.memory_mut(|m| m.request_focus(query_id()));
        crate::testutil::discard_egui_output(ctx.run_ui(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| {
                assert_eq!(f.toolbar(ui, &rows).0, Some(2));
            },
        ));
        f.pending_reveal = f.step(true);
        crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
            assert_eq!(f.toolbar(ui, &rows).0, Some(0));
        }));
        crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
            assert_eq!(f.toolbar(ui, &rows).0, None);
        }));
    }
    #[test]
    fn literal_unicode_case_words_regex_errors_and_cached_navigation() {
        let rows = lines("Кот\nкотик\nкот кот\na.b\naxb");
        let mut f = Find {
            query: "кот".into(),
            ..Default::default()
        };
        assert!(f.refresh(&rows));
        assert_eq!(f.rows, [0, 1, 2]);
        assert_eq!(f.step(true), Some(2));
        assert!(!f.refresh(&rows));
        assert_eq!(f.selected(), Some(2));
        f.words = true;
        f.refresh(&rows);
        assert_eq!(f.rows, [0, 2]);
        f.case = true;
        f.refresh(&rows);
        assert_eq!(f.rows, [2]);
        f.query = "a.b".into();
        f.words = false;
        f.refresh(&rows);
        assert_eq!(f.rows, [3]);
        f.regex = true;
        f.refresh(&rows);
        assert_eq!(f.rows, [3, 4]);
        f.query = "[".into();
        f.refresh(&rows);
        assert!(f.error.is_some());
        assert!(f.rows.is_empty());
        assert_eq!(f.step(false), None);
        f.query.clear();
        f.refresh(&rows);
        assert!(f.error.is_none());
        assert!(!f.focused);
    }
    #[test]
    fn limits_and_hidden_equal_rows_keep_original_ids() {
        let rows = lines(&"match\n".repeat(4100));
        let mut f = Find {
            query: "match".into(),
            ..Default::default()
        };
        f.refresh(&rows);
        assert_eq!(f.rows.len(), 4096);
        assert!(f.limited);
        f.query = "x".repeat(257);
        f.refresh(&rows);
        assert!(f.error.is_some());
        assert!(f.rows.is_empty());
        let rows = crate::textdiff::diff_lines("a\nfind\nb\nold\n", "a\nfind\nb\nnew\n").unwrap();
        let nav = crate::diff_navigation::Navigation::new(&rows);
        let mut view = crate::diff_context::Context::default();
        view.compact = true;
        view.context = 0;
        view.rebuild(rows.len(), &nav.hunks);
        f.query = "find".into();
        f.refresh(&rows);
        let original = f.selected().unwrap();
        assert_eq!(original, 1);
        assert!(view.reveal(original, &nav.hunks));
        assert_eq!(
            view.row(view.display_index(original).unwrap()),
            Some(crate::diff_context::Row::Line(original))
        );
        assert!(!view.reveal(original, &nav.hunks));
        assert!(!view.reveal(999, &nav.hunks));
    }
}
