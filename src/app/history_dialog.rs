//! Filesystem-aware undo/redo review.

use super::*;

impl App {
    pub(crate) fn push_history_notice(&mut self, ctx: &egui::Context, message: &str, error: bool) {
        self.toasts.push(crate::toasts::Toast::new(
            message,
            if error {
                crate::toasts::ToastKind::Error
            } else {
                crate::toasts::ToastKind::Info
            },
            false,
            ctx.input(|input| input.time),
        ));
    }

    pub(crate) fn show_history_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.history_preview.take() else {
            return;
        };
        let t = self.colors;
        let is_undo = state.mode == HistoryReplayMode::Undo;
        let title = if is_undo {
            "Undo preview"
        } else {
            "Redo preview"
        };
        let command = if is_undo { "Undo" } else { "Redo" };
        let replay_blocker = self.ws.history_replay_blocker();
        let paths_ready = state.preview.can_execute();
        let can_execute = paths_ready && replay_blocker.is_none();
        let ready = state.preview.paths.len() - state.preview.blocked_count();
        let mut cancel = false;
        let mut confirm = false;

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                Frame::NONE
                    .fill(t.bg_panel)
                    .inner_margin(Margin::same(14))
                    .stroke(Stroke::new(1.0, t.border)),
            )
            .show(ctx, |ui| {
                ui.set_width(620.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(title)
                                .size(14.0)
                                .strong()
                                .color(t.text_primary),
                        );
                        ui.label(
                            egui::RichText::new(state.preview.action.description())
                                .size(11.0)
                                .color(t.text_secondary),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let color = if paths_ready { t.accent } else { t.accent_red };
                        ui.label(
                            egui::RichText::new(format!(
                                "{ready}/{} ready",
                                state.preview.paths.len()
                            ))
                            .size(11.0)
                            .strong()
                            .color(color),
                        );
                    });
                });

                if let Some(error) = &state.error {
                    ui.add_space(7.0);
                    ui.label(egui::RichText::new(error).size(11.0).color(t.accent_red));
                }
                if let Some(error) = &replay_blocker {
                    ui.add_space(7.0);
                    ui.label(
                        egui::RichText::new(error)
                            .size(11.0)
                            .color(t.accent_warning),
                    );
                }
                for warning in &state.preview.warnings {
                    ui.add_space(7.0);
                    ui.label(
                        egui::RichText::new(warning)
                            .size(11.0)
                            .color(t.accent_warning),
                    );
                }

                ui.add_space(8.0);
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(330.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for path in &state.preview.paths {
                            let (status, status_color, reason) = match &path.eligibility {
                                crate::undo::ReplayEligibility::Ready => ("READY", t.accent, None),
                                crate::undo::ReplayEligibility::Blocked(reason) => {
                                    ("BLOCKED", t.accent_red, Some(reason.as_str()))
                                }
                            };
                            Frame::NONE
                                .fill(t.bg_card)
                                .inner_margin(Margin::symmetric(9, 7))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(status)
                                                .size(9.0)
                                                .strong()
                                                .color(status_color),
                                        );
                                        ui.vertical(|ui| {
                                            ui.label(
                                                egui::RichText::new(
                                                    path.from.display().to_string(),
                                                )
                                                .size(10.0)
                                                .monospace()
                                                .color(t.text_primary),
                                            );
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "\u{2192} {}",
                                                    path.to.display()
                                                ))
                                                .size(10.0)
                                                .monospace()
                                                .color(t.text_secondary),
                                            );
                                            if let Some(reason) = reason {
                                                ui.label(
                                                    egui::RichText::new(reason)
                                                        .size(10.0)
                                                        .color(t.accent_red),
                                                );
                                            }
                                        });
                                    });
                                });
                            ui.add_space(4.0);
                        }
                    });

                ui.separator();
                ui.add_space(7.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Cancel")
                                    .size(12.0)
                                    .color(t.text_primary),
                            )
                            .fill(t.bg_card)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                    {
                        cancel = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                can_execute,
                                egui::Button::new(
                                    egui::RichText::new(command)
                                        .size(12.0)
                                        .color(Color32::WHITE),
                                )
                                .fill(t.accent)
                                .corner_radius(CornerRadius::ZERO),
                            )
                            .clicked()
                        {
                            confirm = true;
                        }
                    });
                });

                if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
                if can_execute && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                    confirm = true;
                }
            });

        if cancel {
            return;
        }
        if confirm {
            let repaint = ctx.clone();
            let result = if is_undo {
                self.ws.perform_undo(move || repaint.request_repaint())
            } else {
                self.ws.perform_redo(move || repaint.request_repaint())
            };
            match result {
                Ok(()) => {
                    if is_undo {
                        self.toasts.dismiss_undoable();
                    }
                    return;
                }
                Err(error) => {
                    state.error = Some(error);
                    let refreshed = if is_undo {
                        self.ws.preview_undo()
                    } else {
                        self.ws.preview_redo()
                    };
                    if let Some(preview) = refreshed {
                        state.preview = preview;
                    }
                }
            }
        }
        self.history_preview = Some(state);
    }
}
