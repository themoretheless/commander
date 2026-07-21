//! Saved-search (smart folder) picker: lists persisted queries; selecting one
//! reopens the Find sheet with it and runs it. Storage lives in
//! `crate::smart_folder`.

use super::*;

impl App {
    pub(crate) fn open_saved_search(&mut self) {
        self.smart_folders_mut();
        self.saved_search_open = true;
    }

    pub(crate) fn show_saved_search_dialog(&mut self, ctx: &egui::Context) {
        let escape_requested =
            self.take_modal_escape(crate::accessibility::ModalSurface::SavedSearch);
        if !self.saved_search_open {
            return;
        }
        let t = self.colors;
        let mut open_def: Option<crate::smart_folder::Definition> = None;
        let mut delete: Option<String> = None;
        let mut close = false;

        {
            let empty: &[crate::smart_folder::Definition] = &[];
            let items = self
                .smart_folders
                .as_ref()
                .map(|s| s.items.as_slice())
                .unwrap_or(empty);

            egui::Window::new("Saved searches")
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
                    ui.set_width(460.0);
                    ui.label(
                        egui::RichText::new("Saved searches")
                            .size(13.0)
                            .strong()
                            .color(t.text_primary),
                    );
                    ui.add_space(8.0);

                    if items.is_empty() {
                        ui.label(
                            egui::RichText::new("None yet. Save one from the Find sheet.")
                                .size(12.0)
                                .color(t.text_muted),
                        );
                    } else {
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for def in items {
                                    ui.horizontal(|ui| {
                                        if ui
                                            .small_button(
                                                egui::RichText::new("\u{00d7}").color(t.accent_red),
                                            )
                                            .on_hover_text("Delete")
                                            .clicked()
                                        {
                                            delete = Some(def.name.clone());
                                        }
                                        ui.add_space(4.0);
                                        let resp = ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(format!(
                                                    "{}   \u{00b7}   {}",
                                                    def.name,
                                                    def.root.display()
                                                ))
                                                .size(12.0)
                                                .color(t.text_secondary),
                                            )
                                            .sense(Sense::click()),
                                        );
                                        if resp.hovered() {
                                            ui.ctx()
                                                .set_cursor_icon(egui::CursorIcon::PointingHand);
                                        }
                                        if resp.clicked() {
                                            open_def = Some(def.clone());
                                        }
                                    });
                                }
                            });
                    }

                    ui.add_space(10.0);
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Close")
                                    .size(13.0)
                                    .color(t.text_primary),
                            )
                            .fill(t.bg_card)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                        || escape_requested
                    {
                        close = true;
                    }
                });
        }

        if let Some(name) = delete {
            self.smart_folders_mut().remove(&name);
            if !crate::smart_folder::save(self.smart_folders_mut()) {
                let now = ctx.input(|i| i.time);
                self.toasts.push(crate::toasts::Toast::new(
                    "Could not save saved searches to disk",
                    crate::toasts::ToastKind::Error,
                    false,
                    now,
                ));
            }
        }
        if let Some(def) = open_def {
            let mut state = FindState::from_definition(&def);
            state.index_exclusions = super::find_dialog::format_index_exclusions(
                &state.root,
                &self.content_index.exclusions(&state.root),
            );
            self.find = Some(state);
            self.saved_search_open = false;
            return;
        }
        if close {
            self.saved_search_open = false;
        }
    }
}
