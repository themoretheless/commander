//! Read-only, bounded ZIP browser. Archive parsing stays on a worker thread.

use super::*;

/// Indexes of the archive members matching the filter text. A listing holds
/// up to 100k members and the sheet repaints on pointer moves and spinners,
/// so the lowercased paths are built once per listing and the matches only
/// when the filter text changes.
///
/// It also holds the member list's cursor: the member last clicked or reached
/// with the arrow keys, so it resets with the listing too.
#[derive(Default)]
pub struct MemberFilter {
    lower_paths: Option<Vec<String>>,
    needle: Option<String>,
    visible: Vec<usize>,
    focused: Option<usize>,
    /// The cursor moved by key and its row should scroll into view.
    scroll_to_focused: bool,
}

impl MemberFilter {
    fn visible(&mut self, listing: &crate::archive::ArchiveListing, filter: &str) -> &[usize] {
        let needle = filter.trim().to_lowercase();
        if self.needle.as_deref() != Some(needle.as_str()) {
            self.visible = if needle.is_empty() {
                (0..listing.members.len()).collect()
            } else {
                let lower = self.lower_paths.get_or_insert_with(|| {
                    listing
                        .members
                        .iter()
                        .map(|member| member.path.to_string_lossy().to_lowercase())
                        .collect()
                });
                lower
                    .iter()
                    .enumerate()
                    .filter(|(_, path)| path.contains(&needle))
                    .map(|(index, _)| index)
                    .collect()
            };
            self.needle = Some(needle);
        }
        &self.visible
    }
}

/// Inline extract errors shown at most; the rest are summarized as a count.
const MAX_INLINE_ERRORS: usize = 5;

/// Toast and inline text for a partly failed extraction. A read-only
/// destination fails every member, so the toast names only the count and
/// the first error and the inline list stays capped.
fn extract_error_summary(errors: &[String]) -> (String, String) {
    let toast = match errors {
        [only] => only.clone(),
        [first, ..] => format!("{} members failed: {first}", errors.len()),
        [] => String::new(),
    };
    let mut inline = errors
        .iter()
        .take(MAX_INLINE_ERRORS)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    if errors.len() > MAX_INLINE_ERRORS {
        inline.push_str(&format!(
            "\n… and {} more",
            errors.len() - MAX_INLINE_ERRORS
        ));
    }
    (toast, inline)
}

/// Label of the extract-selection button. Selected members the filter hides
/// are still extracted, so their count is named.
fn extract_selected_label(selected: usize, hidden: usize) -> String {
    match (selected, hidden) {
        (0, _) => "Extract selected to other panel".to_string(),
        (n, 0) => format!("Extract {n} to other panel"),
        (n, hidden) => format!("Extract {n} ({hidden} hidden) to other panel"),
    }
}

/// Label of the extract-everything button: a capped listing only holds the
/// first members, and only those are extracted.
fn extract_all_label(listing: &crate::archive::ArchiveListing) -> String {
    if listing.truncated {
        format!("Extract {} listed", listing.members.len())
    } else {
        "Extract all".to_string()
    }
}

/// Move the member cursor one row through `visible` (member indexes in list
/// order). A cursor the filter hides restarts at the first visible row.
fn step_member_cursor(
    visible: &[usize],
    focused: Option<usize>,
    up: bool,
    down: bool,
) -> Option<usize> {
    if !(up || down) || visible.is_empty() {
        return focused;
    }
    let Some(row) = focused.and_then(|index| visible.iter().position(|&v| v == index)) else {
        return visible.first().copied();
    };
    let row = if down {
        (row + 1).min(visible.len() - 1)
    } else {
        row.saturating_sub(1)
    };
    Some(visible[row])
}

impl App {
    pub(crate) fn open_archive(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.open_archive_on(path, self.ws.active.other(), ctx);
    }

    /// Open the archive tool over pane `side`. Retry passes the tool's own
    /// side: clicking in the tool makes its pane active, so recomputing it
    /// from focus would flip the tool to the other pane.
    fn open_archive_on(&mut self, path: PathBuf, side: ActivePanel, ctx: &egui::Context) {
        let repaint = ctx.clone();
        let run = crate::archive::start_listing(
            path.clone(),
            std::sync::Arc::new(move || repaint.request_repaint()),
        );
        self.ui.tools.replace_on(side);
        self.ui.tools.archive = Some(ArchiveState {
            path,
            filter: String::new(),
            run: Some(run),
            extract: None,
            listing: None,
            member_filter: MemberFilter::default(),
            selected: std::collections::BTreeSet::new(),
            status: None,
            error: None,
        });
        Self::mark_tool_opened(ctx, "archive");
    }

    /// The visible pane beside the tool, which extraction writes into.
    fn archive_extract_dest(&self) -> PathBuf {
        let side = self.ui.tools.side.unwrap_or(self.ws.active).other();
        match side {
            ActivePanel::Left => self.ws.left.current_path.clone(),
            ActivePanel::Right => self.ws.right.current_path.clone(),
        }
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
                    state.member_filter = MemberFilter::default();
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
                        let (toast, inline) = extract_error_summary(&report.errors);
                        state.error = Some(inline);
                        self.toast_at(now, crate::toasts::ToastKind::Error, toast, false);
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
        let mut stop_extract = false;
        let dest = self.archive_extract_dest();
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
                // Up/Down in the filter move the member cursor and Enter
                // toggles its selection, so the list works without a mouse.
                let filter_id = egui::Id::new("archive-filter");
                let (cursor_up, cursor_down) = if ui.memory(|m| m.has_focus(filter_id)) {
                    ui.input_mut(|input| {
                        (
                            input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                            input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                        )
                    })
                } else {
                    (false, false)
                };
                let filter = ui.add(
                    egui::TextEdit::singleline(&mut state.filter)
                        .id(filter_id)
                        .desired_width(f32::INFINITY)
                        .hint_text("Filter archive members (Up/Down to move, Enter to select)")
                        .margin(egui::vec2(8.0, 6.0)),
                );
                let toggle_cursor =
                    filter.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if Self::take_tool_opened(ui.ctx(), "archive") || toggle_cursor {
                    filter.request_focus();
                }

                ui.add_space(6.0);
                if state.run.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("Reading archive contents…")
                                .size(11.0)
                                .color(t.text_muted),
                        );
                    });
                }
                let status = if state.extract.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        let status = ui.label(
                            egui::RichText::new(state.status.as_deref().unwrap_or("Extracting…"))
                                .size(11.0)
                                .color(t.text_muted),
                        );
                        if ui
                            .small_button("Stop")
                            .on_hover_text("Stop extracting; members already written stay")
                            .clicked()
                        {
                            stop_extract = true;
                        }
                        Some(status)
                    })
                    .inner
                } else {
                    state.status.as_ref().map(|status| {
                        ui.label(
                            egui::RichText::new(status)
                                .size(11.0)
                                .color(t.text_secondary),
                        )
                    })
                };
                // Announce progress and the result to assistive technology.
                if let Some(status) = status {
                    ui.ctx().accesskit_node_builder(status.id, |node| {
                        node.set_role(egui::accesskit::Role::Status);
                        node.set_live(egui::accesskit::Live::Polite);
                    });
                }
                if let Some(error) = &state.error {
                    ui.add(
                        egui::Label::new(egui::RichText::new(error).size(11.0).color(t.accent_red))
                            .wrap(),
                    );
                    // Once listed, errors come from extraction: re-listing
                    // would drop the selection, and Extract retries it.
                    if state.listing.is_none() && ui.button("Retry").clicked() {
                        retry = true;
                    }
                }

                if let Some(listing) = &state.listing {
                    state.member_filter.visible(listing, &state.filter);
                    let MemberFilter {
                        visible,
                        focused,
                        scroll_to_focused,
                        ..
                    } = &mut state.member_filter;
                    let visible: &[usize] = visible;
                    if cursor_up || cursor_down {
                        *focused = step_member_cursor(visible, *focused, cursor_up, cursor_down);
                        *scroll_to_focused = true;
                    }
                    if toggle_cursor
                        && let Some(index) = *focused
                        && !state.selected.remove(&index)
                    {
                        state.selected.insert(index);
                    }
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
                                    "{} with unsafe paths hidden",
                                    listing.unsafe_members
                                ))
                                .size(11.0)
                                .color(t.accent_warning),
                            )
                            .on_hover_text(
                                "These entries have absolute paths or ../ parts that would \
                                 write outside the destination, so they are left out",
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
                            // The cursor row may lie outside the rows drawn,
                            // so scroll to where it would be.
                            if std::mem::take(scroll_to_focused)
                                && let Some(row) = focused
                                    .and_then(|index| visible.iter().position(|&v| v == index))
                            {
                                let pitch = 30.0 + ui.spacing().item_spacing.y;
                                let top =
                                    ui.max_rect().top() + (row as f32 - rows.start as f32) * pitch;
                                ui.scroll_to_rect(
                                    egui::Rect::from_min_size(
                                        egui::pos2(ui.max_rect().left(), top),
                                        egui::vec2(1.0, pitch),
                                    ),
                                    None,
                                );
                            }
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
                                    .stroke(if *focused == Some(index) {
                                        Stroke::new(1.0_f32, t.accent)
                                    } else {
                                        Stroke::NONE
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
                                response.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::WidgetType::SelectableLabel,
                                        true,
                                        selected,
                                        member.path.display().to_string(),
                                    )
                                });
                                if response.clicked() {
                                    *focused = Some(index);
                                    if !state.selected.remove(&index) {
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
                    // The member last clicked or moved to, not the lowest
                    // selected index.
                    if let Some(member) = focused
                        .or_else(|| state.selected.iter().next().copied())
                        .and_then(|index| listing.members.get(index))
                    {
                        ui.separator();
                        ui.label(
                            egui::RichText::new(format!(
                                "#{}  |  {}",
                                member.index,
                                member.path.display()
                            ))
                            .size(10.0)
                            .color(t.text_muted),
                        );
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!visible.is_empty(), egui::Button::new("Select all"))
                            .on_hover_text("Select every member the filter shows")
                            .clicked()
                        {
                            state.selected.extend(visible.iter().copied());
                        }
                        if ui
                            .add_enabled(!state.selected.is_empty(), egui::Button::new("Clear"))
                            .on_hover_text("Clear the selection")
                            .clicked()
                        {
                            state.selected.clear();
                        }
                    });
                    ui.add_space(4.0);
                    let dest_hint = format!("Extract into {}", dest.display());
                    ui.horizontal(|ui| {
                        let n = state.selected.len();
                        let hidden = if state.filter.trim().is_empty() {
                            0
                        } else {
                            state
                                .selected
                                .iter()
                                .filter(|index| visible.binary_search(index).is_err())
                                .count()
                        };
                        if ui
                            .add_enabled(
                                n > 0 && state.extract.is_none(),
                                super::dialog_button(
                                    t,
                                    super::ButtonRole::Primary,
                                    egui::RichText::new(extract_selected_label(n, hidden))
                                        .size(13.0),
                                ),
                            )
                            .on_hover_text(&dest_hint)
                            .on_disabled_hover_text(if n == 0 {
                                "Click members, or move with Up/Down and press Enter, to \
                                 select them"
                            } else {
                                "An extraction is running"
                            })
                            .clicked()
                        {
                            extract_selected = true;
                        }
                        let all_hint = if listing.truncated {
                            format!(
                                "{dest_hint}. Member cap reached: only the {} listed members \
                                 are extracted",
                                listing.members.len()
                            )
                        } else {
                            dest_hint.clone()
                        };
                        if ui
                            .add_enabled(
                                !listing.members.is_empty() && state.extract.is_none(),
                                super::dialog_button(
                                    t,
                                    super::ButtonRole::Secondary,
                                    egui::RichText::new(extract_all_label(listing)).size(13.0),
                                ),
                            )
                            .on_hover_text(all_hint)
                            .clicked()
                        {
                            extract_all = true;
                        }
                    });
                }
            });
        }

        if !window_open || escape_requested {
            // Dropping the run cancels it after the member in progress.
            if self
                .ui
                .tools
                .archive
                .as_ref()
                .is_some_and(|state| state.extract.is_some())
            {
                self.toast_at(
                    now,
                    crate::toasts::ToastKind::Info,
                    "Extraction cancelled; some members may already be extracted".to_string(),
                    false,
                );
            }
            self.ui.tools.archive = None;
            return;
        }
        if stop_extract && let Some(state) = self.ui.tools.archive.as_mut() {
            // Dropping the run cancels it after the member in progress.
            state.extract = None;
            state.status =
                Some("Extraction stopped; members already written stay in place".to_string());
            self.ws.left.refresh();
            self.ws.right.refresh();
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
            let side = self.ui.tools.side.unwrap_or_else(|| self.ws.active.other());
            self.open_archive_on(path, side, ctx);
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
        let dest = self.archive_extract_dest();
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

#[cfg(test)]
mod tests {
    use super::{
        MemberFilter, extract_all_label, extract_error_summary, extract_selected_label,
        step_member_cursor,
    };

    fn listing(paths: &[&str]) -> crate::archive::ArchiveListing {
        crate::archive::ArchiveListing {
            kind: crate::archive::ArchiveKind::Zip,
            members: paths
                .iter()
                .enumerate()
                .map(|(index, path)| crate::archive::ArchiveMember {
                    index,
                    path: std::path::PathBuf::from(path),
                    is_dir: false,
                    size: 1,
                    compressed_size: 1,
                    tar_name: None,
                    modified: None,
                })
                .collect(),
            declared_members: paths.len(),
            declared_uncompressed_bytes: paths.len() as u128,
            unsafe_members: 0,
            unreadable_members: 0,
            truncated: false,
        }
    }

    #[test]
    fn member_filter_matches_case_insensitively_and_tracks_the_text() {
        let listing = listing(&["src/Main.rs", "README.md", "src/lib.rs"]);
        let mut filter = MemberFilter::default();
        assert_eq!(filter.visible(&listing, ""), [0, 1, 2]);
        assert_eq!(filter.visible(&listing, "  SRC "), [0, 2]);
        assert_eq!(filter.visible(&listing, "main"), [0]);
        assert_eq!(filter.visible(&listing, "nothing"), [] as [usize; 0]);
        assert_eq!(filter.visible(&listing, ""), [0, 1, 2]);
    }

    #[test]
    fn unchanged_filter_reuses_its_matches() {
        let listing = listing(&["a.txt", "b.txt"]);
        let mut filter = MemberFilter::default();
        let first = filter.visible(&listing, "a").as_ptr();
        // Same text, even with a different listing: no rescan until reset.
        let other = self::listing(&["b.txt", "a.txt"]);
        assert_eq!(filter.visible(&other, "a").as_ptr(), first);
        assert_eq!(filter.visible(&other, "a"), [0]);
    }

    #[test]
    fn many_extract_errors_toast_a_count_and_cap_the_inline_list() {
        let one = vec!["Could not create /ro/a: denied".to_string()];
        assert_eq!(
            extract_error_summary(&one),
            (one[0].clone(), one[0].clone())
        );

        let errors: Vec<String> = (0..8)
            .map(|i| format!("Could not create /ro/{i}"))
            .collect();
        let (toast, inline) = extract_error_summary(&errors);
        assert_eq!(toast, "8 members failed: Could not create /ro/0");
        assert_eq!(inline.lines().count(), 6);
        assert!(inline.ends_with("… and 3 more"));
    }

    #[test]
    fn extract_labels_name_hidden_selection_and_capped_listings() {
        assert_eq!(
            extract_selected_label(0, 0),
            "Extract selected to other panel"
        );
        assert_eq!(extract_selected_label(3, 0), "Extract 3 to other panel");
        assert_eq!(
            extract_selected_label(3, 2),
            "Extract 3 (2 hidden) to other panel"
        );

        let mut capped = listing(&["a", "b"]);
        assert_eq!(extract_all_label(&capped), "Extract all");
        capped.truncated = true;
        assert_eq!(extract_all_label(&capped), "Extract 2 listed");
    }

    #[test]
    fn member_cursor_steps_through_visible_rows_only() {
        let visible = [2, 5, 9];
        assert_eq!(step_member_cursor(&visible, None, false, false), None);
        assert_eq!(step_member_cursor(&visible, None, false, true), Some(2));
        assert_eq!(step_member_cursor(&visible, Some(2), false, true), Some(5));
        assert_eq!(step_member_cursor(&visible, Some(9), false, true), Some(9));
        assert_eq!(step_member_cursor(&visible, Some(5), true, false), Some(2));
        assert_eq!(step_member_cursor(&visible, Some(2), true, false), Some(2));
        // A cursor the filter now hides restarts at the top.
        assert_eq!(step_member_cursor(&visible, Some(4), true, false), Some(2));
        assert_eq!(step_member_cursor(&[], Some(4), false, true), Some(4));
    }
}
