//! Saved-search (smart folder) picker: lists persisted queries; selecting one
//! reopens the Find sheet with it and runs it. Storage lives in
//! `crate::smart_folder`.

use super::*;

#[derive(Debug, PartialEq, Eq)]
enum SavedSearchTransition<T> {
    Stay,
    Close,
    Open(T),
}

fn saved_search_transition<T>(cancel_requested: bool, open: Option<T>) -> SavedSearchTransition<T> {
    if cancel_requested {
        SavedSearchTransition::Close
    } else if let Some(value) = open {
        SavedSearchTransition::Open(value)
    } else {
        SavedSearchTransition::Stay
    }
}

/// Temp-memory key of the saved search whose delete is armed.
const DELETE_ARMED: &str = "saved_search_delete_armed";
/// Temp-memory flag: the sheet was open last frame (detects the opening frame).
const SHEET_SHOWN: &str = "saved_search_sheet_shown";

/// Remove `name` and persist. A failed save restores the previous list, so
/// the search is not gone in memory yet back after a restart. Returns
/// whether the save succeeded.
fn remove_and_save(
    store: &mut crate::smart_folder::SmartFolders,
    name: &str,
    save: impl FnOnce(&crate::smart_folder::SmartFolders) -> bool,
) -> bool {
    let previous = store.clone();
    store.remove(name);
    if save(store) {
        true
    } else {
        *store = previous;
        false
    }
}

/// The Find sheet's error for a saved search whose root folder is gone.
fn missing_root_error(root: &std::path::Path, root_is_dir: bool) -> Option<String> {
    (!root_is_dir).then(|| {
        format!(
            "Folder no longer exists: {}. Choose another folder to search, or delete this saved search.",
            root.display()
        )
    })
}

impl App {
    pub(crate) fn open_saved_search(&mut self) {
        self.smart_folders_mut();
        self.ui.modals.saved_search_open = true;
    }

    pub(crate) fn show_saved_search_dialog(&mut self, ctx: &egui::Context) {
        let escape_requested =
            self.take_modal_escape(crate::accessibility::ModalSurface::SavedSearch);
        let shown_id = egui::Id::new(SHEET_SHOWN);
        let armed_id = egui::Id::new(DELETE_ARMED);
        if !self.ui.modals.saved_search_open {
            ctx.data_mut(|data| data.remove::<bool>(shown_id));
            return;
        }
        // `open_saved_search` has no context to mark the modal opened, so
        // the opening frame is the first one shown.
        let just_opened = ctx.data_mut(|data| {
            let shown = data.get_temp::<bool>(shown_id).unwrap_or(false);
            data.insert_temp(shown_id, true);
            !shown
        });
        if just_opened {
            super::ui_state::set_armed_row(ctx, armed_id, None);
        }
        let mut armed = super::ui_state::armed_row(ctx, armed_id);
        let t = self.colors;
        let mut open_def: Option<crate::smart_folder::Definition> = None;
        let mut delete: Option<String> = None;
        let mut open_find = false;
        let mut close = false;

        {
            let empty: &[crate::smart_folder::Definition] = &[];
            let items = self
                .smart_folders
                .as_ref()
                .map(|s| s.items.as_slice())
                .unwrap_or(empty);

            super::sheet_window("Saved searches", t, 14).show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(
                    egui::RichText::new("Saved searches")
                        .size(13.0)
                        .strong()
                        .color(t.text_primary),
                );
                ui.add_space(8.0);

                let mut first_focus: Option<egui::Response> = None;
                if items.is_empty() {
                    ui.label(
                        egui::RichText::new("None yet. Save one from the Find sheet.")
                            .size(12.0)
                            .color(t.text_muted),
                    );
                    ui.add_space(6.0);
                    let resp = ui.add(super::dialog_button(
                        t,
                        super::ButtonRole::Primary,
                        egui::RichText::new("Open Find").size(13.0),
                    ));
                    if resp.clicked() {
                        open_find = true;
                    }
                    first_focus = Some(resp);
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(300.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for def in items {
                                ui.horizontal(|ui| {
                                    let text =
                                        format!("{}   \u{00b7}   {}", def.name, def.root.display());
                                    // A focusable button, so Tab and Enter
                                    // open it; it comes before the delete
                                    // button in the focus order.
                                    let open_width = (ui.available_width() - 64.0).max(0.0);
                                    let resp = ui
                                        .scope(|ui| {
                                            ui.set_max_width(open_width);
                                            ui.add(
                                                egui::Button::new(
                                                    egui::RichText::new(&text)
                                                        .size(12.0)
                                                        .color(t.text_secondary),
                                                )
                                                .frame(false)
                                                .truncate(),
                                            )
                                        })
                                        .inner
                                        .on_hover_text(&text);
                                    if resp.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    if resp.clicked() {
                                        open_def = Some(def.clone());
                                    }
                                    if first_focus.is_none() {
                                        first_focus = Some(resp);
                                    }
                                    ui.add_space(4.0);
                                    let is_armed = armed.as_deref() == Some(def.name.as_str());
                                    let (glyph, hover, label) = if is_armed {
                                        (
                                            "Delete?",
                                            "Click again to delete",
                                            format!("Confirm delete saved search {}", def.name),
                                        )
                                    } else {
                                        (
                                            "\u{00d7}",
                                            "Delete",
                                            format!("Delete saved search {}", def.name),
                                        )
                                    };
                                    let delete_resp = ui
                                        .small_button(
                                            egui::RichText::new(glyph).color(t.accent_red),
                                        )
                                        .on_hover_text(hover);
                                    delete_resp.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::WidgetType::Button,
                                            true,
                                            &label,
                                        )
                                    });
                                    if delete_resp.clicked()
                                        && super::ui_state::confirm_on_second_click(
                                            &mut armed, &def.name,
                                        )
                                    {
                                        delete = Some(def.name.clone());
                                    }
                                });
                            }
                        });
                }

                ui.add_space(10.0);
                let close_resp = ui.add(super::dialog_button(
                    t,
                    super::ButtonRole::Secondary,
                    egui::RichText::new("Close").size(13.0),
                ));
                if close_resp.clicked() || escape_requested {
                    close = true;
                }
                if just_opened {
                    first_focus.unwrap_or(close_resp).request_focus();
                }
            });
        }
        super::ui_state::set_armed_row(ctx, armed_id, armed);

        let transition = saved_search_transition(close, open_def);
        if transition == SavedSearchTransition::Close {
            self.ui.modals.saved_search_open = false;
            return;
        }
        if open_find {
            self.ui.modals.saved_search_open = false;
            self.open_find(ctx);
            return;
        }
        if let Some(name) = delete
            && !remove_and_save(self.smart_folders_mut(), &name, crate::smart_folder::save)
        {
            self.toast(
                ctx,
                crate::toasts::ToastKind::Error,
                format!(
                    "Could not save saved searches, so \"{name}\" was not deleted. Check that the settings folder is writable."
                ),
                false,
            );
        }
        if let SavedSearchTransition::Open(def) = transition {
            let mut state = FindState::from_definition(&def);
            state.index_exclusions = super::find_dialog::format_index_exclusions(
                &state.root,
                &self.content_index.exclusions(&state.root),
            );
            if let Some(error) = missing_root_error(&state.root, state.root.is_dir()) {
                // Searching a missing root would only show zero results.
                state.error = Some(error);
                state.pending_rerun = false;
            }
            self.ui.modals.find = Some(state);
            self.ui.modals.saved_search_open = false;
            Self::mark_modal_opened(ctx, UiModal::Find);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SavedSearchTransition, missing_root_error, remove_and_save, saved_search_transition,
    };
    use crate::smart_folder::{Definition, SmartFolders};

    fn store(names: &[&str]) -> SmartFolders {
        SmartFolders {
            items: names
                .iter()
                .map(|name| Definition {
                    name: (*name).to_string(),
                    root: std::path::PathBuf::from("/tmp"),
                    query: crate::query::Query::default(),
                })
                .collect(),
        }
    }

    #[test]
    fn cancel_wins_over_saved_search_transition_in_the_same_frame() {
        assert_eq!(
            saved_search_transition(true, Some(7)),
            SavedSearchTransition::Close
        );
        assert_eq!(
            saved_search_transition(false, Some(7)),
            SavedSearchTransition::Open(7)
        );
    }

    #[test]
    fn failed_save_after_delete_restores_the_search() {
        let mut folders = store(&["a", "b"]);
        assert!(!remove_and_save(&mut folders, "a", |_| false));
        assert_eq!(folders, store(&["a", "b"]));
        let mut saved = None;
        assert!(remove_and_save(&mut folders, "a", |s| {
            saved = Some(s.clone());
            true
        }));
        assert_eq!(folders, store(&["b"]));
        assert_eq!(saved, Some(store(&["b"])));
    }

    #[test]
    fn missing_root_is_reported_by_path() {
        let root = std::path::Path::new("/gone/photos");
        let error = missing_root_error(root, false).expect("missing root");
        assert!(error.contains(&root.display().to_string()), "{error}");
        assert_eq!(missing_root_error(root, true), None);
    }
}
