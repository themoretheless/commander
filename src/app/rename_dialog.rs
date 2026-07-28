//! Inline rename editor: a small modal seeded from the cursor entry, with
//! live name validation. Commit orchestration lives in `workspace`; lexical
//! validation lives in `pathname`.

use super::*;

impl App {
    pub(crate) fn open_rename(&mut self, path: std::path::PathBuf) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        self.ui.modals.renaming = Some(RenameState {
            siblings: crate::workspace::Workspace::rename_siblings(&path),
            path,
            buffer: name,
            error: None,
            focused: false,
        });
    }

    pub(crate) fn show_rename_dialog(&mut self, ctx: &egui::Context) {
        let escape_requested = self.take_modal_escape(crate::accessibility::ModalSurface::Rename);

        let Some(state) = &mut self.ui.modals.renaming else {
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

                // Validate against the directory snapshot captured when the
                // editor opened. Commit validates once more against live disk.
                let old_name = state
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                state.error = if state.buffer.trim() == old_name {
                    None
                } else {
                    crate::pathname::validate_new_name(&state.buffer, &state.siblings)
                        .err()
                        .map(|error| error.to_string())
                };
                let valid = state.error.is_none();

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
                    if enter && can_commit {
                        commit = true;
                    }
                    if escape_requested {
                        cancel = true;
                    }
                });
            });

        if cancel {
            self.ui.modals.renaming = None;
            return;
        }
        if commit {
            let (path, buffer, changed) = {
                let s = self.ui.modals.renaming.as_ref().unwrap();
                let old_name = s
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy())
                    .unwrap_or_default();
                (
                    s.path.clone(),
                    s.buffer.clone(),
                    s.buffer.trim() != old_name,
                )
            };
            match self.ws.commit_rename(&path, &buffer) {
                Ok(()) => {
                    self.ui.modals.renaming = None;
                    if changed {
                        let now = ctx.input(|i| i.time);
                        self.toasts.push(crate::toasts::Toast::new(
                            "Renamed 1 item",
                            crate::toasts::ToastKind::Success,
                            true,
                            now,
                        ));
                        if let Some(action) = self.ws.top_undo_action().cloned()
                            && let Some(jump_to) = action.jump_to()
                        {
                            self.receipts.push(crate::receipts::Receipt {
                                verb: action.verb(),
                                item_count: action.item_count(),
                                timestamp: now,
                                jump_to,
                                undo_action: Some(action),
                            });
                        }
                    }
                }
                Err(msg) => {
                    if let Some(s) = &mut self.ui.modals.renaming {
                        s.error = Some(msg);
                    }
                }
            }
        }
    }
}
