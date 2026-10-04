//! Omnibar (Cmd+K / Cmd+L / Cmd+P): one input for jumping to folders, typing
//! a path, running a command, starting a search, or a shell line. The input
//! grammar lives in `crate::omnibar`; command ranking in `crate::command`.

use super::*;
use crate::omnibar::OmnibarMode;

/// Open omnibar state. The path probe exists only while the input is a path,
/// so folder validation stays off the UI thread exactly as before.
pub(crate) struct OmnibarState {
    pub(crate) input: String,
    pub(crate) opening_panel: ActivePanel,
    pub(crate) selected: usize,
    pub(crate) probe: Option<crate::path_probe::PathProbeController>,
}

#[derive(Clone)]
enum OmniRow {
    Folder {
        path: std::path::PathBuf,
        pinned: bool,
    },
    Command(usize),
}

enum OmniAction {
    Go(std::path::PathBuf),
    Run(&'static str, crate::command::Command),
    Search(String),
    Shell(String),
}

impl App {
    pub(crate) fn open_palette(&mut self, ctx: &egui::Context) {
        self.open_omnibar(ctx, String::new());
    }

    pub(crate) fn open_omnibar(&mut self, ctx: &egui::Context, input: String) {
        self.ui.modals.palette_input = Some(OmnibarState {
            input,
            opening_panel: self.ws.active,
            selected: 0,
            probe: None,
        });
        Self::mark_modal_opened(ctx, UiModal::Palette);
    }

    /// Cmd+L: the omnibar pre-filled with the active folder as a path.
    pub(crate) fn open_path(&mut self, ctx: &egui::Context) {
        let mut current = self
            .ws
            .active_panel_ref()
            .current_path
            .display()
            .to_string();
        if let Some(home) = dirs::home_dir().map(|h| h.display().to_string())
            && let Some(rest) = current.strip_prefix(&home)
        {
            current = format!("~{rest}");
        }
        if !current.ends_with('/') {
            current.push('/');
        }
        self.open_omnibar(ctx, current);
    }

    /// Cmd+P: the omnibar empty, which lists recent folders first.
    pub(crate) fn open_recent(&mut self, ctx: &egui::Context) {
        self.open_omnibar(ctx, String::new());
    }

    pub(crate) fn show_palette_dialog(&mut self, ctx: &egui::Context) {
        let just_opened = Self::take_modal_opened(ctx, UiModal::Palette);
        let escape_requested = self.take_modal_escape(crate::accessibility::ModalSurface::Palette);
        let Some(state) = self.ui.modals.palette_input.as_ref() else {
            return;
        };
        let t = self.colors;
        let now = ctx.input(|input| input.time);
        let input = state.input.clone();
        let mode = OmnibarMode::parse(&input);

        // Keep a path probe alive only in path mode, fed from the shared input.
        let state = self.ui.modals.palette_input.as_mut().unwrap();
        match mode {
            OmnibarMode::Path(path) => {
                if state.probe.is_none() {
                    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
                    let id = {
                        self.ui.transient_nonce = self.ui.transient_nonce.wrapping_add(1);
                        self.ui.transient_nonce
                    };
                    let state = self.ui.modals.palette_input.as_mut().unwrap();
                    state.probe = Some(crate::path_probe::PathProbeController::new(
                        id,
                        path.to_string(),
                        home,
                        now,
                    ));
                }
                let state = self.ui.modals.palette_input.as_mut().unwrap();
                let probe = state.probe.as_mut().unwrap();
                if probe.input() != path {
                    probe.replace_input(path, now);
                }
                let repaint = ctx.clone();
                let notify: std::sync::Arc<dyn Fn() + Send + Sync> =
                    std::sync::Arc::new(move || repaint.request_repaint());
                probe.drive(
                    now,
                    &self.workload,
                    std::sync::Arc::clone(&self.directory_probe),
                    notify,
                );
                if let Some(delay) = probe.repaint_after(now) {
                    ctx.request_repaint_after(delay);
                }
            }
            _ => state.probe = None,
        }

        // Rows for the current mode.
        let command_query = match mode {
            OmnibarMode::Jump(q) | OmnibarMode::Command(q) => Some(q),
            _ => None,
        };
        let matches = command_query
            .map(|q| crate::command::rank(q, &self.palette_usage, self.palette_tick))
            .unwrap_or_default();
        let command_context = self.ws.command_context();
        let availabilities: Vec<crate::command::CommandAvailability> = matches
            .iter()
            .map(|m| crate::command::availability(m.command, &command_context))
            .collect();
        let previews: Vec<String> = matches
            .iter()
            .map(|m| self.palette_command_preview(m.command))
            .collect();

        let mut rows: Vec<OmniRow> = Vec::new();
        match mode {
            OmnibarMode::Jump(q) => {
                for (_, path) in self.palette_bookmark_matches(q).into_iter().take(5) {
                    rows.push(OmniRow::Folder { path, pinned: true });
                }
                let (visited, stats) = crate::panel::visit_snapshot();
                for item in crate::panel::rank_visited(&visited, q, self.recent_order, &stats)
                    .into_iter()
                    .take(if q.is_empty() { 8 } else { 6 })
                {
                    if !rows.iter().any(
                        |row| matches!(row, OmniRow::Folder { path, .. } if *path == item.path),
                    ) {
                        rows.push(OmniRow::Folder {
                            path: item.path,
                            pinned: false,
                        });
                    }
                }
                if !q.is_empty() {
                    rows.extend((0..matches.len().min(8)).map(OmniRow::Command));
                }
            }
            OmnibarMode::Command(_) => rows.extend((0..matches.len()).map(OmniRow::Command)),
            OmnibarMode::Path(_) | OmnibarMode::Search(_) | OmnibarMode::Shell(_) => {}
        }
        let row_enabled = |row: &OmniRow| match row {
            OmniRow::Folder { .. } => true,
            OmniRow::Command(i) => availabilities[*i].enabled,
        };

        let state = self.ui.modals.palette_input.as_mut().unwrap();
        let (up, down, enter) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Enter),
            )
        });
        if rows.is_empty() {
            state.selected = 0;
        } else {
            if down {
                state.selected = (state.selected + 1).min(rows.len() - 1);
            }
            if up {
                state.selected = state.selected.saturating_sub(1);
            }
            state.selected = state.selected.min(rows.len() - 1);
            // Never rest on a row that cannot run.
            if !row_enabled(&rows[state.selected])
                && let Some(next) = rows.iter().position(row_enabled)
            {
                state.selected = next;
            }
        }
        let selected = state.selected;
        let validated_path = state.probe.as_ref().and_then(|p| {
            let exact = p.input().to_string();
            p.validated_path(&exact).map(std::path::Path::to_path_buf)
        });
        let probe_status = state
            .probe
            .as_ref()
            .map(|p| (p.status().message(), p.status().clone()));
        let opening_panel = state.opening_panel;

        let mut action: Option<OmniAction> = None;
        let cancel = escape_requested;
        let width = (ctx.content_rect().width() - 32.0).clamp(280.0, 640.0);

        egui::Window::new("Omnibar")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 56.0])
            .frame(
                Frame::NONE
                    .fill(t.bg_panel)
                    .corner_radius(crate::theme::ROUNDING_MD)
                    .inner_margin(Margin::same(10))
                    .stroke(Stroke::new(1.0_f32, t.border))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_black_alpha(60),
                    }),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                let state = self.ui.modals.palette_input.as_mut().unwrap();
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(mode.label())
                            .size(11.0)
                            .strong()
                            .color(t.accent),
                    );
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut state.input)
                            .desired_width(f32::INFINITY)
                            .frame(egui::Frame::NONE)
                            .hint_text("Go to folder, run a command, search\u{2026}")
                            .font(egui::FontId::proportional(15.0))
                            .margin(egui::vec2(6.0, 6.0)),
                    );
                    ui.ctx().accesskit_node_builder(resp.id, |node| {
                        node.set_label("Omnibar");
                    });
                    if just_opened {
                        resp.request_focus();
                        if let Some(mut text_state) = egui::TextEdit::load_state(ui.ctx(), resp.id)
                        {
                            let end = egui::text::CCursor::new(state.input.chars().count());
                            text_state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::one(end)));
                            text_state.store(ui.ctx(), resp.id);
                        }
                    }
                    if resp.changed() {
                        state.selected = 0;
                    }
                });
                ui.add_space(4.0);
                ui.separator();

                match mode {
                    OmnibarMode::Path(_) => {
                        if let Some((message, status)) = &probe_status {
                            let color = match status {
                                crate::path_probe::ProbeStatus::Valid => t.accent,
                                crate::path_probe::ProbeStatus::Error(_)
                                | crate::path_probe::ProbeStatus::WorkerFailed(_) => t.accent_red,
                                _ => t.text_muted,
                            };
                            let r = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(message).size(11.0).color(color),
                                )
                                .truncate(),
                            );
                            ui.ctx().accesskit_node_builder(r.id, |node| {
                                node.set_role(egui::accesskit::Role::Status);
                                node.set_live(egui::accesskit::Live::Polite);
                            });
                        }
                        if enter && let Some(path) = &validated_path {
                            action = Some(OmniAction::Go(path.clone()));
                        }
                    }
                    OmnibarMode::Search(q) => {
                        Self::omnibar_hint(
                            ui,
                            t,
                            &format!(
                                "Enter searches {} recursively",
                                Self::palette_path_label(&self.ws.active_panel_ref().current_path)
                            ),
                        );
                        if enter && !q.is_empty() {
                            action = Some(OmniAction::Search(q.to_string()));
                        }
                    }
                    OmnibarMode::Shell(line) => {
                        Self::omnibar_hint(
                            ui,
                            t,
                            &format!(
                                "Enter runs in {}",
                                Self::palette_path_label(&self.ws.active_panel_ref().current_path)
                            ),
                        );
                        if enter && !line.is_empty() {
                            action = Some(OmniAction::Shell(line.to_string()));
                        }
                    }
                    OmnibarMode::Jump(_) | OmnibarMode::Command(_) => {
                        if rows.is_empty() {
                            Self::omnibar_hint(ui, t, "No matches");
                        }
                        egui::ScrollArea::vertical()
                            .max_height(360.0)
                            .show(ui, |ui| {
                                let mut last_folder = None;
                                for (index, row) in rows.iter().enumerate() {
                                    let is_folder = matches!(row, OmniRow::Folder { .. });
                                    if last_folder != Some(is_folder)
                                        && matches!(mode, OmnibarMode::Jump(_))
                                    {
                                        ui.add_space(4.0);
                                        ui.label(
                                            egui::RichText::new(if is_folder {
                                                "FOLDERS"
                                            } else {
                                                "COMMANDS"
                                            })
                                            .size(9.5)
                                            .color(t.text_muted),
                                        );
                                        last_folder = Some(is_folder);
                                    }
                                    let is_selected = index == selected;
                                    let enabled = row_enabled(row);
                                    let response = Frame::NONE
                                        .fill(if is_selected {
                                            t.accent.linear_multiply(0.14)
                                        } else {
                                            Color32::TRANSPARENT
                                        })
                                        .corner_radius(crate::theme::ROUNDING_SM)
                                        .inner_margin(Margin::symmetric(8, 5))
                                        .show(ui, |ui| {
                                            ui.set_width(ui.available_width());
                                            match row {
                                                OmniRow::Folder { path, pinned } => {
                                                    ui.horizontal(|ui| {
                                                        ui.label(
                                                            egui::RichText::new(
                                                                Self::palette_path_label(path),
                                                            )
                                                            .size(13.0)
                                                            .color(t.text_primary),
                                                        );
                                                        ui.add(
                                                            egui::Label::new(
                                                                egui::RichText::new(
                                                                    path.parent()
                                                                        .map(|p| {
                                                                            p.display().to_string()
                                                                        })
                                                                        .unwrap_or_default(),
                                                                )
                                                                .size(11.0)
                                                                .color(t.text_muted),
                                                            )
                                                            .truncate(),
                                                        );
                                                        if *pinned {
                                                            ui.with_layout(
                                                                Layout::right_to_left(
                                                                    Align::Center,
                                                                ),
                                                                |ui| {
                                                                    ui.label(
                                                                        egui::RichText::new(
                                                                            "\u{2605}",
                                                                        )
                                                                        .size(11.0)
                                                                        .color(t.text_muted),
                                                                    );
                                                                },
                                                            );
                                                        }
                                                    });
                                                }
                                                OmniRow::Command(i) => {
                                                    let m = &matches[*i];
                                                    let availability = availabilities[*i];
                                                    ui.horizontal(|ui| {
                                                        ui.add(egui::Label::new(
                                                            Self::palette_row_job(
                                                                "", m, enabled, t,
                                                            ),
                                                        ));
                                                        let detail = availability
                                                            .reason
                                                            .map(str::to_string)
                                                            .unwrap_or_else(|| {
                                                                previews[*i].clone()
                                                            });
                                                        if !detail.is_empty() {
                                                            ui.add(
                                                                egui::Label::new(
                                                                    egui::RichText::new(detail)
                                                                        .size(11.0)
                                                                        .color(t.text_muted),
                                                                )
                                                                .truncate(),
                                                            );
                                                        }
                                                        ui.with_layout(
                                                            Layout::right_to_left(Align::Center),
                                                            |ui| {
                                                                if !m.shortcut.is_empty() {
                                                                    ui.label(
                                                                        egui::RichText::new(
                                                                            m.shortcut,
                                                                        )
                                                                        .size(10.5)
                                                                        .color(t.text_muted),
                                                                    );
                                                                }
                                                            },
                                                        );
                                                    });
                                                }
                                            }
                                        })
                                        .response;
                                    if is_selected && (up || down) {
                                        response.scroll_to_me(None);
                                    }
                                    let response = response.interact(if enabled {
                                        Sense::click()
                                    } else {
                                        Sense::hover()
                                    });
                                    let activate = (enabled && response.clicked())
                                        || (is_selected && enter && enabled);
                                    if activate {
                                        action = Some(match row {
                                            OmniRow::Folder { path, .. } => {
                                                OmniAction::Go(path.clone())
                                            }
                                            OmniRow::Command(i) => OmniAction::Run(
                                                matches[*i].label,
                                                matches[*i].command,
                                            ),
                                        });
                                    }
                                }
                            });
                        if input.trim().is_empty() {
                            ui.add_space(4.0);
                            Self::omnibar_hint(ui, t, crate::omnibar::GRAMMAR_HINT);
                        }
                    }
                }
            });

        if cancel || action.is_some() {
            self.ui.modals.palette_input = None;
        }
        match action {
            None => {}
            Some(OmniAction::Go(path)) => match opening_panel {
                ActivePanel::Left => self.ws.left.navigate_to(path),
                ActivePanel::Right => self.ws.right.navigate_to(path),
            },
            Some(OmniAction::Run(label, cmd)) => {
                self.palette_tick += 1;
                self.palette_usage.record(label, self.palette_tick);
                self.ws.execute(cmd);
            }
            Some(OmniAction::Search(query)) => {
                self.open_find();
                if let Some(find) = self.ui.modals.find.as_mut() {
                    find.expression = query;
                    find.pending_rerun = true;
                    find.last_edit_at = 0.0;
                }
            }
            Some(OmniAction::Shell(line)) => {
                self.open_run_command(ctx);
                if let Some(run) = self.ui.modals.run_command.as_mut() {
                    run.line = line;
                }
            }
        }
    }

    fn omnibar_hint(ui: &mut egui::Ui, t: ThemeColors, text: &str) {
        ui.add(egui::Label::new(egui::RichText::new(text).size(11.0).color(t.text_muted)).wrap());
    }

    fn palette_bookmark_matches(&self, query: &str) -> Vec<(String, std::path::PathBuf)> {
        let needle = query.trim().to_lowercase();
        self.ws
            .bookmarks
            .items
            .iter()
            .filter(|bookmark| {
                needle.is_empty()
                    || bookmark.name.to_lowercase().contains(&needle)
                    || bookmark
                        .path
                        .to_string_lossy()
                        .to_lowercase()
                        .contains(&needle)
            })
            .map(|bookmark| (bookmark.name.clone(), bookmark.path.clone()))
            .take(12)
            .collect()
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
                .cursor_entry()
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
                .cursor_entry()
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
                if active.show_hidden() {
                    "currently on".to_string()
                } else {
                    "currently off".to_string()
                }
            }
            Command::CycleDensity => {
                let next = crate::density::cycle(active.density());
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

    /// Build a palette row as a [`LayoutJob`], tinting fuzzy-matched characters
    /// in the accent colour so the user sees why the row matched.
    fn palette_row_job(
        lead: &str,
        m: &crate::command::CommandMatch,
        enabled: bool,
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
            let color = if !enabled {
                t.text_muted
            } else if hit {
                t.accent
            } else {
                t.text_primary
            };
            job.append(ch.encode_utf8(&mut buf), 0.0, fmt(color));
        }
        job
    }
}
