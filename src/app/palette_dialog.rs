//! Command palette (Cmd+K): fuzzy-filter every user-facing command, ranked by
//! recency/frequency, and run it. The catalog and ranking live in
//! `crate::command`.

use super::*;

impl App {
    pub(crate) fn show_palette_dialog(&mut self, ctx: &egui::Context) {
        let just_opened = std::mem::take(&mut self.ws.palette_request);
        if just_opened {
            self.palette_input = Some(String::new());
        }
        if self.palette_input.is_none() {
            return;
        }
        let t = self.colors;

        // Rank from the query at frame start (owned, so editing the buffer
        // below does not conflict with reading the usage history).
        let query = self.palette_input.clone().unwrap();
        let matches = crate::command::rank(&query, &self.palette_usage, self.palette_tick);
        let previews: Vec<String> = matches
            .iter()
            .map(|m| self.palette_command_preview(m.command))
            .collect();
        let buffer = self.palette_input.as_mut().unwrap();
        let mut run: Option<(&'static str, crate::command::Command)> = None;
        let mut cancel = false;

        egui::Window::new("Command palette")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                Frame::NONE
                    .fill(t.bg_panel)
                    .inner_margin(Margin::same(12))
                    .stroke(Stroke::new(1.0_f32, t.border)),
            )
            .show(ctx, |ui| {
                ui.set_width(520.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Command Palette")
                            .size(13.0)
                            .strong()
                            .color(t.text_primary),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("\u{2318}K")
                                .size(11.0)
                                .color(t.text_muted),
                        );
                    });
                });
                ui.add_space(8.0);
                let resp = ui.add(
                    egui::TextEdit::singleline(buffer)
                        .desired_width(f32::INFINITY)
                        .hint_text("Search commands, shortcuts, views\u{2026}")
                        .margin(egui::vec2(8.0, 6.0)),
                );
                if just_opened {
                    resp.request_focus();
                }
                ui.add_space(6.0);

                if matches.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "No matching command. Try file, view, select, or cmd h.",
                        )
                        .size(11.0)
                        .color(t.text_muted),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            for (i, m) in matches.iter().enumerate() {
                                let fill = if i == 0 {
                                    t.accent.linear_multiply(0.12)
                                } else {
                                    Color32::TRANSPARENT
                                };
                                let lead = if i == 0 { "\u{25b8} " } else { "  " };
                                let job = Self::palette_row_job(lead, m, t);
                                let resp = Frame::NONE
                                    .fill(fill)
                                    .corner_radius(crate::theme::ROUNDING_SM)
                                    .inner_margin(Margin::symmetric(8, 6))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.vertical(|ui| {
                                                ui.add(egui::Label::new(job));
                                                let preview =
                                                    previews.get(i).map_or("", String::as_str);
                                                let detail = if preview.is_empty() {
                                                    Self::palette_category(m.command).to_string()
                                                } else {
                                                    format!(
                                                        "{} \u{00b7} {}",
                                                        Self::palette_category(m.command),
                                                        preview
                                                    )
                                                };
                                                ui.label(
                                                    egui::RichText::new(detail)
                                                        .size(10.0)
                                                        .color(t.text_muted),
                                                );
                                            });
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    if !m.shortcut.is_empty() {
                                                        Frame::NONE
                                                            .fill(t.bg_card)
                                                            .corner_radius(
                                                                crate::theme::ROUNDING_SM,
                                                            )
                                                            .inner_margin(Margin::symmetric(7, 2))
                                                            .show(ui, |ui| {
                                                                ui.label(
                                                                    egui::RichText::new(m.shortcut)
                                                                        .size(10.0)
                                                                        .color(t.text_secondary),
                                                                );
                                                            });
                                                    }
                                                },
                                            );
                                        });
                                    })
                                    .response
                                    .interact(Sense::click())
                                    .on_hover_text(m.shortcut);
                                if resp.clicked() {
                                    run = Some((m.label, m.command));
                                }
                            }
                        });
                }

                if ui.input(|i| i.key_pressed(egui::Key::Enter))
                    && let Some(m) = matches.first()
                {
                    run = Some((m.label, m.command));
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });

        if cancel {
            self.palette_input = None;
            return;
        }
        if let Some((label, cmd)) = run {
            // Close the palette first; the command may open another dialog.
            self.palette_input = None;
            // Record the run so it ranks higher next time.
            self.palette_tick += 1;
            self.palette_usage.record(label, self.palette_tick);
            self.ws.execute(cmd);
        }
    }

    fn palette_command_preview(&self, command: crate::command::Command) -> String {
        use crate::command::Command;
        let active = self.ws.active_panel_ref();
        let inactive = self.ws.inactive_panel();
        let picked = active
            .selected_or_cursor()
            .map_or(0, |entries| entries.len());
        let active_path = Self::palette_path_label(&active.current_path);
        let inactive_path = Self::palette_path_label(&inactive.current_path);
        match command {
            Command::RequestCopy => format!("{picked} item(s) -> {inactive_path}"),
            Command::RequestMove => format!("{picked} item(s) -> {inactive_path}"),
            Command::MoveIntoCursorFolder | Command::CopyIntoCursorFolder => active
                .filtered_get(active.cursor.saturating_sub(1))
                .filter(|entry| entry.is_dir)
                .map(|entry| {
                    format!(
                        "{} {picked} selected item(s) -> {}",
                        if command == Command::CopyIntoCursorFolder {
                            "Copy"
                        } else {
                            "Move"
                        },
                        entry.name
                    )
                })
                .unwrap_or_else(|| "highlight a destination folder".to_string()),
            Command::RequestDelete => format!("{picked} item(s) to Trash"),
            Command::CreateDir => format!("in {active_path}"),
            Command::BeginRename => active
                .filtered_get(active.cursor.saturating_sub(1))
                .map(|e| e.name.clone())
                .unwrap_or_else(|| "cursor item".to_string()),
            Command::BeginBatchRename => format!("{picked} item(s)"),
            Command::BeginSync => format!("{active_path} <-> {inactive_path}"),
            Command::FindDuplicates | Command::DiskTreemap | Command::BeginFind => active_path,
            Command::OpenSavedSearch => "saved smart folders".to_string(),
            Command::OpenProjectCollections => "multi-root project views".to_string(),
            Command::OpenRecoveryCenter => format!(
                "{} interrupted, {} staging",
                self.recovery.operations.len(),
                self.recovery.orphans.len()
            ),
            Command::CopyPath
            | Command::CopyName
            | Command::CopyParentPath
            | Command::CopyFileUrl
            | Command::CopyShellPath
            | Command::CopyRelativePath => format!("{picked} item(s)"),
            Command::ShelfAdd => format!("{picked} item(s)"),
            Command::ShelfDrain => format!("{} staged -> {active_path}", self.ws.shelf.len()),
            Command::ToggleInfo => "opposite panel inspector".to_string(),
            Command::BeginGoToPath => "jump to folder".to_string(),
            Command::BeginRecent => "recent folders".to_string(),
            Command::SelectAll | Command::InvertSelection => {
                format!("{} visible item(s)", active.filtered_count())
            }
            Command::SelectSameNamed => format!("against {inactive_path}"),
            Command::BeginSelectMask => "glob selection".to_string(),
            Command::ToggleHidden => {
                if active.show_hidden {
                    "currently on".to_string()
                } else {
                    "currently off".to_string()
                }
            }
            Command::CycleDensity => {
                let next = crate::density::cycle(active.density, 1);
                format!("next: {}", crate::density::label(next))
            }
            Command::TogglePreview => "opposite panel preview".to_string(),
            Command::EqualizePanels => format!("{inactive_path} -> {active_path}"),
            Command::SwapPanels => "exchange left and right".to_string(),
            Command::Undo => "last reversible operation".to_string(),
            Command::Redo => "last undone operation".to_string(),
            _ => String::new(),
        }
    }

    fn palette_path_label(path: &std::path::Path) -> String {
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| path.display().to_string())
    }

    fn palette_category(command: crate::command::Command) -> &'static str {
        use crate::command::Command;
        match command {
            Command::RequestCopy
            | Command::RequestMove
            | Command::MoveIntoCursorFolder
            | Command::CopyIntoCursorFolder
            | Command::CreateDir
            | Command::RequestDelete
            | Command::BeginRename
            | Command::BeginBatchRename
            | Command::FindDuplicates
            | Command::BeginFind
            | Command::OpenSavedSearch
            | Command::OpenProjectCollections => "File",
            Command::BeginGoToPath | Command::BeginRecent => "Navigation",
            Command::BeginSync | Command::EqualizePanels | Command::SwapPanels => "Panels",
            Command::SelectAll
            | Command::InvertSelection
            | Command::SelectSameNamed
            | Command::BeginSelectMask => "Selection",
            Command::ToggleHidden
            | Command::TogglePreview
            | Command::CycleDensity
            | Command::DiskTreemap
            | Command::DiffFiles
            | Command::ToggleInfo => "View",
            Command::CopyPath
            | Command::CopyName
            | Command::CopyParentPath
            | Command::CopyFileUrl
            | Command::CopyShellPath
            | Command::CopyRelativePath => "Clipboard",
            Command::ShelfAdd | Command::ShelfDrain => "Shelf",
            Command::Undo | Command::Redo | Command::OpenRecoveryCenter => "History",
            _ => "Command",
        }
    }

    /// Build a palette row as a [`LayoutJob`], tinting fuzzy-matched characters
    /// in the accent colour so the user sees why the row matched.
    fn palette_row_job(
        lead: &str,
        m: &crate::command::CommandMatch,
        t: ThemeColors,
    ) -> egui::text::LayoutJob {
        use egui::text::{LayoutJob, TextFormat};
        let font = egui::FontId::proportional(12.0);
        let mut job = LayoutJob::default();
        let fmt = |color| TextFormat {
            font_id: font.clone(),
            color,
            ..Default::default()
        };
        job.append(lead, 0.0, fmt(t.text_muted));
        let mut buf = [0u8; 4];
        for (idx, ch) in m.label.chars().enumerate() {
            let hit = m.matched.iter().any(|&(s, e)| idx >= s && idx < e);
            let color = if hit { t.accent } else { t.text_primary };
            job.append(ch.encode_utf8(&mut buf), 0.0, fmt(color));
        }
        job
    }
}
