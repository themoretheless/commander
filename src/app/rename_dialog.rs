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

/// Characters to preselect when the editor opens: the stem, so typing
/// replaces the name but keeps the extension. Folders, dotfiles (".bashrc")
/// and names without an extension select the whole name.
fn initial_selection_chars(name: &str, is_dir: bool) -> usize {
    match name.rfind('.') {
        Some(dot) if dot > 0 && !is_dir => name[..dot].chars().count(),
        _ => name.chars().count(),
    }
}

/// Temp-memory slot for the last failed commit: the buffer it was made with
/// and the error. Commit errors arrive after the frame drew its live
/// validation, so they are kept here until the user edits the name.
const COMMIT_ERROR: &str = "rename_commit_error";

/// Temp-memory slot for whether the renamed item lists as a folder, taken
/// from the pane listing when the editor opens (a folder symlink counts as a
/// folder), so the first frame needs no stat on the UI thread.
const RENAME_IS_DIR: &str = "rename_is_dir";

/// The error line under the field: live validation first, else the failed
/// commit's error while the buffer is still the one that was committed.
fn shown_error(
    live: Option<String>,
    commit: Option<&(String, String)>,
    buffer: &str,
) -> Option<String> {
    live.or_else(|| {
        commit
            .filter(|(committed, _)| committed == buffer)
            .map(|(_, error)| error.clone())
    })
}

impl App {
    pub(crate) fn open_rename(&mut self, path: std::path::PathBuf, ctx: &egui::Context) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let is_dir = [self.ws.active_panel_ref(), self.ws.inactive_panel()]
            .into_iter()
            .flat_map(|panel| panel.entries())
            .find(|entry| entry.path == path)
            .is_some_and(|entry| entry.is_dir);
        self.ui.modals.renaming = Some(RenameState {
            siblings: crate::workspace::Workspace::rename_siblings(&path),
            path,
            buffer: name,
            error: None,
        });
        ctx.data_mut(|data| {
            data.remove::<(String, String)>(egui::Id::new(COMMIT_ERROR));
            data.insert_temp(egui::Id::new(RENAME_IS_DIR), is_dir);
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
            let label = ui.label(
                egui::RichText::new("Rename to:")
                    .size(12.0)
                    .color(t.text_muted),
            );
            ui.add_space(6.0);

            let edit = egui::TextEdit::singleline(&mut state.buffer)
                .desired_width(f32::INFINITY)
                .margin(egui::vec2(8.0, 6.0));
            let resp = ui.add(edit).labelled_by(label.id);

            // Grab focus and select the basename on the first frame.
            if Self::take_modal_opened(ui.ctx(), UiModal::Rename) {
                resp.request_focus();
                let is_dir = ui
                    .ctx()
                    .data(|data| data.get_temp::<bool>(egui::Id::new(RENAME_IS_DIR)))
                    .unwrap_or(false);
                let stem = initial_selection_chars(&state.buffer, is_dir);
                let mut text_state =
                    egui::TextEdit::load_state(ui.ctx(), resp.id).unwrap_or_default();
                text_state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(stem),
                )));
                text_state.store(ui.ctx(), resp.id);
            }

            // Validate against the directory snapshot captured when the
            // editor opened. Commit validates once more against live disk.
            let old_name = state
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let live_error = if rename_is_noop(&state.buffer, &old_name) {
                None
            } else {
                crate::pathname::validate_new_name(&state.buffer, &state.siblings)
                    .err()
                    .map(|error| error.to_string())
            };
            let valid = live_error.is_none();
            let commit_error = ui
                .ctx()
                .data(|data| data.get_temp::<(String, String)>(egui::Id::new(COMMIT_ERROR)));
            state.error = shown_error(live_error, commit_error.as_ref(), &state.buffer);

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
                let r = ui.label(egui::RichText::new(err).size(11.0).color(t.accent_red));
                ui.ctx().accesskit_node_builder(r.id, |node| {
                    node.set_role(egui::accesskit::Role::Status);
                    node.set_live(egui::accesskit::Live::Polite);
                });
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
                    ctx.data_mut(|data| {
                        data.remove::<(String, String)>(egui::Id::new(COMMIT_ERROR))
                    });
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
                    // Shown from the next frame on, until the name is edited.
                    ctx.data_mut(|data| {
                        data.insert_temp(egui::Id::new(COMMIT_ERROR), (buffer, msg.clone()));
                    });
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
    use super::{initial_selection_chars, name_is_lossy, rename_is_noop, shown_error};

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

    #[test]
    fn commit_error_stays_until_the_name_is_edited() {
        let commit = ("b.txt".to_string(), "b.txt already exists".to_string());
        // Live validation passed (the commit button was enabled): the commit
        // error is what the user sees.
        assert_eq!(
            shown_error(None, Some(&commit), "b.txt").as_deref(),
            Some("b.txt already exists")
        );
        assert_eq!(shown_error(None, Some(&commit), "c.txt"), None);
        assert_eq!(
            shown_error(Some("Invalid".to_string()), Some(&commit), "b.txt").as_deref(),
            Some("Invalid")
        );
        assert_eq!(shown_error(None, None, "b.txt"), None);
    }

    #[test]
    fn opening_selects_the_stem_of_files_only() {
        assert_eq!(initial_selection_chars("report.final.pdf", false), 12);
        assert_eq!(initial_selection_chars("caf\u{e9}.txt", false), 4);
        assert_eq!(initial_selection_chars(".bashrc", false), 7);
        assert_eq!(initial_selection_chars("Makefile", false), 8);
        assert_eq!(initial_selection_chars("photos.2024", true), 11);
    }
}
