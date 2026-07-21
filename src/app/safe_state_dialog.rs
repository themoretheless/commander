//! Blocking review surface for integrity-uncertain operation failures.

use super::*;

fn review_recovery_request(operation_id: &crate::operation::OperationId) -> UiRequest {
    UiRequest::ReviewRecovery(operation_id.clone())
}

impl App {
    pub(crate) fn show_safe_state_dialog(&mut self, ctx: &egui::Context) {
        if self.recovery.open {
            return;
        }
        let Some(state) = self.ws.safe_state.clone() else {
            return;
        };
        let t = self.colors;
        let mut acknowledge = false;
        let mut inspect = false;
        let mut recovery = false;

        egui::Window::new("Safe state")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                Frame::NONE
                    .fill(t.bg_panel)
                    .inner_margin(Margin::same(16))
                    .stroke(Stroke::new(1.0, t.accent_warning)),
            )
            .show(ctx, |ui| {
                ui.set_width(560.0);
                ui.label(
                    egui::RichText::new("Operation paused for integrity review")
                        .size(13.0)
                        .strong()
                        .color(t.accent_warning),
                );
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(&state.reason)
                        .size(12.0)
                        .color(t.text_primary),
                );
                ui.label(
                    egui::RichText::new(&state.operation_id.0)
                        .size(10.0)
                        .monospace()
                        .color(t.text_muted),
                );
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);

                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for failure in &state.failures {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    egui::RichText::new(failure.class.label())
                                        .size(10.0)
                                        .strong()
                                        .color(t.accent_warning),
                                );
                                ui.label(
                                    egui::RichText::new(&failure.message)
                                        .size(11.0)
                                        .color(t.text_secondary),
                                );
                            });
                            if let Some(path) = &failure.path {
                                ui.label(
                                    egui::RichText::new(path.display().to_string())
                                        .size(10.0)
                                        .monospace()
                                        .color(t.text_muted),
                                );
                            }
                            ui.add_space(4.0);
                        }
                    });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Recovery center")
                                    .size(13.0)
                                    .color(t.text_primary),
                            )
                            .fill(t.bg_card)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                    {
                        recovery = true;
                    }
                    if !state.paths.is_empty()
                        && ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Show location")
                                        .size(13.0)
                                        .color(t.text_primary),
                                )
                                .fill(t.bg_card)
                                .corner_radius(CornerRadius::ZERO),
                            )
                            .clicked()
                    {
                        inspect = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Review complete")
                                        .size(13.0)
                                        .color(Color32::WHITE),
                                )
                                .fill(t.accent)
                                .corner_radius(CornerRadius::ZERO),
                            )
                            .clicked()
                        {
                            acknowledge = true;
                        }
                    });
                });
            });

        if inspect
            && let Some(parent) = state.paths.first().and_then(|path| path.parent())
            && parent.is_dir()
        {
            self.ws.active_panel().navigate_to(parent.to_path_buf());
        }
        if acknowledge {
            self.ws.acknowledge_safe_state();
        }
        if recovery {
            self.ws
                .emit_ui_request(review_recovery_request(&state.operation_id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_state_review_preserves_the_operation_identity() {
        let operation_id = crate::operation::OperationId("safe-state-op".to_string());

        assert_eq!(
            review_recovery_request(&operation_id),
            UiRequest::ReviewRecovery(operation_id)
        );
    }
}
