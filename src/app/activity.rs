//! Activity: the strip above the key bar that reports the running transfer,
//! and the detailed transfer view inside the Operations panel. Transfers do
//! not take over the screen; the panes stay usable while they run.

use super::*;

/// What the Activity strip shows this frame.
enum ActivityLine {
    SafeState(String),
    Trash(crate::workspace::DeleteActivity),
    Transfer(TransferLine),
    Errors(usize),
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
                        ui.label(
                            egui::RichText::new("Paused for integrity review")
                                .size(11.0)
                                .strong()
                                .color(t.accent_warning),
                        );
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
                    ActivityLine::Trash(activity) => {
                        ui.label(
                            egui::RichText::new("Moving to Trash")
                                .size(11.0)
                                .strong()
                                .color(t.text_primary),
                        );
                        let fraction = if activity.total == 0 {
                            0.0
                        } else {
                            activity.completed as f32 / activity.total as f32
                        };
                        let (rect, _) =
                            ui.allocate_exact_size(Vec2::new(140.0, 4.0), Sense::hover());
                        ui.painter()
                            .rect_filled(rect, CornerRadius::ZERO, t.bg_card);
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                rect.min,
                                Vec2::new(rect.width() * fraction, 4.0),
                            ),
                            CornerRadius::ZERO,
                            t.accent,
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "{} of {}",
                                activity.completed, activity.total
                            ))
                            .size(11.0)
                            .color(t.text_muted),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
                        ui.label(
                            egui::RichText::new(format!(
                                "{count} operation{} need review",
                                if count == 1 { "" } else { "s" }
                            ))
                            .size(11.0)
                            .color(t.accent_red),
                        );
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
            return Some(ActivityLine::Trash(activity));
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

    fn show_transfer_line(&mut self, ui: &mut egui::Ui, line: TransferLine) {
        let t = self.colors;
        let ctx = ui.ctx().clone();
        let status = if line.finished {
            if line.errors == 0 {
                "\u{2713}".to_string()
            } else {
                format!(
                    "{} error{}",
                    line.errors,
                    if line.errors == 1 { "" } else { "s" }
                )
            }
        } else if let Some(reason) = &line.paused {
            reason.clone()
        } else {
            line.phase.to_string()
        };
        let status_color = if line.errors > 0 {
            t.accent_red
        } else if line.paused.is_some() {
            t.accent_warning
        } else {
            t.text_muted
        };
        ui.label(
            egui::RichText::new(&line.label)
                .size(11.0)
                .strong()
                .color(t.text_primary),
        );
        ui.label(egui::RichText::new(status).size(11.0).color(status_color));
        if !line.finished {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(140.0, 4.0), Sense::hover());
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, t.bg_card);
            let fraction = line.fraction.unwrap_or(0.0).clamp(0.0, 1.0);
            ui.painter().rect_filled(
                egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * fraction, 4.0)),
                CornerRadius::ZERO,
                t.accent,
            );
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
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
                let stop = crate::operation_view::CancellationAction::StopAfterCurrentFile;
                if ui
                    .add_enabled(
                        !line.stop_requested,
                        super::dialog_button(
                            t,
                            super::ButtonRole::Secondary,
                            egui::RichText::new(stop.label()).size(11.0),
                        ),
                    )
                    .on_hover_text(stop.consequence())
                    .clicked()
                {
                    self.ws.stop_transfer_after_current();
                }
            }
            if ui
                .add(super::dialog_button(
                    t,
                    super::ButtonRole::Secondary,
                    egui::RichText::new("Details").size(11.0),
                ))
                .on_hover_text("Operations panel (Cmd+J)")
                .clicked()
            {
                self.show_operations_center = true;
                self.operations_tab = OperationsTab::Queue;
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
        ui.label(
            egui::RichText::new(title)
                .size(13.0)
                .strong()
                .color(t.text_primary),
        );
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
            ui.horizontal(|ui| {
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
                    ui.add_sized(
                        [width, 22.0],
                        egui::Button::new(
                            egui::RichText::new(candidate.label())
                                .size(10.0)
                                .color(color),
                        )
                        .fill(fill)
                        .corner_radius(CornerRadius::ZERO)
                        .sense(Sense::hover()),
                    );
                }
            });
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
            } else {
                ui.label(
                    egui::RichText::new(format!("File: {}", current_file))
                        .size(12.0)
                        .color(t.text_primary),
                );
                ui.add_space(4.0);
                Self::draw_progress_bar(
                    ui,
                    file_frac,
                    &format!(
                        "{} / {}",
                        format_size(current_file_copied),
                        format_size(current_file_size),
                    ),
                    t.accent,
                    &t,
                );
            }
            if delta_reused_bytes > 0 {
                ui.label(
                    egui::RichText::new(format!(
                        "Delta reused {}  \u{00b7}  source {}",
                        format_size(delta_reused_bytes),
                        format_size(delta_source_bytes)
                    ))
                    .size(10.0)
                    .color(t.text_secondary),
                );
            }

            // Total progress bar (no rounding)
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Total:").size(11.0).color(t.text_muted));
            ui.add_space(2.0);
            Self::draw_progress_bar(
                ui,
                progress_frac.unwrap_or(0.0),
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
                ui.label(
                    egui::RichText::new(format!("{}/s", format_size(speed as u64)))
                        .size(11.0)
                        .color(t.text_muted),
                );

                ui.add_space(16.0);
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
                    egui::RichText::new(format!("{requeued_files} changed file(s) requeued"))
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
                    egui::RichText::new(format!("{error_count} error(s):"))
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
                                egui::RichText::new(format!("{hidden} earlier error(s) not shown"))
                                    .size(11.0)
                                    .color(t.text_muted),
                            );
                        }
                        for text in &error_rows {
                            ui.label(egui::RichText::new(text).size(11.0).color(t.accent_red));
                        }
                    });
            }

            ui.add_space(8.0);
            if finished {
                if ui
                    .add(super::dialog_button(
                        t,
                        super::ButtonRole::Primary,
                        egui::RichText::new("OK").size(13.0),
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
                        .clicked()
                    {
                        self.ws.stop_transfer_after_current();
                    }
                    if queued > 0 {
                        let cancel_pending =
                            crate::operation_view::CancellationAction::CancelPending;
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(format!(
                                        "{} ({queued})",
                                        cancel_pending.label()
                                    ))
                                    .size(13.0)
                                    .color(t.text_primary),
                                )
                                .fill(t.bg_card)
                                .corner_radius(CornerRadius::ZERO),
                            )
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

        if error_count > 0 {
            ctx.data_mut(|data| {
                data.insert_temp(egui::Id::new("active_error_surface"), details.response.rect);
            });
        }

        // Keep repainting during transfer
        if !finished {
            ctx.request_repaint();
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

    /// Draw a flat progress bar without rounding.
    fn draw_progress_bar(
        ui: &mut egui::Ui,
        frac: f32,
        text: &str,
        fill_color: Color32,
        t: &ThemeColors,
    ) {
        let bar_h = 18.0;
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, bar_h), Sense::hover());
        let p = ui.painter();

        // Background
        p.rect_filled(rect, CornerRadius::ZERO, t.bg_card.linear_multiply(0.5));

        // Filled portion
        let filled_w = rect.width() * frac.clamp(0.0, 1.0);
        if filled_w > 0.0 {
            let filled_rect = egui::Rect::from_min_size(rect.min, Vec2::new(filled_w, bar_h));
            p.rect_filled(filled_rect, CornerRadius::ZERO, fill_color);
        }

        // Text centered
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(10.0),
            t.text_primary,
        );
    }
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
/// the same rows as the full lists.
fn labeled_error_rows(
    errors: &[String],
    failures: &[crate::operation::ClassifiedFailure],
) -> Vec<String> {
    let mut pending = failures.iter().rev().peekable();
    let mut rows: Vec<String> = errors
        .iter()
        .rev()
        .map(|err| {
            let class = pending
                .next_if(|failure| {
                    err.strip_suffix(failure.message.as_str())
                        .is_some_and(|head| head.ends_with(": "))
                })
                .map(|failure| failure.class);
            match class {
                Some(class) => format!("{}: {err}", class.label()),
                None => err.clone(),
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
    use super::{graph_speeds, labeled_error_rows};
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
        let rows = labeled_error_rows(&errors, &failures);
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
}
