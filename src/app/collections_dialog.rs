//! Named multi-root project collections and their read-only virtual view.

use super::*;

impl App {
    fn start_collection_view(&mut self, ctx: &egui::Context, name: &str) {
        let Some(collection) = self.project_collections.get(name).cloned() else {
            return;
        };
        let repaint = ctx.clone();
        let run = crate::collections::spawn_view(
            collection,
            std::sync::Arc::new(move || repaint.request_repaint()),
        );
        if let Some(state) = self.collections_dialog.as_mut() {
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
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(run) = self
            .collections_dialog
            .as_ref()
            .and_then(|state| state.run.as_ref())
        {
            loop {
                match run.try_recv() {
                    Ok(event) => events.push(event),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if let Some(state) = self.collections_dialog.as_mut() {
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

    pub(crate) fn show_collections_dialog(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.ws.collections_request) {
            let selected = self
                .project_collections
                .items
                .first()
                .map(|collection| collection.name.clone());
            self.collections_dialog = Some(CollectionsDialogState {
                selected: selected.clone(),
                ..Default::default()
            });
            if let Some(name) = selected {
                self.start_collection_view(ctx, &name);
            }
        }
        if self.collections_dialog.is_none() {
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

        {
            let state = self.collections_dialog.as_mut().unwrap();
            egui::Window::new("Project collections")
                .open(&mut window_open)
                .collapsible(false)
                .resizable(true)
                .default_size([780.0, 560.0])
                .min_width(620.0)
                .min_height(400.0)
                .frame(
                    Frame::NONE
                        .fill(t.bg_panel)
                        .inner_margin(Margin::same(14))
                        .stroke(Stroke::new(1.0_f32, t.border)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut state.name)
                                .desired_width(180.0)
                                .hint_text("Project name")
                                .margin(egui::vec2(7.0, 5.0)),
                        );
                        ui.checkbox(&mut state.include_left, "Left")
                            .on_hover_text(self.ws.left.current_path.display().to_string());
                        ui.checkbox(&mut state.include_right, "Right")
                            .on_hover_text(self.ws.right.current_path.display().to_string());
                        let can_add = !state.name.trim().is_empty()
                            && (state.include_left || state.include_right);
                        if ui
                            .add_enabled(can_add, egui::Button::new("Add project"))
                            .clicked()
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
                                    if ui
                                        .small_button("Remove")
                                        .on_hover_text("Delete project")
                                        .clicked()
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

        if !window_open {
            self.collections_dialog = None;
            return;
        }
        if add {
            let (name, roots) = {
                let state = self.collections_dialog.as_ref().unwrap();
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
            if self.project_collections.add(collection) {
                if crate::collections::save(&self.project_collections) {
                    if let Some(state) = self.collections_dialog.as_mut() {
                        state.name.clear();
                    }
                    self.start_collection_view(ctx, &name);
                } else {
                    self.project_collections = previous;
                    if let Some(state) = self.collections_dialog.as_mut() {
                        state.error = Some("Could not save project collections".to_string());
                    }
                }
            }
        }
        if let Some(name) = delete {
            let deleting_selected = self
                .collections_dialog
                .as_ref()
                .is_some_and(|state| state.selected.as_deref() == Some(name.as_str()));
            let previous = self.project_collections.clone();
            self.project_collections.remove(&name);
            if !crate::collections::save(&self.project_collections) {
                self.project_collections = previous;
                if let Some(state) = self.collections_dialog.as_mut() {
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
                } else if let Some(state) = self.collections_dialog.as_mut() {
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
        }
    }
}
