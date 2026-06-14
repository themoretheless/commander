//! Inline rename editor: a small modal seeded from the cursor entry, with
//! live name validation. Commit/validation logic lives in `workspace`.

use super::*;

impl App {
    pub(crate) fn show_rename_dialog(&mut self, ctx: &egui::Context) {
        // Pick up a rename request raised by Command::BeginRename.
        if let Some(path) = self.ws.rename_target.take() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            self.renaming = Some(RenameState {
                path,
                buffer: name,
                error: None,
                focused: false,
            });
        }

        let Some(state) = &mut self.renaming else {
            return;
        };
        let t = self.colors;

        let mut commit = false;
        let mut cancel = false;

        egui::Window::new("Rename")
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
                ui.set_width(360.0);
                ui.label(
                    egui::RichText::new("Rename to:")
                        .size(12.0)
                        .color(t.text_muted),
                );
                ui.add_space(6.0);

                let edit = egui::TextEdit::singleline(&mut state.buffer)
                    .desired_width(f32::INFINITY)
                    .margin(egui::vec2(8.0, 6.0));
                let resp = ui.add(edit);

                // Grab focus and select the basename on the first frame.
                if !state.focused {
                    resp.request_focus();
                    state.focused = true;
                }

                // Validate live for inline feedback (siblings come from the
                // active panel, excluding the entry being renamed).
                let old_name = state
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let siblings: Vec<String> = self
                    .ws
                    .active_panel_ref()
                    .entries
                    .iter()
                    .map(|e| e.name.clone())
                    .filter(|n| n != &old_name)
                    .collect();
                let valid = state.buffer.trim() == old_name
                    || crate::workspace::validate_new_name(&state.buffer, &siblings).is_ok();
                state.error = if valid {
                    None
                } else {
                    crate::workspace::validate_new_name(&state.buffer, &siblings).err()
                };

                if let Some(err) = &state.error {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(err).size(11.0).color(t.accent_red));
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let can_commit = valid;
                    if ui
                        .add_enabled(
                            can_commit,
                            egui::Button::new(
                                egui::RichText::new("Rename")
                                    .size(13.0)
                                    .color(Color32::WHITE),
                            )
                            .fill(t.accent)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                    {
                        commit = true;
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

                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    if enter && can_commit {
                        commit = true;
                    }
                    if esc {
                        cancel = true;
                    }
                });
            });

        if cancel {
            self.renaming = None;
            return;
        }
        if commit {
            let (path, buffer) = {
                let s = self.renaming.as_ref().unwrap();
                (s.path.clone(), s.buffer.clone())
            };
            match self.ws.commit_rename(&path, &buffer) {
                Ok(()) => self.renaming = None,
                Err(msg) => {
                    if let Some(s) = &mut self.renaming {
                        s.error = Some(msg);
                    }
                }
            }
        }
    }
}
