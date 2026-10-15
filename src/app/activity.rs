//! Activity: the strip above the key bar that reports the running transfer,
//! and the detailed transfer view inside the Operations panel. Transfers do
//! not take over the screen; the panes stay usable while they run.

use super::*;

/// What the Activity strip shows this frame.
enum ActivityLine {
    SafeState(String),
    /// A Trash job, plus the transfer it hides when one runs alongside.
    Trash(crate::workspace::DeleteActivity, Option<BackgroundTransfer>),
    Transfer(TransferLine),
    Errors(usize),
}

/// A transfer running behind a Trash job: just enough to keep it visible.
struct BackgroundTransfer {
    fraction: Option<f32>,
    finished: bool,
    errors: usize,
}

/// A frame's snapshot of the running transfer, taken under one lock.
struct TransferLine {
    label: String,
    fraction: Option<f32>,
    speed: f64,
    eta: Option<f64>,
    phase: &'static str,
    paused: Option<String>,
    finished: bool,
    stop_requested: bool,
    errors: usize,
    queued: usize,
}

impl App {
    /// One line above the key bar while a transfer runs or errors wait for
    /// review; `Cmd+J` or Details opens the full Operations panel.
    pub(crate) fn show_activity_strip(&mut self, ui: &mut egui::Ui) {
        let Some(line) = self.activity_line() else {
            return;
        };
        let t = self.colors;
        egui::Panel::bottom(egui::Id::new("activity_strip"))
            .resizable(false)
            .show_separator_line(false)
            .frame(
                Frame::NONE
                    .fill(t.bg_toolbar)
                    .inner_margin(Margin::symmetric(12, 6))
                    .stroke(Stroke::new(1.0_f32, t.border)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| match line {
                    ActivityLine::SafeState(reason) => {
                        let status = ui.label(
                            egui::RichText::new("Paused for integrity review")
                                .size(11.0)
                                .strong()
                                .color(t.accent_warning),
                        );
                        announce(ui, status.id, true);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(reason)
                                    .size(11.0)
                                    .color(t.text_secondary),
                            )
                            .truncate(),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add(super::dialog_button(
                                    t,
                                    super::ButtonRole::Warning,
                                    egui::RichText::new("Review").size(11.0),
                                ))
                                .clicked()
                            {
                                self.show_operations_center = true;
                                self.operations_tab = OperationsTab::Recovery;
                            }
                        });
                    }
                    ActivityLine::Trash(activity, transfer) => {
                        let status = ui.label(
                            egui::RichText::new("Moving to Trash")
                                .size(11.0)
                                .strong()
                                .color(t.text_primary),
                        );
                        announce(ui, status.id, false);
                        let fraction = if activity.total == 0 {
                            0.0
                        } else {
                            activity.completed as f32 / activity.total as f32
                        };
                        strip_bar(ui, &t, "Moving to Trash", Some(fraction));
                        ui.label(
                            egui::RichText::new(format!(
                                "{} of {}",
                                activity.completed, activity.total
                            ))
                            .size(11.0)
                            .color(t.text_muted),
                        );
                        if let Some(transfer) = &transfer {
                            ui.label(
                                egui::RichText::new(background_transfer_text(transfer))
                                    .size(11.0)
                                    .color(if transfer.errors > 0 {
                                        t.accent_red
                                    } else {
                                        t.text_muted
                                    }),
                            );
                        }
                        right_aligned_in_reading_order(ui, "trash_actions", |ui| {
                            if transfer.is_some() && self.activity_details_button(ui) {
                                self.show_operations_center = true;
                                self.operations_tab = OperationsTab::Queue;
                            }
                            if ui
                                .add_enabled(
                                    !activity.cancel_requested,
                                    super::dialog_button(
                                        t,
                                        super::ButtonRole::Secondary,
                                        egui::RichText::new(if activity.cancel_requested {
                                            "Cancelling..."
                                        } else {
                                            "Cancel"
                                        })
                                        .size(11.0),
                                    ),
                                )
                                .clicked()
                            {
                                self.ws.cancel_delete();
                            }
                        });
                        ui.ctx()
                            .request_repaint_after(std::time::Duration::from_millis(125));
                    }
                    ActivityLine::Transfer(transfer) => self.show_transfer_line(ui, transfer),
                    ActivityLine::Errors(count) => {
                        let status = ui.label(
                            egui::RichText::new(review_text(count))
                                .size(11.0)
                                .color(t.accent_red),
                        );
                        announce(ui, status.id, true);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add(super::dialog_button(
                                    t,
                                    super::ButtonRole::Secondary,
                                    egui::RichText::new("Review").size(11.0),
                                ))
                                .clicked()
                            {
                                self.show_operations_center = true;
                                self.operations_tab = OperationsTab::Errors;
                            }
                        });
                    }
                });
            });
    }

    fn activity_line(&self) -> Option<ActivityLine> {
        if let Some(state) = &self.ws.safe_state {
            return Some(ActivityLine::SafeState(state.reason.clone()));
        }
        if let Some(activity) = self.ws.delete_activity() {
            // Deletes may run during a transfer; keep the transfer (and a
            // retained error waiting for Dismiss) visible beside the job.
            let transfer = self.ws.active_transfer_view().map(|active| {
                let s = crate::lock_util::recover(&active.progress);
                BackgroundTransfer {
                    fraction: s.progress_fraction(),
                    finished: s.finished,
                    errors: s.errors.len(),
                }
            });
            return Some(ActivityLine::Trash(activity, transfer));
        }
        if let Some(active) = self.ws.active_transfer_view() {
            let s = crate::lock_util::recover(&active.progress);
            return Some(ActivityLine::Transfer(TransferLine {
                label: active.submitted.label(),
                fraction: s.progress_fraction(),
                speed: s.speed_bps(),
                eta: s.phase_eta_secs(),
                phase: s.phase.label(),
                paused: s
                    .pause_reason
                    .as_ref()
                    .map(|reason| reason.label().to_string()),
                finished: s.finished,
                stop_requested: s.stop_requested,
                errors: s.errors.len(),
                queued: self.ws.queued_count(),
            }));
        }
        let unresolved = self.operation_failures.notices().len();
        (unresolved > 0).then_some(ActivityLine::Errors(unresolved))
    }

    /// The strip's Details button; true when it was clicked.
    fn activity_details_button(&self, ui: &mut egui::Ui) -> bool {
        ui.add(super::dialog_button(
            self.colors,
            super::ButtonRole::Secondary,
            egui::RichText::new("Details").size(11.0),
        ))
        .on_hover_text(operations_panel_hint())
        .clicked()
    }

    fn show_transfer_line(&mut self, ui: &mut egui::Ui, line: TransferLine) {
        let t = self.colors;
        let ctx = ui.ctx().clone();
        let status = transfer_status_text(&line);
        let status_color = if line.errors > 0 {
            t.accent_red
        } else if line.paused.is_some() {
            t.accent_warning
        } else {
            t.text_muted
        };
        // A long destination name must not push the controls off-screen;
        // the elided label shows the full text on hover.
        ui.scope(|ui| {
            ui.set_max_width((ui.available_width() * 0.4).max(80.0));
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&line.label)
                        .size(11.0)
                        .strong()
                        .color(t.text_primary),
                )
                .truncate(),
            );
        });
        let status_label = ui.label(egui::RichText::new(status).size(11.0).color(status_color));
        announce(ui, status_label.id, line.finished && line.errors > 0);
        if !line.finished {
            strip_bar(ui, &t, &line.label, line.fraction);
            let mut meta = line.fraction.map_or_else(
                || "estimating".to_string(),
                |f| format!("{:.0}%", f * 100.0),
            );
            if line.speed > 0.0 {
                meta.push_str(&format!("  \u{00b7}  {}/s", format_size(line.speed as u64)));
            }
            if let Some(eta) = line.eta.filter(|eta| *eta > 0.0) {
                meta.push_str(&format!(
                    "  \u{00b7}  {}:{:02} left",
                    (eta / 60.0) as u64,
                    (eta % 60.0) as u64
                ));
            }
            ui.label(egui::RichText::new(meta).size(11.0).color(t.text_muted));
        }
        if line.queued > 0 {
            ui.label(
                egui::RichText::new(format!("+{} queued", line.queued))
                    .size(11.0)
                    .color(t.accent),
            );
        }
        // Reading order, so Tab reaches Details before the destructive Cancel.
        right_aligned_in_reading_order(ui, "transfer_actions", |ui| {
            if self.activity_details_button(ui) {
                self.show_operations_center = true;
                self.operations_tab = OperationsTab::Queue;
            }
            if line.finished {
                if ui
                    .add(super::dialog_button(
                        t,
                        super::ButtonRole::Primary,
                        egui::RichText::new("Dismiss").size(11.0),
                    ))
                    .clicked()
                {
                    let c = ctx.clone();
                    if let Ok(report) = self.ws.try_dismiss_transfer(move || c.request_repaint()) {
                        self.capture_terminal_report(
                            report,
                            (ctx.input(|input| input.time) * 1_000.0) as u64,
                        );
                    }
                }
            } else {
                let stop = crate::operation_view::CancellationAction::StopAfterCurrentFile;
                if ui
                    .add_enabled(
                        !line.stop_requested,
                        super::dialog_button(
                            t,
                            super::ButtonRole::Secondary,
                            egui::RichText::new(if line.stop_requested {
                                "Stopping..."
                            } else {
                                stop.label()
                            })
                            .size(11.0),
                        ),
                    )
                    .on_hover_text(stop.consequence())
                    .on_disabled_hover_text(stop.consequence())
                    .clicked()
                {
                    self.ws.stop_transfer_after_current();
                }
                let cancel = crate::operation_view::CancellationAction::CancelCurrent;
                if ui
                    .add(super::dialog_button(
                        t,
                        super::ButtonRole::Danger,
                        egui::RichText::new(cancel.label()).size(11.0),
                    ))
                    .on_hover_text(cancel.consequence())
                    .clicked()
                {
                    self.ws.cancel_transfer();
                }
            }
        });
        if !line.finished {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// Full detail for the active transfer: phases, backend, per-file and
    /// total progress, speed graph, errors and controls.
    pub(crate) fn show_transfer_details(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let Some(active) = self.ws.active_transfer_view() else {
            return;
        };
        let submitted = Some(active.submitted);
        let state = active.progress;
        let _latency =
            crate::measurement::LatencyGuard::new(crate::measurement::MetricName::OperationDialog);
        let t = self.colors;

        let s = crate::lock_util::recover(&state);
        let progress_frac = s.progress_fraction();
        let file_frac = if s.current_file_size > 0 {
            s.current_file_copied as f32 / s.current_file_size as f32
        } else {
            0.0
        };
        let speed = s.speed_bps();
        let eta = s.phase_eta_secs();
        let phase = s.phase;
        let current_file = s.current_file.clone();
        let current_file_copied = s.current_file_copied;
        let current_file_size = s.current_file_size;
        let files_done = s.files_done;
        let files_total = s.files_total;
        let copied = s.copied_bytes;
        let total = s.total_bytes;
        let samples: Vec<(f64, f64)> = s.speed_samples.clone();
        let finished = s.finished;
        let stop_requested = s.stop_requested;
        let stopped = s.stopped;
        let requeued_files = s.requeued_files;
        let backend_label = s.backend_label.clone();
        let backend_reason = s.backend_reason.clone();
        let p95_latency_ms = s.p95_latency_ms;
        let adaptive_concurrency = s.adaptive_concurrency;
        let bandwidth_limit = s.bandwidth_limit;
        let waiting_reason = s.waiting_reason.clone();
        let pause_reason = s.pause_reason.clone();
        let latest_fast_path = s.fast_paths.last().copied();
        let delta_reused_bytes = s.delta_reused_bytes;
        let delta_source_bytes = s.delta_source_bytes;
        let active_workers = s.active_workers;
        // Copy only the newest rows: the error list is unbounded and this runs
        // every frame under the progress lock.
        let error_count = s.errors.len();
        let error_tail = s.errors[error_count.saturating_sub(DETAIL_ERROR_ROWS)..].to_vec();
        let failure_tail =
            s.failures[s.failures.len().saturating_sub(DETAIL_ERROR_ROWS)..].to_vec();
        drop(s);
        let error_rows = labeled_error_rows(&error_tail, &failure_tail);
        // Transfers waiting behind this one in the queue.
        let queued = self.ws.queued_count();

        let title = if pause_reason.is_some() && !finished {
            "Transfer Paused"
        } else if finished && stopped {
            "Stopped Safely"
        } else if finished {
            if error_count == 0 {
                "Transfer Complete"
            } else {
                "Completed with Errors"
            }
        } else if stop_requested {
            "Finishing Current File..."
        } else {
            "Transferring..."
        };
        let title_label = ui.label(
            egui::RichText::new(title)
                .size(13.0)
                .strong()
                .color(t.text_primary),
        );
        // Speak the terminal state (Completed with Errors, Stopped Safely).
        announce(ui, title_label.id, false);
        let mut reveal: Option<std::path::PathBuf> = None;
        let details = ui.vertical(|ui| {
            if let Some(summary) = &submitted {
                ui.label(
                    egui::RichText::new(summary.label())
                        .size(12.0)
                        .strong()
                        .color(t.text_primary),
                );
                ui.label(
                    egui::RichText::new(summary.detail())
                        .size(10.0)
                        .color(t.text_muted),
                );
                ui.add_space(6.0);
            }
            phase_row(ui, &t, phase);
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{}  \u{00b7}  p95 {:.0} ms  \u{00b7}  {} worker{}",
                        backend_label,
                        p95_latency_ms,
                        adaptive_concurrency,
                        if adaptive_concurrency == 1 { "" } else { "s" }
                    ))
                    .size(10.0)
                    .color(t.text_muted),
                )
                .on_hover_text(backend_reason);
                if let Some(limit) = bandwidth_limit {
                    ui.label(
                        egui::RichText::new(format!("Limit {}/s", format_size(limit)))
                            .size(10.0)
                            .color(t.text_secondary),
                    );
                }
                if let Some(path) = latest_fast_path {
                    ui.label(egui::RichText::new(path.label()).size(10.0).color(t.accent));
                }
            });
            if let Some(reason) = &pause_reason {
                ui.label(
                    egui::RichText::new(reason.label())
                        .size(11.0)
                        .color(t.accent_warning),
                );
                ui.label(
                    egui::RichText::new(reason.resume_condition())
                        .size(10.0)
                        .color(t.text_muted),
                );
            } else if let Some(reason) = &waiting_reason {
                ui.label(
                    egui::RichText::new(reason)
                        .size(11.0)
                        .color(t.accent_warning),
                );
            }

            if active_workers > 1 {
                ui.label(
                    egui::RichText::new(format!("{active_workers} files in parallel"))
                        .size(12.0)
                        .color(t.text_primary),
                );
            } else if let Some(line) = file_line(&current_file, finished) {
                match line {
                    FileLine::Preparing => {
                        ui.label(
                            egui::RichText::new("Preparing...")
                                .size(12.0)
                                .color(t.text_primary),
                        );
                    }
                    FileLine::File => {
                        ui.label(
                            egui::RichText::new(format!("File: {}", current_file))
                                .size(12.0)
                                .color(t.text_primary),
                        );
                        ui.add_space(4.0);
                        Self::draw_progress_bar(
                            ui,
                            "Current file",
                            Some(file_frac),
                            &format!(
                                "{} / {}",
                                format_size(current_file_copied),
                                format_size(current_file_size),
                            ),
                            t.accent,
                            &t,
                        );
                    }
                }
            }
            if delta_reused_bytes > 0 {
                ui.label(
                    egui::RichText::new(delta_text(delta_reused_bytes, delta_source_bytes))
                        .size(10.0)
                        .color(t.text_secondary),
                )
                .on_hover_text(
                    "Unchanged parts of the file already at the destination were kept \
                     instead of being copied again",
                );
            }

            // Total progress bar (no rounding)
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Total:").size(11.0).color(t.text_muted));
            ui.add_space(2.0);
            Self::draw_progress_bar(
                ui,
                "Total progress",
                progress_frac,
                &progress_frac.map_or_else(
                    || "Estimating total...".to_string(),
                    |fraction| {
                        format!(
                            "{} / {} ({:.0}%)",
                            format_size(copied),
                            format_size(total),
                            fraction * 100.0,
                        )
                    },
                ),
                t.accent.linear_multiply(0.7),
                &t,
            );

            // Speed + ETA + files
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                // A finished transfer has no current rate to report.
                if let Some(speed) = speed_text(speed, finished) {
                    ui.label(egui::RichText::new(speed).size(11.0).color(t.text_muted));
                    ui.add_space(16.0);
                }
                if let Some(eta) = eta.filter(|eta| *eta > 0.0 && !finished) {
                    let mins = (eta / 60.0) as u64;
                    let secs = (eta % 60.0) as u64;
                    ui.label(
                        egui::RichText::new(format!("{} ETA: {}:{:02}", phase.label(), mins, secs))
                            .size(11.0)
                            .color(t.text_muted),
                    );
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{}/{} files", files_done, files_total))
                            .size(11.0)
                            .color(t.text_muted),
                    );
                });
            });

            // Queued-behind indicator: transfers waiting for this to finish.
            if queued > 0 {
                ui.add_space(2.0);
                let item = if queued == 1 { "transfer" } else { "transfers" };
                ui.label(
                    egui::RichText::new(format!("{queued} more {item} queued"))
                        .size(11.0)
                        .color(t.accent),
                );
            }
            if requeued_files > 0 {
                ui.label(
                    egui::RichText::new(requeued_text(requeued_files))
                        .size(11.0)
                        .color(t.accent_warning),
                );
            }

            if !self.accessibility_preferences.reduced_motion {
                Self::draw_speed_graph(ui, &samples, &t);
            }

            // Errors collected during the transfer
            if error_count > 0 {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!("{}:", plural(error_count, "error")))
                        .size(12.0)
                        .strong()
                        .color(t.accent_red),
                );
                egui::ScrollArea::vertical()
                    .id_salt("transfer_errors")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        let hidden = error_count - error_rows.len();
                        if hidden > 0 {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} not shown",
                                    plural(hidden, "earlier error")
                                ))
                                .size(11.0)
                                .color(t.text_muted),
                            );
                        }
                        for row in &error_rows {
                            let text = egui::RichText::new(&row.text)
                                .size(11.0)
                                .color(t.accent_red);
                            // A row that names its file links to it.
                            match &row.path {
                                Some(path) => {
                                    if ui
                                        .add(egui::Link::new(text))
                                        .on_hover_text(format!(
                                            "Show {} in the active panel",
                                            path.display()
                                        ))
                                        .clicked()
                                    {
                                        reveal = Some(path.clone());
                                    }
                                }
                                None => {
                                    ui.label(text);
                                }
                            }
                        }
                    });
            }

            ui.add_space(8.0);
            if finished {
                if ui
                    .add(super::dialog_button(
                        t,
                        super::ButtonRole::Primary,
                        egui::RichText::new("Dismiss").size(13.0),
                    ))
                    .clicked()
                {
                    // Retire the finished job and start the next queued one;
                    // just clearing active_transfer would strand the job
                    // Running and wedge the queue.
                    let c = ctx.clone();
                    if let Ok(report) = self.ws.try_dismiss_transfer(move || c.request_repaint()) {
                        self.capture_terminal_report(
                            report,
                            (ctx.input(|input| input.time) * 1_000.0) as u64,
                        );
                    }
                }
            } else {
                ui.horizontal(|ui| {
                    let stop = crate::operation_view::CancellationAction::StopAfterCurrentFile;
                    if ui
                        .add_enabled(
                            !stop_requested,
                            super::dialog_button(
                                t,
                                super::ButtonRole::Warning,
                                egui::RichText::new(if stop_requested {
                                    "Stopping after file"
                                } else {
                                    stop.label()
                                })
                                .size(13.0),
                            ),
                        )
                        .on_hover_text(stop.consequence())
                        .on_disabled_hover_text(stop.consequence())
                        .clicked()
                    {
                        self.ws.stop_transfer_after_current();
                    }
                    if queued > 0 {
                        let cancel_pending =
                            crate::operation_view::CancellationAction::CancelPending;
                        if ui
                            .add(super::dialog_button(
                                t,
                                super::ButtonRole::Secondary,
                                egui::RichText::new(format!(
                                    "{} ({queued})",
                                    cancel_pending.label()
                                ))
                                .size(13.0),
                            ))
                            .on_hover_text(cancel_pending.consequence())
                            .clicked()
                        {
                            self.ws.cancel_pending_transfers();
                        }
                    }
                    let cancel_current = crate::operation_view::CancellationAction::CancelCurrent;
                    if ui
                        .add(super::dialog_button(
                            t,
                            super::ButtonRole::Danger,
                            egui::RichText::new(cancel_current.label()).size(13.0),
                        ))
                        .on_hover_text(cancel_current.consequence())
                        .clicked()
                    {
                        self.ws.cancel_transfer();
                    }
                });
            }
        });

        if let Some(path) = reveal {
            self.ws.reveal(&path);
        }
        if error_count > 0 {
            ctx.data_mut(|data| {
                data.insert_temp(egui::Id::new("active_error_surface"), details.response.rect);
            });
        }

        // Keep repainting during transfer at the strip's cadence; progress
        // samples only arrive every 500 ms, so repainting at display rate
        // just burns CPU while the Operations panel is open.
        if !finished {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    fn draw_speed_graph(ui: &mut egui::Ui, samples: &[(f64, f64)], t: &ThemeColors) {
        if samples.len() <= 2 {
            return;
        }
        ui.add_space(8.0);
        let graph_h = 60.0;
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), graph_h), Sense::hover());

        let speeds = graph_speeds(samples);
        let max_speed = speeds.iter().cloned().fold(1.0f64, f64::max);
        let p = ui.painter();

        // Background
        p.rect_filled(rect, CornerRadius::same(3), t.bg_card.linear_multiply(0.3));

        // Draw speed line
        if speeds.len() >= 2 {
            let n = speeds.len();
            let points: Vec<egui::Pos2> = speeds
                .iter()
                .enumerate()
                .map(|(i, &s)| {
                    let x = rect.left() + (i as f32 / (n - 1) as f32) * rect.width();
                    let y = rect.bottom() - (s as f32 / max_speed as f32) * rect.height() * 0.9;
                    egui::pos2(x, y)
                })
                .collect();

            for w in points.windows(2) {
                p.line_segment([w[0], w[1]], Stroke::new(1.5_f32, t.accent));
            }
        }

        // Max speed label
        p.text(
            egui::pos2(rect.left() + 4.0, rect.top() + 2.0),
            egui::Align2::LEFT_TOP,
            format!("{}/s", format_size(max_speed as u64)),
            egui::FontId::proportional(9.0),
            t.text_muted,
        );
    }

    /// Draw a flat progress bar without rounding. `name` and the fraction
    /// reach assistive tech as a progressbar; `None` is still estimating.
    fn draw_progress_bar(
        ui: &mut egui::Ui,
        name: &str,
        frac: Option<f32>,
        text: &str,
        fill_color: Color32,
        t: &ThemeColors,
    ) {
        let bar_h = 18.0;
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, bar_h), Sense::hover());
        response.widget_info(|| progress_info(name, frac));
        let p = ui.painter();

        // Background
        p.rect_filled(rect, CornerRadius::ZERO, t.bg_card.linear_multiply(0.5));

        // Filled portion
        let filled_w = rect.width() * frac.unwrap_or(0.0).clamp(0.0, 1.0);
        let filled_rect = egui::Rect::from_min_size(rect.min, Vec2::new(filled_w, bar_h));
        if filled_w > 0.0 {
            p.rect_filled(filled_rect, CornerRadius::ZERO, fill_color);
        }

        // Text centered, in a color that reads on each side of the fill edge.
        let caption = |painter: egui::Painter, color: Color32| {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(10.0),
                color,
            );
        };
        caption(
            p.with_clip_rect(rect.with_min_x(filled_rect.right())),
            t.text_primary,
        );
        if filled_w > 0.0 {
            caption(
                p.with_clip_rect(filled_rect),
                caption_on_fill(fill_color, t.bg_panel, t.text_primary),
            );
        }
    }
}

/// The five-phase row: painted status cells that speak as one label
/// ("Phase 3 of 5: Transfer").
fn phase_row(ui: &mut egui::Ui, t: &ThemeColors, phase: crate::operation_view::OperationPhase) {
    let phases = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let width = ((ui.available_width() - 8.0) / 5.0).max(58.0);
        for candidate in crate::operation_view::OperationPhase::ALL {
            let active = candidate == phase;
            let complete = candidate.ordinal() < phase.ordinal();
            let color = if active {
                Color32::WHITE
            } else if complete {
                t.accent
            } else {
                t.text_muted
            };
            let fill = if active {
                t.accent
            } else {
                t.bg_card.linear_multiply(0.45)
            };
            // Painted, not buttons: the phases are status, not
            // controls, and the row speaks as one label below.
            let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 22.0), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                candidate.label(),
                egui::FontId::proportional(10.0),
                color,
            );
        }
    });
    phases.response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, phase_progress_text(phase))
    });
}

/// Mark a status label as a live region so assistive tech speaks its
/// changes; `urgent` ones (integrity pause, errors) interrupt.
fn announce(ui: &egui::Ui, id: egui::Id, urgent: bool) {
    ui.ctx().accesskit_node_builder(id, |node| {
        if urgent {
            node.set_role(egui::accesskit::Role::Alert);
            node.set_live(egui::accesskit::Live::Assertive);
        } else {
            node.set_role(egui::accesskit::Role::Status);
            node.set_live(egui::accesskit::Live::Polite);
        }
    });
}

/// Progressbar semantics for a painted bar: its label and percent value.
fn progress_info(label: &str, fraction: Option<f32>) -> egui::WidgetInfo {
    let mut info = egui::WidgetInfo::labeled(egui::WidgetType::ProgressIndicator, true, label);
    info.value = fraction.map(|f| (f64::from(f.clamp(0.0, 1.0)) * 100.0).round());
    info
}

/// The strip's thin progress bar.
fn strip_bar(ui: &mut egui::Ui, t: &ThemeColors, label: &str, fraction: Option<f32>) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(140.0, 4.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::ZERO, t.bg_card);
    let filled = fraction.unwrap_or(0.0).clamp(0.0, 1.0);
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * filled, 4.0)),
        CornerRadius::ZERO,
        t.accent,
    );
    response.widget_info(|| progress_info(label, fraction));
}

/// Lay out a button group flush right but create it in reading order, so
/// Tab visits it left to right (a right-to-left layout reverses that). The
/// group's width comes from the previous pass; a changed width discards the
/// frame so the group never paints out of place.
fn right_aligned_in_reading_order<R>(
    ui: &mut egui::Ui,
    id_salt: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let id = ui.id().with(("reading_order_group", id_salt));
    let remembered = ui.data(|data| data.get_temp::<f32>(id));
    let gap = ui.available_width() - remembered.unwrap_or(0.0);
    if gap > 0.0 {
        ui.add_space(gap);
    }
    let group = ui.horizontal(add_contents);
    let width = group.response.rect.width();
    if remembered.is_none_or(|known| (known - width).abs() > 0.5) {
        ui.data_mut(|data| data.insert_temp(id, width));
        ui.ctx()
            .request_discard("activity strip button group resized");
    }
    group.inner
}

fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

/// The strip's line for operations whose failures wait for review.
fn review_text(count: usize) -> String {
    let verb = if count == 1 { "needs" } else { "need" };
    format!("{} {verb} review", plural(count, "operation"))
}

/// The transfer's status word on the strip. A finished transfer reads as
/// "Done", not a bare check mark that assistive tech speaks as a glyph name.
fn transfer_status_text(line: &TransferLine) -> String {
    if line.finished {
        if line.errors == 0 {
            "Done \u{2713}".to_string()
        } else {
            plural(line.errors, "error")
        }
    } else if let Some(reason) = &line.paused {
        reason.clone()
    } else {
        line.phase.to_string()
    }
}

/// The compact note for a transfer running behind a Trash job.
fn background_transfer_text(transfer: &BackgroundTransfer) -> String {
    if transfer.finished {
        if transfer.errors == 0 {
            "+ transfer done".to_string()
        } else {
            format!("+ transfer: {}", plural(transfer.errors, "error"))
        }
    } else {
        transfer.fraction.map_or_else(
            || "+ transfer estimating".to_string(),
            |f| format!("+ transfer {:.0}%", f * 100.0),
        )
    }
}

/// Hover text for Details, with the Operations panel shortcut as it is
/// pressed on this platform.
fn operations_panel_hint() -> String {
    super::toolbar::command_shortcut(crate::command::Command::ToggleQueuePanel).map_or_else(
        || "Operations panel".to_string(),
        |shortcut| format!("Operations panel ({shortcut})"),
    )
}

/// The phase row as one sentence for assistive tech ("Phase 3 of 5: Transfer").
fn phase_progress_text(phase: crate::operation_view::OperationPhase) -> String {
    let total = crate::operation_view::OperationPhase::ALL.len();
    format!(
        "Phase {} of {total}: {}",
        phase.ordinal() + 1,
        phase.label()
    )
}

/// What a delta copy saved, in words rather than the backend's terms.
fn delta_text(reused: u64, source: u64) -> String {
    format!(
        "Reused {} of {} from the existing file",
        format_size(reused),
        format_size(source)
    )
}

/// What the details view shows about the file in flight.
#[derive(Debug, PartialEq, Eq)]
enum FileLine {
    /// Scan and plan run before the first file starts.
    Preparing,
    /// The current file's name and per-file bar.
    File,
}

/// The file block for `current_file`: "Preparing..." only while the
/// transfer still runs; nothing once a transfer that never reached a file
/// has finished (failed scan, nothing to copy, early cancel).
fn file_line(current_file: &str, finished: bool) -> Option<FileLine> {
    match (current_file.is_empty(), finished) {
        (true, false) => Some(FileLine::Preparing),
        (true, true) => None,
        (false, _) => Some(FileLine::File),
    }
}

/// The details view's rate; none once the transfer has finished.
fn speed_text(speed: f64, finished: bool) -> Option<String> {
    (!finished).then(|| format!("{}/s", format_size(speed as u64)))
}

/// Files that changed mid-copy and go back in the queue.
fn requeued_text(count: usize) -> String {
    format!(
        "{} changed while copying and will be copied again",
        plural(count, "file")
    )
}

/// Caption color over a progress fill: white or `dark`, whichever contrasts
/// more with `fill` as it shows over `base` (fills may be translucent).
fn caption_on_fill(fill: Color32, base: Color32, dark: Color32) -> Color32 {
    let alpha = f32::from(fill.a()) / 255.0;
    // Color32 is premultiplied: blending over an opaque base adds the rest.
    let shown = |fill: u8, base: u8| f32::from(fill) + f32::from(base) * (1.0 - alpha);
    let luminance = |rgb: [f32; 3]| {
        let lin = |c: f32| {
            let c = c / 255.0;
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2])
    };
    let contrast = |a: f32, b: f32| (a.max(b) + 0.05) / (a.min(b) + 0.05);
    let under = luminance([
        shown(fill.r(), base.r()),
        shown(fill.g(), base.g()),
        shown(fill.b(), base.b()),
    ]);
    let white = luminance([255.0; 3]);
    let dark_luminance = luminance([
        f32::from(dark.r()),
        f32::from(dark.g()),
        f32::from(dark.b()),
    ]);
    if contrast(white, under) >= contrast(dark_luminance, under) {
        Color32::WHITE
    } else {
        dark
    }
}

/// One row of the details view's error list.
#[derive(Debug, PartialEq)]
struct ErrorRow {
    text: String,
    path: Option<std::path::PathBuf>,
}

/// Newest transfer errors shown in the details view. Older ones stay in the
/// report; cloning and laying out thousands of rows each frame stalls the UI.
const DETAIL_ERROR_ROWS: usize = 200;

/// Error rows prefixed with their failure class. `errors` and `failures` are
/// not parallel: a backend may push an error with no classified failure (the
/// macOS copyfile callback does), so index pairing would shift labels onto the
/// wrong rows. Each failure is recorded right after its own error as
/// "<context>: <message>", so pair them by that suffix, walking back from the
/// newest entries. Matching from the end means the tails of both lists give
/// the same rows as the full lists. A row keeps its failure's path, so the
/// details view can link to the file.
fn labeled_error_rows(
    errors: &[String],
    failures: &[crate::operation::ClassifiedFailure],
) -> Vec<ErrorRow> {
    let mut pending = failures.iter().rev().peekable();
    let mut rows: Vec<ErrorRow> = errors
        .iter()
        .rev()
        .map(|err| {
            let failure = pending.next_if(|failure| {
                err.strip_suffix(failure.message.as_str())
                    .is_some_and(|head| head.ends_with(": "))
            });
            match failure {
                Some(failure) => ErrorRow {
                    text: format!("{}: {err}", failure.class.label()),
                    path: failure.path.clone(),
                },
                None => ErrorRow {
                    text: err.clone(),
                    path: None,
                },
            }
        })
        .collect();
    rows.reverse();
    rows
}

/// Per-interval speeds (bytes/s) for the speed graph. Samples closer than
/// 10 ms apart (per-file samples on small-file copies) are merged into the
/// next interval rather than plotted as false 0 B/s drops, and a rewound byte
/// counter (a mount retry restarts the attempt) plots as 0, never below the
/// graph box.
fn graph_speeds(samples: &[(f64, f64)]) -> Vec<f64> {
    let mut speeds = Vec::new();
    let Some(&first) = samples.first() else {
        return speeds;
    };
    let mut anchor = first;
    for &(time, bytes) in &samples[1..] {
        let dt = time - anchor.0;
        if dt <= 0.01 {
            continue;
        }
        speeds.push(((bytes - anchor.1) / dt).max(0.0));
        anchor = (time, bytes);
    }
    speeds
}

#[cfg(test)]
mod tests {
    use super::{
        BackgroundTransfer, FileLine, TransferLine, announce, background_transfer_text,
        caption_on_fill, delta_text, file_line, graph_speeds, labeled_error_rows,
        operations_panel_hint, phase_progress_text, phase_row, requeued_text, review_text,
        right_aligned_in_reading_order, speed_text, strip_bar, transfer_status_text,
    };
    use crate::operation::{ClassifiedFailure, FailureClass};

    #[test]
    fn error_labels_skip_errors_without_a_classified_failure() {
        // The macOS copyfile callback pushes an unclassified error before the
        // executor records the classified one for the same entry.
        let errors = vec![
            "a.txt: disk full".to_string(),
            "Failed to copy: b.txt".to_string(),
            "b.txt: permission denied".to_string(),
        ];
        let failures = vec![
            ClassifiedFailure::message(FailureClass::Blocked, None, "disk full".to_string()),
            ClassifiedFailure::message(
                FailureClass::UserDecision,
                None,
                "permission denied".to_string(),
            ),
        ];
        let rows: Vec<String> = labeled_error_rows(&errors, &failures)
            .into_iter()
            .map(|row| row.text)
            .collect();
        assert_eq!(
            rows,
            vec![
                format!("{}: a.txt: disk full", FailureClass::Blocked.label()),
                "Failed to copy: b.txt".to_string(),
                format!(
                    "{}: b.txt: permission denied",
                    FailureClass::UserDecision.label()
                ),
            ]
        );
    }

    #[test]
    fn error_labels_from_tails_match_full_lists() {
        let mut errors = Vec::new();
        let mut failures = Vec::new();
        for index in 0..10 {
            if index % 3 == 0 {
                errors.push(format!("Failed to copy: f{index}"));
            }
            let message = format!("failure {index}");
            errors.push(format!("f{index}: {message}"));
            failures.push(ClassifiedFailure::message(
                FailureClass::Retryable,
                None,
                message,
            ));
        }
        let full = labeled_error_rows(&errors, &failures);
        let tail = 5;
        let from_tails = labeled_error_rows(
            &errors[errors.len() - tail..],
            &failures[failures.len() - tail..],
        );
        assert_eq!(from_tails, full[full.len() - tail..]);
    }

    #[test]
    fn graph_speeds_merge_close_samples_instead_of_dropping_to_zero() {
        // Two per-file samples 1 ms apart, then a regular one.
        let samples = [(0.0, 0.0), (1.0, 100.0), (1.001, 110.0), (2.0, 200.0)];
        let speeds = graph_speeds(&samples);
        assert_eq!(speeds.len(), 2);
        assert!((speeds[0] - 100.0).abs() < 1e-9);
        assert!((speeds[1] - 100.0).abs() < 1e-9);
    }

    #[test]
    fn graph_speeds_clamp_rewound_counter_to_zero() {
        let samples = [(0.0, 0.0), (1.0, 500.0), (2.0, 100.0), (3.0, 300.0)];
        let speeds = graph_speeds(&samples);
        assert_eq!(speeds, vec![500.0, 0.0, 200.0]);
    }

    fn line(finished: bool, errors: usize) -> TransferLine {
        TransferLine {
            label: "Copy 3 items".to_string(),
            fraction: Some(0.5),
            speed: 0.0,
            eta: None,
            phase: "Transfer",
            paused: None,
            finished,
            stop_requested: false,
            errors,
            queued: 0,
        }
    }

    #[test]
    fn strip_text_agrees_in_number_and_names_the_finished_state() {
        assert_eq!(review_text(1), "1 operation needs review");
        assert_eq!(review_text(3), "3 operations need review");
        assert_eq!(transfer_status_text(&line(true, 0)), "Done \u{2713}");
        assert_eq!(transfer_status_text(&line(true, 1)), "1 error");
        assert_eq!(transfer_status_text(&line(true, 2)), "2 errors");
        assert_eq!(transfer_status_text(&line(false, 0)), "Transfer");
    }

    #[test]
    fn a_transfer_behind_a_trash_job_stays_visible() {
        let running = BackgroundTransfer {
            fraction: Some(0.42),
            finished: false,
            errors: 0,
        };
        assert_eq!(background_transfer_text(&running), "+ transfer 42%");
        let failed = BackgroundTransfer {
            fraction: Some(1.0),
            finished: true,
            errors: 2,
        };
        assert_eq!(background_transfer_text(&failed), "+ transfer: 2 errors");
    }

    #[test]
    fn details_hint_uses_the_platform_shortcut() {
        let expected = super::super::toolbar::platform_shortcut("Cmd+J");
        assert_eq!(
            operations_panel_hint(),
            format!("Operations panel ({expected})")
        );
    }

    #[test]
    fn file_line_says_preparing_only_while_running() {
        assert_eq!(file_line("", false), Some(FileLine::Preparing));
        assert_eq!(file_line("", true), None);
        assert_eq!(file_line("a.txt", false), Some(FileLine::File));
        assert_eq!(file_line("a.txt", true), Some(FileLine::File));
    }

    fn accesskit_nodes(run: impl FnMut(&mut egui::Ui)) -> Vec<egui::accesskit::Node> {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let output = crate::testutil::take_egui_output(ctx.run_ui(egui::RawInput::default(), run));
        output
            .platform_output
            .accesskit_update
            .expect("headless AccessKit output")
            .nodes
            .into_iter()
            .map(|(_, node)| node)
            .collect()
    }

    /// A widget node's spoken text: egui puts a label's text in `value`
    /// (text-run children, which repeat it, are skipped).
    fn spoken(node: &egui::accesskit::Node) -> Option<&str> {
        if node.role() == egui::accesskit::Role::TextRun {
            return None;
        }
        node.label().or(node.value())
    }

    #[test]
    fn announce_marks_status_polite_and_urgent_assertive() {
        use egui::accesskit::{Live, Role};
        let nodes = accesskit_nodes(|ui| {
            let calm = ui.label("Copying");
            announce(ui, calm.id, false);
            let urgent = ui.label("Finished with errors");
            announce(ui, urgent.id, true);
        });
        let calm = nodes
            .iter()
            .find(|node| spoken(node) == Some("Copying"))
            .expect("status node");
        assert_eq!(calm.role(), Role::Status);
        assert_eq!(calm.live(), Some(Live::Polite));
        let urgent = nodes
            .iter()
            .find(|node| spoken(node) == Some("Finished with errors"))
            .expect("alert node");
        assert_eq!(urgent.role(), Role::Alert);
        assert_eq!(urgent.live(), Some(Live::Assertive));
    }

    #[test]
    fn phase_row_speaks_as_one_label() {
        let t = crate::theme::ThemeColors::for_preferences(
            crate::theme::ThemeMode::Light,
            crate::accessibility::Preferences::default(),
        );
        let nodes = accesskit_nodes(|ui| {
            phase_row(ui, &t, crate::operation_view::OperationPhase::Transfer);
        });
        let labels: Vec<_> = nodes.iter().filter_map(|node| spoken(node)).collect();
        assert_eq!(labels, vec!["Phase 3 of 5: Transfer"], "{labels:?}");
    }

    #[test]
    fn strip_bar_publishes_progressbar_semantics() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let t = crate::theme::ThemeColors::for_preferences(
            crate::theme::ThemeMode::Light,
            crate::accessibility::Preferences::default(),
        );
        let output =
            crate::testutil::take_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
                strip_bar(ui, &t, "Copy 3 items", Some(0.42));
            }));
        let update = output
            .platform_output
            .accesskit_update
            .expect("headless AccessKit output");
        let bar = update
            .nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.role() == egui::accesskit::Role::ProgressIndicator)
            .expect("progress node");
        assert_eq!(bar.label(), Some("Copy 3 items"));
        assert_eq!(bar.numeric_value(), Some(42.0));
    }

    #[test]
    fn strip_buttons_sit_right_in_reading_order() {
        let ctx = egui::Context::default();
        let mut rects = Vec::new();
        let mut right_edge = 0.0;
        for _ in 0..2 {
            crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.horizontal(|ui| {
                    right_edge = ui.max_rect().right();
                    ui.label("status");
                    rects = right_aligned_in_reading_order(ui, "test", |ui| {
                        vec![ui.button("Details").rect, ui.button("Cancel").rect]
                    });
                });
            }));
        }
        assert!(rects[0].left() < rects[1].left(), "{rects:?}");
        assert!(
            (rects[1].right() - right_edge).abs() < 1.0,
            "{rects:?} vs {right_edge}"
        );
    }

    #[test]
    fn error_rows_keep_their_failure_path_for_reveal() {
        let path = std::path::PathBuf::from("/data/b.txt");
        let errors = vec![
            "Failed to copy: a.txt".to_string(),
            "b.txt: permission denied".to_string(),
        ];
        let failures = vec![ClassifiedFailure::message(
            FailureClass::UserDecision,
            Some(path.clone()),
            "permission denied".to_string(),
        )];
        let rows = labeled_error_rows(&errors, &failures);
        assert_eq!(rows[0].path, None);
        assert_eq!(rows[1].path, Some(path));
    }

    #[test]
    fn details_text_is_plain_and_counted() {
        use crate::operation_view::OperationPhase;
        assert_eq!(
            phase_progress_text(OperationPhase::Transfer),
            "Phase 3 of 5: Transfer"
        );
        assert_eq!(
            phase_progress_text(OperationPhase::Scan),
            "Phase 1 of 5: Scan"
        );
        assert_eq!(
            requeued_text(1),
            "1 file changed while copying and will be copied again"
        );
        assert_eq!(
            requeued_text(3),
            "3 files changed while copying and will be copied again"
        );
        assert!(delta_text(40, 120).starts_with("Reused "));
        assert!(delta_text(40, 120).ends_with(" from the existing file"));
        assert!(speed_text(1024.0, false).is_some());
        assert_eq!(speed_text(0.0, true), None);
    }

    #[test]
    fn bar_caption_contrasts_with_the_accent_fill() {
        let dark_text = egui::Color32::from_rgb(46, 42, 38);
        let light_bg = egui::Color32::from_rgb(253, 252, 249);
        // The light theme's accent: dark text there reads at about 2.4:1.
        let accent = egui::Color32::from_rgb(22, 102, 170);
        assert_eq!(
            caption_on_fill(accent, light_bg, dark_text),
            egui::Color32::WHITE
        );
        // A pale fill keeps the dark caption.
        let pale = egui::Color32::from_rgb(230, 236, 245);
        assert_eq!(caption_on_fill(pale, light_bg, dark_text), dark_text);
    }
}
