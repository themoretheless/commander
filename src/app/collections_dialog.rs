//! Named multi-root project collections and their read-only virtual view.

use super::*;

/// Temp-memory key of the project whose Remove is armed.
const REMOVE_ARMED: &str = "collections_remove_armed";

/// Adding a name that already exists (any case) replaces its roots; say so
/// instead of replacing silently.
fn replaced_project_notice(existed: bool, name: &str) -> Option<String> {
    existed.then(|| format!("Updated project {name} with the current folders"))
}

impl App {
    fn start_collection_view(&mut self, ctx: &egui::Context, name: &str) {
        let Some(collection) = self.project_collections.get(name).cloned() else {
            return;
        };
        let repaint = ctx.clone();
        let run = crate::collections::spawn_view(
            collection,
            self.ws.active_panel_ref().show_hidden(),
            std::sync::Arc::new(move || repaint.request_repaint()),
        );
        if let Some(state) = self.ui.modals.collections_dialog.as_mut() {
            state.selected = Some(name.to_string());
            state.rows.clear();
            state.run = Some(run);
            state.scanned = 0;
            state.unavailable_roots.clear();
            state.truncated = false;
            state.error = None;
        }
    }

    fn poll_collection_view(&mut self) {
        let (events, disconnected) = self
            .ui
            .modals
            .collections_dialog
            .as_ref()
            .and_then(|state| state.run.as_ref())
            .map_or_else(Default::default, |run| {
                super::drain_events(|| run.try_recv())
            });
        if let Some(state) = self.ui.modals.collections_dialog.as_mut() {
            for event in events {
                match event {
                    crate::collections::ViewEvent::Batch(rows) => state.rows.extend(rows),
                    crate::collections::ViewEvent::Complete {
                        scanned,
                        unavailable_roots,
                        truncated,
                        cancelled,
                    } => {
                        state.scanned = scanned;
                        state.unavailable_roots = unavailable_roots;
                        state.truncated = truncated;
                        state.run = None;
                        if cancelled {
                            state.error = Some("Collection refresh was cancelled".to_string());
                        }
                    }
                }
            }
            if disconnected && state.run.is_some() {
                state.run = None;
                state.error = Some("Collection worker stopped before completion".to_string());
            }
        }
    }

    pub(crate) fn open_collections(&mut self, ctx: &egui::Context) {
        let selected = self
            .project_collections
            .items
            .first()
            .map(|collection| collection.name.clone());
        self.ui.modals.collections_dialog = Some(CollectionsDialogState {
            selected: selected.clone(),
            ..Default::default()
        });
        if let Some(name) = selected {
            self.start_collection_view(ctx, &name);
        }
        super::ui_state::set_armed_row(ctx, egui::Id::new(REMOVE_ARMED), None);
        Self::mark_modal_opened(ctx, UiModal::Collections);
    }

    pub(crate) fn show_collections_dialog(&mut self, ctx: &egui::Context) {
        let escape_requested =
            self.take_modal_escape(crate::accessibility::ModalSurface::Collections);
        if self.ui.modals.collections_dialog.is_none() {
            return;
        }
        if super::ui_state::modal_close_requested(true, escape_requested) {
            self.ui.modals.collections_dialog = None;
            return;
        }
        self.poll_collection_view();

        let t = self.colors;
        let collection_names: Vec<String> = self
            .project_collections
            .items
            .iter()
            .map(|collection| collection.name.clone())
            .collect();
        let mut window_open = true;
        let mut add = false;
        let mut select = None;
        let mut delete = None;
        let mut reveal = None;
        let just_opened = Self::take_modal_opened(ctx, UiModal::Collections);
        let armed_id = egui::Id::new(REMOVE_ARMED);
        let mut armed = super::ui_state::armed_row(ctx, armed_id);

        {
            let state = self.ui.modals.collections_dialog.as_mut().unwrap();
            egui::Window::new("Project collections")
                .open(&mut window_open)
                .collapsible(false)
                .resizable(true)
                .default_size([780.0, 560.0])
                .min_width(620.0)
                .min_height(400.0)
                .frame(super::dialog_frame(t, 14))
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        let name_resp = ui.add(
                            egui::TextEdit::singleline(&mut state.name)
                                .desired_width(180.0)
                                .hint_text("Project name")
                                .margin(egui::vec2(7.0, 5.0)),
                        );
                        ui.ctx().accesskit_node_builder(name_resp.id, |node| {
                            node.set_label("Project name");
                        });
                        if just_opened {
                            name_resp.request_focus();
                        }
                        ui.checkbox(&mut state.include_left, "Left")
                            .on_hover_text(self.ws.left.current_path.display().to_string());
                        ui.checkbox(&mut state.include_right, "Right")
                            .on_hover_text(self.ws.right.current_path.display().to_string());
                        let can_add = !state.name.trim().is_empty()
                            && (state.include_left || state.include_right);
                        // Enter in the name field adds, like the button.
                        let enter = name_resp.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if ui
                            .add_enabled(can_add, egui::Button::new("Add project"))
                            .clicked()
                            || (enter && can_add)
                        {
                            add = true;
                        }
                    });
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(180.0);
                            ui.label(
                                egui::RichText::new("Projects")
                                    .size(11.0)
                                    .strong()
                                    .color(t.text_muted),
                            );
                            ui.add_space(4.0);
                            for name in &collection_names {
                                ui.horizontal(|ui| {
                                    // Two clicks: the first arms "Confirm?".
                                    let is_armed = armed.as_deref() == Some(name.as_str());
                                    let (text, hover, label) = if is_armed {
                                        (
                                            "Confirm?",
                                            "Click again to delete this project",
                                            format!("Confirm remove project {name}"),
                                        )
                                    } else {
                                        (
                                            "Remove",
                                            "Delete project",
                                            format!("Remove project {name}"),
                                        )
                                    };
                                    let remove = ui.small_button(text).on_hover_text(hover);
                                    remove.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::WidgetType::Button,
                                            true,
                                            &label,
                                        )
                                    });
                                    if remove.clicked()
                                        && super::ui_state::confirm_on_second_click(
                                            &mut armed, name,
                                        )
                                    {
                                        delete = Some(name.clone());
                                    }
                                    if ui
                                        .selectable_label(
                                            state.selected.as_ref() == Some(name),
                                            egui::RichText::new(name).size(12.0),
                                        )
                                        .clicked()
                                    {
                                        select = Some(name.clone());
                                    }
                                });
                            }
                            if collection_names.is_empty() {
                                ui.label(
                                    egui::RichText::new("No projects")
                                        .size(11.0)
                                        .color(t.text_muted),
                                );
                            }
                        });

                        ui.separator();
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            let Some(selected) = state.selected.as_ref() else {
                                ui.label(
                                    egui::RichText::new("Select or add a project")
                                        .size(12.0)
                                        .color(t.text_muted),
                                );
                                return;
                            };
                            let roots = self
                                .project_collections
                                .get(selected)
                                .map(|collection| collection.roots.clone())
                                .unwrap_or_default();
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    egui::RichText::new(selected)
                                        .size(13.0)
                                        .strong()
                                        .color(t.text_primary),
                                );
                                for root in &roots {
                                    ui.label(
                                        egui::RichText::new(root.display().to_string())
                                            .size(10.0)
                                            .color(t.text_muted),
                                    );
                                }
                            });
                            let status = if state.run.is_some() {
                                format!("Loading {} items", state.rows.len())
                            } else {
                                format!("{} items", state.rows.len())
                            };
                            ui.label(egui::RichText::new(status).size(10.0).color(t.text_muted));
                            for root in &state.unavailable_roots {
                                ui.label(
                                    egui::RichText::new(format!("Unavailable: {}", root.display()))
                                        .size(10.0)
                                        .color(t.accent_warning),
                                );
                            }
                            if state.truncated {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "View capped at {} items",
                                        crate::collections::VIEW_CAP
                                    ))
                                    .size(10.0)
                                    .color(t.accent_warning),
                                );
                            }
                            if let Some(error) = &state.error {
                                ui.label(egui::RichText::new(error).size(10.0).color(t.accent_red));
                            }
                            ui.separator();

                            let list_height = (ui.available_height() - 16.0).max(180.0);
                            egui::ScrollArea::vertical()
                                .id_salt("project-collection-view")
                                .max_height(list_height)
                                .auto_shrink([false, false])
                                .show_rows(ui, 30.0, state.rows.len(), |ui, rows| {
                                    for index in rows {
                                        let row = &state.rows[index];
                                        let path = row.entry.path.clone();
                                        let root_label = row
                                            .root
                                            .file_name()
                                            .map(|name| name.to_string_lossy().to_string())
                                            .unwrap_or_else(|| row.root.display().to_string());
                                        let response = ui.add_sized(
                                            [ui.available_width(), 28.0],
                                            egui::Label::new(
                                                egui::RichText::new(format!(
                                                    "{}  |  {}",
                                                    row.entry.name, root_label
                                                ))
                                                .size(12.0)
                                                .color(t.text_secondary),
                                            )
                                            .truncate()
                                            .sense(Sense::click()),
                                        );
                                        if response.clicked() {
                                            reveal = Some((path, row.entry.is_dir));
                                        }
                                    }
                                });
                        });
                    });
                });
        }
        super::ui_state::set_armed_row(ctx, armed_id, armed);

        if super::ui_state::modal_close_requested(window_open, false) {
            self.ui.modals.collections_dialog = None;
            return;
        }
        if add {
            let (name, roots) = {
                let state = self.ui.modals.collections_dialog.as_ref().unwrap();
                let mut roots = Vec::new();
                if state.include_left {
                    roots.push(self.ws.left.current_path.clone());
                }
                if state.include_right {
                    roots.push(self.ws.right.current_path.clone());
                }
                (state.name.trim().to_string(), roots)
            };
            let collection = crate::collections::ProjectCollection::new(name.clone(), roots);
            let previous = self.project_collections.clone();
            let existed = self.project_collections.get(&name).is_some();
            if self.project_collections.add(collection) {
                if crate::collections::save(&self.project_collections) {
                    if let Some(state) = self.ui.modals.collections_dialog.as_mut() {
                        state.name.clear();
                    }
                    if let Some(notice) = replaced_project_notice(existed, &name) {
                        self.toast(ctx, crate::toasts::ToastKind::Info, notice, false);
                    }
                    self.start_collection_view(ctx, &name);
                } else {
                    self.project_collections = previous;
                    if let Some(state) = self.ui.modals.collections_dialog.as_mut() {
                        state.error = Some("Could not save project collections".to_string());
                    }
                }
            }
        }
        if let Some(name) = delete {
            let deleting_selected = self
                .ui
                .modals
                .collections_dialog
                .as_ref()
                .is_some_and(|state| state.selected.as_deref() == Some(name.as_str()));
            let previous = self.project_collections.clone();
            self.project_collections.remove(&name);
            if !crate::collections::save(&self.project_collections) {
                self.project_collections = previous;
                if let Some(state) = self.ui.modals.collections_dialog.as_mut() {
                    state.error = Some("Could not save project collections".to_string());
                }
            } else if deleting_selected {
                let next = self
                    .project_collections
                    .items
                    .first()
                    .map(|collection| collection.name.clone());
                if let Some(next) = next {
                    self.start_collection_view(ctx, &next);
                } else if let Some(state) = self.ui.modals.collections_dialog.as_mut() {
                    state.selected = None;
                    state.rows.clear();
                    state.run = None;
                }
            }
        }
        if let Some(name) = select {
            self.start_collection_view(ctx, &name);
        }
        if let Some((path, is_dir)) = reveal {
            if is_dir {
                self.ws.active_panel().navigate_to(path);
            } else {
                self.ws.reveal(&path);
            }
            // The window covers most of the panes; close it so the user
            // sees where the row led.
            self.ui.modals.collections_dialog = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::replaced_project_notice;

    #[test]
    fn replacing_an_existing_project_is_announced() {
        assert_eq!(replaced_project_notice(false, "web"), None);
        assert_eq!(
            replaced_project_notice(true, "Web").as_deref(),
            Some("Updated project Web with the current folders")
        );
    }
}
