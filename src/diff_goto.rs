//! Explicit source-operand line jumps over the retained edit-script coordinates.
pub fn input_id() -> egui::Id {
    egui::Id::new("diff-goto-input")
}
#[derive(Default)]
pub struct Jump {
    pub plus: bool,
    pub query: String,
    pub error: Option<String>,
    pub selected: Option<usize>,
}
impl Jump {
    pub fn resolve(&mut self, coordinates: &[(Option<usize>, Option<usize>)]) -> Option<usize> {
        let query = self.query.trim();
        let line = (!query.is_empty() && query.bytes().all(|b| b.is_ascii_digit()))
            .then(|| query.parse::<usize>().ok())
            .flatten()
            .filter(|n| *n > 0);
        let row = line.and_then(|line| {
            coordinates.iter().position(|&(minus, plus)| {
                if self.plus {
                    plus == Some(line)
                } else {
                    minus == Some(line)
                }
            })
        });
        if let Some(row) = row {
            self.error = None;
            self.selected = Some(row);
        } else {
            let total = coordinates
                .iter()
                .rev()
                .find_map(|&(minus, plus)| if self.plus { plus } else { minus })
                .unwrap_or(0);
            self.error = Some(if total == 0 {
                "This operand has no source lines.".into()
            } else {
                format!("Enter a source line from 1 to {total}.")
            });
        }
        row
    }
    pub fn toolbar(
        &mut self,
        ui: &mut egui::Ui,
        coordinates: &[(Option<usize>, Option<usize>)],
    ) -> (Option<usize>, Vec<egui::Id>) {
        let mut row = None;
        let mut ids = Vec::new();
        ui.horizontal_wrapped(|ui| {
            ui.label("Go to source line");
            for (plus, label) in [(false, "- file"), (true, "+ file")] {
                let response = ui.selectable_value(&mut self.plus, plus, label);
                ids.push(response.id);
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|data| {
                    data.insert_temp(egui::Id::new(("diff-goto-side", plus)), response.rect)
                });
                if response.changed() {
                    self.error = None;
                }
            }
            let enter = ui.ctx().memory(|m| m.has_focus(input_id()))
                && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .id(input_id())
                    .char_limit(20)
                    .desired_width(75.0)
                    .hint_text("Line"),
            );
            if response.changed() {
                self.error = None;
            }
            let response = ui.button("Go to line");
            ids.push(response.id);
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx()
                .data_mut(|data| data.insert_temp(egui::Id::new("diff-goto-go"), response.rect));
            if response.clicked() || enter {
                row = self.resolve(coordinates);
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        (row, ids)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operand_lines_map_to_original_insert_delete_and_equal_rows() {
        let rows = crate::textdiff::diff_lines("a\ngone\nb\n", "a\nb\nadded\nextra\n").unwrap();
        let nav = crate::diff_navigation::Navigation::new(&rows);
        let mut jump = Jump::default();
        for plus in [false, true] {
            jump.plus = plus;
            for line in 1..=if plus { 4 } else { 3 } {
                jump.query = line.to_string();
                let row = jump.resolve(&nav.source_numbers).unwrap();
                assert_eq!(
                    if plus {
                        nav.source_numbers[row].1
                    } else {
                        nav.source_numbers[row].0
                    },
                    Some(line)
                );
                assert!(jump.error.is_none());
            }
        }
        let retained = jump.selected;
        for query in [
            "0",
            "-1",
            "+1",
            "1.0",
            "1e2",
            "x",
            "",
            "999999999999999999999999999999999",
        ] {
            jump.query = query.into();
            assert_eq!(jump.resolve(&nav.source_numbers), None);
            assert!(jump.error.is_some());
            assert_eq!(jump.selected, retained);
        }
        jump.query = " 1 ".into();
        assert_eq!(jump.resolve(&nav.source_numbers), Some(0));
        assert_eq!(jump.resolve(&[]), None);
        assert!(jump.error.as_ref().unwrap().contains("no source lines"));
    }
    #[test]
    fn focused_enter_resolves_hidden_line_without_changing_query() {
        let rows = crate::textdiff::diff_lines("a\nseek\nb\nold\n", "a\nseek\nb\nnew\n").unwrap();
        let nav = crate::diff_navigation::Navigation::new(&rows);
        let mut view = crate::diff_context::Context::default();
        view.compact = true;
        view.context = 0;
        view.rebuild(rows.len(), &nav.hunks);
        let ctx = egui::Context::default();
        let mut jump = Jump {
            query: "2".into(),
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
            jump.toolbar(ui, &nav.source_numbers);
        }));
        ctx.memory_mut(|m| m.request_focus(input_id()));
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
                let row = jump.toolbar(ui, &nav.source_numbers).0.unwrap();
                assert_eq!(row, 1);
                assert!(view.reveal(row, &nav.hunks));
                assert_eq!(
                    view.row(view.display_index(row).unwrap()),
                    Some(crate::diff_context::Row::Line(1))
                );
            },
        ));
        assert_eq!(jump.query, "2");
    }
}
