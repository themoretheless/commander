//! Recent-directories quick switcher (Cmd+P): type to filter the visited
//! list, click or Enter to jump the active panel. Logic lives in `panel`.

use super::*;

impl App {
    pub(crate) fn open_recent(&mut self, ctx: &egui::Context) {
        self.ui.modals.recent_input = Some(String::new());
        Self::mark_modal_opened(ctx, UiModal::Recent);
    }

    pub(crate) fn show_recent_dialog(&mut self, ctx: &egui::Context) {
        let just_opened = Self::take_modal_opened(ctx, UiModal::Recent);
        let escape_requested = self.take_modal_escape(crate::accessibility::ModalSurface::Recent);
        let Some(buffer) = &mut self.ui.modals.recent_input else {
            return;
        };
        let t = self.colors;

        let (visited, stats) = crate::panel::visit_snapshot();
        let mut order = self.recent_order;
        let matches = crate::panel::rank_visited(&visited, buffer, order, &stats);

        let mut go: Option<std::path::PathBuf> = None;
        let mut cancel = false;

        // The title is hidden (title_bar(false)); the string serves as the
        // window's egui Id, so keep it unique and stable.
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
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut order,
                        crate::panel::RecentOrder::Frecency,
                        "Frecency",
                    );
                    ui.selectable_value(
                        &mut order,
                        crate::panel::RecentOrder::Chronological,
                        "Recent",
                    );
                });
                ui.add_space(4.0);

                if matches.is_empty() {
                    let message = if visited.is_empty() {
                        "No recent folders yet".to_string()
                    } else if buffer.trim().is_empty() {
                        "No recent folders".to_string()
                    } else {
                        format!("No matches for \"{}\"", buffer.trim())
                    };
                    ui.label(
                        egui::RichText::new(message)
                            .size(11.0)
                            .color(t.text_muted),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .show(ui, |ui| {
                            for (i, item) in matches.iter().enumerate() {
                                let path = &item.path;
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
                    && let Some(item) = matches.first()
                {
                    go = Some(item.path.clone());
                }
                if escape_requested {
                    cancel = true;
                }
            });

        self.recent_order = order;

        if cancel {
            self.ui.modals.recent_input = None;
            return;
        }
        if let Some(path) = go {
            self.ui.modals.recent_input = None;
            self.ws.active_panel().navigate_to(path);
        }
    }
}
