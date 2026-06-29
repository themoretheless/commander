//! Recent-directories quick switcher (Cmd+P): type to filter the visited
//! list, click or Enter to jump the active panel. Logic lives in `panel`.

use super::*;

impl App {
    pub(crate) fn show_recent_dialog(&mut self, ctx: &egui::Context) {
        let just_opened = std::mem::take(&mut self.ws.recent_request);
        if just_opened {
            self.recent_input = Some(String::new());
        }
        let Some(buffer) = &mut self.recent_input else {
            return;
        };
        let t = self.colors;

        let visited = crate::panel::visited_paths();
        let matches = crate::panel::filter_visited(&visited, buffer);

        let mut go: Option<std::path::PathBuf> = None;
        let mut cancel = false;

        egui::Window::new("Recent folders")
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
                        .hint_text("Filter recent folders\u{2026}")
                        .margin(egui::vec2(8.0, 6.0)),
                );
                if just_opened {
                    resp.request_focus();
                }
                ui.add_space(6.0);

                if matches.is_empty() {
                    ui.label(
                        egui::RichText::new("No recent folders")
                            .size(11.0)
                            .color(t.text_muted),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .show(ui, |ui| {
                            for (i, path) in
                                matches.iter().enumerate().take(crate::panel::VISITED_CAP)
                            {
                                let name = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| path.display().to_string());
                                let parent = path
                                    .parent()
                                    .map(|p| p.display().to_string())
                                    .unwrap_or_default();
                                let resp = ui
                                    .add(
                                        egui::Label::new(egui::RichText::new(format!(
                                            "{}{}   {}",
                                            if i == 0 { "\u{25b8} " } else { "   " },
                                            name,
                                            parent
                                        )))
                                        .sense(Sense::click()),
                                    )
                                    .on_hover_text(path.display().to_string());
                                if resp.clicked() {
                                    go = Some(path.clone());
                                }
                            }
                        });
                }

                if ui.input(|i| i.key_pressed(egui::Key::Enter))
                    && let Some(p) = matches.first()
                {
                    go = Some(p.clone());
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });

        if cancel {
            self.recent_input = None;
            return;
        }
        if let Some(path) = go {
            self.recent_input = None;
            self.ws.active_panel().navigate_to(path);
        }
    }
}
