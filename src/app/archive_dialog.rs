//! Read-only, bounded ZIP browser. Archive parsing stays on a worker thread.

use super::*;

impl App {
    pub(crate) fn open_archive(&mut self, path: PathBuf, ctx: &egui::Context) {
        let repaint = ctx.clone();
        let run = crate::archive::start_listing(
            path.clone(),
            std::sync::Arc::new(move || repaint.request_repaint()),
        );
        self.ui.archive = Some(ArchiveState {
            path,
            filter: String::new(),
            run: Some(run),
            listing: None,
            selected: None,
            error: None,
            focused: false,
        });
    }

    fn poll_archive(&mut self) {
        let event = self.ui.archive.as_ref().and_then(|state| {
            let run = state.run.as_ref()?;
            match run.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Archive worker stopped before completion".to_string()))
                }
            }
        });
        if let Some(result) = event
            && let Some(state) = self.ui.archive.as_mut()
        {
            state.run = None;
            match result {
                Ok(listing) => {
                    state.listing = Some(listing);
                    state.error = None;
                }
                Err(error) => state.error = Some(error),
            }
        }
    }

    pub(crate) fn show_archive_dialog(&mut self, ctx: &egui::Context) {
        let escape_requested = self.take_modal_escape(crate::accessibility::ModalSurface::Archive);
        if self.ui.archive.is_none() {
            return;
        }
        self.poll_archive();

        let t = self.colors;
        let mut window_open = true;
        let mut reveal = false;
        let mut retry = false;
        {
            let state = self.ui.archive.as_mut().unwrap();
            let title = state.path.file_name().map_or_else(
                || "Archive".to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            egui::Window::new(title)
                .open(&mut window_open)
                .collapsible(false)
                .resizable(true)
                .default_size([760.0, 580.0])
                .min_width(540.0)
                .min_height(360.0)
                .frame(
                    Frame::NONE
                        .fill(t.bg_panel)
                        .inner_margin(Margin::same(14))
                        .stroke(Stroke::new(1.0, t.border)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [(ui.available_width() - 82.0).max(120.0), 22.0],
                            egui::Label::new(
                                egui::RichText::new(state.path.display().to_string())
                                    .size(11.0)
                                    .color(t.text_muted),
                            )
                            .truncate(),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Reveal").clicked() {
                                reveal = true;
                            }
                        });
                    });
                    ui.add_space(6.0);
                    let filter = ui.add(
                        egui::TextEdit::singleline(&mut state.filter)
                            .desired_width(f32::INFINITY)
                            .hint_text("Filter archive members...")
                            .margin(egui::vec2(8.0, 6.0)),
                    );
                    if !state.focused {
                        filter.request_focus();
                        state.focused = true;
                    }

                    ui.add_space(6.0);
                    if state.run.is_some() {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(
                                egui::RichText::new("Reading central directory")
                                    .size(11.0)
                                    .color(t.text_muted),
                            );
                        });
                    }
                    if let Some(error) = &state.error {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(error).size(11.0).color(t.accent_red));
                            if ui.button("Retry").clicked() {
                                retry = true;
                            }
                        });
                    }

                    if let Some(listing) = &state.listing {
                        let needle = state.filter.trim().to_lowercase();
                        let visible: Vec<usize> = listing
                            .members
                            .iter()
                            .enumerate()
                            .filter(|(_, member)| {
                                needle.is_empty()
                                    || member
                                        .path
                                        .to_string_lossy()
                                        .to_lowercase()
                                        .contains(&needle)
                            })
                            .map(|(index, _)| index)
                            .collect();
                        let declared_bytes = listing
                            .declared_uncompressed_bytes
                            .min(u128::from(u64::MAX))
                            as u64;
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} of {} members  |  {} unpacked",
                                    visible.len(),
                                    listing.declared_members,
                                    format_size(declared_bytes)
                                ))
                                .size(11.0)
                                .color(t.text_muted),
                            );
                            if listing.unsafe_members > 0 {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} unsafe hidden",
                                        listing.unsafe_members
                                    ))
                                    .size(11.0)
                                    .color(t.accent_warning),
                                );
                            }
                            if listing.unreadable_members > 0 {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} unreadable",
                                        listing.unreadable_members
                                    ))
                                    .size(11.0)
                                    .color(t.accent_warning),
                                );
                            }
                            if listing.truncated {
                                ui.label(
                                    egui::RichText::new("Member cap reached")
                                        .size(11.0)
                                        .color(t.accent_warning),
                                );
                            }
                        });
                        ui.add_space(4.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [(ui.available_width() - 230.0).max(180.0), 20.0],
                                egui::Label::new(
                                    egui::RichText::new("Member")
                                        .size(10.0)
                                        .strong()
                                        .color(t.text_muted),
                                ),
                            );
                            for (label, width) in
                                [("Size", 72.0), ("Packed", 72.0), ("Ratio", 54.0)]
                            {
                                ui.add_sized(
                                    [width, 20.0],
                                    egui::Label::new(
                                        egui::RichText::new(label)
                                            .size(10.0)
                                            .strong()
                                            .color(t.text_muted),
                                    ),
                                );
                            }
                        });
                        ui.separator();

                        let rows_height = (ui.available_height() - 38.0).max(120.0);
                        egui::ScrollArea::vertical()
                            .id_salt("archive-members")
                            .auto_shrink([false, false])
                            .max_height(rows_height)
                            .show_rows(ui, 30.0, visible.len(), |ui, rows| {
                                for row in rows {
                                    let index = visible[row];
                                    let member = &listing.members[index];
                                    let selected = state.selected == Some(index);
                                    let response = Frame::NONE
                                        .fill(if selected {
                                            t.bg_selected.linear_multiply(0.55)
                                        } else if row % 2 == 0 {
                                            t.bg_card.linear_multiply(0.18)
                                        } else {
                                            Color32::TRANSPARENT
                                        })
                                        .inner_margin(Margin::symmetric(4, 3))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                let path = member.path.display().to_string();
                                                ui.add_sized(
                                                    [
                                                        (ui.available_width() - 230.0).max(180.0),
                                                        22.0,
                                                    ],
                                                    egui::Label::new(
                                                        egui::RichText::new(path)
                                                            .size(11.0)
                                                            .color(t.text_secondary),
                                                    )
                                                    .truncate(),
                                                );
                                                let size = if member.is_dir {
                                                    String::new()
                                                } else {
                                                    format_size(member.size)
                                                };
                                                let packed = if member.is_dir {
                                                    String::new()
                                                } else {
                                                    format_size(member.compressed_size)
                                                };
                                                let ratio = if member.is_dir
                                                    || member.compressed_size == 0
                                                {
                                                    String::new()
                                                } else {
                                                    format!(
                                                        "{:.1}x",
                                                        member.size as f64
                                                            / member.compressed_size as f64
                                                    )
                                                };
                                                for (value, width) in
                                                    [(size, 72.0), (packed, 72.0), (ratio, 54.0)]
                                                {
                                                    ui.add_sized(
                                                        [width, 22.0],
                                                        egui::Label::new(
                                                            egui::RichText::new(value)
                                                                .size(10.0)
                                                                .color(t.text_muted),
                                                        ),
                                                    );
                                                }
                                            });
                                        })
                                        .response
                                        .interact(Sense::click());
                                    if response.clicked() {
                                        state.selected = Some(index);
                                    }
                                }
                            });
                        if visible.is_empty() {
                            ui.label(
                                egui::RichText::new("No matching members")
                                    .size(11.0)
                                    .color(t.text_muted),
                            );
                        }
                        if let Some(selected) =
                            state.selected.and_then(|index| listing.members.get(index))
                        {
                            ui.separator();
                            ui.label(
                                egui::RichText::new(format!(
                                    "#{}  |  {}",
                                    selected.index,
                                    selected.path.display()
                                ))
                                .size(10.0)
                                .color(t.text_muted),
                            );
                        }
                    }
                });
        }

        if !window_open || escape_requested {
            self.ui.archive = None;
            return;
        }
        if reveal && let Some(state) = &self.ui.archive {
            self.ws.reveal(&state.path);
        }
        if retry && let Some(path) = self.ui.archive.as_ref().map(|state| state.path.clone()) {
            self.open_archive(path, ctx);
        }
    }
}
