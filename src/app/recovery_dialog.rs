//! Durable operation recovery, rollback planning, and orphan-staging cleanup.

use super::*;

impl RecoveryState {
    pub(crate) fn scan(workspace: &Workspace) -> Self {
        let mut state = Self::default();
        state.start_scan(workspace);
        state
    }

    pub(crate) fn start_scan(&mut self, workspace: &Workspace) {
        self.error = None;
        self.scanning = true;
        let roots = workspace.recovery_seed_roots();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(RecoveryScanResult {
                inventory: crate::operation_journal::recovery_inventory(&roots),
            });
        });
        self.scan_rx = Some(receiver);
    }

    pub(crate) fn poll_scan(&mut self) {
        let Some(receiver) = &self.scan_rx else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.scanning = false;
                self.scan_rx = None;
                self.error = Some("Recovery scan stopped unexpectedly".to_string());
                return;
            }
        };
        self.scanning = false;
        self.scan_rx = None;
        match result.inventory {
            Ok(inventory) => {
                self.error = None;
                self.operations = inventory.operations;
                self.orphans = inventory.orphans;
            }
            Err(error) => {
                self.error = Some(error);
                self.operations.clear();
                self.orphans.clear();
            }
        }
        if self
            .selected
            .as_ref()
            .is_none_or(|selected| !self.operations.iter().any(|record| &record.id == selected))
        {
            self.selected = self.operations.first().map(|record| record.id.clone());
        }
        self.refresh_selected();
    }

    pub(crate) fn select(&mut self, operation_id: crate::operation::OperationId) {
        self.selected = Some(operation_id);
        self.detail = RecoveryDetail::Inspect;
        self.outcome = None;
        self.refresh_selected();
    }

    fn refresh_selected(&mut self) {
        self.repair_plan = None;
        self.versions.clear();
        self.repair_loaded = false;
    }

    fn load_repair(&mut self) {
        if self.repair_loaded {
            return;
        }
        self.repair_loaded = true;
        let Some(operation_id) = self.selected.as_ref() else {
            return;
        };
        match crate::operation_journal::repair_plan(operation_id) {
            Ok(plan) => self.repair_plan = Some(plan),
            Err(error) => self.error = Some(error),
        }
        self.versions = crate::version_store::records_for(operation_id);
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(18).collect()
}

fn operation_kind(kind: TransferKind) -> &'static str {
    match kind {
        TransferKind::Copy => "Copy",
        TransferKind::Move => "Move",
    }
}

fn operation_time(seconds: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(seconds as i64, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%b %e, %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "Unknown time".to_string())
}

fn status_color(status: crate::operation_journal::OperationStatus, t: ThemeColors) -> Color32 {
    use crate::operation_journal::OperationStatus;
    match status {
        OperationStatus::NeedsReview | OperationStatus::Running => t.accent_warning,
        OperationStatus::Failed => t.accent_red,
        OperationStatus::Stopped | OperationStatus::Planned => t.text_secondary,
        OperationStatus::Completed | OperationStatus::RolledBack => t.accent,
    }
}

fn show_operation_list(
    ui: &mut egui::Ui,
    state: &RecoveryState,
    t: ThemeColors,
    selected: &mut Option<crate::operation::OperationId>,
) {
    ui.set_width(236.0);
    if state.operations.is_empty() {
        ui.label(
            egui::RichText::new(if state.scanning {
                "Scanning operations..."
            } else {
                "No interrupted operations"
            })
            .size(11.0)
            .color(t.text_muted),
        );
        return;
    }
    egui::ScrollArea::vertical()
        .max_height(430.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for record in &state.operations {
                let active = state.selected.as_ref() == Some(&record.id);
                let target = record
                    .target
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| record.target.display().to_string());
                let label = format!(
                    "{}  {}\n{}  \u{00b7}  {}",
                    record.status.label(),
                    operation_kind(record.kind),
                    target,
                    operation_time(record.updated_at_secs)
                );
                let response = ui.add_sized(
                    [228.0, 48.0],
                    egui::Button::new(egui::RichText::new(label).size(10.0).color(if active {
                        Color32::WHITE
                    } else {
                        t.text_primary
                    }))
                    .selected(active)
                    .corner_radius(CornerRadius::ZERO),
                );
                if response.clicked() {
                    *selected = Some(record.id.clone());
                }
                response.on_hover_text(format!("{}\n{}", record.id.0, record.target.display()));
                ui.add_space(3.0);
            }
        });
}

fn show_inspect(
    ui: &mut egui::Ui,
    record: &crate::operation_journal::OperationRecord,
    t: ThemeColors,
) {
    egui::Grid::new("recovery_summary")
        .num_columns(2)
        .spacing([14.0, 5.0])
        .show(ui, |ui| {
            for (label, value) in [
                ("Operation", record.id.0.clone()),
                ("Target", record.target.display().to_string()),
                ("Durability", record.durability.label().to_string()),
                ("Completed", record.completed_steps().to_string()),
                ("Failed", record.failed_steps().to_string()),
            ] {
                ui.label(egui::RichText::new(label).size(10.0).color(t.text_muted));
                ui.label(
                    egui::RichText::new(value)
                        .size(10.0)
                        .monospace()
                        .color(t.text_primary),
                );
                ui.end_row();
            }
        });
    if let Some(group) = &record.group_id {
        ui.label(
            egui::RichText::new(format!("Group {}", short_id(&group.0)))
                .size(10.0)
                .monospace()
                .color(t.text_muted),
        )
        .on_hover_text(&group.0);
    }
    ui.add_space(7.0);
    ui.separator();
    egui::ScrollArea::vertical()
        .max_height(295.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for step in &record.steps {
                Frame::NONE
                    .fill(t.bg_card)
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(step.status.label())
                                    .size(9.0)
                                    .strong()
                                    .color(match step.status {
                                        crate::operation_journal::StepStatus::Failed => {
                                            t.accent_red
                                        }
                                        crate::operation_journal::StepStatus::Running => {
                                            t.accent_warning
                                        }
                                        _ => t.text_secondary,
                                    }),
                            );
                            ui.label(
                                egui::RichText::new(step.source.display().to_string())
                                    .size(10.0)
                                    .monospace()
                                    .color(t.text_primary),
                            );
                        });
                        let effect = step.landing.as_ref().unwrap_or(&step.destination);
                        ui.label(
                            egui::RichText::new(format!("\u{2192} {}", effect.display()))
                                .size(10.0)
                                .monospace()
                                .color(t.text_secondary),
                        );
                        if let Some(fast_path) = step.fast_path {
                            ui.label(
                                egui::RichText::new(format!("Fast path: {}", fast_path.label()))
                                    .size(9.0)
                                    .color(t.text_muted),
                            );
                        }
                        if let Some(checkpoint) = &step.checkpoint {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Resumable at {}  \u{00b7}  {}",
                                    crate::panel::format_size(checkpoint.offset),
                                    checkpoint.layout.label()
                                ))
                                .size(9.0)
                                .color(t.accent_warning),
                            )
                            .on_hover_text(checkpoint.staging.display().to_string());
                        }
                        if let Some(failure) = &step.failure {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}: {}",
                                    failure.class.label(),
                                    failure.message
                                ))
                                .size(10.0)
                                .color(t.accent_red),
                            );
                        }
                    });
                ui.add_space(4.0);
            }
        });
}

fn show_repair(
    ui: &mut egui::Ui,
    state: &RecoveryState,
    record: &crate::operation_journal::OperationRecord,
    t: ThemeColors,
    apply: &mut Option<crate::operation::OperationId>,
) {
    let Some(plan) = &state.repair_plan else {
        ui.label(
            egui::RichText::new("Repair plan is unavailable")
                .size(11.0)
                .color(t.accent_red),
        );
        return;
    };
    let automatic = plan.remaining.iter().filter(|item| item.automatic).count();
    let manual = plan.remaining.len().saturating_sub(automatic);
    ui.label(
        egui::RichText::new(format!(
            "{automatic} automatic  \u{00b7}  {manual} manual  \u{00b7}  {} completed",
            plan.completed.len()
        ))
        .size(11.0)
        .color(t.text_secondary),
    );
    ui.add_space(6.0);
    egui::ScrollArea::vertical()
        .max_height(285.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for item in plan.completed.iter().chain(plan.remaining.iter()) {
                let done = plan.completed.iter().any(|completed| completed == item);
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(if done {
                            "DONE"
                        } else if item.automatic {
                            "AUTO"
                        } else {
                            "MANUAL"
                        })
                        .size(9.0)
                        .strong()
                        .color(if done || item.automatic {
                            t.accent
                        } else {
                            t.accent_warning
                        }),
                    );
                    ui.label(
                        egui::RichText::new(&item.action)
                            .size(10.0)
                            .color(t.text_primary),
                    );
                });
                ui.label(
                    egui::RichText::new(item.path.display().to_string())
                        .size(10.0)
                        .monospace()
                        .color(t.text_muted),
                );
                ui.add_space(4.0);
            }
            if !state.versions.is_empty() {
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "{} verified local version{}",
                        state.versions.len(),
                        if state.versions.len() == 1 { "" } else { "s" }
                    ))
                    .size(10.0)
                    .strong()
                    .color(t.text_secondary),
                );
                for version in &state.versions {
                    ui.label(
                        egui::RichText::new(version.original.display().to_string())
                            .size(10.0)
                            .monospace()
                            .color(t.text_muted),
                    );
                }
            }
        });
    ui.add_space(7.0);
    if ui
        .add_enabled(
            !state.scanning && (record.completed_steps() > 0 || record.rollback_cleanup.is_some()),
            egui::Button::new(
                egui::RichText::new("Apply verified rollback")
                    .size(12.0)
                    .color(Color32::WHITE),
            )
            .fill(t.accent_red)
            .corner_radius(CornerRadius::ZERO),
        )
        .clicked()
    {
        *apply = Some(record.id.clone());
    }
}

impl App {
    fn push_recovery_toast(&mut self, ctx: &egui::Context, message: String, error: bool) {
        self.toasts.push(crate::toasts::Toast::new(
            message,
            if error {
                crate::toasts::ToastKind::Error
            } else {
                crate::toasts::ToastKind::Success
            },
            false,
            ctx.input(|input| input.time),
        ));
    }

    pub(crate) fn show_recovery_dialog(&mut self, ctx: &egui::Context) {
        let escape_requested = self.take_modal_escape(crate::accessibility::ModalSurface::Recovery);
        let requested = std::mem::take(&mut self.ws.recovery_request);
        let mut state = std::mem::take(&mut self.recovery);
        state.poll_scan();
        if requested {
            state.open = true;
            if !state.scanning {
                state.start_scan(&self.ws);
            }
        }
        if state.scanning {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if !state.open {
            self.recovery = state;
            return;
        }

        let t = self.colors;
        let mut close = false;
        let mut refresh = false;
        let mut selected = None;
        let mut resume = None;
        let mut rollback = None;
        let mut clean_orphan = None;
        let selected_record = state
            .selected
            .as_ref()
            .and_then(|id| state.operations.iter().find(|record| &record.id == id))
            .cloned();

        egui::Window::new("Recovery center")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                Frame::NONE
                    .fill(t.bg_panel)
                    .inner_margin(Margin::same(14))
                    .stroke(Stroke::new(1.0, t.border)),
            )
            .show(ctx, |ui| {
                ui.set_width(790.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("Recovery center")
                                .size(14.0)
                                .strong()
                                .color(t.text_primary),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "{} interrupted operation{}  \u{00b7}  {} staging item{}",
                                state.operations.len(),
                                if state.operations.len() == 1 { "" } else { "s" },
                                state.orphans.len(),
                                if state.orphans.len() == 1 { "" } else { "s" }
                            ))
                            .size(10.0)
                            .color(t.text_muted),
                        );
                        if state.scanning {
                            ui.label(
                                egui::RichText::new("Scanning recovery paths")
                                    .size(9.0)
                                    .color(t.text_secondary),
                            );
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("\u{00d7}").on_hover_text("Close").clicked() {
                            close = true;
                        }
                        if ui
                            .add_enabled(!state.scanning, egui::Button::new("\u{21bb}"))
                            .on_hover_text("Refresh")
                            .clicked()
                        {
                            refresh = true;
                        }
                    });
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut state.section,
                        RecoverySection::Operations,
                        format!("Operations {}", state.operations.len()),
                    );
                    ui.selectable_value(
                        &mut state.section,
                        RecoverySection::Staging,
                        format!("Staging cleanup {}", state.orphans.len()),
                    );
                });
                ui.separator();

                if let Some(error) = &state.error {
                    ui.label(
                        egui::RichText::new(error)
                            .size(11.0)
                            .color(t.accent_red),
                    );
                    ui.add_space(5.0);
                }
                if let Some(outcome) = &state.outcome {
                    ui.label(
                        egui::RichText::new(outcome)
                            .size(11.0)
                            .color(t.accent),
                    );
                    ui.add_space(5.0);
                }

                match state.section {
                    RecoverySection::Operations => {
                        ui.horizontal_top(|ui| {
                            show_operation_list(ui, &state, t, &mut selected);
                            ui.separator();
                            ui.vertical(|ui| {
                                ui.set_width(528.0);
                                if let Some(record) = &selected_record {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{} {}",
                                                operation_kind(record.kind),
                                                short_id(&record.id.0)
                                            ))
                                            .size(12.0)
                                            .strong()
                                            .color(t.text_primary),
                                        );
                                        ui.label(
                                            egui::RichText::new(record.status.label())
                                                .size(10.0)
                                                .strong()
                                                .color(status_color(record.status, t)),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if ui
                                                    .add_enabled(
                                                        !state.scanning,
                                                        egui::Button::new("Resume")
                                                            .corner_radius(CornerRadius::ZERO),
                                                    )
                                                    .clicked()
                                                {
                                                    resume = Some(record.id.clone());
                                                }
                                            },
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        ui.selectable_value(
                                            &mut state.detail,
                                            RecoveryDetail::Inspect,
                                            "Inspect",
                                        );
                                        ui.selectable_value(
                                            &mut state.detail,
                                            RecoveryDetail::Repair,
                                            "Repair plan",
                                        );
                                    });
                                    ui.add_space(5.0);
                                    match state.detail {
                                        RecoveryDetail::Inspect => show_inspect(ui, record, t),
                                        RecoveryDetail::Repair => {
                                            show_repair(ui, &state, record, t, &mut rollback)
                                        }
                                    }
                                } else {
                                    ui.label(
                                        egui::RichText::new("Select an operation to inspect")
                                            .size(11.0)
                                            .color(t.text_muted),
                                    );
                                }
                            });
                        });
                    }
                    RecoverySection::Staging => {
                        if state.orphans.is_empty() {
                            ui.label(
                                egui::RichText::new(if state.scanning {
                                    "Scanning staging paths..."
                                } else {
                                    "No unreferenced staging paths"
                                })
                                    .size(11.0)
                                    .color(t.text_muted),
                            );
                        } else {
                            egui::ScrollArea::vertical().max_height(430.0).show(ui, |ui| {
                                for (index, orphan) in state.orphans.iter().enumerate() {
                                    Frame::NONE
                                        .fill(t.bg_card)
                                        .inner_margin(Margin::symmetric(9, 7))
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                ui.vertical(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(
                                                            orphan.path.display().to_string(),
                                                        )
                                                        .size(10.0)
                                                        .monospace()
                                                        .color(t.text_primary),
                                                    );
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "{} bytes  \u{00b7}  identity verified at scan",
                                                            orphan.identity.size
                                                        ))
                                                        .size(9.0)
                                                        .color(t.text_muted),
                                                    );
                                                });
                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        if ui
                                                            .add_enabled(
                                                                !state.scanning,
                                                                egui::Button::new("Clean")
                                                                    .corner_radius(
                                                                        CornerRadius::ZERO,
                                                                    ),
                                                            )
                                                            .clicked()
                                                        {
                                                            clean_orphan = Some(index);
                                                        }
                                                    },
                                                );
                                            });
                                        });
                                    ui.add_space(4.0);
                                }
                            });
                        }
                    }
                }

                if escape_requested {
                    close = true;
                }
            });

        if let Some(operation_id) = selected {
            state.select(operation_id);
        }
        if refresh {
            state.start_scan(&self.ws);
        }
        if let Some(index) = clean_orphan
            && let Some(orphan) = state.orphans.get(index).cloned()
        {
            match self.ws.clean_recovery_orphan(&orphan) {
                Ok(()) => {
                    state.outcome = Some(format!("Cleaned {}", orphan.path.display()));
                    state.start_scan(&self.ws);
                }
                Err(error) => state.error = Some(error),
            }
        }
        if let Some(operation_id) = rollback {
            match self.ws.rollback_recovery(&operation_id) {
                Ok(plan) => {
                    state.outcome = Some(format!(
                        "Rollback completed {} step{}; {} item{} need review",
                        plan.completed.len(),
                        if plan.completed.len() == 1 { "" } else { "s" },
                        plan.remaining.len(),
                        if plan.remaining.len() == 1 { "" } else { "s" }
                    ));
                    state.repair_plan = Some(plan);
                    state.repair_loaded = true;
                    state.start_scan(&self.ws);
                }
                Err(error) => state.error = Some(error),
            }
        }
        if let Some(operation_id) = resume {
            let repaint = ctx.clone();
            match self
                .ws
                .resume_recovery(&operation_id, move || repaint.request_repaint())
            {
                Ok(count) => {
                    state.open = false;
                    self.push_recovery_toast(
                        ctx,
                        if count == 0 {
                            "Finalizing verified operation".to_string()
                        } else {
                            format!("Resuming {count} manifest entries")
                        },
                        false,
                    );
                }
                Err(error) => state.error = Some(error),
            }
        }
        if close {
            state.open = false;
        }
        if state.detail == RecoveryDetail::Repair && !state.repair_loaded {
            state.load_repair();
        }
        self.recovery = state;
    }
}
