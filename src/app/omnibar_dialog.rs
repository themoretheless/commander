//! Omnibar (Cmd+K / Cmd+L / Cmd+P): one input for jumping to folders, typing
//! a path, running a command, starting a search, or a shell line. The input
//! grammar lives in `crate::omnibar`; command ranking in `crate::command`.

use super::*;
use crate::omnibar::OmnibarMode;

/// Open omnibar state. The path probe exists only while the input is a path,
/// so folder validation stays off the UI thread exactly as before.
pub struct OmnibarState {
    pub(crate) input: String,
    pub(crate) opening_panel: ActivePanel,
    pub(crate) selected: usize,
    pub(crate) probe: Option<crate::path_probe::PathProbeController>,
    /// Path input submitted with Enter before the probe finished; it goes
    /// once the probe validates that same input.
    pub(crate) pending_go: Option<String>,
    pub(crate) assigning_slot: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum OmniRow {
    Folder {
        path: std::path::PathBuf,
        pinned: bool,
    },
    Command(usize),
    QuickSlot {
        slot: u8,
        name: String,
        path: std::path::PathBuf,
    },
    AssignSlotPrompt,
    AssignSlot(u8),
}

enum OmniAction {
    Go(std::path::PathBuf),
    Run(String, crate::command::Command),
    ChooseSlot,
    Search(String),
    Shell(String),
}

fn row_action(row: &OmniRow, matches: &[crate::command::CommandMatch]) -> OmniAction {
    match row {
        OmniRow::Folder { path, .. } => OmniAction::Go(path.clone()),
        OmniRow::Command(index) => {
            OmniAction::Run(matches[*index].label.to_string(), matches[*index].command)
        }
        OmniRow::QuickSlot { slot, .. } => OmniAction::Run(
            format!("Jump to slot {slot}"),
            crate::command::Command::JumpSlot(*slot),
        ),
        OmniRow::AssignSlotPrompt => OmniAction::ChooseSlot,
        OmniRow::AssignSlot(slot) => OmniAction::Run(
            format!("Assign quick-jump slot {slot}"),
            crate::command::Command::AssignSlot(*slot),
        ),
    }
}

fn enter_target(selected: usize, focused: Option<usize>, enabled: &[bool]) -> Option<usize> {
    let index = focused.unwrap_or(selected);
    enabled
        .get(index)
        .copied()
        .unwrap_or(false)
        .then_some(index)
}

/// Show `path` with the home folder abbreviated to `~`. Component-aware, so
/// a sibling such as `/home/user2` is never turned into `~2`.
fn home_relative_display(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => path.display().to_string(),
    }
}

/// Jump-mode rows: pinned folders, then visited folders not already listed,
/// then `commands` ranked command rows.
fn jump_rows(
    pinned: impl IntoIterator<Item = std::path::PathBuf>,
    visited: impl IntoIterator<Item = std::path::PathBuf>,
    commands: usize,
) -> Vec<OmniRow> {
    let mut rows: Vec<OmniRow> = pinned
        .into_iter()
        .map(|path| OmniRow::Folder { path, pinned: true })
        .collect();
    for path in visited {
        if !rows
            .iter()
            .any(|row| matches!(row, OmniRow::Folder { path: listed, .. } if *listed == path))
        {
            rows.push(OmniRow::Folder {
                path,
                pinned: false,
            });
        }
    }
    rows.extend((0..commands).map(OmniRow::Command));
    rows
}

/// Move the selection one step, skipping rows that cannot run, and never
/// rest on a disabled row. `enabled` has one flag per row.
fn step_selection(enabled: &[bool], selected: usize, up: bool, down: bool) -> usize {
    let Some(last) = enabled.len().checked_sub(1) else {
        return 0;
    };
    let mut selected = selected.min(last);
    if down && let Some(next) = (selected + 1..=last).find(|&row| enabled[row]) {
        selected = next;
    }
    if up && let Some(previous) = (0..selected).rev().find(|&row| enabled[row]) {
        selected = previous;
    }
    if !enabled[selected]
        && let Some(first) = enabled.iter().position(|&row| row)
    {
        selected = first;
    }
    selected
}

/// Enter in path mode. Goes when the probe has validated this exact input;
/// while the probe is still waiting or checking, the submit is remembered
/// and fires once it turns valid. Editing the input or a probe error drops
/// it.
fn path_submit(
    enter: bool,
    pending: &mut Option<String>,
    input: &str,
    status: Option<&crate::path_probe::ProbeStatus>,
    validated: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    use crate::path_probe::ProbeStatus;
    if pending.as_deref().is_some_and(|pending| pending != input) {
        *pending = None;
    }
    if !enter && pending.is_none() {
        return None;
    }
    if let Some(path) = validated {
        *pending = None;
        return Some(path.to_path_buf());
    }
    *pending = matches!(
        status,
        Some(ProbeStatus::Waiting | ProbeStatus::Busy | ProbeStatus::Checking)
    )
    .then(|| input.to_string());
    None
}

/// "1 saved search", "N saved searches".
fn saved_searches_label(count: usize) -> String {
    if count == 1 {
        "1 saved search".to_string()
    } else {
        format!("{count} saved searches")
    }
}

/// The clipboard style each copy-path command uses.
const fn copy_style(command: crate::command::Command) -> Option<crate::clipboard::PathStyle> {
    use crate::clipboard::PathStyle;
    use crate::command::Command;
    match command {
        Command::CopyPath => Some(PathStyle::FullPath),
        Command::CopyName => Some(PathStyle::NameOnly),
        Command::CopyParentPath => Some(PathStyle::ParentPath),
        Command::CopyFileUrl => Some(PathStyle::FileUrl),
        Command::CopyShellPath => Some(PathStyle::ShellEscaped),
        Command::CopyRelativePath => Some(PathStyle::RelativeToOther),
        _ => None,
    }
}

/// Preview for a copy-path command: the exact text it would copy when one
/// item is picked (the six rows differ only there), else the item count.
fn copy_preview(
    style: crate::clipboard::PathStyle,
    paths: &[std::path::PathBuf],
    other_root: &std::path::Path,
    picked: usize,
) -> String {
    if picked == 1 && paths.len() == 1 {
        crate::clipboard::format(paths, style, Some(other_root))
    } else {
        format!("{picked} item(s)")
    }
}

/// Accessible name of the omnibar input: includes the mode, which is shown
/// only as a separate badge and changes as a prefix is typed.
fn input_accessible_label(mode: &OmnibarMode<'_>) -> String {
    format!("Omnibar, {} mode", mode.label())
}

fn dynamic_row_matches(label: &str, query: &str) -> bool {
    let label = label.to_lowercase();
    query
        .split_whitespace()
        .all(|word| label.contains(&word.to_lowercase()))
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
            pending_go: None,
            assigning_slot: false,
        });
        Self::mark_modal_opened(ctx, UiModal::Palette);
    }

    /// Cmd+L: the omnibar pre-filled with the active folder as a path.
    pub(crate) fn open_path(&mut self, ctx: &egui::Context) {
        let mut current = home_relative_display(
            &self.ws.active_panel_ref().current_path,
            dirs::home_dir().as_deref(),
        );
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
        let assigning_slot = state.assigning_slot;
        let mode = OmnibarMode::parse(&input);

        // Keep a path probe alive only in path mode, fed from the shared input.
        let state = self.ui.modals.palette_input.as_mut().unwrap();
        match mode {
            OmnibarMode::Path(typed) => {
                // `./` and `../` input resolves from the active folder before
                // probing, so the probe and Go see an absolute path.
                let relative = crate::omnibar::resolve_relative(
                    typed,
                    &self.ws.active_panel_ref().current_path,
                );
                let path = relative.as_deref().unwrap_or(typed);
                let state = self.ui.modals.palette_input.as_mut().unwrap();
                if state.probe.is_none() {
                    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
                    let id = self.issue_transient_nonce();
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
            _ => {
                // Leaving Path mode drops a deferred Enter, so retyping the
                // same path later needs a fresh Enter to navigate.
                state.probe = None;
                state.pending_go = None;
            }
        }

        // Rows for the current mode.
        let command_query = match mode {
            OmnibarMode::Jump(q) | OmnibarMode::Command(q) => Some(q),
            _ => None,
        };
        // Only commands that get a row are ranked, checked and previewed.
        let shown_commands = command_rows_shown(&mode);
        let mut matches = command_query
            .filter(|_| shown_commands > 0)
            .map(|q| crate::command::rank(q, &self.palette_usage, self.palette_tick))
            .unwrap_or_default();
        matches.truncate(shown_commands);
        let command_context = self.ws.command_context();
        let availabilities: Vec<crate::command::CommandAvailability> = matches
            .iter()
            .map(|m| crate::command::availability(m.command, &command_context))
            .collect();
        let picked = picked_count(self.ws.active_panel_ref());
        let previews: Vec<String> = matches
            .iter()
            .map(|m| self.palette_command_preview(m.command, picked))
            .collect();

        let mut rows: Vec<OmniRow> = Vec::new();
        match mode {
            OmnibarMode::Jump(q) => {
                let (visited, stats) = crate::panel::visit_snapshot();
                rows = jump_rows(
                    self.palette_bookmark_matches(q)
                        .into_iter()
                        .take(5)
                        .map(|(_, path)| path),
                    crate::panel::rank_visited(&visited, q, self.recent_order, &stats)
                        .into_iter()
                        .take(if q.is_empty() { 8 } else { 6 })
                        .map(|item| item.path),
                    matches.len(),
                );
            }
            OmnibarMode::Command(_) => rows.extend((0..matches.len()).map(OmniRow::Command)),
            OmnibarMode::Path(_) | OmnibarMode::Search(_) | OmnibarMode::Shell(_) => {}
        }
        if assigning_slot {
            rows = (1..=crate::bookmarks::MAX_SLOT)
                .map(OmniRow::AssignSlot)
                .collect();
        } else if let Some(query) = command_query {
            for bookmark in self
                .ws
                .bookmarks
                .items
                .iter()
                .filter(|bookmark| bookmark.slot.is_some())
            {
                let slot = bookmark.slot.unwrap_or_default();
                let label = format!("Jump to slot {slot}: {}", bookmark.name);
                if dynamic_row_matches(&label, query) {
                    rows.push(OmniRow::QuickSlot {
                        slot,
                        name: bookmark.name.clone(),
                        path: bookmark.path.clone(),
                    });
                }
            }
            let label = "Assign quick-jump slot";
            if dynamic_row_matches(label, query) {
                rows.push(OmniRow::AssignSlotPrompt);
            }
        }
        let row_enabled = |row: &OmniRow| match row {
            OmniRow::Folder { .. } => true,
            OmniRow::Command(i) => availabilities[*i].enabled,
            OmniRow::QuickSlot { .. } | OmniRow::AssignSlotPrompt | OmniRow::AssignSlot(_) => true,
        };

        let state = self.ui.modals.palette_input.as_mut().unwrap();
        let (up, down, enter) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Enter),
            )
        });
        let enabled: Vec<bool> = rows.iter().map(row_enabled).collect();
        state.selected = step_selection(&enabled, state.selected, up, down);
        let selected = state.selected;
        let validated_path = state.probe.as_ref().and_then(|p| {
            let exact = p.input().to_string();
            p.validated_path(&exact).map(std::path::Path::to_path_buf)
        });
        let probe_status = state
            .probe
            .as_ref()
            .map(|p| (p.status().message(), p.status().clone()));
        let probe_input = state.probe.as_ref().map(|p| p.input().to_string());
        let opening_panel = state.opening_panel;

        let mut action: Option<OmniAction> = None;
        let cancel = escape_requested;
        let width = (ctx.content_rect().width() - 32.0).clamp(280.0, 640.0);
        let mut input_id = None;
        let mut selected_row_id = None;
        let mut focused_row = None;

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
                    let label = input_accessible_label(&mode);
                    ui.ctx().accesskit_node_builder(resp.id, |node| {
                        node.set_label(label);
                    });
                    input_id = Some(resp.id);
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
                        if let Some(path) = path_submit(
                            enter,
                            &mut state.pending_go,
                            probe_input.as_deref().unwrap_or_default(),
                            probe_status.as_ref().map(|(_, status)| status),
                            validated_path.as_deref(),
                        ) {
                            action = Some(OmniAction::Go(path));
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
                                                OmniRow::QuickSlot { slot, name, path } => {
                                                    ui.horizontal(|ui| {
                                                        ui.label(format!(
                                                            "Jump to slot {slot}: {name}"
                                                        ));
                                                        ui.add(
                                                            egui::Label::new(
                                                                egui::RichText::new(
                                                                    Self::palette_path_label(path),
                                                                )
                                                                .size(11.0)
                                                                .color(t.text_muted),
                                                            )
                                                            .truncate(),
                                                        );
                                                        ui.with_layout(
                                                            Layout::right_to_left(Align::Center),
                                                            |ui| {
                                                                ui.label(
                                                                    egui::RichText::new(format!(
                                                                        "Cmd+{slot}"
                                                                    ))
                                                                    .size(10.5)
                                                                    .color(t.text_muted),
                                                                );
                                                            },
                                                        );
                                                    });
                                                }
                                                OmniRow::AssignSlotPrompt => {
                                                    ui.label("Assign quick-jump slot…");
                                                }
                                                OmniRow::AssignSlot(slot) => {
                                                    let held = self
                                                        .ws
                                                        .bookmarks
                                                        .by_slot(*slot)
                                                        .map(|bookmark| bookmark.name.as_str())
                                                        .unwrap_or("empty");
                                                    ui.horizontal(|ui| {
                                                        ui.label(format!(
                                                            "Assign current folder to slot {slot}"
                                                        ));
                                                        ui.label(
                                                            egui::RichText::new(held)
                                                                .size(11.0)
                                                                .color(t.text_muted),
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
                                    // The arrow-selected row gets the file
                                    // list's cursor outline, a Tab-focused
                                    // row its double focus ring.
                                    if is_selected {
                                        ui.painter().rect_stroke(
                                            response.rect.shrink(1.0),
                                            crate::theme::ROUNDING_SM,
                                            Stroke::new(1.0, t.accent),
                                            egui::StrokeKind::Inside,
                                        );
                                        selected_row_id = Some(response.id);
                                    }
                                    if response.has_focus() {
                                        focused_row = Some(index);
                                        ui.painter().rect_stroke(
                                            response.rect.shrink(2.0),
                                            crate::theme::ROUNDING_SM,
                                            Stroke::new(2.0, t.text_primary),
                                            egui::StrokeKind::Inside,
                                        );
                                        ui.painter().rect_stroke(
                                            response.rect.shrink(4.0),
                                            crate::theme::ROUNDING_SM,
                                            Stroke::new(1.0, t.bg_panel),
                                            egui::StrokeKind::Inside,
                                        );
                                    }
                                    let row_label = match row {
                                        OmniRow::Folder { path, .. } => {
                                            Self::palette_path_label(path)
                                        }
                                        OmniRow::Command(i) => matches[*i].label.to_string(),
                                        OmniRow::QuickSlot { slot, name, .. } => {
                                            format!("Jump to slot {slot}: {name}")
                                        }
                                        OmniRow::AssignSlotPrompt => {
                                            "Assign quick-jump slot".to_string()
                                        }
                                        OmniRow::AssignSlot(slot) => {
                                            format!("Assign current folder to slot {slot}")
                                        }
                                    };
                                    response.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::WidgetType::SelectableLabel,
                                            enabled,
                                            is_selected,
                                            &row_label,
                                        )
                                    });
                                    if enabled && response.clicked() {
                                        action = Some(row_action(row, &matches));
                                    }
                                }
                            });
                        if action.is_none()
                            && enter
                            && let Some(index) = enter_target(selected, focused_row, &enabled)
                            && let Some(row) = rows.get(index)
                        {
                            action = Some(row_action(row, &matches));
                        }
                        if input.trim().is_empty() {
                            ui.add_space(4.0);
                            Self::omnibar_hint(ui, t, crate::omnibar::GRAMMAR_HINT);
                        }
                    }
                }
            });

        if let Some(input_id) = input_id {
            // Expose the arrow-selected row as the input's active
            // descendant, so AT announces what Enter would run.
            ctx.accesskit_node_builder(input_id, |node| match selected_row_id {
                Some(row) => node.set_active_descendant(row.accesskit_id()),
                None => node.clear_active_descendant(),
            });
            // A singleline TextEdit gives up focus on Enter; when Enter ran
            // nothing (no match, empty query, path still being checked),
            // keep typing in the input.
            if enter && action.is_none() && !cancel {
                ctx.memory_mut(|memory| memory.request_focus(input_id));
            }
        }
        if cancel
            || action
                .as_ref()
                .is_some_and(|action| !matches!(action, OmniAction::ChooseSlot))
        {
            self.ui.modals.palette_input = None;
        }
        match action {
            None => {}
            Some(OmniAction::ChooseSlot) => {
                if let Some(state) = self.ui.modals.palette_input.as_mut() {
                    state.assigning_slot = true;
                    state.input.clear();
                    state.selected = 0;
                }
                if let Some(input_id) = input_id {
                    ctx.memory_mut(|memory| memory.request_focus(input_id));
                }
            }
            Some(OmniAction::Go(path)) if !path.is_dir() => {
                // A recent/pinned folder may have been deleted since it was
                // recorded; navigating would land in a Gone state and
                // re-record the dead path as a fresh visit.
                let (mut recent, stats) = crate::panel::visit_snapshot();
                recent.retain(|visited| *visited != path);
                crate::panel::restore_visit_snapshot(&recent, &stats);
                self.toast(
                    ctx,
                    crate::toasts::ToastKind::Error,
                    format!("Folder no longer exists: {}", path.display()),
                    false,
                );
            }
            Some(OmniAction::Go(path)) => match opening_panel {
                ActivePanel::Left => self.ws.left.navigate_to(path),
                ActivePanel::Right => self.ws.right.navigate_to(path),
            },
            Some(OmniAction::Run(label, cmd)) => {
                self.palette_tick += 1;
                self.palette_usage.record(&label, self.palette_tick);
                if super::update::switch_into_covered_pane(&self.ui.tools, self.ws.active, cmd) {
                    self.toast(
                        ctx,
                        crate::toasts::ToastKind::Info,
                        super::update::SWITCH_INTO_COVERED_PANE,
                        false,
                    );
                } else {
                    self.ws.execute(cmd);
                }
            }
            Some(OmniAction::Search(query)) => {
                self.open_find(ctx);
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

    /// Preview text for a palette command; `picked` is the active pane's
    /// selection-or-cursor count, computed once per frame by the caller.
    fn palette_command_preview(&self, command: crate::command::Command, picked: usize) -> String {
        use crate::command::Command;
        let active = self.ws.active_panel_ref();
        let inactive = self.ws.inactive_panel();
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
            Command::OpenSavedSearch => match self.smart_folders.as_ref() {
                Some(store) => saved_searches_label(store.items.len()),
                None => "saved searches".to_string(),
            },
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
            | Command::CopyRelativePath => {
                let Some(style) = copy_style(command) else {
                    return format!("{picked} item(s)");
                };
                // Clone the one picked entry only; larger selections show
                // just their count.
                let paths: Vec<std::path::PathBuf> = if picked == 1 {
                    active
                        .selected_or_cursor()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|entry| entry.path)
                        .collect()
                } else {
                    Vec::new()
                };
                copy_preview(style, &paths, &inactive.current_path, picked)
            }
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
        let (plain, hit) = if enabled {
            (t.text_primary, t.accent)
        } else {
            (t.text_muted, t.text_muted)
        };
        super::file_list::append_highlighted(&mut job, m.label, &m.matched, &fmt(plain), &fmt(hit));
        job
    }
}

/// Command rows the omnibar shows: all of them after `>`, at most eight
/// under a typed Jump query, and none for an empty Jump query or the
/// path/search/shell modes.
fn command_rows_shown(mode: &OmnibarMode<'_>) -> usize {
    match mode {
        OmnibarMode::Jump("") => 0,
        OmnibarMode::Jump(_) => 8,
        OmnibarMode::Command(_) => usize::MAX,
        OmnibarMode::Path(_) | OmnibarMode::Search(_) | OmnibarMode::Shell(_) => 0,
    }
}

/// How many entries a selection command would act on: the selection, or the
/// cursor entry when nothing is selected. Equals
/// `selected_or_cursor().map_or(0, len)` without cloning any entry.
fn picked_count(panel: &PanelState) -> usize {
    if panel.selected_count() == 0 {
        usize::from(panel.cursor_entry().is_some())
    } else {
        panel.visible_selected_count()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OmniRow, OmnibarMode, command_rows_shown, copy_preview, copy_style, dynamic_row_matches,
        enter_target, home_relative_display, input_accessible_label, jump_rows, path_submit,
        picked_count, saved_searches_label, step_selection,
    };

    #[test]
    fn saved_search_count_agrees_with_its_noun() {
        assert_eq!(saved_searches_label(0), "0 saved searches");
        assert_eq!(saved_searches_label(1), "1 saved search");
        assert_eq!(saved_searches_label(2), "2 saved searches");
    }

    #[test]
    fn quick_slot_rows_match_name_slot_and_assignment_terms() {
        assert!(dynamic_row_matches("Jump to slot 3: Projects", "slot 3"));
        assert!(dynamic_row_matches("Jump to slot 3: Projects", "projects"));
        assert!(dynamic_row_matches("Assign quick-jump slot", "assign slot"));
        assert!(!dynamic_row_matches("Jump to slot 3: Projects", "photos"));
    }

    #[test]
    fn only_commands_that_get_rows_are_ranked() {
        assert_eq!(command_rows_shown(&OmnibarMode::Jump("")), 0);
        assert_eq!(command_rows_shown(&OmnibarMode::Jump("co")), 8);
        assert_eq!(command_rows_shown(&OmnibarMode::Command("")), usize::MAX);
        assert_eq!(command_rows_shown(&OmnibarMode::Path("/tmp")), 0);
        assert_eq!(command_rows_shown(&OmnibarMode::Search("x")), 0);
        assert_eq!(command_rows_shown(&OmnibarMode::Shell("ls")), 0);
    }

    #[test]
    fn enter_prefers_the_focused_row_over_the_arrow_selection() {
        let enabled = [true, true, false];
        assert_eq!(enter_target(0, Some(1), &enabled), Some(1));
        assert_eq!(enter_target(0, None, &enabled), Some(0));
        assert_eq!(enter_target(0, Some(2), &enabled), None);
    }

    #[test]
    fn picked_count_matches_selected_or_cursor() {
        let temp = crate::testutil::TempDir::new();
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(temp.path().join(name), "x").expect("write");
        }
        let mut panel = crate::panel::PanelState::new(temp.path().to_path_buf());
        panel.refresh();
        let expected = |panel: &crate::panel::PanelState| {
            panel
                .selected_or_cursor()
                .map_or(0, |entries| entries.len())
        };
        // Cursor on "..": nothing picked.
        panel.set_cursor(0);
        assert_eq!(picked_count(&panel), expected(&panel));
        panel.set_cursor(1);
        assert_eq!(picked_count(&panel), 1);
        assert_eq!(picked_count(&panel), expected(&panel));
        panel.toggle_select(temp.path().join("a.txt"));
        panel.toggle_select(temp.path().join("c.txt"));
        assert_eq!(picked_count(&panel), 2);
        assert_eq!(picked_count(&panel), expected(&panel));
    }
    use std::path::{MAIN_SEPARATOR, Path};

    #[test]
    fn home_abbreviation_is_component_aware() {
        let home = Path::new("/home/user");
        assert_eq!(
            home_relative_display(Path::new("/home/user2"), Some(home)),
            "/home/user2"
        );
        assert_eq!(home_relative_display(home, Some(home)), "~");
        assert_eq!(
            home_relative_display(&home.join("docs"), Some(home)),
            format!("~{MAIN_SEPARATOR}docs")
        );
        assert_eq!(
            home_relative_display(Path::new("/srv"), None),
            Path::new("/srv").display().to_string()
        );
    }

    #[test]
    fn jump_rows_list_pinned_then_unpinned_visits_then_commands() {
        use std::path::PathBuf;
        let folder = |path: &str, pinned| OmniRow::Folder {
            path: PathBuf::from(path),
            pinned,
        };
        let rows = jump_rows(
            [PathBuf::from("/work")],
            ["/work", "/music", "/music"].map(PathBuf::from),
            2,
        );
        assert_eq!(
            rows,
            vec![
                folder("/work", true),
                folder("/music", false),
                OmniRow::Command(0),
                OmniRow::Command(1),
            ]
        );
        assert_eq!(jump_rows([], [], 0), Vec::new());
    }

    #[test]
    fn arrows_skip_disabled_rows_in_the_direction_of_travel() {
        let enabled = [true, false, true, false];
        assert_eq!(step_selection(&enabled, 0, false, true), 2);
        assert_eq!(step_selection(&enabled, 2, true, false), 0);
        // Nothing enabled below: stay put.
        assert_eq!(step_selection(&enabled, 2, false, true), 2);
        // Resting on a disabled row falls back to the first enabled one.
        assert_eq!(step_selection(&enabled, 3, false, false), 0);
        assert_eq!(step_selection(&[], 3, true, true), 0);
    }

    #[test]
    fn enter_before_the_probe_finishes_goes_once_it_validates() {
        use crate::path_probe::ProbeStatus;
        let mut pending = None;
        let checking = Some(&ProbeStatus::Checking);
        let valid = Some(Path::new("/tmp"));
        assert_eq!(
            path_submit(true, &mut pending, "/tmp", checking, None),
            None
        );
        assert_eq!(pending.as_deref(), Some("/tmp"));
        // Next frames: no Enter, still checking, then valid.
        assert_eq!(
            path_submit(false, &mut pending, "/tmp", checking, None),
            None
        );
        assert_eq!(
            path_submit(
                false,
                &mut pending,
                "/tmp",
                Some(&ProbeStatus::Valid),
                valid
            ),
            Some(Path::new("/tmp").to_path_buf())
        );
        assert_eq!(pending, None);
        // Without a pending submit, a valid probe alone does nothing.
        assert_eq!(
            path_submit(
                false,
                &mut pending,
                "/tmp",
                Some(&ProbeStatus::Valid),
                valid
            ),
            None
        );
    }

    #[test]
    fn pending_path_submit_is_dropped_by_edits_and_errors() {
        use crate::path_probe::ProbeStatus;
        let mut pending = Some("/tm".to_string());
        assert_eq!(
            path_submit(
                false,
                &mut pending,
                "/tmp",
                Some(&ProbeStatus::Valid),
                Some(Path::new("/tmp"))
            ),
            None,
            "an edited input never goes on the old Enter"
        );
        assert_eq!(pending, None);
        let mut pending = Some("/nope".to_string());
        let error = ProbeStatus::WorkerFailed("gone".to_string());
        assert_eq!(
            path_submit(false, &mut pending, "/nope", Some(&error), None),
            None
        );
        assert_eq!(pending, None);
    }

    #[test]
    fn copy_commands_preview_the_text_they_would_copy() {
        use crate::clipboard::PathStyle;
        use crate::command::Command;
        assert_eq!(copy_style(Command::CopyName), Some(PathStyle::NameOnly));
        assert_eq!(copy_style(Command::SwapPanels), None);
        let path = Path::new("/home/user/docs/a.txt").to_path_buf();
        let other = Path::new("/home/user");
        assert_eq!(
            copy_preview(PathStyle::NameOnly, std::slice::from_ref(&path), other, 1),
            "a.txt"
        );
        assert_eq!(
            copy_preview(PathStyle::FullPath, std::slice::from_ref(&path), other, 1),
            crate::clipboard::format(
                std::slice::from_ref(&path),
                PathStyle::FullPath,
                Some(other)
            )
        );
        assert_eq!(
            copy_preview(PathStyle::NameOnly, &[], other, 3),
            "3 item(s)"
        );
    }

    #[test]
    fn input_label_names_the_mode() {
        assert_eq!(
            input_accessible_label(&OmnibarMode::parse(">copy")),
            "Omnibar, Command mode"
        );
        assert_eq!(
            input_accessible_label(&OmnibarMode::parse("")),
            "Omnibar, Go mode"
        );
    }
}
