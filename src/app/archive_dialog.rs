//! Read-only, bounded ZIP browser. Archive parsing stays on a worker thread.

use super::*;

impl App {
    pub(crate) fn open_archive(&mut self, path: PathBuf, ctx: &egui::Context) {
        let repaint = ctx.clone();
        let run = crate::archive::start_listing(
            path.clone(),
            std::sync::Arc::new(move || repaint.request_repaint()),
        );
        self.ui.tools.replace_on(self.ws.active.other());
        self.ui.tools.archive = Some(ArchiveState {
            path,
            filter: String::new(),
            run: Some(run),
            extract: None,
            listing: None,
            selected: std::collections::BTreeSet::new(),
            status: None,
            error: None,
        });
        Self::mark_tool_opened(ctx, "archive");
    }

    fn poll_archive(&mut self, now: f64) {
        let event = self.ui.tools.archive.as_ref().and_then(|state| {
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
            && let Some(state) = self.ui.tools.archive.as_mut()
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

        let extract_event = self.ui.tools.archive.as_ref().and_then(|state| {
            let run = state.extract.as_ref()?;
            match run.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Extract worker stopped before completion".to_string()))
                }
            }
        });
        if let Some(result) = extract_event
            && let Some(state) = self.ui.tools.archive.as_mut()
        {
            state.extract = None;
            match result {
                Ok(report) => {
                    let status = format!(
                        "Extracted {} · skipped existing {} · dirs {}",
                        report.extracted, report.skipped_existing, report.skipped_dirs
                    );
                    state.status = Some(status.clone());
                    if report.errors.is_empty() {
                        state.error = None;
                        self.toast_at(now, crate::toasts::ToastKind::Info, status, false);
                    } else {
                        let detail = report.errors.join("; ");
                        state.error = Some(detail.clone());
                        self.toast_at(now, crate::toasts::ToastKind::Error, detail, false);
                    }
                    self.ws.left.refresh();
                    self.ws.right.refresh();
                }
                Err(error) => {
                    // Drop the "Extracting…" line so it is not left beside the error.
                    state.status = None;
                    state.error = Some(error.clone());
                    self.toast_at(now, crate::toasts::ToastKind::Error, error, false);
                }
            }
        }
    }

    pub(crate) fn show_archive_tool(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let escape_requested =
            self.take_escape_request(crate::accessibility::EscapeRoute::PaneTool);
        if self.ui.tools.archive.is_none() {
            return;
        }
        let now = ctx.input(|input| input.time);
        self.poll_archive(now);

        let t = self.colors;
        let window_open = true;
        let mut reveal = false;
        let mut retry = false;
        let mut extract_selected = false;
        let mut extract_all = false;
        {
            let state = self.ui.tools.archive.as_mut().unwrap();
            let title = state.path.file_name().map_or_else(
                || "Archive".to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .size(13.0)
                        .strong()
                        .color(t.text_primary),
                );
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2((ui.available_width() - 82.0).max(120.0), 22.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(state.path.display().to_string())
                                        .size(11.0)
                                        .color(t.text_muted),
                                )
                                .truncate(),
                            )
                        },
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
                if Self::take_tool_opened(ui.ctx(), "archive") {
                    filter.request_focus();
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
                if state.extract.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new(state.status.as_deref().unwrap_or("Extracting…"))
                                .size(11.0)
                                .color(t.text_muted),
                        );
                    });
                } else if let Some(status) = &state.status {
                    ui.label(
                        egui::RichText::new(status)
                            .size(11.0)
                            .color(t.text_secondary),
                    );
                }
                if let Some(error) = &state.error {
                    ui.add(
                        egui::Label::new(egui::RichText::new(error).size(11.0).color(t.accent_red))
                            .wrap(),
                    );
                    if ui.button("Retry").clicked() {
                        retry = true;
                    }
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
                        .min(u128::from(u64::MAX)) as u64;
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {} of {} members  |  {} unpacked",
                                listing.kind.label(),
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
                        // Rows sit in a frame with a 4 pt inset; match it so
                        // the header columns line up with the values.
                        ui.add_space(4.0);
                        left_cell(
                            ui,
                            (ui.available_width() - 4.0 - 230.0).max(180.0),
                            20.0,
                            egui::Label::new(
                                egui::RichText::new("Member")
                                    .size(10.0)
                                    .strong()
                                    .color(t.text_muted),
                            ),
                        );
                        for (label, width) in [("Size", 72.0), ("Packed", 72.0), ("Ratio", 54.0)] {
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
                                let selected = state.selected.contains(&index);
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
                                            left_cell(
                                                ui,
                                                (ui.available_width() - 230.0).max(180.0),
                                                22.0,
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
                                            let ratio =
                                                if member.is_dir || member.compressed_size == 0 {
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
                                    if state.selected.contains(&index) {
                                        state.selected.remove(&index);
                                    } else {
                                        state.selected.insert(index);
                                    }
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
                    if let Some(selected) = state
                        .selected
                        .iter()
                        .next()
                        .and_then(|index| listing.members.get(*index))
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

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let n = state.selected.len();
                        if ui
                            .add_enabled(
                                n > 0 && state.extract.is_none(),
                                super::dialog_button(
                                    t,
                                    super::ButtonRole::Primary,
                                    egui::RichText::new(format!("Extract {n} to other panel"))
                                        .size(13.0),
                                ),
                            )
                            .clicked()
                        {
                            extract_selected = true;
                        }
                        if ui
                            .add_enabled(
                                state.extract.is_none(),
                                super::dialog_button(
                                    t,
                                    super::ButtonRole::Secondary,
                                    egui::RichText::new("Extract all").size(13.0),
                                ),
                            )
                            .clicked()
                        {
                            extract_all = true;
                        }
                    });
                }
            });
        }

        if !window_open || escape_requested {
            self.ui.tools.archive = None;
            return;
        }
        if reveal && let Some(state) = &self.ui.tools.archive {
            self.ws.reveal(&state.path);
        }
        if retry
            && let Some(path) = self
                .ui
                .tools
                .archive
                .as_ref()
                .map(|state| state.path.clone())
        {
            self.open_archive(path, ctx);
            return;
        }
        if extract_selected || extract_all {
            self.start_archive_extract(ctx, extract_all);
        }
    }

    fn start_archive_extract(&mut self, ctx: &egui::Context, all: bool) {
        let Some(state) = self.ui.tools.archive.as_ref() else {
            return;
        };
        let Some(listing) = state.listing.as_ref() else {
            return;
        };
        let indexes: Vec<usize> = if all {
            listing.members.iter().map(|m| m.index).collect()
        } else {
            state
                .selected
                .iter()
                .filter_map(|row| listing.members.get(*row).map(|m| m.index))
                .collect()
        };
        if indexes.is_empty() {
            return;
        }
        let archive = state.path.clone();
        let dest = self.ws.inactive_panel().current_path.clone();
        let repaint = ctx.clone();
        let run = crate::archive::start_extract(
            archive,
            dest,
            indexes,
            std::sync::Arc::new(move || repaint.request_repaint()),
        );
        if let Some(state) = self.ui.tools.archive.as_mut() {
            state.extract = Some(run);
            state.error = None;
            state.status = Some("Extracting…".to_string());
        }
    }
}

/// A fixed-width table cell whose label starts at the left edge
/// (`add_sized` would center it).
fn left_cell(ui: &mut egui::Ui, width: f32, height: f32, label: egui::Label) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_width(width);
            ui.add(label)
        },
    )
    .inner
}
