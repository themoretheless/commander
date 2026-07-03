//! Transfer-queue panel: lists every job waiting behind (or paused ahead of)
//! the active transfer, with pause/resume/reorder/cancel controls. The
//! currently-running job's own progress lives in the transfer dialog; this
//! panel manages what else is queued. Non-modal, toggled via the command
//! palette ("Transfer queue") so it can stay open alongside normal work.

use super::*;
use crate::opqueue::JobState;

impl App {
    pub(crate) fn show_queue_panel(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.ws.queue_panel_request) {
            self.show_queue_panel = !self.show_queue_panel;
        }
        if !self.show_queue_panel {
            return;
        }
        let t = self.colors;
        let rows = self.ws.queue_snapshot();

        let mut pause: Option<crate::opqueue::JobId> = None;
        let mut resume: Option<crate::opqueue::JobId> = None;
        let mut promote: Option<crate::opqueue::JobId> = None;
        let mut move_up: Option<crate::opqueue::JobId> = None;
        let mut move_down: Option<crate::opqueue::JobId> = None;
        let mut cancel: Option<crate::opqueue::JobId> = None;
        let mut clear_finished = false;
        let mut close = false;

        egui::Window::new("Transfer queue")
            .resizable(true)
            .collapsible(false)
            .default_width(420.0)
            .frame(
                Frame::NONE
                    .fill(t.bg_panel)
                    .inner_margin(Margin::same(14))
                    .stroke(Stroke::new(1.0_f32, t.border)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Transfer queue")
                            .size(13.0)
                            .strong()
                            .color(t.text_primary),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("\u{2715}").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(8.0);

                if rows.is_empty() {
                    ui.label(
                        egui::RichText::new("Nothing queued")
                            .size(12.0)
                            .color(t.text_muted),
                    );
                    return;
                }

                let pending_count = rows.iter().filter(|r| r.state == JobState::Pending).count();
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for row in &rows {
                            let (glyph, color) = match row.state {
                                JobState::Running => ("\u{25b6}", t.accent),
                                JobState::Pending => ("\u{2026}", t.text_muted),
                                JobState::Paused => ("\u{23f8}", t.accent_warning),
                                JobState::Done => ("\u{2713}", t.accent),
                                JobState::Failed => ("\u{2715}", t.accent_red),
                                JobState::Cancelled => ("\u{2715}", t.text_muted),
                            };
                            Frame::NONE
                                .fill(t.bg_card.linear_multiply(0.3))
                                .corner_radius(crate::theme::ROUNDING_SM)
                                .inner_margin(Margin::symmetric(8, 6))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(glyph).size(12.0).color(color),
                                        );
                                        ui.label(
                                            egui::RichText::new(&row.label)
                                                .size(12.0)
                                                .color(t.text_primary),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                let btn =
                                                    |ui: &mut egui::Ui, glyph: &str, hint: &str| {
                                                        ui.add(
                                                            egui::Button::new(
                                                                egui::RichText::new(glyph)
                                                                    .size(12.0),
                                                            )
                                                            .small(),
                                                        )
                                                        .on_hover_text(hint)
                                                        .clicked()
                                                    };
                                                // Cancel is always available while the job
                                                // hasn't already finished.
                                                if !row.state.is_terminal()
                                                    && btn(ui, "\u{2715}", "Cancel")
                                                {
                                                    cancel = Some(row.id);
                                                }
                                                match row.state {
                                                    JobState::Pending => {
                                                        if pending_count > 1 {
                                                            if btn(ui, "\u{2193}", "Move down") {
                                                                move_down = Some(row.id);
                                                            }
                                                            if btn(ui, "\u{2191}", "Move up") {
                                                                move_up = Some(row.id);
                                                            }
                                                            if btn(ui, "\u{23eb}", "Run next") {
                                                                promote = Some(row.id);
                                                            }
                                                        }
                                                        if btn(ui, "\u{23f8}", "Pause") {
                                                            pause = Some(row.id);
                                                        }
                                                    }
                                                    JobState::Paused
                                                        if btn(ui, "\u{25b6}", "Resume") =>
                                                    {
                                                        resume = Some(row.id);
                                                    }
                                                    _ => {}
                                                }
                                            },
                                        );
                                    });
                                });
                            ui.add_space(3.0);
                        }
                    });

                let any_finished = rows.iter().any(|r| r.state.is_terminal());
                if any_finished {
                    ui.add_space(6.0);
                    if ui.small_button("Clear finished").clicked() {
                        clear_finished = true;
                    }
                }
            });

        if let Some(id) = pause {
            self.ws.queue_pause(id);
        }
        if let Some(id) = resume {
            self.ws.queue_resume(id);
        }
        if let Some(id) = promote {
            self.ws.queue_promote(id);
        }
        if let Some(id) = move_up {
            self.ws.queue_move(id, -1);
        }
        if let Some(id) = move_down {
            self.ws.queue_move(id, 1);
        }
        if let Some(id) = cancel {
            self.ws.queue_cancel(id);
        }
        if clear_finished {
            self.ws.queue_clear_finished();
        }
        if close {
            self.show_queue_panel = false;
        }
    }
}
