//! Unified, non-modal Operations Center for queue, history, failures, and
//! durable recovery. The full-height side surface keeps normal file work
//! available and replaces three competing floating windows.

use super::*;
use crate::opqueue::JobState;

impl App {
    pub(crate) fn toggle_queue_panel(&mut self) {
        if self.show_operations_center && self.operations_tab == OperationsTab::Queue {
            self.show_operations_center = false;
        } else {
            self.show_operations_center = true;
            self.operations_tab = OperationsTab::Queue;
        }
    }

    pub(crate) fn open_operation_history(&mut self) {
        self.show_operations_center = true;
        self.operations_tab = OperationsTab::History;
    }

    pub(crate) fn open_recovery_center(&mut self) {
        self.show_operations_center = true;
        self.operations_tab = OperationsTab::Recovery;
        if !self.recovery.scanning {
            self.recovery.start_scan(&self.ws);
        }
    }

    pub(crate) fn capture_operation_failures(&mut self, ctx: &egui::Context) {
        let failed = self.ws.active_transfer().and_then(|state| {
            let progress = crate::lock_util::recover(state);
            if !progress.finished || progress.errors.is_empty() {
                return None;
            }
            Some(crate::operation_view::FailureNotice {
                operation_id: progress.operation_id.clone()?,
                summary: progress.submitted.clone()?,
                errors: progress.errors.clone(),
                created_at_millis: (ctx.input(|input| input.time) * 1_000.0) as u64,
            })
        });
        if let Some(notice) = failed
            && self.failure_notice_seen.insert(notice.operation_id.clone())
        {
            self.operation_failures.upsert(notice);
            self.show_operations_center = true;
            self.operations_tab = OperationsTab::Errors;
        }
    }

    pub(crate) fn show_operations_center(&mut self, ui: &mut egui::Ui) {
        if !self.show_operations_center {
            return;
        }
        let t = self.colors;
        let mut close = false;
        let panel = match crate::accessibility::operations_placement(ui.available_width()) {
            crate::accessibility::OperationsPlacement::Right => {
                egui::Panel::right(crate::accessibility::FocusRegion::Operations.id())
                    .default_size(410.0)
                    .size_range(340.0..=560.0)
            }
            crate::accessibility::OperationsPlacement::Bottom => {
                egui::Panel::bottom(crate::accessibility::FocusRegion::Operations.id())
                    .default_size(140.0)
                    .size_range(100.0..=220.0)
            }
        };
        panel
            .resizable(true)
            .frame(Frame::NONE.fill(t.bg_panel).inner_margin(Margin::same(12)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Operations")
                            .size(14.0)
                            .strong()
                            .color(t.text_primary),
                    );
                    if !self.operation_failures.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} unresolved",
                                self.operation_failures.notices().len()
                            ))
                            .size(10.0)
                            .color(t.accent_red),
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [24.0, 24.0],
                                egui::Button::new(
                                    egui::RichText::new("\u{2715}").color(t.text_muted),
                                )
                                .frame(false),
                            )
                            .on_hover_text("Close Operations Center")
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
                ui.add_space(7.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let width = ((ui.available_width() - 6.0) / 4.0).max(68.0);
                    for tab in OperationsTab::ALL {
                        let label = if tab == OperationsTab::Errors
                            && !self.operation_failures.is_empty()
                        {
                            format!(
                                "{} {}",
                                tab.label(),
                                self.operation_failures.notices().len()
                            )
                        } else {
                            tab.label().to_string()
                        };
                        let selected = tab == self.operations_tab;
                        if ui
                            .add_sized(
                                [width, 26.0],
                                egui::Button::new(egui::RichText::new(label).size(10.0).color(
                                    if selected {
                                        Color32::WHITE
                                    } else {
                                        t.text_secondary
                                    },
                                ))
                                .fill(if selected { t.accent } else { t.bg_card })
                                .corner_radius(CornerRadius::ZERO),
                            )
                            .clicked()
                        {
                            self.operations_tab = tab;
                        }
                    }
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(5.0);
                match self.operations_tab {
                    OperationsTab::Queue => self.show_operations_queue(ui),
                    OperationsTab::History => self.show_operations_history(ui),
                    OperationsTab::Errors => self.show_operations_errors(ui),
                    OperationsTab::Recovery => self.show_operations_recovery(ui),
                }
            });
        if close {
            self.show_operations_center = false;
        }
    }

    fn show_operations_queue(&mut self, ui: &mut egui::Ui) {
        let t = self.colors;
        let rows = self.ws.queue_snapshot();
        let running = self.ws.active_transfer().map(|state| {
            let progress = crate::lock_util::recover(state);
            (progress.phase, progress.pause_reason.clone())
        });
        if rows.is_empty() {
            ui.label(
                egui::RichText::new("Nothing queued")
                    .size(11.0)
                    .color(t.text_muted),
            );
            return;
        }

        let pending_count = rows
            .iter()
            .filter(|row| row.state == JobState::Pending)
            .count();
        let mut pause = None;
        let mut resume = None;
        let mut promote = None;
        let mut move_up = None;
        let mut move_down = None;
        let mut cancel = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for row in &rows {
                    let (state_label, state_color) = match row.state {
                        JobState::Running => ("Running", t.accent),
                        JobState::Pending => ("Waiting", t.text_muted),
                        JobState::Paused => ("Paused", t.accent_warning),
                        JobState::Done => ("Complete", t.accent),
                        JobState::Failed => ("Failed", t.accent_red),
                        JobState::Cancelled => ("Cancelled", t.text_muted),
                    };
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(&row.label)
                                    .size(11.0)
                                    .strong()
                                    .color(t.text_primary),
                            );
                            ui.label(
                                egui::RichText::new(row.summary.detail())
                                    .size(9.0)
                                    .color(t.text_muted),
                            );
                            let detail = match row.state {
                                JobState::Running => running.as_ref().map_or_else(
                                    || state_label.to_string(),
                                    |(phase, pause_reason)| {
                                        pause_reason.as_ref().map_or_else(
                                            || format!("{} phase", phase.label()),
                                            |reason| {
                                                format!(
                                                    "{}; {}",
                                                    reason.label(),
                                                    reason.resume_condition()
                                                )
                                            },
                                        )
                                    },
                                ),
                                JobState::Paused => {
                                    let reason = crate::operation_view::PauseReason::User;
                                    format!("{}; {}", reason.label(), reason.resume_condition())
                                }
                                _ => state_label.to_string(),
                            };
                            ui.label(egui::RichText::new(detail).size(9.0).color(state_color));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let icon = |ui: &mut egui::Ui, glyph: &str, hint: &str| {
                                ui.add_sized([24.0, 24.0], egui::Button::new(glyph))
                                    .on_hover_text(hint)
                                    .clicked()
                            };
                            if !row.state.is_terminal()
                                && icon(
                                    ui,
                                    "\u{2715}",
                                    if row.state == JobState::Running {
                                        crate::operation_view::CancellationAction::CancelCurrent
                                            .consequence()
                                    } else {
                                        crate::operation_view::CancellationAction::CancelPending
                                            .consequence()
                                    },
                                )
                            {
                                cancel = Some(row.id);
                            }
                            match row.state {
                                JobState::Pending => {
                                    if pending_count > 1 {
                                        if icon(ui, "\u{2193}", "Move later") {
                                            move_down = Some(row.id);
                                        }
                                        if icon(ui, "\u{2191}", "Move earlier") {
                                            move_up = Some(row.id);
                                        }
                                        if icon(ui, "\u{23eb}", "Run next") {
                                            promote = Some(row.id);
                                        }
                                    }
                                    if icon(ui, "\u{23f8}", "Pause pending operation") {
                                        pause = Some(row.id);
                                    }
                                }
                                JobState::Paused if icon(ui, "\u{25b6}", "Resume") => {
                                    resume = Some(row.id);
                                }
                                _ => {}
                            }
                        });
                    });
                    ui.add_space(5.0);
                    ui.separator();
                    ui.add_space(5.0);
                }
            });

        if let Some(id) = pause {
            self.ws.queue_pause(id);
        }
        if let Some(id) = resume {
            let ctx = ui.ctx().clone();
            self.ws.queue_resume(id, move || ctx.request_repaint());
        }
        if let Some(id) = promote {
            self.ws.queue_promote(id);
        }
        if let Some(id) = move_up {
            self.ws.queue_move(id, -1);
        }
        if let Some(id) = move_down {
            self.ws.queue_move(id, 1);
        }
        if let Some(id) = cancel {
            self.ws.queue_cancel(id);
        }
        if rows.iter().any(|row| row.state.is_terminal())
            && ui.small_button("Clear completed").clicked()
        {
            self.ws.queue_clear_finished();
        }
    }

    fn show_operations_history(&mut self, ui: &mut egui::Ui) {
        let t = self.colors;
        ui.add(
            egui::TextEdit::singleline(&mut self.operations_search)
                .desired_width(f32::INFINITY)
                .hint_text("Search history...")
                .margin(egui::vec2(7.0, 5.0)),
        );
        ui.add_space(6.0);
        let receipts = self
            .receipts
            .search(&self.operations_search)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if receipts.is_empty() {
            ui.label(
                egui::RichText::new("No matching operations")
                    .size(11.0)
                    .color(t.text_muted),
            );
            return;
        }
        let now = ui.input(|input| input.time);
        let top = self.ws.stack.peek_undo().cloned();
        let mut jump = None;
        let mut undo = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for receipt in &receipts {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(receipt.label())
                                .size(11.0)
                                .strong()
                                .color(t.text_primary),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "{}  \u{00b7}  {}",
                                crate::receipts::format_elapsed(now, receipt.timestamp),
                                receipt.jump_to.display()
                            ))
                            .size(9.0)
                            .color(t.text_muted),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if receipt.still_undoable(top.as_ref()) && ui.small_button("Undo").clicked()
                        {
                            undo = true;
                            jump = None;
                        }
                        if ui.small_button("Jump").clicked() {
                            jump = Some(receipt.jump_to.clone());
                        }
                    });
                });
                ui.add_space(5.0);
                ui.separator();
                ui.add_space(5.0);
            }
        });
        if undo {
            self.ws.execute(crate::command::Command::Undo);
        } else if let Some(path) = jump {
            self.ws.active_panel().navigate_to(path);
        }
    }

    fn show_operations_errors(&mut self, ui: &mut egui::Ui) {
        let t = self.colors;
        let notices = self.operation_failures.notices().to_vec();
        if notices.is_empty() {
            ui.label(
                egui::RichText::new("No unresolved operation errors")
                    .size(11.0)
                    .color(t.text_muted),
            );
            return;
        }
        let can_undo = self.ws.preview_undo().is_some();
        let mut view = None;
        let mut retry = None;
        let mut rollback = None;
        let mut dismiss = None;
        let mut undo = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for notice in &notices {
                ui.label(
                    egui::RichText::new(notice.summary.label())
                        .size(11.0)
                        .strong()
                        .color(t.text_primary),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "{}: {}",
                        notice.title(),
                        notice
                            .errors
                            .first()
                            .map_or("Unknown failure", String::as_str)
                    ))
                    .size(10.0)
                    .color(t.accent_red),
                );
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("View").clicked() {
                        view = Some(notice.operation_id.clone());
                    }
                    if ui.small_button("Retry").clicked() {
                        retry = Some(notice.operation_id.clone());
                    }
                    if ui
                        .add_enabled(can_undo, egui::Button::new("Undo").small())
                        .on_disabled_hover_text("No reversible completed action")
                        .clicked()
                    {
                        undo = true;
                    }
                    let roll_back = crate::operation_view::CancellationAction::RollBack;
                    if ui
                        .small_button(roll_back.label())
                        .on_hover_text(roll_back.consequence())
                        .clicked()
                    {
                        rollback = Some(notice.operation_id.clone());
                    }
                    if ui.small_button("Dismiss").clicked() {
                        dismiss = Some(notice.operation_id.clone());
                    }
                });
                ui.add_space(7.0);
                ui.separator();
                ui.add_space(7.0);
            }
        });
        if let Some(operation_id) = view {
            let active = self.ws.active_transfer().is_some_and(|state| {
                crate::lock_util::recover(state).operation_id.as_ref() == Some(&operation_id)
            });
            if active {
                self.show_operations_center = false;
            } else {
                self.open_recovery_operation(operation_id, RecoveryDetail::Inspect);
            }
        }
        if let Some(operation_id) = retry {
            self.open_recovery_operation(operation_id, RecoveryDetail::Inspect);
        }
        if let Some(operation_id) = rollback {
            self.open_recovery_operation(operation_id, RecoveryDetail::Repair);
        }
        if undo {
            self.ws.execute(crate::command::Command::Undo);
        }
        if let Some(operation_id) = dismiss {
            self.operation_failures.dismiss(&operation_id);
        }
    }

    fn show_operations_recovery(&mut self, ui: &mut egui::Ui) {
        let t = self.colors;
        let operations = self.recovery.operations.clone();
        let mut refresh = false;
        let mut inspect = None;
        let mut rollback = None;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "{} interrupted  \u{00b7}  {} staging",
                    operations.len(),
                    self.recovery.orphans.len()
                ))
                .size(10.0)
                .color(t.text_muted),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_sized([24.0, 24.0], egui::Button::new("\u{21bb}"))
                    .on_hover_text("Refresh recovery inventory")
                    .clicked()
                {
                    refresh = true;
                }
            });
        });
        if self.recovery.scanning {
            ui.label(
                egui::RichText::new("Scanning operation journals...")
                    .size(10.0)
                    .color(t.accent),
            );
        }
        if let Some(error) = &self.recovery.error {
            ui.label(egui::RichText::new(error).size(10.0).color(t.accent_red));
        }
        ui.add_space(5.0);
        if operations.is_empty() && !self.recovery.scanning {
            ui.label(
                egui::RichText::new("No interrupted operations")
                    .size(11.0)
                    .color(t.text_muted),
            );
        } else {
            egui::ScrollArea::vertical().show(ui, |ui| {
                for record in &operations {
                    let kind = match record.kind {
                        TransferKind::Copy => "Copy",
                        TransferKind::Move => "Move",
                    };
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}  \u{00b7}  {}",
                                    record.status.label(),
                                    kind
                                ))
                                .size(11.0)
                                .strong()
                                .color(t.text_primary),
                            );
                            ui.label(
                                egui::RichText::new(record.target.display().to_string())
                                    .size(9.0)
                                    .color(t.text_muted),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let roll_back = crate::operation_view::CancellationAction::RollBack;
                            if ui
                                .small_button(roll_back.label())
                                .on_hover_text(roll_back.consequence())
                                .clicked()
                            {
                                rollback = Some(record.id.clone());
                            }
                            if ui.small_button("Inspect").clicked() {
                                inspect = Some(record.id.clone());
                            }
                        });
                    });
                    ui.add_space(5.0);
                    ui.separator();
                    ui.add_space(5.0);
                }
            });
        }
        if refresh {
            self.recovery.start_scan(&self.ws);
        }
        if let Some(operation_id) = inspect {
            self.open_recovery_operation(operation_id, RecoveryDetail::Inspect);
        }
        if let Some(operation_id) = rollback {
            self.open_recovery_operation(operation_id, RecoveryDetail::Repair);
        }
    }

    pub(crate) fn open_recovery_operation(
        &mut self,
        operation_id: crate::operation::OperationId,
        detail: RecoveryDetail,
    ) {
        if !self
            .recovery
            .operations
            .iter()
            .any(|record| record.id == operation_id)
            && !self.recovery.scanning
        {
            self.recovery.start_scan(&self.ws);
        }
        self.recovery.select(operation_id);
        self.recovery.detail = detail;
        self.recovery.open = true;
    }
}
