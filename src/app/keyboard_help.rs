use super::*;

impl App {
    pub(super) fn show_keyboard_help(&mut self, ctx: &egui::Context) {
        if !self.ui.modals.keyboard_help {
            return;
        }
        let just_opened = Self::take_modal_opened(ctx, UiModal::KeyboardHelp);
        let mut close = self.take_escape_request(crate::accessibility::EscapeRoute::Modal(
            crate::accessibility::ModalSurface::KeyboardHelp,
        ));
        let context = self.ws.command_context();
        super::sheet_window("Keyboard shortcuts", self.colors, 14).show(ctx, |ui| {
            ui.set_width(480.0_f32.min(ui.available_width()));
            ui.heading("Keyboard shortcuts");
            ui.label("Tab switches panes. Arrow keys move the cursor; Shift extends selection. Enter opens, Backspace goes up.");
            if self.ws.active_panel_ref().browser_mode() == crate::panel::BrowserMode::Icons {
                ui.label("Icons: Left/Right move one item, Up/Down move one grid row.");
            }
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                egui::Grid::new("keyboard_reference").striped(true).show(ui, |ui| {
                    for entry in crate::command::CATALOG.iter().filter(|entry| !entry.shortcut.is_empty()) {
                        let available = crate::command::availability(entry.command, &context);
                        ui.label(egui::RichText::new(entry.shortcut).monospace());
                        ui.vertical(|ui| {
                            ui.add_enabled(available.enabled, egui::Label::new(entry.label));
                            if let Some(reason) = available.reason { ui.small(reason); }
                        });
                        ui.end_row();
                    }
                });
            });
            ui.label("More commands and view modes: Cmd+K. Vim motions: j/k, counted motions such as 5j, gg for first item, ss reverses sorting.");
            let button = ui.button("Close");
            if just_opened { button.request_focus(); }
            close |= button.clicked();
        });
        if close {
            self.ui.modals.keyboard_help = false;
        }
    }
}
