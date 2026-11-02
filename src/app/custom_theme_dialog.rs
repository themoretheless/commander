use super::*;
impl App {
    pub(super) fn show_custom_theme(&mut self, ctx: &egui::Context) {
        if !self.ui.modals.custom_theme_open {
            return;
        }
        let just_opened = Self::take_modal_opened(ctx, UiModal::CustomTheme);
        let mut close = self.take_escape_request(crate::accessibility::EscapeRoute::Modal(
            crate::accessibility::ModalSurface::CustomTheme,
        ));
        let mut effect = None;
        let response = super::sheet_window("Custom theme", self.colors, 14).show(ctx, |ui| {
            ui.set_width(480.0_f32.min(ui.available_width()));
            egui::ScrollArea::vertical()
                .id_salt("custom-theme-body")
                .max_height((ctx.content_rect().height() - 160.).max(160.))
                .show(ui, |ui| {
                    effect = self.custom_theme.editor(ui, self.colors);
                });
            let button = ui.button("Close");
            if just_opened {
                button.request_focus();
            }
            close |= button.clicked();
        });
        #[cfg(not(feature = "visual-qa"))]
        let _ = response;
        #[cfg(feature = "visual-qa")]
        if let Some(response) = response {
            crate::visual_qa::record_response(
                ctx,
                crate::visual_qa::ProbeId::Confirmation,
                &response.response,
            );
        }
        if let Some(effect) = effect {
            match effect {
                crate::custom_theme::Effect::Applied => {
                    self.theme_palette = None;
                    self.follow_system_theme = false;
                    if let Some(colors) = self.custom_theme.colors() {
                        self.theme_mode = if colors.is_dark() {
                            ThemeMode::Dark
                        } else {
                            ThemeMode::Light
                        };
                    }
                    self.apply_current_palette(ctx);
                }
                crate::custom_theme::Effect::Reset => self.apply_current_palette(ctx),
                crate::custom_theme::Effect::Copy(text) => {
                    self.write_clipboard(&text, "custom theme JSON", ctx)
                }
            }
        }
        if close {
            self.ui.modals.custom_theme_open = false;
        }
    }
}
