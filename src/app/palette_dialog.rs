//! Command palette (Cmd+K): fuzzy-filter every user-facing command and run
//! it. The catalog and filter live in `crate::command`.

use super::*;
use crate::command::filter_commands;

impl App {
    pub(crate) fn show_palette_dialog(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.ws.palette_request) {
            self.palette_input = Some(String::new());
        }
        let Some(buffer) = &mut self.palette_input else {
            return;
        };
        let t = self.colors;

        let matches = filter_commands(buffer);
        let mut run: Option<crate::command::Command> = None;
        let mut cancel = false;
        let mut first = false;

        egui::Window::new("Command palette")
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
                ui.set_width(460.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(buffer)
                        .desired_width(f32::INFINITY)
                        .hint_text("Type a command\u{2026}")
                        .margin(egui::vec2(8.0, 6.0)),
                );
                if !first {
                    resp.request_focus();
                    first = true;
                }
                ui.add_space(6.0);

                if matches.is_empty() {
                    ui.label(
                        egui::RichText::new("No matching command")
                            .size(11.0)
                            .color(t.text_muted),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            for (i, (label, shortcut, cmd)) in matches.iter().enumerate() {
                                let lead = if i == 0 { "\u{25b8} " } else { "   " };
                                let resp = ui
                                    .add(
                                        egui::Label::new(
                                            egui::RichText::new(format!("{lead}{label}"))
                                                .size(12.0)
                                                .color(t.text_primary),
                                        )
                                        .sense(Sense::click()),
                                    )
                                    .on_hover_text(*shortcut);
                                if resp.clicked() {
                                    run = Some(*cmd);
                                }
                            }
                        });
                }

                if ui.input(|i| i.key_pressed(egui::Key::Enter))
                    && let Some((_, _, cmd)) = matches.first()
                {
                    run = Some(*cmd);
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });

        if cancel {
            self.palette_input = None;
            return;
        }
        if let Some(cmd) = run {
            // Close the palette first; the command may open another dialog.
            self.palette_input = None;
            self.ws.execute(cmd);
        }
    }
}
