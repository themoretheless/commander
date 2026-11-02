//! Permission preview and explicit commit. All metadata I/O runs on workers.
use super::*;
use crate::permissions::{self, Change, Plan, Report, Run, Target};

pub struct PermissionState {
    pub(crate) edit_times: bool,
    modified_time: String,
    accessed_time: String,
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
    #[cfg(feature = "visual-qa")]
    pub fn timestamp_qa(&self) -> serde_json::Value {
        serde_json::json!({"timestamps":self.edit_times,"modified":self.modified_time,"accessed":self.accessed_time,"busy":self.inspect.is_some()||self.applying.is_some(),"needs_reload":self.needs_reload,"error":self.error,"report":self.report.as_ref().map(|r|serde_json::json!({"changed":r.changed,"unchanged":r.unchanged,"errors":r.errors,"cancelled":r.cancelled})),"rows":self.plan.as_ref().map(|p|p.rows().iter().map(|r|serde_json::json!({"path":r.target.path,"modified":r.times.map(|t|crate::timestamps::format(t.modified)),"accessed":r.times.map(|t|crate::timestamps::format(t.accessed)),"error":r.error})).collect::<Vec<_>>())})
    }
    fn root(&self) -> PathBuf {
        self.targets
            .first()
            .and_then(|target| target.path.parent())
            .unwrap_or(std::path::Path::new("/"))
            .into()
    }
    fn inspect(&mut self, ctx: &egui::Context) {
        let targets = self.targets.clone();
        let binding = if self.edit_times {
            permissions::Binding::Object
        } else {
            permissions::Binding::Version
        };
        let repaint = ctx.clone();
        match permissions::start(
            self.root(),
            false,
            std::sync::Arc::new(move || repaint.request_repaint()),
            move |cancel| match binding {
                permissions::Binding::Version => permissions::inspect(targets, cancel),
                permissions::Binding::Object => {
                    permissions::inspect_with_binding(targets, binding, cancel)
                }
            },
        ) {
            Ok(run) => {
                self.inspect = Some(run);
                self.plan = None;
                self.error = None;
                self.change = Change::default();
                self.octal.clear();
                self.modified_time.clear();
                self.accessed_time.clear();
                self.needs_reload = false;
            }
            Err(error) => self.error = Some(error),
        }
    }
}

impl App {
    pub(crate) fn open_permissions(&mut self, targets: Vec<Target>, ctx: &egui::Context) {
        let mut state = PermissionState {
            edit_times: false,
            modified_time: String::new(),
            accessed_time: String::new(),
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
                    state.error = Some("Metadata worker stopped; some changes may already have applied. Reload to inspect.".into());
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
        let title = if state.edit_times {
            "Change file timestamps"
        } else {
            "Change permissions"
        };
        let window = super::sheet_window(title, t, 14);
        let window = if state.edit_times {
            window.default_height(
                (ctx.input(|i| i.viewport_rect().height()) - 100.).clamp(240., 560.),
            )
        } else {
            window
        };
        let dialog_response = window.show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt("permission-sheet").max_height((ctx.input(|input| input.viewport_rect().height()) - 64.0).max(100.0)).show(ui, |ui| {
            ui.set_width(560.0);
            ui.strong(title);
            ui.label(format!("{} selected item{}", state.targets.len(), if state.targets.len() == 1 { "" } else { "s" }));
            ui.small("Selected items only; folder contents are not changed.");
            if state.edit_times {ui.small("UTC display · blank fields keep current times · symbolic links are not followed.");ui.small("Hard-link names share timestamps. Timestamp changes are not added to Undo history.");}
            else {ui.small("Read/write/execute bits only; existing special bits are retained.");ui.small("Permission changes are not added to Undo history.");}
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
                if state.edit_times {
                    (apply,reload)=timestamp_controls(ui,plan,&mut state.modified_time,&mut state.accessed_time,(busy,state.needs_reload,blocked.as_deref()),t);
                } else {
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
            }
            if !busy && state.plan.is_none() && ui.button("Retry inspection").clicked() { reload = true; }
            if state.applying.is_some() {
                if (ui.button("Stop").on_hover_text("Already applied metadata changes remain").clicked() || escape)
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
            let time_change = state
                .edit_times
                .then(|| {
                    crate::timestamps::Change::parse(&state.modified_time, &state.accessed_time)
                })
                .transpose();
            let Ok(time_change) = time_change else {
                state.error = Some("Invalid timestamp; no changes applied".into());
                return;
            };
            let repaint = ctx.clone();
            match permissions::start(
                state.root(),
                true,
                std::sync::Arc::new(move || repaint.request_repaint()),
                move |cancel| {
                    if let Some(change) = time_change {
                        crate::timestamps::apply(plan, change, cancel)
                    } else {
                        permissions::apply(plan, change, cancel)
                    }
                },
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

fn timestamp_controls(
    ui: &mut egui::Ui,
    plan: &Plan,
    modified: &mut String,
    accessed: &mut String,
    status: (bool, bool, Option<&str>),
    t: ThemeColors,
) -> (bool, bool) {
    let (busy, needs_reload, blocked) = status;
    ui.add_enabled_ui(!busy,|ui| {
        for (label,value,key) in [("Modified time",&mut *modified,"timestamp-modified"),("Accessed time",&mut *accessed,"timestamp-accessed")] {
            ui.horizontal(|ui| {
                ui.label(label);
                let response=ui.add(egui::TextEdit::singleline(value).id(egui::Id::new(key)).desired_width(380.).char_limit(64).hint_text("Leave unchanged"));
                response.widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::TextEdit,response.enabled(),label));
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(key).with("rect"),response.rect));
            });
        }
        ui.small("YYYY-MM-DD means midnight UTC; a time needs a timezone, e.g. 2026-10-31T14:30:00+04:00.");
        ui.horizontal(|ui| {
            let now=ui.button("Use current time");
            #[cfg(any(test,feature="visual-qa"))]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("timestamp-now"),now.rect));
            if now.clicked(){let value=crate::timestamps::format(std::time::SystemTime::now());*modified=value.clone();*accessed=value;}
            let reset=ui.button("Keep existing times");
            #[cfg(any(test,feature="visual-qa"))]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("timestamp-reset"),reset.rect));
            if reset.clicked(){modified.clear();accessed.clear();}
            #[cfg(feature="visual-qa")]
            timestamp_pointer_diagnostic(ui,"reset",&reset);
        });
    });
    let change = crate::timestamps::Change::parse(modified, accessed);
    if let Err(error) = &change {
        ui.colored_label(t.accent_red, error);
    }
    let changes = change
        .as_ref()
        .map_or(0, |c| crate::timestamps::changed_count(plan, *c));
    ui.label(format!("{changes} selected items would change"));
    if !crate::timestamps::ready(plan) {
        ui.colored_label(t.accent_red,"Some selected items or their timestamps are unavailable; remove them from the selection before applying.");
    }
    egui::ScrollArea::vertical()
        .id_salt("timestamp-preview")
        .max_height(180.)
        .show_rows(ui, 60., plan.rows().len(), |ui, range| {
            for row in &plan.rows()[range] {
                let name = row
                    .target
                    .path
                    .file_name()
                    .unwrap_or(row.target.path.as_os_str())
                    .to_string_lossy();
                let label = ui.add(egui::Label::new(name).truncate());
                ui.ctx().accesskit_node_builder(label.id, |node| {
                    node.set_description(row.target.path.display().to_string())
                });
                label.on_hover_text(row.target.path.display().to_string());
                if let Some(before) = row.times {
                    for (label, old, new) in [
                        (
                            "Modified",
                            before.modified,
                            change.as_ref().ok().and_then(|c| c.modified),
                        ),
                        (
                            "Accessed",
                            before.accessed,
                            change.as_ref().ok().and_then(|c| c.accessed),
                        ),
                    ] {
                        ui.small(format!(
                            "{label}: {} -> {}",
                            crate::timestamps::format(old),
                            new.map_or_else(|| "keep current".into(), crate::timestamps::format)
                        ));
                    }
                } else {
                    ui.colored_label(
                        t.accent_red,
                        row.error.as_deref().unwrap_or("Timestamps unavailable"),
                    );
                }
            }
        });
    if needs_reload {
        ui.small("Reload timestamps before applying another change; Reload clears the edits.");
    }
    if let Some(reason) = blocked {
        ui.small(reason);
    }
    let mut result = (false, false);
    ui.horizontal(|ui| {
        let apply = ui.add_enabled(
            !busy
                && !needs_reload
                && blocked.is_none()
                && change.is_ok()
                && crate::timestamps::ready(plan)
                && changes > 0,
            super::dialog_button(
                t,
                super::ButtonRole::Primary,
                egui::RichText::new("Apply timestamps"),
            ),
        );
        let reload = ui.add_enabled(!busy, egui::Button::new("Reload timestamps"));
        #[cfg(feature = "visual-qa")]
        timestamp_pointer_diagnostic(ui, "apply", &apply);
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("timestamp-apply"), apply.rect);
            d.insert_temp(egui::Id::new("timestamp-apply-enabled"), apply.enabled());
            d.insert_temp(egui::Id::new("timestamp-reload"), reload.rect);
        });
        result = (apply.clicked(), reload.clicked());
    });
    result
}

#[cfg(feature = "visual-qa")]
fn timestamp_pointer_diagnostic(ui: &egui::Ui, kind: &str, response: &egui::Response) {
    if !ui.input(|i| i.pointer.any_pressed() || i.pointer.any_released()) {
        return;
    }
    let value = serde_json::json!({"kind":kind,"id":format!("{:?}",response.id),"rect":[response.rect.min.x,response.rect.min.y,response.rect.max.x,response.rect.max.y],"enabled":response.enabled(),"hovered":response.hovered(),"clicked":response.clicked(),"pointer":ui.input(|i|i.pointer.hover_pos()).map(|p|[p.x,p.y]),"down":ui.input(|i|i.pointer.primary_down())});
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new(("timestamp-pointer", kind)), value));
}

#[cfg(test)]
mod timestamp_tests {
    use super::*;
    #[test]
    fn timestamp_buttons_clear_invalid_and_current_time_drafts_without_writing_files() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.file("file", "content");
        let target = Target {
            path: path.clone(),
            identity: crate::panel::ListingIdentity::Captured(
                crate::path_identity::PathIdentity::observe(&path).unwrap(),
            ),
            capture_on_worker: false,
        };
        let plan = permissions::inspect(vec![target], &|| false).unwrap();
        let before = plan.rows()[0].times.unwrap();
        let ctx = egui::Context::default();
        let mut modified = "not a date".to_string();
        let mut accessed = String::new();
        let mut frame = |events: Vec<egui::Event>| {
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800., 700.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(560.);
                    timestamp_controls(
                        ui,
                        &plan,
                        &mut modified,
                        &mut accessed,
                        (false, false, None),
                        ThemeColors::dark(),
                    );
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(
                            egui::Id::new("timestamp-test-drafts"),
                            (modified.clone(), accessed.clone()),
                        )
                    });
                },
            ));
        };
        frame(vec![]);
        for key in ["timestamp-reset", "timestamp-now", "timestamp-reset"] {
            let pos = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new(key)))
                .unwrap()
                .center();
            frame(vec![egui::Event::PointerMoved(pos)]);
            frame(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }]);
            frame(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }]);
            let (a, b) = ctx
                .data(|d| d.get_temp::<(String, String)>(egui::Id::new("timestamp-test-drafts")))
                .unwrap();
            if key == "timestamp-reset" {
                assert!(a.is_empty() && b.is_empty());
            } else {
                assert!(!a.is_empty() && a == b);
            }
        }
        assert!(modified.is_empty() && accessed.is_empty());
        let actual =
            crate::timestamps::Snapshot::from_metadata(&std::fs::metadata(path).unwrap()).unwrap();
        assert_eq!(actual, before);
    }
}
