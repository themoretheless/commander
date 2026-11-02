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
        let mut copy = None;
        super::sheet_window("Keyboard shortcuts", self.colors, 14).show(ctx, |ui| {
            ui.set_width(480.0_f32.min(ui.available_width()));
            ui.heading("Keyboard shortcuts");
            egui::ScrollArea::vertical().id_salt("keyboard-help-body").max_height((ctx.content_rect().height()-160.).max(160.)).show(ui,|ui|{
            ui.label("Workspace keys below include applied custom bindings. Fields, dialogs and viewers retain their own keys.");
            if self.ws.active_panel_ref().browser_mode() == crate::panel::BrowserMode::Icons {
                ui.label("Icons: vertical cursor and selection motions follow grid rows.");
            }
            let reference=self.custom_shortcuts.reference();
            egui::ScrollArea::vertical().id_salt("shortcut-reference").max_height(220.).show_rows(ui,40.,reference.len(),|ui,range|{
                for index in range {let(label,command,keys)=&reference[index];let available=crate::command::availability(*command,&context);
                    let hint=if keys.is_empty(){"Unassigned".into()}else{keys.join(" / ")};
                    ui.horizontal(|ui|{
                        ui.add_sized([160.,40.],egui::Label::new(egui::RichText::new(&hint).monospace()).truncate()).on_hover_text(hint);
                        let width=ui.available_width().max(120.);
                        ui.allocate_ui_with_layout(egui::vec2(width,40.),egui::Layout::top_down(egui::Align::Min),|ui|{
                            ui.add_enabled(available.enabled,egui::Label::new(*label).truncate()).on_hover_text(*label);
                            if let Some(reason)=available.reason {ui.add(egui::Label::new(egui::RichText::new(reason).small()).truncate()).on_hover_text(reason);}
                        });
                    });
                }
            });
            ui.label("More commands and view modes are available from the toolbar command field. Built-in Vim motions remain available unless their first key is rebound.");
            copy=self.custom_shortcuts.editor(ui);
            });
            let button = ui.button("Close");
            if just_opened { button.request_focus(); }
            close |= button.clicked();
        });
        if let Some(text) = copy {
            self.write_clipboard(&text, "applied shortcut set", ctx);
        }
        if close {
            self.ui.modals.keyboard_help = false;
        }
    }
}
