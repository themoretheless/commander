//! Select-by-mask input: a small modal that toggles the selection by a
//! glob/extension mask. The matching logic lives in `panel`.

use super::*;

impl App {
    pub(crate) fn show_mask_dialog(&mut self, ctx: &egui::Context) {
        let Some(buffer) = &mut self.ui.mask_input else {
            return;
        };
        let t = self.colors;

        let mut commit = false;
        let mut cancel = false;
        let mut focus = false;

        // Live match count against the active panel.
        let count = self.ws.active_panel_ref().mask_match_count(buffer);

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
                if resp.lost_focus() {
                    // Keep focus across frames until committed/cancelled.
                    focus = true;
                }
                resp.request_focus();

                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(format!("{count} match"))
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
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancel = true;
                    }
                });
            });
        let _ = focus;

        if cancel {
            self.ui.mask_input = None;
            return;
        }
        if commit && let Some(buf) = self.ui.mask_input.take() {
            self.ws.active_panel().select_by_mask(&buf);
        }
    }
}
