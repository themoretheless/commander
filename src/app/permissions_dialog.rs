//! Permission preview and explicit commit. All metadata I/O runs on workers.
use super::*;
use crate::permissions::{self, Change, Plan, Report, Run, Target};

pub struct PermissionState {
    targets: Vec<Target>,
    pub(crate) plan: Option<Plan>,
    inspect: Option<Run<Result<Plan, String>>>,
    applying: Option<Run<Report>>,
    pub(crate) change: Change,
    octal: String,
    error: Option<String>,
    report: Option<Report>,
    needs_reload: bool,
    focus_pending: bool,
}

impl PermissionState {
    fn root(&self) -> PathBuf {
        self.targets
            .first()
            .and_then(|target| target.path.parent())
            .unwrap_or(std::path::Path::new("/"))
            .into()
    }
    fn inspect(&mut self, ctx: &egui::Context) {
        let targets = self.targets.clone();
        let repaint = ctx.clone();
        match permissions::start(
            self.root(),
            false,
            std::sync::Arc::new(move || repaint.request_repaint()),
            move |cancel| permissions::inspect(targets, cancel),
        ) {
            Ok(run) => {
                self.inspect = Some(run);
                self.plan = None;
                self.error = None;
                self.change = Change::default();
                self.octal.clear();
                self.needs_reload = false;
            }
            Err(error) => self.error = Some(error),
        }
    }
}

impl App {
    pub(crate) fn open_permissions(&mut self, targets: Vec<Target>, ctx: &egui::Context) {
        let mut state = PermissionState {
            targets,
            plan: None,
            inspect: None,
            applying: None,
            change: Change::default(),
            octal: String::new(),
            error: None,
            report: None,
            needs_reload: false,
            focus_pending: true,
        };
        state.inspect(ctx);
        self.ui.modals.permissions = Some(state);
        Self::mark_modal_opened(ctx, UiModal::Permissions);
    }

    pub(crate) fn show_permissions_dialog(&mut self, ctx: &egui::Context) {
        let escape = self.take_escape_request(crate::accessibility::EscapeRoute::Modal(
            crate::accessibility::ModalSurface::Permissions,
        ));
        let _ = Self::take_modal_opened(ctx, UiModal::Permissions);
        let mut blocked = self.ws.mutation_block_reason("changing permissions");
        if self.ws.known_active_volume_read_only() {
            blocked = Some("This pane is read-only".into());
        }
        if self.ws.has_unfinished_transfer_work() {
            blocked =
                Some("Wait for the transfer queue to finish before changing permissions".into());
        }
        let Some(state) = self.ui.modals.permissions.as_mut() else {
            return;
        };
        if let Some(run) = &state.inspect {
            match run.poll() {
                Ok(Ok(plan)) => {
                    state.plan = Some(plan);
                    state.inspect = None;
                }
                Ok(Err(error)) => {
                    state.error = Some(error);
                    state.inspect = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    state.error = Some("Permission inspection worker stopped".into());
                    state.inspect = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut refresh_panels = false;
        if let Some(run) = &state.applying {
            match run.poll() {
                Ok(report) => {
                    state.report = Some(report);
                    state.applying = None;
                    state.needs_reload = true;
                    refresh_panels = true;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    state.error = Some("Permission worker stopped; some changes may already have applied. Reload permissions to inspect.".into());
                    state.applying = None;
                    state.needs_reload = true;
                    refresh_panels = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        let busy = state.inspect.is_some() || state.applying.is_some();
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let mut close = false;
        let mut apply = false;
        let mut reload = false;
        let t = self.colors;
        let dialog_response = super::sheet_window("Change permissions", t, 14).show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt("permission-sheet").max_height((ctx.input(|input| input.viewport_rect().height()) - 64.0).max(100.0)).show(ui, |ui| {
            ui.set_width(560.0);
            ui.strong("Change permissions");
            ui.label(format!("{} selected item{}", state.targets.len(), if state.targets.len() == 1 { "" } else { "s" }));
            ui.small("Selected items only; folder contents are not changed.");
            ui.small("Read/write/execute bits only; existing special bits are retained.");
            ui.small("Permission changes are not added to Undo history.");
            if busy { ui.spinner(); }
            if let Some(error) = &state.error { ui.colored_label(t.accent_red, error); }
            if let Some(report) = &state.report {
                ui.label(format!("Changed {} · unchanged {} · errors {}{}", report.changed, report.unchanged, report.errors.len(), if report.cancelled { " · stopped" } else { "" }));
                egui::ScrollArea::vertical().id_salt("permission-errors").max_height(100.0).show(ui, |ui| {
                    for error in report.errors.iter().take(5) { ui.add(egui::Label::new(error).wrap()); }
                    if report.errors.len() > 5 { ui.label(format!("{} more errors", report.errors.len() - 5)); }
                });
            }
            let mut valid_mode = true;
            if let Some(plan) = &state.plan {
                ui.add_enabled_ui(!busy, |ui| {
                    let input = ui.add(egui::TextEdit::singleline(&mut state.octal).hint_text("Octal mode, e.g. 644 or 0755").char_limit(4));
                    input.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, input.enabled(), "Octal permissions"));
                    if state.focus_pending { input.request_focus(); state.focus_pending = false; }
                    if input.changed() {
                        if state.octal.is_empty() { state.change = Change::default(); }
                        else if let Ok(mode) = permissions::parse_mode(&state.octal) { state.change = Change::exact(mode); }
                    }
                    if !state.octal.is_empty() && let Err(error) = permissions::parse_mode(&state.octal) { valid_mode = false; ui.colored_label(t.accent_red, error); }
                    egui::Grid::new("permission-bits").show(ui, |ui| {
                        for (group, shift) in [("Owner", 6), ("Group", 3), ("Others", 0)] {
                            for (label, value) in [("read", 4), ("write", 2), ("execute", 1)] {
                                let bit = value << shift;
                                let (all, mixed) = plan.bit_state(bit);
                                let touched = (state.change.set | state.change.clear) & bit != 0;
                                let mut checked = (all || state.change.set & bit != 0) && state.change.clear & bit == 0;
                                if ui.add(egui::Checkbox::new(&mut checked, format!("{group} {label}")).indeterminate(mixed && !touched)).changed() {
                                    state.change.set_bit(bit, checked); state.octal.clear();
                                }
                            }
                            ui.end_row();
                        }
                    });
                    if ui.button("Reset edits").clicked() { state.change = Change::default(); state.octal.clear(); }
                });
                let (owner_read, mixed_read) = plan.bit_state(0o400);
                if state.change.clear & 0o400 != 0 && (owner_read || mixed_read) {
                    ui.small("Removing owner read access can prevent this editor from reopening the item.");
                }
                let changes = plan.changed_count(state.change);
                ui.label(format!("{changes} item{} would change", if changes == 1 { "" } else { "s" }));
                if !plan.ready() { ui.colored_label(t.accent_red, "Some selected items are unavailable; see row details. Remove them from the selection before applying."); }
                egui::ScrollArea::vertical().id_salt("permission-preview").max_height(180.0).show_rows(ui, 22.0, plan.rows().len(), |ui, rows| {
                    for index in rows {
                        let row = &plan.rows()[index];
                        ui.horizontal(|ui| {
                            let name = row.target.path.file_name().unwrap_or(row.target.path.as_os_str()).to_string_lossy();
                            let label = ui.add_sized([330.0, 22.0], egui::Label::new(name).truncate());
                            ui.ctx().accesskit_node_builder(label.id, |node| node.set_description(row.target.path.display().to_string()));
                            label.on_hover_text(row.target.path.display().to_string());
                            if let Some(mode) = row.mode { ui.monospace(format!("{mode:04o} -> {:04o}", state.change.apply(mode))); }
                            else { ui.colored_label(t.accent_red, "Unavailable").on_hover_text(row.error.as_deref().unwrap_or("Unknown error")); }
                        });
                    }
                });
                if state.needs_reload { ui.small("Reload permissions before applying another change."); }
                if let Some(reason) = &blocked { ui.small(reason); }
                ui.horizontal(|ui| {
                    apply = ui.add_enabled(!busy && !state.needs_reload && blocked.is_none() && valid_mode && plan.ready() && plan.changed_count(state.change) > 0, super::dialog_button(t, super::ButtonRole::Primary, egui::RichText::new("Apply permissions"))).clicked();
                    reload = ui.add_enabled(!busy, egui::Button::new("Reload permissions")).clicked();
                });
            }
            if !busy && state.plan.is_none() && ui.button("Retry inspection").clicked() { reload = true; }
            if state.applying.is_some() {
                if (ui.button("Stop").on_hover_text("Already applied permission changes remain").clicked() || escape)
                    && let Some(run) = &state.applying { run.cancel(); }
            } else { close = ui.button("Close").clicked() || escape; }
            });
        });
        #[cfg(not(feature = "visual-qa"))]
        let _ = dialog_response;
        #[cfg(feature = "visual-qa")]
        if let Some(response) = dialog_response {
            crate::visual_qa::record_response(
                ctx,
                crate::visual_qa::ProbeId::Confirmation,
                &response.response,
            );
        }
        if apply && let Some(plan) = state.plan.clone() {
            let change = state.change;
            let repaint = ctx.clone();
            match permissions::start(
                state.root(),
                true,
                std::sync::Arc::new(move || repaint.request_repaint()),
                move |cancel| permissions::apply(plan, change, cancel),
            ) {
                Ok(run) => {
                    state.applying = Some(run);
                    state.report = None;
                    state.error = None;
                }
                Err(error) => state.error = Some(error),
            }
        }
        if reload {
            state.inspect(ctx);
        }
        if close {
            self.ui.modals.permissions = None;
        }
        if refresh_panels {
            self.ws.left.refresh();
            self.ws.right.refresh();
        }
    }
}
