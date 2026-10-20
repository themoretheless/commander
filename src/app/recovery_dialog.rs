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
        self.version_usage_label = None;
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
        // An explicitly requested operation that is missing from the
        // inventory stays selected so the panel can say it no longer needs
        // recovery, rather than silently showing another operation's detail.
        // A stale selection (resolved since it was picked) falls back to the
        // first operation.
        let requested = self.requested.take();
        if !self.selected_in_inventory() && (self.selected.is_none() || self.selected != requested)
        {
            self.selected = self.operations.first().map(|record| record.id.clone());
        }
        self.refresh_selected();
    }

    /// Select an operation the user opened directly; unlike a list click,
    /// it stays selected through the next scan even if that scan omits it.
    pub(crate) fn request(&mut self, operation_id: crate::operation::OperationId) {
        self.select(operation_id.clone());
        self.requested = Some(operation_id);
    }

    /// A click in the operation list. Re-clicking the selected operation
    /// keeps its tab and loaded repair plan.
    fn pick(&mut self, operation_id: crate::operation::OperationId) {
        if self.selected.as_ref() == Some(&operation_id) {
            self.requested = None;
            return;
        }
        self.select(operation_id);
    }

    pub(crate) fn select(&mut self, operation_id: crate::operation::OperationId) {
        self.requested = None;
        self.selected = Some(operation_id);
        self.detail = RecoveryDetail::Inspect;
        self.outcome = None;
        // An error from the previous operation's load, rollback or resume
        // does not describe this one.
        self.error = None;
        self.refresh_selected();
    }

    fn refresh_selected(&mut self) {
        self.repair_plan = None;
        self.versions.clear();
        self.repair_loaded = false;
        // Dropping the receiver discards a load for the previous selection.
        self.repair_rx = None;
    }

    fn selected_in_inventory(&self) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|selected| self.operations.iter().any(|record| &record.id == selected))
    }

    fn load_repair(&mut self) {
        if self.repair_loaded {
            return;
        }
        self.repair_loaded = true;
        if !self.selected_in_inventory() {
            return;
        }
        let Some(operation_id) = self.selected.clone() else {
            return;
        };
        // Both read journal and version manifests from disk, which stalls
        // the UI thread on large journals.
        self.spawn_repair_load(operation_id, |operation_id| RepairLoadResult {
            plan: crate::operation_journal::repair_plan(&operation_id),
            versions: crate::version_store::records_for(&operation_id),
            operation_id,
        });
    }

    fn spawn_repair_load(
        &mut self,
        operation_id: crate::operation::OperationId,
        load: impl FnOnce(crate::operation::OperationId) -> RepairLoadResult + Send + 'static,
    ) {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(load(operation_id));
        });
        self.repair_rx = Some(receiver);
    }

    /// Apply a finished repair-plan load; runs every frame.
    pub(crate) fn poll_repair(&mut self) {
        let Some(receiver) = &self.repair_rx else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.repair_rx = None;
                self.error = Some("Repair plan load stopped unexpectedly".to_string());
                return;
            }
        };
        self.repair_rx = None;
        if self.selected.as_ref() != Some(&result.operation_id) {
            return;
        }
        match result.plan {
            Ok(plan) => self.repair_plan = Some(plan),
            Err(error) => self.error = Some(error),
        }
        match result.versions {
            Ok(versions) => self.versions = versions,
            Err(error) => {
                self.versions.clear();
                // A plan error explains more; keep it if both failed.
                self.error.get_or_insert(error);
            }
        }
    }
}

/// How long the version-store usage line is reused before the manifest is
/// read again, so a running transfer's saved versions appear within seconds
/// without reading the manifest every frame.
const VERSION_USAGE_REFRESH: std::time::Duration = std::time::Duration::from_secs(5);

/// Return the cached usage line, re-reading it via `read` when it is missing
/// or older than `VERSION_USAGE_REFRESH`.
fn cached_version_usage(
    slot: &mut Option<(std::time::Instant, String)>,
    now: std::time::Instant,
    read: impl FnOnce() -> String,
) -> String {
    match slot {
        Some((read_at, label))
            if now.saturating_duration_since(*read_at) < VERSION_USAGE_REFRESH =>
        {
            label.clone()
        }
        _ => {
            let label = read();
            *slot = Some((now, label.clone()));
            label
        }
    }
}

/// Header summary. Only recoverable records count as interrupted; the
/// inventory also keeps settled operations whose cleanup data remains.
fn recovery_summary(
    operations: &[crate::operation_journal::OperationRecord],
    orphans: usize,
) -> String {
    let plural = |count: usize| if count == 1 { "" } else { "s" };
    let interrupted = operations
        .iter()
        .filter(|record| record.status.recoverable())
        .count();
    let settled = operations.len() - interrupted;
    let mut summary = format!("{interrupted} interrupted operation{}", plural(interrupted));
    if settled > 0 {
        summary.push_str(&format!(
            "  \u{00b7}  {settled} with leftover recovery data"
        ));
    }
    summary.push_str(&format!(
        "  \u{00b7}  {orphans} staging item{}",
        plural(orphans)
    ));
    summary
}

/// Why Resume is unavailable for `record`, shown as the disabled hover text.
fn resume_block_reason(
    record: &crate::operation_journal::OperationRecord,
    scanning: bool,
    transfer_busy: bool,
    preparing: bool,
) -> Option<&'static str> {
    if scanning {
        Some("Wait for the recovery scan to finish")
    } else if preparing {
        Some("Wait for recovery preparation to finish")
    } else if !record.status.recoverable() {
        Some("Nothing left to resume")
    } else if transfer_busy {
        Some("Wait for the transfer queue to finish")
    } else {
        None
    }
}

fn resume_message(count: usize) -> String {
    match count {
        0 => "Finalizing verified operation".to_string(),
        1 => "Resuming 1 item".to_string(),
        count => format!("Resuming {count} items"),
    }
}

fn rollback_outcome(completed: usize, remaining: usize) -> RecoveryOutcome {
    let mut message = format!(
        "Rollback completed {completed} step{}",
        if completed == 1 { "" } else { "s" }
    );
    match remaining {
        0 => {}
        1 => message.push_str("; 1 item needs review"),
        remaining => message.push_str(&format!("; {remaining} items need review")),
    }
    RecoveryOutcome {
        message,
        needs_review: remaining > 0,
    }
}

fn clean_orphan_label(path: &std::path::Path) -> String {
    format!("Delete leftover staging data at {}", path.display())
}

fn short_id(id: &str) -> String {
    id.chars().take(18).collect()
}

const fn operation_kind(kind: TransferKind) -> &'static str {
    match kind {
        TransferKind::Copy => "Copy",
        TransferKind::Move => "Move",
    }
}

fn operation_time(seconds: u64) -> String {
    // A corrupt journal can hold any u64; casting would wrap huge values to
    // negative seconds and show a 1969 date.
    i64::try_from(seconds)
        .ok()
        .and_then(|seconds| chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0))
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%b %e, %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "Unknown time".to_string())
}

const fn status_color(
    status: crate::operation_journal::OperationStatus,
    t: ThemeColors,
) -> Color32 {
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
                if response.clicked() && !active {
                    *selected = Some(record.id.clone());
                }
                response.on_hover_text(format!("{}\n{}", record.id.0, record.target.display()));
                ui.add_space(3.0);
            }
        });
}

/// Most journal steps or repair items laid out at once. `ScrollArea::show`
/// lays out every row each frame, so operations with tens of thousands of
/// steps would otherwise stall the panel.
const RECOVERY_ROW_LIMIT: usize = 500;

/// Up to `limit` items in their original order. When there are more, settled
/// items give way first so the ones needing attention are never hidden.
fn capped_rows<T>(items: &[T], limit: usize, settled: impl Fn(&T) -> bool) -> Vec<&T> {
    if items.len() <= limit {
        return items.iter().collect();
    }
    let unsettled = items.iter().filter(|item| !settled(item)).count();
    let mut settled_budget = limit.saturating_sub(unsettled);
    let mut unsettled_budget = limit;
    items
        .iter()
        .filter(|item| {
            let budget = if settled(item) {
                &mut settled_budget
            } else {
                &mut unsettled_budget
            };
            let shown = *budget > 0;
            *budget = budget.saturating_sub(1);
            shown
        })
        .collect()
}

/// Repair plan rows as (item, done): completed items first, then remaining
/// ones, capped at `limit`. Remaining items are kept over the oldest completed
/// ones, which are dropped first.
fn repair_rows(
    plan: &crate::operation_journal::RepairPlan,
    limit: usize,
) -> Vec<(&crate::operation_journal::RepairItem, bool)> {
    let remaining = plan.remaining.len().min(limit);
    let completed = plan.completed.len().min(limit - remaining);
    plan.completed[plan.completed.len() - completed..]
        .iter()
        .map(|item| (item, true))
        .chain(plan.remaining[..remaining].iter().map(|item| (item, false)))
        .collect()
}

fn hidden_rows_label(ui: &mut egui::Ui, hidden: usize, noun: &str, t: ThemeColors) {
    if hidden > 0 {
        ui.label(
            egui::RichText::new(format!("{hidden} more {noun} not shown"))
                .size(10.0)
                .color(t.text_muted),
        );
    }
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
            use crate::operation_journal::StepStatus;
            let steps = capped_rows(&record.steps, RECOVERY_ROW_LIMIT, |step| {
                matches!(
                    step.status,
                    StepStatus::Completed | StepStatus::Skipped | StepStatus::RolledBack
                )
            });
            for step in &steps {
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
                            egui::RichText::new(format!("\u{00bb} {}", effect.display()))
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
            hidden_rows_label(ui, record.steps.len() - steps.len(), "step(s)", t);
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
        if state.repair_rx.is_some() || !state.repair_loaded {
            ui.label(
                egui::RichText::new("Loading repair plan\u{2026}")
                    .size(11.0)
                    .color(t.text_muted),
            );
        } else {
            ui.label(
                egui::RichText::new("Repair plan is unavailable")
                    .size(11.0)
                    .color(t.accent_red),
            );
        }
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
    if manual > 0 {
        ui.label(
            egui::RichText::new(
                "Rollback leaves MANUAL items alone: follow each item's note and fix the file yourself.",
            )
            .size(10.0)
            .color(t.accent_warning),
        );
    }
    ui.add_space(6.0);
    egui::ScrollArea::vertical()
        .max_height(285.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            let rows = repair_rows(plan, RECOVERY_ROW_LIMIT);
            hidden_rows_label(
                ui,
                plan.completed.len() + plan.remaining.len() - rows.len(),
                "item(s)",
                t,
            );
            for (item, done) in rows {
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
            super::dialog_button(
                t,
                super::ButtonRole::Danger,
                egui::RichText::new("Apply verified rollback").size(12.0),
            ),
        )
        .clicked()
    {
        *apply = Some(record.id.clone());
    }
}

impl App {
    fn push_recovery_toast(&mut self, ctx: &egui::Context, message: String, error: bool) {
        self.toast(
            ctx,
            if error {
                crate::toasts::ToastKind::Error
            } else {
                crate::toasts::ToastKind::Success
            },
            message,
            false,
        );
    }

    /// Advance the background recovery scan; runs every frame.
    pub(crate) fn poll_recovery(&mut self, ctx: &egui::Context) {
        self.recovery.poll_scan();
        self.recovery.poll_repair();
        let resume_result =
            self.recovery
                .resume_rx
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(result) => Some(Ok(result)),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(())),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                });
        if let Some(result) = resume_result {
            self.recovery.resume_rx = None;
            let operation_id = self.recovery.preparing_resume.take();
            match (result, operation_id) {
                (Ok(result), Some(operation_id)) if result.operation_id == operation_id => {
                    let repaint = ctx.clone();
                    match self
                        .ws
                        .finish_resume_recovery(&operation_id, result.spec, move || {
                            repaint.request_repaint()
                        }) {
                        Ok(count) => {
                            let message = resume_message(count);
                            self.recovery.rescan_after_resume = true;
                            self.assistive_timeline.note_recovery(
                                Some(operation_id),
                                message.clone(),
                                super::frame_millis(ctx),
                            );
                            self.push_recovery_toast(ctx, message, false);
                        }
                        Err(error) => {
                            self.recovery.outcome = None;
                            self.recovery.error = Some(error);
                        }
                    }
                }
                (_, Some(operation_id)) => {
                    self.ws.cancel_resume_recovery(&operation_id);
                    self.recovery.outcome = None;
                    self.recovery.error = Some(
                        "Recovery preparation stopped before returning a matching result"
                            .to_string(),
                    );
                }
                _ => {
                    self.recovery.outcome = None;
                    self.recovery.error =
                        Some("Recovery preparation stopped unexpectedly".to_string());
                }
            }
        }
        if self.recovery.rescan_after_resume
            && !self.recovery.scanning
            && !self.ws.has_unfinished_transfer_work()
        {
            self.recovery.rescan_after_resume = false;
            self.recovery.start_scan(&self.ws);
        }
        if self.recovery.scanning
            || self.recovery.repair_rx.is_some()
            || self.recovery.resume_rx.is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// The Recovery tab of the Operations panel: interrupted operations with
    /// inspect, repair plan, resume and rollback, plus staging cleanup.
    pub(crate) fn show_recovery_center(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let mut state = std::mem::take(&mut self.recovery);
        let t = self.colors;
        let mut refresh = false;
        let mut selected = None;
        let mut resume = None;
        let mut rollback = None;
        let mut clean_orphan = None;
        // The tab selectors edit copies so the panel can borrow `state`
        // (and the selected record with all its steps) instead of cloning.
        let mut section = state.section;
        let mut detail = state.detail;
        let usage_label = cached_version_usage(
            &mut state.version_usage_label,
            std::time::Instant::now(),
            || {
                let usage = crate::version_store::store_usage();
                if usage.over_quota() {
                    format!("Version store {} · over quota", usage.label())
                } else {
                    format!("Version store {}", usage.label())
                }
            },
        );
        let transfer_busy = self.ws.has_unfinished_transfer_work();
        let preparing_resume = state.resume_rx.is_some();
        let selected_record = state
            .selected
            .as_ref()
            .and_then(|id| state.operations.iter().find(|record| &record.id == id));

        // In the narrow bottom placement the panel is short; scroll the whole
        // body so the details and rollback button stay reachable.
        egui::ScrollArea::vertical()
            .id_salt("recovery_center_body")
            .auto_shrink([false, false])
            .show(ui, |ui| {
        ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("Recovery center")
                                .size(14.0)
                                .strong()
                                .color(t.text_primary),
                        );
                        ui.label(
                            egui::RichText::new(recovery_summary(
                                &state.operations,
                                state.orphans.len(),
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
                        } else if preparing_resume {
                            ui.label(
                                egui::RichText::new("Preparing recovery in background")
                                    .size(9.0)
                                    .color(t.text_secondary),
                            );
                        }
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let response = ui
                            .add_enabled(!state.scanning, egui::Button::new("\u{21bb}"))
                            .on_hover_text("Refresh");
                        // The glyph alone is not a usable accessible name.
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                !state.scanning,
                                "Refresh",
                            )
                        });
                        if response.clicked() {
                            refresh = true;
                        }
                    });
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.selectable_value(
                        &mut section,
                        RecoverySection::Operations,
                        format!("Operations {}", state.operations.len()),
                    );
                    ui.selectable_value(
                        &mut section,
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
                        egui::RichText::new(&outcome.message)
                            .size(11.0)
                            .color(if outcome.needs_review {
                                t.accent_warning
                            } else {
                                t.accent
                            }),
                    );
                    ui.add_space(5.0);
                }
                ui.label(
                    egui::RichText::new(usage_label)
                    .size(10.0)
                    .color(t.text_muted),
                );
                ui.add_space(4.0);

                match section {
                    RecoverySection::Operations => {
                        ui.vertical(|ui| {
                            show_operation_list(ui, &state, t, &mut selected);
                            ui.separator();
                            ui.vertical(|ui| {
                                if let Some(record) = selected_record {
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
                                                let blocked = resume_block_reason(
                                                    record,
                                                    state.scanning,
                                                    transfer_busy,
                                                    preparing_resume,
                                                );
                                                if ui
                                                    .add_enabled(
                                                        blocked.is_none(),
                                                        egui::Button::new("Resume")
                                                            .corner_radius(CornerRadius::ZERO),
                                                    )
                                                    .on_disabled_hover_text(
                                                        blocked.unwrap_or_default(),
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
                                            &mut detail,
                                            RecoveryDetail::Inspect,
                                            "Inspect",
                                        );
                                        ui.selectable_value(
                                            &mut detail,
                                            RecoveryDetail::Repair,
                                            "Repair plan",
                                        );
                                    });
                                    ui.add_space(5.0);
                                    match detail {
                                        RecoveryDetail::Inspect => show_inspect(ui, record, t),
                                        RecoveryDetail::Repair => {
                                            show_repair(ui, &state, record, t, &mut rollback)
                                        }
                                    }
                                } else {
                                    ui.label(
                                        egui::RichText::new(if state.selected.is_none() {
                                            "Select an operation to inspect"
                                        } else if state.scanning {
                                            "Scanning operations..."
                                        } else {
                                            "Operation no longer needs recovery"
                                        })
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
                                    "No leftover temporary files from interrupted operations"
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
                                                            "{}  \u{00b7}  identity verified at scan",
                                                            crate::panel::format_size(
                                                                orphan.identity.size
                                                            )
                                                        ))
                                                        .size(9.0)
                                                        .color(t.text_muted),
                                                    );
                                                });
                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        let response = ui
                                                            .add_enabled(
                                                                !state.scanning,
                                                                egui::Button::new("Clean")
                                                                    .corner_radius(
                                                                        CornerRadius::ZERO,
                                                                    ),
                                                            )
                                                            .on_hover_text(
                                                                "Permanently delete this leftover staging data",
                                                            );
                                                        // Name the path: every row's button reads "Clean".
                                                        let label = clean_orphan_label(&orphan.path);
                                                        response.widget_info(|| {
                                                            egui::WidgetInfo::labeled(
                                                                egui::WidgetType::Button,
                                                                !state.scanning,
                                                                &label,
                                                            )
                                                        });
                                                        if response.clicked() {
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
            });
            });

        state.section = section;
        state.detail = detail;
        if let Some(operation_id) = selected {
            state.pick(operation_id);
        }
        if refresh {
            // A manual refresh starts over; the last result no longer applies.
            state.outcome = None;
            state.start_scan(&self.ws);
        }
        if let Some(index) = clean_orphan
            && let Some(orphan) = state.orphans.get(index).cloned()
        {
            match self.ws.clean_recovery_orphan(&orphan) {
                Ok(()) => {
                    state.outcome = Some(RecoveryOutcome {
                        message: format!(
                            "Deleted leftover staging data at {}",
                            orphan.path.display()
                        ),
                        needs_review: false,
                    });
                    state.start_scan(&self.ws);
                }
                Err(error) => {
                    state.outcome = None;
                    state.error = Some(error);
                }
            }
        }
        if let Some(operation_id) = rollback {
            match self.ws.rollback_recovery(&operation_id) {
                Ok(plan) => {
                    let now_millis = super::frame_millis(ctx);
                    self.assistive_timeline.note_recovery(
                        Some(operation_id.clone()),
                        format!(
                            "Rollback completed {} step(s); {} remaining",
                            plan.completed.len(),
                            plan.remaining.len()
                        ),
                        now_millis,
                    );
                    self.ws
                        .left
                        .change_provenance
                        .record(crate::change_provenance::ChangeProvenance::Recovery);
                    self.ws
                        .right
                        .change_provenance
                        .record(crate::change_provenance::ChangeProvenance::Recovery);
                    state.outcome =
                        Some(rollback_outcome(plan.completed.len(), plan.remaining.len()));
                    state.repair_plan = Some(plan);
                    state.repair_loaded = true;
                    // A load still in flight predates the rollback.
                    state.repair_rx = None;
                    state.start_scan(&self.ws);
                }
                Err(error) => {
                    state.outcome = None;
                    state.error = Some(error);
                }
            }
        }
        if let Some(operation_id) = resume {
            match self.ws.begin_resume_recovery(&operation_id) {
                Ok(()) => {
                    let (sender, receiver) = std::sync::mpsc::channel();
                    let worker_id = operation_id.clone();
                    std::thread::spawn(move || {
                        let spec = crate::operation_journal::build_resume_spec(&worker_id);
                        let _ = sender.send(ResumePreparationResult {
                            operation_id: worker_id,
                            spec,
                        });
                    });
                    state.preparing_resume = Some(operation_id);
                    state.resume_rx = Some(receiver);
                    state.error = None;
                }
                Err(error) => {
                    state.outcome = None;
                    state.error = Some(error);
                }
            }
        }
        if state.detail == RecoveryDetail::Repair && !state.repair_loaded {
            state.load_repair();
        }
        self.recovery = state;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deliver(
        state: &mut RecoveryState,
        operations: Vec<crate::operation_journal::OperationRecord>,
    ) {
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(RecoveryScanResult {
                inventory: Ok(crate::operation_journal::RecoveryInventory {
                    operations,
                    orphans: Vec::new(),
                }),
            })
            .unwrap();
        state.scanning = true;
        state.scan_rx = Some(receiver);
        state.poll_scan();
    }

    #[test]
    fn a_requested_operation_missing_from_the_scan_stays_selected() {
        let requested = crate::operation::OperationId("op-gone".to_string());
        let mut state = RecoveryState::default();
        state.request(requested.clone());
        state.detail = RecoveryDetail::Repair;
        deliver(&mut state, Vec::new());

        assert_eq!(state.selected, Some(requested));
        assert!(!state.selected_in_inventory());
        state.load_repair();
        assert!(state.repair_plan.is_none());
        assert!(state.error.is_none());
    }

    #[test]
    fn operation_time_rejects_timestamps_beyond_i64() {
        assert_eq!(operation_time(u64::MAX), "Unknown time");
        assert_eq!(operation_time(i64::MAX as u64 + 1), "Unknown time");
        assert_ne!(operation_time(0), "Unknown time");
    }

    #[test]
    fn version_usage_is_reread_only_after_the_refresh_interval() {
        let start = std::time::Instant::now();
        let mut slot = None;
        let mut reads = 0;
        let mut read = |slot: &mut Option<(std::time::Instant, String)>, now| {
            cached_version_usage(slot, now, || {
                reads += 1;
                format!("read {reads}")
            })
        };
        assert_eq!(read(&mut slot, start), "read 1");
        assert_eq!(
            read(&mut slot, start + std::time::Duration::from_secs(1)),
            "read 1"
        );
        assert_eq!(read(&mut slot, start + VERSION_USAGE_REFRESH), "read 2");
    }

    #[test]
    fn a_completed_scan_drops_the_cached_version_usage() {
        let mut state = RecoveryState {
            version_usage_label: Some((
                std::time::Instant::now(),
                "Version store 1 KB".to_string(),
            )),
            ..RecoveryState::default()
        };
        deliver(&mut state, Vec::new());
        assert_eq!(state.version_usage_label, None);
    }

    #[test]
    fn capped_rows_keep_order_and_never_hide_unsettled_items() {
        let items: Vec<u32> = (0..10).collect();
        // Odd numbers are settled.
        let settled = |item: &u32| item % 2 == 1;
        assert_eq!(capped_rows(&items, 10, settled).len(), 10);
        let shown: Vec<u32> = capped_rows(&items, 7, settled)
            .into_iter()
            .copied()
            .collect();
        assert_eq!(shown, vec![0, 1, 2, 3, 4, 6, 8]);
        let shown: Vec<u32> = capped_rows(&items, 3, settled)
            .into_iter()
            .copied()
            .collect();
        assert_eq!(shown, vec![0, 2, 4]);
        assert!(capped_rows(&items, 0, settled).is_empty());
    }

    #[test]
    fn repair_rows_mark_done_by_list_and_cap_oldest_completed_first() {
        let item = |name: &str, automatic| crate::operation_journal::RepairItem {
            path: std::path::PathBuf::from(name),
            action: format!("restore {name}"),
            automatic,
        };
        let plan = crate::operation_journal::RepairPlan {
            operation_id: None,
            completed: vec![item("a", true), item("b", true)],
            remaining: vec![item("c", true), item("d", false)],
        };
        let names = |rows: Vec<(&crate::operation_journal::RepairItem, bool)>| {
            rows.into_iter()
                .map(|(item, done)| (item.path.display().to_string(), done))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(repair_rows(&plan, 10)),
            vec![
                ("a".to_string(), true),
                ("b".to_string(), true),
                ("c".to_string(), false),
                ("d".to_string(), false),
            ]
        );
        assert_eq!(
            names(repair_rows(&plan, 3)),
            vec![
                ("b".to_string(), true),
                ("c".to_string(), false),
                ("d".to_string(), false),
            ]
        );
        assert_eq!(names(repair_rows(&plan, 1)), vec![("c".to_string(), false)]);
    }

    fn wait_for_repair(state: &mut RecoveryState) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while state.repair_rx.is_some() {
            assert!(std::time::Instant::now() < deadline, "repair load hung");
            state.poll_repair();
            std::thread::yield_now();
        }
    }

    #[test]
    fn repair_plan_loads_in_the_background() {
        let mut state = RecoveryState::default();
        let id = record("op-a").id;
        state.select(id.clone());
        let (release, gate) = std::sync::mpsc::channel::<()>();
        state.spawn_repair_load(id.clone(), move |operation_id| {
            let _ = gate.recv();
            RepairLoadResult {
                plan: Ok(crate::operation_journal::RepairPlan {
                    operation_id: Some(operation_id.clone()),
                    ..Default::default()
                }),
                versions: Ok(Vec::new()),
                operation_id,
            }
        });
        // The loader is blocked; polling must not wait for it.
        state.poll_repair();
        assert!(state.repair_rx.is_some());
        assert!(state.repair_plan.is_none());
        release.send(()).unwrap();
        wait_for_repair(&mut state);
        assert_eq!(
            state.repair_plan.and_then(|plan| plan.operation_id),
            Some(id)
        );
    }

    #[test]
    fn a_repair_load_for_a_previous_selection_is_discarded() {
        let mut state = RecoveryState::default();
        state.select(record("op-a").id);
        let (release, gate) = std::sync::mpsc::channel::<()>();
        state.spawn_repair_load(record("op-a").id, move |operation_id| {
            let _ = gate.recv();
            RepairLoadResult {
                plan: Err("stale".to_string()),
                versions: Ok(Vec::new()),
                operation_id,
            }
        });
        state.select(record("op-b").id);
        let _ = release.send(());
        assert!(state.repair_rx.is_none());
        state.poll_repair();
        assert!(state.error.is_none());
        assert!(state.repair_plan.is_none());
    }

    #[test]
    fn an_unreadable_version_manifest_is_reported_not_shown_as_no_versions() {
        let mut state = RecoveryState::default();
        let id = record("op-a").id;
        state.select(id.clone());
        state.spawn_repair_load(id, |operation_id| RepairLoadResult {
            plan: Ok(crate::operation_journal::RepairPlan::default()),
            versions: Err("Version manifest is unreadable".to_string()),
            operation_id,
        });
        wait_for_repair(&mut state);
        assert!(state.repair_plan.is_some());
        assert!(state.versions.is_empty());
        assert_eq!(
            state.error.as_deref(),
            Some("Version manifest is unreadable")
        );
    }

    #[test]
    fn selecting_another_operation_clears_the_previous_error() {
        let mut state = RecoveryState::default();
        state.select(record("op-a").id);
        state.error = Some("Rollback of op-a failed".to_string());
        state.select(record("op-b").id);
        assert_eq!(state.error, None);
    }

    #[test]
    fn reclicking_the_selected_operation_keeps_its_repair_plan() {
        let mut state = RecoveryState::default();
        state.select(record("op-a").id);
        state.detail = RecoveryDetail::Repair;
        state.repair_plan = Some(crate::operation_journal::RepairPlan::default());
        state.repair_loaded = true;
        state.pick(record("op-a").id);
        assert_eq!(state.detail, RecoveryDetail::Repair);
        assert!(state.repair_plan.is_some());
        state.pick(record("op-b").id);
        assert_eq!(state.detail, RecoveryDetail::Inspect);
        assert!(state.repair_plan.is_none());
    }

    #[test]
    fn the_summary_counts_only_recoverable_operations_as_interrupted() {
        use crate::operation_journal::OperationStatus;
        let mut completed = record("op-done");
        completed.status = OperationStatus::Completed;
        assert_eq!(
            recovery_summary(&[record("op-a"), completed], 1),
            "1 interrupted operation  \u{00b7}  1 with leftover recovery data  \u{00b7}  1 staging item"
        );
        assert_eq!(
            recovery_summary(&[record("op-a"), record("op-b")], 0),
            "2 interrupted operations  \u{00b7}  0 staging items"
        );
    }

    #[test]
    fn resume_is_blocked_for_settled_operations_and_a_busy_queue() {
        use crate::operation_journal::OperationStatus;
        let failed = record("op-a");
        assert_eq!(resume_block_reason(&failed, false, false, false), None);
        assert!(resume_block_reason(&failed, true, false, false).is_some());
        assert_eq!(
            resume_block_reason(&failed, false, true, false),
            Some("Wait for the transfer queue to finish")
        );
        assert_eq!(
            resume_block_reason(&failed, false, false, true),
            Some("Wait for recovery preparation to finish")
        );
        for status in [OperationStatus::Completed, OperationStatus::RolledBack] {
            let mut settled = record("op-b");
            settled.status = status;
            assert_eq!(
                resume_block_reason(&settled, false, false, false),
                Some("Nothing left to resume")
            );
        }
    }

    #[test]
    fn recovery_messages_use_plain_words_and_correct_plurals() {
        assert_eq!(resume_message(0), "Finalizing verified operation");
        assert_eq!(resume_message(1), "Resuming 1 item");
        assert_eq!(resume_message(3), "Resuming 3 items");
        assert_eq!(
            rollback_outcome(2, 0),
            RecoveryOutcome {
                message: "Rollback completed 2 steps".to_string(),
                needs_review: false,
            }
        );
        assert_eq!(
            rollback_outcome(1, 1),
            RecoveryOutcome {
                message: "Rollback completed 1 step; 1 item needs review".to_string(),
                needs_review: true,
            }
        );
        assert_eq!(
            rollback_outcome(0, 4).message,
            "Rollback completed 0 steps; 4 items need review"
        );
        assert_eq!(
            clean_orphan_label(std::path::Path::new("/t/.stage")),
            "Delete leftover staging data at /t/.stage"
        );
    }

    #[test]
    fn no_request_and_an_empty_scan_selects_nothing() {
        let mut state = RecoveryState::default();
        deliver(&mut state, Vec::new());
        assert_eq!(state.selected, None);
    }

    fn record(id: &str) -> crate::operation_journal::OperationRecord {
        crate::operation_journal::OperationRecord {
            id: crate::operation::OperationId(id.to_string()),
            group_id: None,
            kind: TransferKind::Copy,
            target: std::path::PathBuf::from("/target"),
            policy: crate::transfer::OverwritePolicy::OverwriteAll,
            method: crate::transfer::CopyMethod::Native,
            durability: crate::operation::DurabilityProfile::Verified,
            version_retention: crate::operation::VersionRetentionPolicy::default(),
            name_policy: crate::filesystem_policy::NamePolicy::default(),
            symlink_policy: crate::filesystem_policy::SymlinkPolicy::default(),
            post_success: None,
            rollback_cleanup: None,
            rollback_cleanup_identity: None,
            rollback_cleanup_quarantine: None,
            status: crate::operation_journal::OperationStatus::Failed,
            created_at_secs: 1,
            updated_at_secs: 1,
            steps: Vec::new(),
        }
    }

    #[test]
    fn a_stale_selection_falls_back_to_the_first_operation() {
        // Picked from the list, then resolved (rolled back) before a rescan.
        let mut state = RecoveryState::default();
        state.select(crate::operation::OperationId("op-resolved".to_string()));
        deliver(&mut state, vec![record("op-a"), record("op-b")]);
        assert_eq!(state.selected, Some(record("op-a").id));

        // A request is honoured by one scan only; a later rescan that no
        // longer lists it moves on.
        state.request(crate::operation::OperationId("op-gone".to_string()));
        deliver(&mut state, vec![record("op-b")]);
        assert_eq!(
            state.selected.as_ref().map(|id| id.0.as_str()),
            Some("op-gone")
        );
        deliver(&mut state, vec![record("op-b")]);
        assert_eq!(state.selected, Some(record("op-b").id));

        // A listed selection is kept.
        state.select(record("op-b").id);
        deliver(&mut state, vec![record("op-a"), record("op-b")]);
        assert_eq!(state.selected, Some(record("op-b").id));
    }

    #[test]
    fn a_failed_scan_clears_the_lists_and_the_selection() {
        let mut state = RecoveryState::default();
        deliver(&mut state, vec![record("op-a")]);
        assert_eq!(state.selected, Some(record("op-a").id));

        let (sender, receiver) = std::sync::mpsc::channel();
        state.scanning = true;
        state.scan_rx = Some(receiver);
        // Nothing yet: still scanning, the old inventory stays.
        state.poll_scan();
        assert!(state.scanning && state.scan_rx.is_some());
        assert_eq!(state.operations.len(), 1);

        sender
            .send(RecoveryScanResult {
                inventory: Err("journal unreadable".to_string()),
            })
            .unwrap();
        state.poll_scan();
        assert!(!state.scanning && state.scan_rx.is_none());
        assert_eq!(state.error.as_deref(), Some("journal unreadable"));
        assert!(state.operations.is_empty() && state.orphans.is_empty());
        // The old selection is not shown against an inventory that failed.
        assert_eq!(state.selected, None);
    }

    #[test]
    fn a_scan_worker_that_dies_reports_it_and_stops_scanning() {
        let mut state = RecoveryState::default();
        deliver(&mut state, vec![record("op-a")]);
        let (sender, receiver) = std::sync::mpsc::channel::<RecoveryScanResult>();
        drop(sender);
        state.scanning = true;
        state.scan_rx = Some(receiver);
        state.poll_scan();
        assert!(!state.scanning && state.scan_rx.is_none());
        assert_eq!(
            state.error.as_deref(),
            Some("Recovery scan stopped unexpectedly")
        );
        // The last good inventory stays listed.
        assert_eq!(state.selected, Some(record("op-a").id));
        assert_eq!(state.operations.len(), 1);
    }
}
