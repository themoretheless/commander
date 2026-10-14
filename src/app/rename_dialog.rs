//! Inline rename editor: a small modal seeded from the cursor entry, with
//! live name validation. Commit orchestration lives in `workspace`; lexical
//! validation lives in `pathname`.

use super::*;

/// Committing `buffer` would leave the name as it is. The untrimmed buffer is
/// checked too: for a name with edge spaces (" Foo.txt") the trimmed buffer
/// differs from the name, and committing it unedited would strip the spaces.
fn rename_is_noop(buffer: &str, old_name: &str) -> bool {
    buffer == old_name || buffer.trim() == old_name
}

/// The name is not valid UTF-8, so the editor holds a lossy copy and any
/// committed edit writes U+FFFD in place of the original bytes.
fn name_is_lossy(path: &std::path::Path) -> bool {
    path.file_name().is_some_and(|name| name.to_str().is_none())
}

impl App {
    pub(crate) fn open_rename(&mut self, path: std::path::PathBuf, ctx: &egui::Context) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        self.ui.modals.renaming = Some(RenameState {
            siblings: crate::workspace::Workspace::rename_siblings(&path),
            path,
            buffer: name,
            error: None,
        });
        Self::mark_modal_opened(ctx, UiModal::Rename);
    }

    pub(crate) fn show_rename_dialog(&mut self, ctx: &egui::Context) {
        let escape_requested = self.take_modal_escape(crate::accessibility::ModalSurface::Rename);

        let Some(state) = &mut self.ui.modals.renaming else {
            return;
        };
        let t = self.colors;

        let mut commit = false;
        let mut cancel = false;

        super::sheet_window("Rename", t, 14).show(ctx, |ui| {
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
            if Self::take_modal_opened(ui.ctx(), UiModal::Rename) {
                resp.request_focus();
            }

            // Validate against the directory snapshot captured when the
            // editor opened. Commit validates once more against live disk.
            let old_name = state
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            state.error = if rename_is_noop(&state.buffer, &old_name) {
                None
            } else {
                crate::pathname::validate_new_name(&state.buffer, &state.siblings)
                    .err()
                    .map(|error| error.to_string())
            };
            let valid = state.error.is_none();

            if name_is_lossy(&state.path) {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "This name has bytes that are not valid UTF-8; renaming replaces them with \u{fffd}.",
                    )
                    .size(11.0)
                    .color(t.accent_warning),
                );
            }

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
                        super::dialog_button(
                            t,
                            super::ButtonRole::Primary,
                            egui::RichText::new("Rename").size(13.0),
                        ),
                    )
                    .clicked()
                {
                    commit = true;
                }
                ui.add_space(8.0);
                if ui
                    .add(super::dialog_button(
                        t,
                        super::ButtonRole::Secondary,
                        egui::RichText::new("Cancel").size(13.0),
                    ))
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
                    !rename_is_noop(&s.buffer, &old_name),
                )
            };
            if !changed {
                self.ui.modals.renaming = None;
                return;
            }
            match self.ws.commit_rename(&path, &buffer) {
                Ok(()) => {
                    self.ui.modals.renaming = None;
                    if changed {
                        let now = ctx.input(|i| i.time);
                        self.toast(
                            ctx,
                            crate::toasts::ToastKind::Success,
                            "Renamed 1 item",
                            true,
                        );
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

#[cfg(test)]
mod tests {
    use super::{name_is_lossy, rename_is_noop};

    #[test]
    fn unedited_name_with_edge_spaces_is_a_noop() {
        assert!(rename_is_noop(" Foo.txt", " Foo.txt"));
        assert!(rename_is_noop("Foo.txt ", "Foo.txt"));
        assert!(!rename_is_noop("Bar.txt", "Foo.txt"));
        assert!(!rename_is_noop("Foo.txt", " Foo.txt"));
    }

    #[test]
    fn utf8_names_are_not_lossy() {
        assert!(!name_is_lossy(std::path::Path::new("/tmp/caf\u{e9}.txt")));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_names_are_flagged_as_lossy() {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::OsStr::from_bytes(b"bad\xffname");
        assert!(name_is_lossy(&std::path::Path::new("/tmp").join(name)));
    }
}
