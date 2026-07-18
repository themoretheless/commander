//! Compact runtime budgets, diagnostics exports, and rollout controls.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BudgetState {
    NoSamples,
    Within,
    Over,
}

fn budget_state(latency: crate::measurement::LatencyPercentiles, hard_p95_ms: f64) -> BudgetState {
    if latency.samples == 0 {
        BudgetState::NoSamples
    } else if latency.p95_ms <= hard_p95_ms {
        BudgetState::Within
    } else {
        BudgetState::Over
    }
}

impl App {
    pub(crate) fn show_developer_panel(&mut self, ctx: &egui::Context) {
        if !self.show_developer_panel {
            return;
        }
        let t = self.colors;
        let workload = crate::workload::stats();
        let persistence = crate::persistence::health_snapshot();
        let watcher = crate::watcher_health::snapshot();
        let active_watchers = usize::from(self.ws.left.watcher_active())
            + usize::from(self.ws.right.watcher_active());
        let watcher_errors = watcher
            .backend_errors
            .saturating_add(watcher.start_failures)
            .saturating_add(watcher.watch_failures);
        let image_cache = self.image_cache.stats();
        let active_root = self.ws.active_panel_ref().current_path.clone();
        let index = self.content_index.status(&active_root);
        let runtime_metrics = crate::measurement::snapshots();
        let metric = |name| {
            runtime_metrics
                .iter()
                .find(|(candidate, _)| *candidate == name)
                .map_or_else(
                    crate::measurement::LatencyPercentiles::default,
                    |(_, value)| *value,
                )
        };
        let frame_latency = metric(crate::measurement::MetricName::FrameTime);
        let budgets = crate::measurement::ci_budgets().ok();
        let startup = crate::measurement::latest_startup();
        let screen = ctx.input(|input| input.viewport_rect());
        let width = 480.0_f32.min((screen.width() - 32.0).max(320.0));
        let height = 680.0_f32.min((screen.height() - 32.0).max(360.0));
        let mut open = self.show_developer_panel;

        egui::Window::new("Developer diagnostics")
            .open(&mut open)
            .default_size(Vec2::new(width, height))
            .min_width(320.0)
            .max_width(560.0)
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    section_label(ui, "Runtime", t.text_primary);
                    egui::Grid::new("developer_runtime_grid")
                        .num_columns(2)
                        .striped(true)
                        .spacing([16.0, 6.0])
                        .show(ui, |ui| {
                            metric_row(ui, "Workers active", workload.running, t.text_primary);
                            metric_row(ui, "Queued I/O", workload.queued, t.text_primary);
                            text_row(
                                ui,
                                "In-flight bytes",
                                &format_size(workload.inflight_bytes),
                                t.text_primary,
                            );
                            text_row(
                                ui,
                                "Image cache",
                                &format!(
                                    "{} / {} entries",
                                    format_size(
                                        u64::try_from(image_cache.bytes).unwrap_or(u64::MAX)
                                    ),
                                    image_cache.entries
                                ),
                                t.text_primary,
                            );
                            text_row(
                                ui,
                                "Preview jobs",
                                &format!(
                                    "{} loading / {} failed",
                                    image_cache.pending, image_cache.failed
                                ),
                                if image_cache.failed == 0 {
                                    t.text_primary
                                } else {
                                    t.accent_warning
                                },
                            );
                            text_row(
                                ui,
                                "Preview providers",
                                &format!(
                                    "ImageIO {}/{} / standard {}/{} / video {}/{}",
                                    image_cache.providers.image_io.successes,
                                    image_cache.providers.image_io.attempts,
                                    image_cache.providers.standard.successes,
                                    image_cache.providers.standard.attempts,
                                    image_cache.providers.video.successes,
                                    image_cache.providers.video.attempts,
                                ),
                                t.text_primary,
                            );
                            text_row(
                                ui,
                                "Provider recovery",
                                &format!(
                                    "{} fallbacks / {} timeouts / {} active",
                                    image_cache.providers.fallbacks,
                                    image_cache.providers.timeouts,
                                    image_cache.providers.active_decoders,
                                ),
                                if image_cache.providers.timeouts == 0 {
                                    t.text_primary
                                } else {
                                    t.accent_warning
                                },
                            );
                            text_row(
                                ui,
                                "Filesystem watchers",
                                &format!("{active_watchers}/2 active / {} events", watcher.events),
                                if active_watchers == 2 {
                                    t.text_primary
                                } else {
                                    t.accent_warning
                                },
                            );
                            text_row(
                                ui,
                                "Watcher recovery",
                                &format!(
                                    "{} rescans / {} errors / {} reconnects / {} reconciled",
                                    watcher.rescan_signals,
                                    watcher_errors,
                                    watcher.reconnects,
                                    watcher.gap_reconciliations
                                ),
                                if active_watchers == 2 {
                                    t.text_primary
                                } else {
                                    t.accent_warning
                                },
                            );
                            text_row(
                                ui,
                                "Watcher batches",
                                &format!(
                                    "{} batches / {} events merged",
                                    watcher.event_batches, watcher.coalesced_events
                                ),
                                t.text_primary,
                            );
                            text_row(
                                ui,
                                "Watcher policy",
                                &format!(
                                    "{} native / {} polling / {} shallow / {} fallbacks",
                                    watcher.native_starts,
                                    watcher.polling_starts,
                                    watcher.shallow_starts,
                                    watcher.backend_fallbacks,
                                ),
                                if watcher.backend_fallbacks == 0 {
                                    t.text_primary
                                } else {
                                    t.accent_warning
                                },
                            );
                            text_row(
                                ui,
                                "Index cache",
                                &format_size(
                                    u64::try_from(index.progress.content_bytes).unwrap_or(u64::MAX),
                                ),
                                t.text_primary,
                            );
                            text_row(
                                ui,
                                "Frame time",
                                &percentile_text(frame_latency),
                                if frame_latency.p95_ms <= 16.7 || frame_latency.samples == 0 {
                                    t.text_primary
                                } else {
                                    t.accent_warning
                                },
                            );
                            text_row(
                                ui,
                                "Cancelled / stale",
                                &format!("{} / {}", workload.cancelled, workload.stale_results),
                                if workload.stale_results == 0 {
                                    t.text_primary
                                } else {
                                    t.accent_warning
                                },
                            );
                            text_row(
                                ui,
                                "Cancel latency p95",
                                &format!(
                                    "{:.2} ms / {} samples",
                                    workload.cancellation_latency_p95_micros as f64 / 1_000.0,
                                    workload.cancellation_latency_samples
                                ),
                                t.text_primary,
                            );
                            text_row(
                                ui,
                                "Settings recovery",
                                &format!(
                                    "{} stores / {} kept / {} rejected / {} unreadable",
                                    persistence.recovered_stores,
                                    persistence.recovered_items,
                                    persistence.rejected_items,
                                    persistence.unreadable_stores
                                ),
                                if persistence.rejected_items == 0
                                    && persistence.unreadable_stores == 0
                                {
                                    t.text_primary
                                } else {
                                    t.accent_warning
                                },
                            );
                        });

                    ui.add_space(12.0);
                    section_label(ui, "Performance budgets", t.text_primary);
                    if let Some(budgets) = &budgets {
                        egui::Grid::new("developer_budget_grid")
                            .num_columns(3)
                            .striped(true)
                            .spacing([12.0, 6.0])
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new("Metric").size(10.0).color(t.text_muted),
                                );
                                ui.label(
                                    egui::RichText::new("p50 / p95 / p99")
                                        .size(10.0)
                                        .color(t.text_muted),
                                );
                                ui.label(
                                    egui::RichText::new("Budget").size(10.0).color(t.text_muted),
                                );
                                ui.end_row();
                                for budget in &budgets.metrics {
                                    let latency = metric(budget.metric);
                                    let state = budget_state(latency, budget.hard_p95_ms);
                                    let color = match state {
                                        BudgetState::NoSamples => t.text_muted,
                                        BudgetState::Within => t.accent,
                                        BudgetState::Over => t.accent_red,
                                    };
                                    ui.label(
                                        egui::RichText::new(budget.metric.label())
                                            .size(11.0)
                                            .color(t.text_primary),
                                    );
                                    ui.label(
                                        egui::RichText::new(percentile_text(latency))
                                            .size(10.0)
                                            .color(color),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{:.1} ms",
                                            budget.hard_p95_ms
                                        ))
                                        .size(10.0)
                                        .color(color),
                                    );
                                    ui.end_row();
                                }
                            });
                    }

                    if let Some(startup) = &startup {
                        ui.add_space(8.0);
                        egui::CollapsingHeader::new(format!(
                            "Startup phases ({:.1} ms)",
                            startup.total_ms
                        ))
                        .default_open(false)
                        .show(ui, |ui| {
                            egui::Grid::new("developer_startup_grid")
                                .num_columns(2)
                                .striped(true)
                                .spacing([16.0, 5.0])
                                .show(ui, |ui| {
                                    for span in &startup.phases {
                                        text_row(
                                            ui,
                                            span.phase.label(),
                                            &format!("{:.1} ms", span.duration_ms),
                                            t.text_primary,
                                        );
                                    }
                                });
                        });
                    }

                    ui.add_space(12.0);
                    section_label(ui, "Runtime controls", t.text_primary);
                    for state in crate::feature_flags::snapshots() {
                        let mut enabled = !state.runtime_killed;
                        let mut rollout = state.rollout_percent;
                        ui.horizontal(|ui| {
                            ui.set_min_height(crate::accessibility::MIN_CONTROL_POINTS);
                            ui.add_sized(
                                [140.0, crate::accessibility::MIN_CONTROL_POINTS],
                                egui::Label::new(
                                    egui::RichText::new(state.feature.label())
                                        .size(11.0)
                                        .color(t.text_primary),
                                ),
                            );
                            let enabled_response = ui.add_enabled(
                                !state.environment_killed,
                                egui::Checkbox::new(&mut enabled, "Enabled"),
                            );
                            if enabled_response.changed()
                                && !crate::feature_flags::set_killed(state.feature, !enabled)
                            {
                                self.developer_notice =
                                    Some(DeveloperNotice::error("Could not save runtime control"));
                            }
                            let effective_enabled =
                                enabled && !state.environment_killed && state.bucket < rollout;
                            let effective = if state.environment_killed {
                                "ENV OFF"
                            } else if effective_enabled {
                                "ON"
                            } else {
                                "OFF"
                            };
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(egui::RichText::new(effective).size(9.0).strong().color(
                                    if effective_enabled {
                                        t.accent
                                    } else {
                                        t.accent_warning
                                    },
                                ))
                                .on_hover_text(format!("Rollout bucket {}", state.bucket));
                            });
                        });
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [60.0, crate::accessibility::MIN_CONTROL_POINTS],
                                egui::Label::new(
                                    egui::RichText::new("Rollout")
                                        .size(10.0)
                                        .color(t.text_muted),
                                ),
                            );
                            let rollout_response = ui
                                .add_enabled_ui(enabled && !state.environment_killed, |ui| {
                                    ui.add_sized(
                                        [
                                            ui.available_width(),
                                            crate::accessibility::MIN_CONTROL_POINTS,
                                        ],
                                        egui::Slider::new(&mut rollout, 0..=100)
                                            .suffix("%")
                                            .show_value(true),
                                    )
                                })
                                .inner;
                            let commit_rollout = rollout_response.drag_stopped()
                                || (rollout_response.changed() && !rollout_response.dragged());
                            if commit_rollout
                                && !crate::feature_flags::set_rollout_percent(
                                    state.feature,
                                    rollout,
                                )
                            {
                                self.developer_notice = Some(DeveloperNotice::error(
                                    "Could not save rollout percentage",
                                ));
                            }
                        });
                        ui.separator();
                    }

                    ui.add_space(12.0);
                    section_label(ui, "Exports", t.text_primary);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Export capabilities").clicked() {
                            let paths = self.diagnostic_paths();
                            self.developer_notice =
                                Some(match crate::capability_diagnostic::export(&paths) {
                                    Ok(path) => {
                                        DeveloperNotice::success("Capability report created", path)
                                    }
                                    Err(error) => DeveloperNotice::error(error),
                                });
                        }
                        if ui.button("Create support bundle").clicked() {
                            let paths = self.diagnostic_paths();
                            self.developer_notice =
                                Some(match crate::support_bundle::export(&paths) {
                                    Ok(path) => {
                                        DeveloperNotice::success("Support bundle created", path)
                                    }
                                    Err(error) => DeveloperNotice::error(error),
                                });
                        }
                    });
                    if let Some(notice) = &self.developer_notice {
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(&notice.message)
                                    .size(10.0)
                                    .color(if notice.error { t.accent_red } else { t.accent }),
                            );
                            if let Some(path) = &notice.path
                                && ui.button("Show in Finder").clicked()
                            {
                                let _ = open::that(path.parent().unwrap_or(path));
                            }
                        });
                        if let Some(path) = &notice.path {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(path.display().to_string())
                                        .size(9.0)
                                        .color(t.text_muted),
                                )
                                .truncate(),
                            )
                            .on_hover_text(path.display().to_string());
                        }
                    }
                });
            });
        self.show_developer_panel = open;
    }

    fn diagnostic_paths(&self) -> [PathBuf; 2] {
        [
            self.ws.left.current_path.clone(),
            self.ws.right.current_path.clone(),
        ]
    }
}

fn section_label(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.label(egui::RichText::new(text).size(12.0).strong().color(color));
    ui.add_space(3.0);
}

fn metric_row(ui: &mut egui::Ui, label: &str, value: usize, color: Color32) {
    text_row(ui, label, &value.to_string(), color);
}

fn text_row(ui: &mut egui::Ui, label: &str, value: &str, color: Color32) {
    ui.label(
        egui::RichText::new(label)
            .size(10.0)
            .color(ui.visuals().weak_text_color()),
    );
    ui.label(egui::RichText::new(value).size(10.0).color(color));
    ui.end_row();
}

fn percentile_text(latency: crate::measurement::LatencyPercentiles) -> String {
    if latency.samples == 0 {
        "No samples".to_string()
    } else {
        format!(
            "{:.1} / {:.1} / {:.1} ms",
            latency.p50_ms, latency.p95_ms, latency.p99_ms
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_state_distinguishes_missing_within_and_over() {
        assert_eq!(
            budget_state(crate::measurement::LatencyPercentiles::default(), 10.0),
            BudgetState::NoSamples
        );
        let within = crate::measurement::LatencyPercentiles {
            p50_ms: 2.0,
            p95_ms: 9.0,
            p99_ms: 12.0,
            samples: 10,
        };
        assert_eq!(budget_state(within, 10.0), BudgetState::Within);
        assert_eq!(budget_state(within, 8.0), BudgetState::Over);
    }
}
