//! Select-by-mask input: a small modal that toggles the selection by a
//! glob/extension mask. The matching logic lives in `panel`.

use super::*;

fn change_count_label(count: usize) -> String {
    if count == 1 {
        "1 change".to_string()
    } else {
        format!("{count} changes")
    }
}

impl App {
    pub(crate) fn open_mask(&mut self, ctx: &egui::Context) {
        self.mask_input = Some(String::new());
        Self::mark_modal_opened(ctx, UiModal::Mask);
    }

    pub(crate) fn show_mask_dialog(&mut self, ctx: &egui::Context) {
        let just_opened = Self::take_modal_opened(ctx, UiModal::Mask);
        let escape_requested = self.take_escape_request(crate::accessibility::EscapeRoute::Modal(
            crate::accessibility::ModalSurface::Mask,
        ));
        let active_panel = self.ws.active_panel_ref();
        let Some(buffer) = &mut self.mask_input else {
            return;
        };
        let t = self.colors;

        let mut commit = false;
        let mut cancel = false;

        egui::Window::new("Select by mask")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                Frame::NONE
                    .fill(t.bg_panel)
                    .inner_margin(Margin::same(14))
                    .stroke(Stroke::new(1.0_f32, t.border)),
            )
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(
                    egui::RichText::new("Select by mask  (e.g.  *.rs, !*test*)")
                        .size(12.0)
                        .color(t.text_muted),
                );
                ui.add_space(6.0);

                let edit = egui::TextEdit::singleline(buffer)
                    .desired_width(f32::INFINITY)
                    .hint_text("*.jpg, !*raw*")
                    .margin(egui::vec2(8.0, 6.0));
                let resp = ui.add(edit);
                // Grab focus on the opening frame only; yanking it every frame
                // would trap the caret and stop the user clicking the buttons.
                if just_opened {
                    resp.request_focus();
                }

                // Count after TextEdit so the label and Enter action observe
                // the exact same buffer from this frame.
                let count = active_panel.mask_match_count(buffer);
                let change_label = change_count_label(count);
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(&change_label)
                        .size(11.0)
                        .color(t.accent),
                );

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Select")
                                    .size(13.0)
                                    .color(Color32::WHITE),
                            )
                            .fill(t.accent)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                    {
                        commit = true;
                    }
                    ui.add_space(8.0);
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Cancel")
                                    .size(13.0)
                                    .color(t.text_primary),
                            )
                            .fill(t.bg_card)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                    {
                        cancel = true;
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        commit = true;
                    }
                    if escape_requested {
                        cancel = true;
                    }
                });
            });

        if cancel {
            self.mask_input = None;
            return;
        }
        if commit && let Some(buf) = self.mask_input.take() {
            self.ws.active_panel().select_by_mask(&buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_labels_selection_changes_not_pattern_matches() {
        assert_eq!(change_count_label(0), "0 changes");
        assert_eq!(change_count_label(1), "1 change");
        assert_eq!(change_count_label(7), "7 changes");
    }
}
