//! Go-to-path input (Cmd+L): type a directory path and jump to it. The
//! resolution/validation lives in `workspace::resolve_dir_input`.

use super::*;

impl App {
    pub(crate) fn open_path(&mut self, ctx: &egui::Context) {
        let current = self
            .ws
            .active_panel_ref()
            .current_path
            .display()
            .to_string();
        self.ui.path_input = Some(current);
        self.ui.path_resolved = None;
        Self::mark_modal_opened(ctx, UiModal::Path);
    }

    pub(crate) fn show_path_dialog(&mut self, ctx: &egui::Context) {
        let just_opened = Self::take_modal_opened(ctx, UiModal::Path);
        let escape_requested = self.take_modal_escape(crate::accessibility::ModalSurface::Path);
        let Some(buffer) = &mut self.ui.path_input else {
            return;
        };
        let t = self.colors;

        // Re-resolve only when the buffer changed since the last frame; this
        // avoids filesystem stat calls on every idle frame the dialog is open.
        let stale = self
            .ui
            .path_resolved
            .as_ref()
            .is_none_or(|(key, _)| key != buffer.as_str());
        if stale {
            let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
            let result = crate::workspace::resolve_dir_input(buffer, &home);
            self.ui.path_resolved = Some((buffer.clone(), result));
        }
        let resolved = self
            .ui
            .path_resolved
            .as_ref()
            .map(|(_, r)| r.clone())
            .expect("path_resolved populated above");
        let mut go: Option<std::path::PathBuf> = None;
        let mut cancel = false;

        egui::Window::new("Go to path")
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
                ui.set_width(440.0);
                ui.label(
                    egui::RichText::new("Go to folder")
                        .size(12.0)
                        .color(t.text_muted),
                );
                ui.add_space(6.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(buffer)
                        .desired_width(f32::INFINITY)
                        .hint_text("~/Documents")
                        .margin(egui::vec2(8.0, 6.0)),
                );
                if just_opened {
                    resp.request_focus();
                }

                ui.add_space(4.0);
                match &resolved {
                    Ok(_) => {
                        ui.label(
                            egui::RichText::new("\u{2713} folder")
                                .size(11.0)
                                .color(t.accent),
                        );
                    }
                    Err(msg) => {
                        ui.label(egui::RichText::new(msg).size(11.0).color(t.accent_red));
                    }
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let ok = resolved.is_ok();
                    if ui
                        .add_enabled(
                            ok,
                            egui::Button::new(
                                egui::RichText::new("Go").size(13.0).color(Color32::WHITE),
                            )
                            .fill(t.accent)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                        && let Ok(p) = &resolved
                    {
                        go = Some(p.clone());
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
                    if ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && let Ok(p) = &resolved
                    {
                        go = Some(p.clone());
                    }
                    if escape_requested {
                        cancel = true;
                    }
                });
            });

        if cancel {
            self.ui.path_input = None;
            self.ui.path_resolved = None;
            return;
        }
        if let Some(path) = go {
            self.ui.path_input = None;
            self.ui.path_resolved = None;
            self.ws.active_panel().navigate_to(path);
        }
    }
}
