//! Command palette (Cmd+K): fuzzy-filter every user-facing command, ranked by
//! recency/frequency, and run it. The catalog and ranking live in
//! `crate::command`.

use super::*;

impl App {
    pub(crate) fn show_palette_dialog(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.ws.requests.palette_request) {
            self.palette_input = Some(String::new());
        }
        if self.palette_input.is_none() {
            return;
        }
        let t = self.colors;

        // Rank from the query at frame start (owned, so editing the buffer
        // below does not conflict with reading the usage history).
        let query = self.palette_input.clone().unwrap();
        let matches = crate::command::rank(&query, &self.palette_usage, self.palette_tick);
        let buffer = self.palette_input.as_mut().unwrap();
        let mut run: Option<(&'static str, crate::command::Command)> = None;
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
                            for (i, m) in matches.iter().enumerate() {
                                let lead = if i == 0 { "\u{25b8} " } else { "   " };
                                let job = Self::palette_row_job(lead, m, t);
                                let resp = ui
                                    .add(egui::Label::new(job).sense(Sense::click()))
                                    .on_hover_text(m.shortcut);
                                if resp.clicked() {
                                    run = Some((m.label, m.command));
                                }
                            }
                        });
                }

                if ui.input(|i| i.key_pressed(egui::Key::Enter))
                    && let Some(m) = matches.first()
                {
                    run = Some((m.label, m.command));
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });

        if cancel {
            self.palette_input = None;
            return;
        }
        if let Some((label, cmd)) = run {
            // Close the palette first; the command may open another dialog.
            self.palette_input = None;
            // Record the run so it ranks higher next time.
            self.palette_tick += 1;
            self.palette_usage.record(label, self.palette_tick);
            self.ws.execute(cmd);
        }
    }

    /// Build a palette row as a [`LayoutJob`], tinting fuzzy-matched characters
    /// in the accent colour so the user sees why the row matched.
    fn palette_row_job(
        lead: &str,
        m: &crate::command::CommandMatch,
        t: ThemeColors,
    ) -> egui::text::LayoutJob {
        use egui::text::{LayoutJob, TextFormat};
        let font = egui::FontId::proportional(12.0);
        let mut job = LayoutJob::default();
        let fmt = |color| TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        };
        job.append(lead, 0.0, fmt(t.text_muted));
        let mut buf = [0u8; 4];
        for (idx, ch) in m.label.chars().enumerate() {
            let hit = m.matched.iter().any(|&(s, e)| idx >= s && idx < e);
            let color = if hit { t.accent } else { t.text_primary };
            job.append(ch.encode_utf8(&mut buf), 0.0, fmt(color));
        }
        job
    }
}
