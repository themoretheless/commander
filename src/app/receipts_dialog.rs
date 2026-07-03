//! Operation history (palette: "Operation history"): a searchable log of
//! completed moves, deletes and batch renames, each with a jump-back button
//! and, while it's still exactly the top of the undo stack, a live Undo
//! button. Logic lives in `crate::receipts`.

use super::*;

impl App {
    pub(crate) fn show_receipts_dialog(&mut self, ctx: &egui::Context) {
        let just_opened = std::mem::take(&mut self.ws.receipts_request);
        if just_opened {
            self.receipts_input = Some(String::new());
        }
        let Some(buffer) = &mut self.receipts_input else {
            return;
        };
        let t = self.colors;
        let now = ctx.input(|i| i.time);
        let top = self.ws.stack.peek_undo().cloned();
        let results = self.receipts.search(buffer);

        let mut jump: Option<std::path::PathBuf> = None;
        let mut undo = false;
        let mut cancel = false;

        egui::Window::new("Operation history")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                Frame::NONE
                    .fill(t.bg_panel)
                    .inner_margin(Margin::same(12))
                    .stroke(Stroke::new(1.0_f32, t.border)),
            )
            .show(ctx, |ui| {
                ui.set_width(480.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(buffer)
                        .desired_width(f32::INFINITY)
                        .hint_text("Search operation history\u{2026}")
                        .margin(egui::vec2(8.0, 6.0)),
                );
                if just_opened {
                    resp.request_focus();
                }
                ui.add_space(6.0);

                if results.is_empty() {
                    ui.label(
                        egui::RichText::new("No matching operations")
                            .size(11.0)
                            .color(t.text_muted),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for receipt in &results {
                                let live_undo = receipt.still_undoable(top.as_ref());
                                Frame::NONE
                                    .inner_margin(Margin::symmetric(4, 3))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.vertical(|ui| {
                                                ui.label(
                                                    egui::RichText::new(receipt.label())
                                                        .size(12.0)
                                                        .color(t.text_primary),
                                                );
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "{} \u{00b7} {}",
                                                        crate::receipts::format_elapsed(
                                                            now,
                                                            receipt.timestamp
                                                        ),
                                                        receipt.jump_to.display()
                                                    ))
                                                    .size(10.0)
                                                    .color(t.text_muted),
                                                );
                                            });
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    if live_undo
                                                        && ui
                                                            .small_button("Undo \u{2318}Z")
                                                            .clicked()
                                                    {
                                                        undo = true;
                                                        jump = None;
                                                    }
                                                    if ui.small_button("Jump").clicked() {
                                                        jump = Some(receipt.jump_to.clone());
                                                    }
                                                },
                                            );
                                        });
                                    });
                            }
                        });
                }

                if ui.input(|i| i.key_pressed(egui::Key::Enter))
                    && let Some(r) = results.first()
                {
                    jump = Some(r.jump_to.clone());
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });

        if cancel {
            self.receipts_input = None;
            return;
        }
        if undo {
            self.receipts_input = None;
            self.ws.undo_request = true;
            return;
        }
        if let Some(path) = jump {
            self.receipts_input = None;
            self.ws.active_panel().navigate_to(path);
        }
    }
}
