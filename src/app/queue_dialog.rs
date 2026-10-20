//! Unified, non-modal Operations panel for queue, history, failures, and
//! durable recovery. The full-height side surface keeps normal file work
//! available and replaces three competing floating windows.

use super::*;
use crate::opqueue::JobState;

fn mark_failure_notice_seen(
    seen: &mut std::collections::HashSet<crate::operation::TransferAttemptId>,
    attempt_id: crate::operation::TransferAttemptId,
) -> bool {
    seen.insert(attempt_id)
}

/// Build a live failure notice only for an attempt not yet ingested: the
/// active transfer stays finished-with-errors for many frames, and cloning
/// its error lists each frame only for ingest to drop them is waste.
fn unseen_failure_notice<T>(
    seen: &std::collections::HashSet<crate::operation::TransferAttemptId>,
    attempt_id: crate::operation::TransferAttemptId,
    build: impl FnOnce() -> Option<T>,
) -> Option<T> {
    if seen.contains(&attempt_id) {
        None
    } else {
        build()
    }
}

/// Undo button label for the top of the undo stack, naming it by its
/// inverse as the replay preview does, without building that preview (which
/// clones every path pair and stats each one).
fn undo_label_for(top: &crate::undo::Action) -> String {
    use crate::undo::Action;
    // `invert` keeps the item count; Trash/RestoreTrash and Gather/Ungather
    // swap verbs.
    let verb = match top {
        Action::Trash { .. } => "Restored",
        Action::RestoreTrash { .. } => "Deleted",
        Action::Gather { .. } => "Ungathered",
        Action::Ungather { .. } => "Gathered",
        other => other.verb(),
    };
    undo_last_label(verb, top.item_count())
}

fn terminal_failure_notice(
    report: crate::workspace::TransferTerminalReport,
    created_at_millis: u64,
) -> Option<crate::operation_view::FailureNotice> {
    if report.errors.is_empty() {
        return None;
    }
    Some(crate::operation_view::FailureNotice {
        attempt_id: report.attempt_id,
        operation_id: report.operation_id,
        summary: report.submitted,
        errors: report.errors,
        failures: report.failures,
        created_at_millis,
    })
}

/// Label for the failure notice's Undo button: the action undo would revert.
fn undo_last_label(verb: &str, items: usize) -> String {
    let noun = if items == 1 { "item" } else { "items" };
    format!("Undo last: {verb} {items} {noun}")
}

/// Tab to show when a new failure arrives: an open panel keeps the tab the
/// user chose (the Errors badge already counts it); a closed one opens on
/// Errors.
fn tab_for_new_failure(open: bool, current: OperationsTab) -> OperationsTab {
    if open { current } else { OperationsTab::Errors }
}

/// Accessible name for a per-row button: the action plus which row it acts
/// on, so a list of identical "Dismiss" buttons can be told apart.
fn row_action_label(action: &str, row: &str) -> String {
    format!("{action}: {row}")
}

/// Give `response` the accessible name `label` (glyph and repeated
/// buttons otherwise announce only their visible text).
fn with_accessible_label(response: egui::Response, label: &str) -> egui::Response {
    let enabled = response.enabled();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    response
}

/// Status line for a running job whose transfer already finished but stays
/// in the queue until its details are acknowledged.
const fn finished_job_detail(has_errors: bool) -> &'static str {
    if has_errors {
        "Finished with errors; press OK above to continue"
    } else {
        "Finished; press OK above to continue"
    }
}

/// Whether a pending job at `index` among `count` pending jobs can move
/// `(earlier, later)`; the ends cannot move further.
const fn pending_moves(index: usize, count: usize) -> (bool, bool) {
    (index > 0, index + 1 < count)
}

/// Empty History tab text: an empty search means nothing has finished yet.
fn history_empty_text(query: &str) -> &'static str {
    if query.trim().is_empty() {
        "No completed operations yet"
    } else {
        "No matching operations"
    }
}

impl App {
    pub(crate) fn toggle_queue_panel(&mut self) {
        if self.show_operations_center && self.operations_tab == OperationsTab::Queue {
            self.show_operations_center = false;
        } else {
            self.show_operations_center = true;
            self.operations_tab = OperationsTab::Queue;
        }
    }

    pub(crate) const fn open_operation_history(&mut self) {
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
        let failed = self.ws.active_transfer_view().and_then(|active| {
            unseen_failure_notice(&self.failure_notice_seen, active.attempt_id, || {
                let progress = crate::lock_util::recover(&active.progress);
                if !progress.finished || progress.errors.is_empty() {
                    return None;
                }
                Some(crate::operation_view::FailureNotice {
                    attempt_id: active.attempt_id,
                    operation_id: active.operation_id,
                    summary: active.submitted,
                    errors: progress.errors.clone(),
                    failures: progress.failures.clone(),
                    created_at_millis: super::frame_millis(ctx),
                })
            })
        });
        if let Some(notice) = failed {
            self.ingest_failure_notice(notice);
        }
    }

    pub(crate) fn capture_terminal_report(
        &mut self,
        report: crate::workspace::TransferTerminalReport,
        created_at_millis: u64,
    ) {
        if let Some(notice) = terminal_failure_notice(report, created_at_millis) {
            self.ingest_failure_notice(notice);
        }
    }

    pub(crate) fn ingest_failure_notice(&mut self, notice: crate::operation_view::FailureNotice) {
        if !mark_failure_notice_seen(&mut self.failure_notice_seen, notice.attempt_id) {
            return;
        }
        self.assistive_timeline.note_failure(
            Some(notice.operation_id.clone()),
            notice.title(),
            notice.created_at_millis,
        );
        self.operation_failures.upsert(notice);
        self.operations_tab = tab_for_new_failure(self.show_operations_center, self.operations_tab);
        self.show_operations_center = true;
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
                let timeline_message = self.assistive_timeline.current().to_string();
                let timeline_events = self.assistive_timeline.events().count();
                if !timeline_message.is_empty() {
                    let live = ui.label(
                        egui::RichText::new(if timeline_events > 1 {
                            format!("{timeline_message} · {timeline_events} events")
                        } else {
                            timeline_message.clone()
                        })
                        .size(10.0)
                        .color(t.text_muted),
                    );
                    ui.ctx().accesskit_node_builder(live.id, |node| {
                        node.set_role(egui::accesskit::Role::Status);
                        node.set_live(egui::accesskit::Live::Polite);
                    });
                }
                if let Some(active) = self.ws.active_transfer_view() {
                    let progress = crate::lock_util::recover(&active.progress);
                    let now_millis = super::frame_millis(ui.ctx());
                    if let Some(reason) = progress.pause_reason.as_ref() {
                        self.assistive_timeline.note_pause(
                            Some(active.operation_id.clone()),
                            reason.label(),
                            now_millis,
                        );
                    } else if !progress.finished {
                        self.assistive_timeline.note_phase(
                            active.operation_id.clone(),
                            progress.phase,
                            active.submitted.label(),
                            now_millis,
                        );
                    }
                }
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
                        let close_button = ui.add_sized(
                            [24.0, 24.0],
                            egui::Button::new(egui::RichText::new("\u{00d7}").color(t.text_muted))
                                .frame(false),
                        );
                        if with_accessible_label(close_button, "Close Operations panel")
                            .on_hover_text("Close Operations panel")
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
                                .corner_radius(CornerRadius::ZERO)
                                .selected(selected),
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
        if self.ws.active_transfer_view().is_some() {
            self.show_transfer_details(ui);
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(5.0);
        }
        let t = self.colors;
        let rows = self.ws.queue_snapshot();
        let running = self.ws.active_transfer_view().map(|active| {
            let progress = crate::lock_util::recover(&active.progress);
            let finished = progress
                .finished
                .then(|| !progress.errors.is_empty() || !progress.failures.is_empty());
            (progress.phase, progress.pause_reason.clone(), finished)
        });
        let running_finished = running.as_ref().is_some_and(|(_, _, done)| done.is_some());
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
        let mut pending_index = 0;
        egui::ScrollArea::vertical()
            .id_salt("ops_queue")
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
                                    |(phase, pause_reason, finished)| {
                                        if let Some(has_errors) = finished {
                                            return finished_job_detail(*has_errors).to_string();
                                        }
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
                            // Glyph buttons get the action plus the job as
                            // their accessible name.
                            let icon = |ui: &mut egui::Ui,
                                        enabled: bool,
                                        glyph: &str,
                                        action: &str,
                                        hint: &str| {
                                let response = ui.add_enabled_ui(enabled, |ui| {
                                    ui.add_sized([24.0, 24.0], egui::Button::new(glyph))
                                });
                                with_accessible_label(
                                    response.inner,
                                    &row_action_label(action, &row.label),
                                )
                                .on_hover_text(hint)
                                .clicked()
                            };
                            match row.state {
                                JobState::Pending => {
                                    if pending_count > 1 {
                                        let (earlier, later) =
                                            pending_moves(pending_index, pending_count);
                                        if icon(ui, later, "\u{25bc}", "Move later", "Move later") {
                                            move_down = Some(row.id);
                                        }
                                        if icon(
                                            ui,
                                            earlier,
                                            "\u{25b2}",
                                            "Move earlier",
                                            "Move earlier",
                                        ) {
                                            move_up = Some(row.id);
                                        }
                                        if icon(ui, earlier, "\u{23eb}", "Run next", "Run next") {
                                            promote = Some(row.id);
                                        }
                                    }
                                    pending_index += 1;
                                    if icon(
                                        ui,
                                        true,
                                        "\u{23f8}",
                                        "Pause",
                                        "Pause pending operation",
                                    ) {
                                        pause = Some(row.id);
                                    }
                                }
                                JobState::Paused
                                    if icon(ui, true, "\u{25b6}", "Resume", "Resume") =>
                                {
                                    resume = Some(row.id);
                                }
                                _ => {}
                            }
                            // Cancel is added last so Tab reaches the safe
                            // actions first; a finished job waiting for OK
                            // has nothing left to cancel.
                            let finished = row.state == JobState::Running && running_finished;
                            if !row.state.is_terminal()
                                && !finished
                                && icon(
                                    ui,
                                    true,
                                    "\u{00d7}",
                                    "Cancel",
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
            let repaint = ctx.clone();
            if let Err(reason) = self.ws.queue_resume(id, move || repaint.request_repaint()) {
                self.toast(&ctx, crate::toasts::ToastKind::Error, reason, false);
            }
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
        let receipts = self.receipts.search(&self.operations_search);
        if receipts.is_empty() {
            ui.label(
                egui::RichText::new(history_empty_text(&self.operations_search))
                    .size(11.0)
                    .color(t.text_muted),
            );
            return;
        }
        let now = ui.input(|input| input.time);
        let top = self.ws.top_undo_entry();
        let mut jump = None;
        let mut undo = false;
        egui::ScrollArea::vertical()
            .id_salt("ops_history")
            .show(ui, |ui| {
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
                            if receipt.still_undoable(top)
                                && with_accessible_label(
                                    ui.small_button("Undo"),
                                    &row_action_label("Undo", &receipt.label()),
                                )
                                .clicked()
                            {
                                undo = true;
                                jump = None;
                            }
                            if with_accessible_label(
                                ui.small_button("Jump"),
                                &format!("Jump to {}", receipt.jump_to.display()),
                            )
                            .clicked()
                            {
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
        let notices = self.operation_failures.notices();
        if notices.is_empty() {
            ui.label(
                egui::RichText::new("No unresolved operation errors")
                    .size(11.0)
                    .color(t.text_muted),
            );
            return;
        }
        // Undo is global (the top of the undo stack), not tied to a notice,
        // so the button names the action it would revert.
        let undo_label = self.ws.top_undo_action().map(undo_label_for);
        let active_operation = self
            .ws
            .active_transfer_view()
            .map(|active| active.operation_id.clone());
        let mut view = None;
        let mut recover = None;
        let mut rollback = None;
        let mut dismiss = None;
        let mut dismiss_all = false;
        let mut undo = false;
        if notices.len() > 1 && ui.small_button("Dismiss all").clicked() {
            dismiss_all = true;
        }
        egui::ScrollArea::vertical()
            .id_salt("ops_errors")
            .show(ui, |ui| {
            for notice in notices {
                let row = notice.summary.label();
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
                    if with_accessible_label(ui.small_button("View"), &row_action_label("View", &row))
                        .clicked()
                    {
                        view = Some(notice.operation_id.clone());
                    }
                    // View already opens the recovery inspector for a
                    // finished operation; only the active one needs both.
                    if active_operation.as_ref() == Some(&notice.operation_id)
                        && with_accessible_label(
                            ui.small_button("Recover\u{2026}"),
                            &row_action_label("Recover", &row),
                        )
                        .on_hover_text("Open the recovery inspector for this operation")
                        .clicked()
                    {
                        recover = Some(notice.operation_id.clone());
                    }
                    if ui
                        .add_enabled(
                            undo_label.is_some(),
                            egui::Button::new(undo_label.as_deref().unwrap_or("Undo")).small(),
                        )
                        .on_hover_text(
                            "Reverts the most recent completed action, which may not be this operation",
                        )
                        .on_disabled_hover_text("No reversible completed action")
                        .clicked()
                    {
                        undo = true;
                    }
                    let roll_back = crate::operation_view::CancellationAction::RollBack;
                    if with_accessible_label(
                        ui.small_button(roll_back.label()),
                        &row_action_label(roll_back.label(), &row),
                    )
                    .on_hover_text(roll_back.consequence())
                    .clicked()
                    {
                        rollback = Some(notice.operation_id.clone());
                    }
                    if with_accessible_label(
                        ui.small_button("Dismiss"),
                        &row_action_label("Dismiss", &row),
                    )
                    .clicked()
                    {
                        dismiss = Some(notice.attempt_id);
                    }
                });
                ui.add_space(7.0);
                ui.separator();
                ui.add_space(7.0);
            }
        });
        if let Some(operation_id) = view {
            let active = self
                .ws
                .active_transfer_view()
                .is_some_and(|active| active.operation_id == operation_id);
            if active {
                // The live transfer's details render on the Queue tab.
                self.operations_tab = OperationsTab::Queue;
            } else {
                self.open_recovery_operation(operation_id, RecoveryDetail::Inspect);
            }
        }
        if let Some(operation_id) = recover {
            self.open_recovery_operation(operation_id, RecoveryDetail::Inspect);
        }
        if let Some(operation_id) = rollback {
            self.open_recovery_operation(operation_id, RecoveryDetail::Repair);
        }
        if undo {
            self.ws.execute(crate::command::Command::Undo);
        }
        if let Some(attempt_id) = dismiss {
            self.operation_failures.dismiss(attempt_id);
        }
        if dismiss_all {
            let attempts = self
                .operation_failures
                .notices()
                .iter()
                .map(|notice| notice.attempt_id)
                .collect::<Vec<_>>();
            for attempt_id in attempts {
                self.operation_failures.dismiss(attempt_id);
            }
        }
    }

    fn show_operations_recovery(&mut self, ui: &mut egui::Ui) {
        if self.ws.safe_state.is_some() {
            self.show_safe_state_details(ui);
            ui.add_space(8.0);
        }
        self.show_recovery_center(ui);
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
        self.recovery.request(operation_id);
        if !self.recovery.scanning {
            // Already listed: no scan is waiting to honour the request.
            self.recovery.requested = None;
        }
        self.recovery.detail = detail;
        self.show_operations_center = true;
        self.operations_tab = OperationsTab::Recovery;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OperationsTab, finished_job_detail, history_empty_text, mark_failure_notice_seen,
        pending_moves, row_action_label, tab_for_new_failure, terminal_failure_notice,
        undo_label_for, undo_last_label, unseen_failure_notice,
    };

    #[test]
    fn new_failure_keeps_the_open_panel_on_the_users_tab() {
        assert_eq!(
            tab_for_new_failure(true, OperationsTab::History),
            OperationsTab::History
        );
        assert_eq!(
            tab_for_new_failure(false, OperationsTab::History),
            OperationsTab::Errors
        );
    }

    #[test]
    fn queue_ends_cannot_move_further() {
        assert_eq!(pending_moves(0, 3), (false, true));
        assert_eq!(pending_moves(1, 3), (true, true));
        assert_eq!(pending_moves(2, 3), (true, false));
    }

    #[test]
    fn finished_job_does_not_read_as_running() {
        assert!(finished_job_detail(true).starts_with("Finished with errors"));
        assert!(!finished_job_detail(false).contains("phase"));
    }

    #[test]
    fn empty_history_without_a_query_is_not_a_filter_miss() {
        assert_eq!(history_empty_text(""), "No completed operations yet");
        assert_eq!(history_empty_text("  "), "No completed operations yet");
        assert_eq!(history_empty_text("copy"), "No matching operations");
    }

    #[test]
    fn row_buttons_name_the_row_they_act_on() {
        assert_eq!(
            row_action_label("Dismiss", "Copy 2 items"),
            "Dismiss: Copy 2 items"
        );
    }
    use std::path::PathBuf;

    #[test]
    fn undo_label_matches_the_replay_preview_without_building_it() {
        use crate::undo::Action;
        let pair = (PathBuf::from("/a/x"), PathBuf::from("/b/x"));
        let actions = [
            Action::Move {
                pairs: vec![pair.clone(), pair.clone()],
            },
            Action::BatchRename {
                dir: PathBuf::from("/a"),
                pairs: vec![("x".into(), "y".into())],
                folding: crate::rename_order::NameFolding::CaseFolded,
            },
            Action::Rename {
                from: pair.0.clone(),
                to: pair.1.clone(),
            },
            Action::Gather {
                folder: PathBuf::from("/a/g"),
                pairs: vec![pair.clone()],
            },
            Action::Ungather {
                folder: PathBuf::from("/a/g"),
                pairs: vec![pair.clone()],
            },
            Action::Trash { items: Vec::new() },
            Action::RestoreTrash { items: Vec::new() },
        ];
        for action in &actions {
            let inverse = crate::undo::invert(action).expect("every action inverts");
            assert_eq!(
                undo_label_for(action),
                undo_last_label(inverse.verb(), inverse.item_count()),
                "{action:?}"
            );
        }
    }

    #[test]
    fn seen_attempt_does_not_rebuild_its_failure_notice() {
        let attempt = crate::operation::TransferAttemptId::new();
        let mut seen = std::collections::HashSet::new();
        assert_eq!(unseen_failure_notice(&seen, attempt, || Some(1)), Some(1));
        seen.insert(attempt);
        let mut built = false;
        let notice = unseen_failure_notice(&seen, attempt, || {
            built = true;
            Some(2)
        });
        assert_eq!(notice, None);
        assert!(!built, "errors must not be cloned for an ingested attempt");
    }

    #[test]
    fn failure_notice_undo_names_the_action_it_reverts() {
        assert_eq!(undo_last_label("Moved", 3), "Undo last: Moved 3 items");
        assert_eq!(undo_last_label("Renamed", 1), "Undo last: Renamed 1 item");
    }

    #[test]
    fn auto_retired_terminal_report_becomes_a_complete_failure_notice() {
        let operation_id = crate::operation::OperationId("auto-retired".to_string());
        let attempt_id = crate::operation::TransferAttemptId::new();
        let failure = crate::operation::ClassifiedFailure::message(
            crate::operation::FailureClass::Blocked,
            None,
            "stopped with an error",
        );
        let report = crate::workspace::TransferTerminalReport {
            attempt_id,
            operation_id: operation_id.clone(),
            submitted: crate::operation_view::SubmittedSummary::capture(
                "Copy",
                &[std::path::PathBuf::from("/source/file")],
                std::path::PathBuf::from("/target"),
            ),
            terminal: crate::workspace::TransferTerminalState::Stopped,
            errors: vec!["stopped with an error".to_string()],
            failures: vec![failure.clone()],
        };

        let notice = terminal_failure_notice(report, 42).expect("failure notice");

        assert_eq!(notice.attempt_id, attempt_id);
        assert_eq!(notice.operation_id, operation_id);
        assert_eq!(notice.errors, ["stopped with an error"]);
        assert_eq!(notice.failures, [failure]);
        assert_eq!(notice.created_at_millis, 42);
    }

    #[test]
    fn live_notice_capture_is_once_per_attempt_not_once_per_operation() {
        let first_attempt = crate::operation::TransferAttemptId::new();
        let retry_attempt = crate::operation::TransferAttemptId::new();
        let mut seen = std::collections::HashSet::new();

        assert!(mark_failure_notice_seen(&mut seen, first_attempt));
        assert!(!mark_failure_notice_seen(&mut seen, first_attempt));
        assert!(mark_failure_notice_seen(&mut seen, retry_attempt));
        assert_eq!(seen.len(), 2);
    }
}
