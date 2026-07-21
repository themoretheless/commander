//! Streaming recursive search sheet. Query parsing and matching live in
//! `crate::query` and `crate::search`; this module owns only interaction state.

use super::*;
use crate::panel::format_size;
use std::time::SystemTime;

const AUTO_RUN_DELAY: f64 = 0.25;

fn index_phase_label(phase: crate::content_index::IndexPhase) -> &'static str {
    match phase {
        crate::content_index::IndexPhase::Disabled => "Off",
        crate::content_index::IndexPhase::Missing => "Not built",
        crate::content_index::IndexPhase::WaitingForIdle => "Waiting for idle",
        crate::content_index::IndexPhase::Building => "Building",
        crate::content_index::IndexPhase::Ready => "Ready",
        crate::content_index::IndexPhase::Error => "Error",
    }
}

fn index_freshness(built_at_secs: Option<u64>) -> String {
    let Some(built_at_secs) = built_at_secs else {
        return "Never".to_string();
    };
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let age = now.saturating_sub(built_at_secs);
    match age {
        0..=59 => "Just now".to_string(),
        60..=3_599 => format!("{}m ago", age / 60),
        3_600..=86_399 => format!("{}h ago", age / 3_600),
        _ => format!("{}d ago", age / 86_400),
    }
}

pub(super) fn format_index_exclusions(root: &std::path::Path, exclusions: &[PathBuf]) -> String {
    exclusions
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl App {
    fn poll_find_events(&mut self) {
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(state) = self.find.as_ref()
            && let Some(run) = state.run.as_ref()
        {
            loop {
                match run.try_recv() {
                    Ok(event) => events.push(event),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        let mut history_record = None;
        if let Some(state) = self.find.as_mut() {
            for event in events {
                match event {
                    crate::search::SearchEvent::Batch {
                        generation,
                        hits,
                        scanned,
                    } if generation == state.generation => {
                        state.scanned = scanned;
                        state.results.extend(hits);
                        state.matched = state.results.len();
                    }
                    crate::search::SearchEvent::Progress {
                        generation,
                        scanned,
                        matched,
                    } if generation == state.generation => {
                        state.scanned = scanned;
                        state.matched = matched;
                    }
                    crate::search::SearchEvent::Complete {
                        generation,
                        summary,
                    } if generation == state.generation => {
                        crate::search::stabilize_hits(&state.stable_order, &mut state.results);
                        state.stable_order = state
                            .results
                            .iter()
                            .map(|hit| hit.identity.clone())
                            .collect();
                        state.searching = false;
                        state.scanned = summary.scanned;
                        state.matched = summary.matched;
                        state.elapsed = summary.elapsed;
                        state.truncated = summary.truncated;
                        state.content_skipped = summary.content_skipped;
                        state.run = None;
                        if !summary.cancelled {
                            let (expression, mode, root) = state.last_query.clone().unwrap_or((
                                state.expression.clone(),
                                state.mode,
                                state.root.clone(),
                            ));
                            history_record = Some((expression, mode, root, summary));
                        }
                    }
                    _ => {}
                }
            }
            if disconnected && state.searching {
                state.searching = false;
                state.run = None;
                state.error = Some("Search worker stopped before completion".to_string());
            }
        }
        if let Some((expression, mode, root, summary)) = history_record {
            self.search_history.record(expression, mode, root, &summary);
        }
    }

    pub(crate) fn begin_find_search(&mut self, ctx: &egui::Context) {
        let Some(state) = self.find.as_ref() else {
            return;
        };
        let query = match state.build_query() {
            Ok(query) => query,
            Err(error) => {
                if let Some(state) = self.find.as_mut() {
                    state.error = Some(error.to_string());
                    state.pending_rerun = false;
                }
                return;
            }
        };
        let root = state.root.clone();
        let query_key = (query.to_expression(), query.mode, root.clone());
        let previous = if state.last_query.as_ref() == Some(&query_key) {
            state
                .results
                .iter()
                .map(|hit| hit.identity.clone())
                .collect()
        } else {
            Vec::new()
        };
        let index_status = self.content_index.status(&root);
        if index_status.enabled
            && matches!(
                index_status.phase,
                crate::content_index::IndexPhase::Missing | crate::content_index::IndexPhase::Error
            )
        {
            let repaint = ctx.clone();
            if self.content_index.start_build(
                root.clone(),
                std::sync::Arc::new(move || repaint.request_repaint()),
            ) && let Some(state) = self.find.as_mut()
            {
                state.index_rerun_after_build = true;
            }
        }
        let index = self
            .content_index
            .is_enabled(&root)
            .then(|| self.content_index.snapshot(&root))
            .flatten();
        let repaint = ctx.clone();
        let notify = std::sync::Arc::new(move || repaint.request_repaint());
        let run_result = match index {
            Some(index) => self.search_engine.start_indexed(
                index,
                query,
                crate::search::DEFAULT_RESULT_CAP,
                notify,
            ),
            None => {
                self.search_engine
                    .start(root, query, crate::search::DEFAULT_RESULT_CAP, notify)
            }
        };
        let run = match run_result {
            Ok(run) => run,
            Err(error) => {
                if let Some(state) = self.find.as_mut() {
                    state.error = Some(error.to_string());
                    state.pending_rerun = false;
                }
                return;
            }
        };
        let search_source = run.provider.to_string();
        if let Some(state) = self.find.as_mut() {
            state.generation = run.snapshot.generation;
            state.run = Some(run);
            state.search_source = search_source;
            state.results.clear();
            state.stable_order = previous;
            state.last_query = Some(query_key);
            state.searching = true;
            state.ran = true;
            state.scanned = 0;
            state.matched = 0;
            state.elapsed = std::time::Duration::ZERO;
            state.truncated = false;
            state.content_skipped = 0;
            state.error = None;
            state.pending_rerun = false;
            state.explanation_open = None;
        }
    }

    fn mark_find_edited(&mut self, ctx: &egui::Context) {
        self.search_engine.cancel();
        if let Some(state) = self.find.as_mut() {
            state.run = None;
            state.searching = false;
            state.pending_rerun = true;
            state.last_edit_at = ctx.input(|input| input.time);
            state.error = None;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    pub(crate) fn open_find(&mut self) {
        self.search_engine.cancel();
        let root = self.ws.active_panel_ref().current_path.clone();
        let index_exclusions =
            format_index_exclusions(&root, &self.content_index.exclusions(&root));
        self.find = Some(FindState {
            root,
            index_exclusions,
            ..Default::default()
        });
    }

    pub(crate) fn show_find_dialog(&mut self, ctx: &egui::Context) {
        if self.find.is_none() {
            return;
        }
        self.poll_find_events();

        let now_seconds = ctx.input(|input| input.time);
        let auto_run = self.find.as_ref().is_some_and(|state| {
            state.pending_rerun && now_seconds - state.last_edit_at >= AUTO_RUN_DELAY
        });
        if auto_run {
            self.begin_find_search(ctx);
        } else if self.find.as_ref().is_some_and(|state| state.pending_rerun) {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

        let t = self.colors;
        let history = self.search_history.entries.clone();
        let index_root = self.find.as_ref().unwrap().root.clone();
        let index_status = self.content_index.status(&index_root);
        let rerun_on_fresh_index = self.find.as_ref().is_some_and(|state| {
            state.index_rerun_after_build
                && index_status.phase == crate::content_index::IndexPhase::Ready
        });
        let index_build_failed = self.find.as_ref().is_some_and(|state| {
            state.index_rerun_after_build
                && index_status.phase == crate::content_index::IndexPhase::Error
        });
        if rerun_on_fresh_index && let Some(state) = self.find.as_mut() {
            state.index_rerun_after_build = false;
        }
        if index_build_failed && let Some(state) = self.find.as_mut() {
            state.index_rerun_after_build = false;
            state.error = index_status
                .last_error
                .clone()
                .or_else(|| Some("Content index build failed".to_string()));
        }
        let mut window_open = true;
        let mut run_now = rerun_on_fresh_index;
        let mut edited = false;
        let mut save = false;
        let mut reveal = None;
        let mut replay = None;
        let mut remove_predicate = None;
        let mut root_change = None;
        let mut explanation_change = None;
        let mut index_enabled_change = None;
        let mut index_rebuild = false;
        let mut index_apply_exclusions = false;

        {
            let state = self.find.as_mut().unwrap();
            egui::Window::new("Search")
                .open(&mut window_open)
                .collapsible(false)
                .resizable(true)
                .default_size([720.0, 620.0])
                .min_width(560.0)
                .min_height(420.0)
                .frame(
                    Frame::NONE
                        .fill(t.bg_panel)
                        .inner_margin(Margin::same(14))
                        .stroke(Stroke::new(1.0_f32, t.border)),
                )
                .show(ctx, |ui| {
                    let crumbs = crate::crumbs::crumbs(&state.root);
                    let layout = crate::crumbs::elide_crumbs(&crumbs, 6);
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .selectable_label(
                                layout.head.full_path == state.root,
                                egui::RichText::new(&layout.head.label).size(11.0),
                            )
                            .on_hover_text(layout.head.full_path.display().to_string())
                            .clicked()
                        {
                            root_change = Some(layout.head.full_path.clone());
                        }
                        if !layout.collapsed.is_empty() {
                            ui.label(egui::RichText::new("/").color(t.text_muted));
                            ui.label(egui::RichText::new("...").color(t.text_muted))
                                .on_hover_text(
                                    layout
                                        .collapsed
                                        .iter()
                                        .map(|crumb| crumb.label.as_str())
                                        .collect::<Vec<_>>()
                                        .join("/"),
                                );
                        }
                        for crumb in &layout.tail {
                            ui.label(egui::RichText::new("/").color(t.text_muted));
                            if ui
                                .selectable_label(
                                    crumb.full_path == state.root,
                                    egui::RichText::new(&crumb.label).size(11.0),
                                )
                                .on_hover_text(crumb.full_path.display().to_string())
                                .clicked()
                            {
                                root_change = Some(crumb.full_path.clone());
                            }
                        }
                    });
                    ui.add_space(6.0);

                    let response = ui.add(
                        egui::TextEdit::singleline(&mut state.expression)
                            .desired_width(f32::INFINITY)
                            .hint_text("Search files...")
                            .margin(egui::vec2(8.0, 7.0)),
                    );
                    if !state.focused {
                        response.request_focus();
                        state.focused = true;
                    }
                    edited |= response.changed();
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        run_now = true;
                    }

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        for mode in crate::query::MatchMode::ALL {
                            edited |= ui
                                .selectable_value(&mut state.mode, mode, mode.label())
                                .changed();
                        }
                        ui.separator();
                        if ui.selectable_label(state.history_open, "History").clicked() {
                            state.history_open = !state.history_open;
                        }
                        ui.separator();
                        let mut index_enabled = index_status.enabled;
                        if ui
                            .checkbox(&mut index_enabled, "Index")
                            .on_hover_text("Use the root content index when it is ready")
                            .changed()
                        {
                            index_enabled_change = Some(index_enabled);
                        }
                        if ui
                            .selectable_label(
                                state.index_details_open,
                                index_phase_label(index_status.phase),
                            )
                            .on_hover_text("Content index status")
                            .clicked()
                        {
                            state.index_details_open = !state.index_details_open;
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(if state.searching {
                                            "Restart"
                                        } else {
                                            "Search"
                                        })
                                        .color(Color32::WHITE),
                                    )
                                    .fill(t.accent)
                                    .corner_radius(CornerRadius::ZERO),
                                )
                                .clicked()
                            {
                                run_now = true;
                            }
                        });
                    });

                    if let Ok(query) = state.build_query()
                        && !query.predicates.is_empty()
                    {
                        ui.add_space(4.0);
                        ui.horizontal_wrapped(|ui| {
                            for (index, chip) in query.chips().iter().enumerate() {
                                if ui
                                    .add(
                                        egui::Button::new(format!("{chip}  x"))
                                            .fill(t.bg_card)
                                            .corner_radius(CornerRadius::same(2)),
                                    )
                                    .on_hover_text("Remove filter")
                                    .clicked()
                                {
                                    remove_predicate = Some(index);
                                }
                            }
                        });
                    }

                    if state.history_open {
                        ui.add_space(6.0);
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .id_salt("search-history")
                            .max_height(130.0)
                            .show(ui, |ui| {
                                if history.is_empty() {
                                    ui.label(
                                        egui::RichText::new("No completed searches")
                                            .size(11.0)
                                            .color(t.text_muted),
                                    );
                                }
                                for entry in &history {
                                    let label = if entry.expression.is_empty() {
                                        "All files"
                                    } else {
                                        &entry.expression
                                    };
                                    let response = ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!(
                                                "{}  |  {} results  |  {} ms",
                                                label, entry.result_count, entry.duration_ms
                                            ))
                                            .size(11.0)
                                            .color(t.text_secondary),
                                        )
                                        .sense(Sense::click()),
                                    );
                                    if response.clicked() {
                                        replay = Some(entry.clone());
                                    }
                                }
                            });
                        ui.separator();
                    }

                    if state.index_details_open {
                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let phase_color = match index_status.phase {
                                crate::content_index::IndexPhase::Error => t.accent_red,
                                crate::content_index::IndexPhase::Building
                                | crate::content_index::IndexPhase::WaitingForIdle => {
                                    t.accent_warning
                                }
                                crate::content_index::IndexPhase::Ready => t.accent,
                                _ => t.text_muted,
                            };
                            ui.label(
                                egui::RichText::new("Content index")
                                    .size(11.0)
                                    .strong()
                                    .color(t.text_secondary),
                            );
                            ui.label(
                                egui::RichText::new(index_phase_label(index_status.phase))
                                    .size(11.0)
                                    .color(phase_color),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add_enabled(index_status.enabled, egui::Button::new("Rebuild"))
                                    .clicked()
                                {
                                    index_rebuild = true;
                                }
                            });
                        });

                        if matches!(
                            index_status.phase,
                            crate::content_index::IndexPhase::Building
                                | crate::content_index::IndexPhase::WaitingForIdle
                        ) {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} scanned  |  {} indexed",
                                        index_status.progress.scanned,
                                        index_status.progress.indexed
                                    ))
                                    .size(11.0)
                                    .color(t.text_muted),
                                );
                            });
                        }

                        let coverage = index_status.coverage_percent();
                        ui.add(
                            egui::ProgressBar::new((coverage / 100.0).clamp(0.0, 1.0)).text(
                                format!(
                                    "{coverage:.0}% text coverage  |  {} files  |  {}",
                                    index_status.progress.files_seen,
                                    format_size(index_status.progress.content_bytes as u64)
                                ),
                            ),
                        );
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Freshness: {}",
                                    index_freshness(index_status.built_at_secs)
                                ))
                                .size(10.0)
                                .color(t.text_muted),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "Skipped: {} binary, {} large, {} unreadable, {} quota",
                                    index_status.skipped_binary,
                                    index_status.skipped_large,
                                    index_status.skipped_unreadable,
                                    index_status.skipped_quota
                                ))
                                .size(10.0)
                                .color(t.text_muted),
                            );
                            if index_status.truncated {
                                ui.label(
                                    egui::RichText::new("Document cap reached")
                                        .size(10.0)
                                        .color(t.accent_warning),
                                );
                            }
                        });
                        if let Some(error) = &index_status.last_error {
                            ui.label(egui::RichText::new(error).size(10.0).color(t.accent_red));
                        }
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Excluded paths ({})",
                                    index_status.excluded_roots.len()
                                ))
                                .size(10.0)
                                .color(t.text_muted),
                            );
                            if ui.button("Apply").clicked() {
                                index_apply_exclusions = true;
                            }
                        });
                        ui.add(
                            egui::TextEdit::multiline(&mut state.index_exclusions)
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                .hint_text("cache\nbuild"),
                        );
                        ui.add_space(4.0);
                        ui.separator();
                    }

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let status = if state.searching {
                            format!(
                                "{}  |  Scanning {} items  |  {} matches",
                                state.search_source,
                                state.scanned,
                                state.results.len()
                            )
                        } else if state.ran {
                            format!(
                                "{}  |  {} results from {} items  |  {} ms{}",
                                state.search_source,
                                state.results.len(),
                                state.scanned,
                                state.elapsed.as_millis(),
                                if state.truncated { "  |  capped" } else { "" }
                            )
                        } else {
                            "Ready".to_string()
                        };
                        ui.label(egui::RichText::new(status).size(11.0).color(t.text_muted));
                        if state.content_skipped > 0 {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} unreadable/binary",
                                    state.content_skipped
                                ))
                                .size(11.0)
                                .color(t.accent_warning),
                            );
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            for pivot in crate::search::TimePivot::ALL.into_iter().rev() {
                                ui.selectable_value(&mut state.time_pivot, pivot, pivot.label());
                            }
                        });
                    });
                    if let Some(error) = &state.error {
                        ui.label(egui::RichText::new(error).size(11.0).color(t.accent_red));
                    }
                    ui.add_space(4.0);
                    ui.separator();

                    let clock = SystemTime::now();
                    let mut indices: Vec<usize> = (0..state.results.len()).collect();
                    if state.time_pivot != crate::search::TimePivot::None {
                        indices.sort_by_key(|index| {
                            (
                                crate::search::time_bucket(
                                    &state.results[*index],
                                    state.time_pivot,
                                    clock,
                                ),
                                *index,
                            )
                        });
                    }
                    let pivot = state.time_pivot;
                    let root = state.root.clone();
                    let open_explanation = state.explanation_open.clone();
                    egui::ScrollArea::vertical()
                        .id_salt("search-results")
                        .auto_shrink([false, false])
                        .max_height((ui.available_height() - 90.0).max(140.0))
                        .show_rows(ui, 42.0, indices.len(), |ui, rows| {
                            for row in rows {
                                let index = indices[row];
                                let hit = &state.results[index];
                                let identity = hit.identity.clone();
                                let path = hit.reveal_path().to_path_buf();
                                let relative = hit.relative_to(&root).display().to_string();
                                let size = if hit.entry.is_dir {
                                    String::new()
                                } else {
                                    format_size(hit.entry.size)
                                };
                                let explanation = hit.explanation.summary();
                                if pivot != crate::search::TimePivot::None {
                                    let bucket = crate::search::time_bucket(hit, pivot, clock);
                                    let previous = row.checked_sub(1).map(|prior_row| {
                                        crate::search::time_bucket(
                                            &state.results[indices[prior_row]],
                                            pivot,
                                            clock,
                                        )
                                    });
                                    if previous != Some(bucket) {
                                        ui.label(
                                            egui::RichText::new(bucket.label())
                                                .size(10.0)
                                                .strong()
                                                .color(t.text_muted),
                                        );
                                    } else {
                                        ui.add_space(12.0);
                                    }
                                } else {
                                    ui.add_space(4.0);
                                }
                                ui.horizontal(|ui| {
                                    let response = ui.add_sized(
                                        [ui.available_width() - 92.0, 22.0],
                                        egui::Label::new(
                                            egui::RichText::new(relative)
                                                .size(12.0)
                                                .color(t.text_secondary),
                                        )
                                        .truncate()
                                        .sense(Sense::click()),
                                    );
                                    if response.clicked() {
                                        reveal = Some(path);
                                    }
                                    ui.label(
                                        egui::RichText::new(size).size(10.0).color(t.text_muted),
                                    );
                                    if ui.small_button("?").on_hover_text(&explanation).clicked() {
                                        explanation_change =
                                            Some(if open_explanation.as_ref() == Some(&identity) {
                                                None
                                            } else {
                                                Some(identity)
                                            });
                                    }
                                });
                            }
                        });

                    if let Some(identity) = &state.explanation_open
                        && let Some(hit) =
                            state.results.iter().find(|hit| &hit.identity == identity)
                    {
                        ui.separator();
                        ui.label(
                            egui::RichText::new(hit.explanation.summary())
                                .size(11.0)
                                .color(t.text_secondary),
                        );
                    }

                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut state.save_name)
                                .desired_width(180.0)
                                .hint_text("Saved search name")
                                .margin(egui::vec2(6.0, 4.0)),
                        );
                        if ui
                            .add_enabled(
                                !state.save_name.trim().is_empty() && state.build_query().is_ok(),
                                egui::Button::new("Save"),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                    });
                });
        }

        if !window_open {
            self.search_engine.cancel();
            self.find = None;
            return;
        }
        if let Some(enabled) = index_enabled_change {
            if self.content_index.set_enabled(index_root.clone(), enabled) {
                if enabled {
                    let repaint = ctx.clone();
                    let started = self.content_index.start_build(
                        index_root.clone(),
                        std::sync::Arc::new(move || repaint.request_repaint()),
                    );
                    if let Some(state) = self.find.as_mut() {
                        state.index_rerun_after_build = started;
                        state.error = None;
                    }
                } else if let Some(state) = self.find.as_mut() {
                    state.index_rerun_after_build = false;
                    state.error = None;
                }
                edited = true;
            } else if let Some(state) = self.find.as_mut() {
                state.error = Some("Could not save content index settings".to_string());
            }
        }
        if index_apply_exclusions {
            let values = self
                .find
                .as_ref()
                .map(|state| {
                    state
                        .index_exclusions
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .map(PathBuf::from)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            match self.content_index.set_exclusions(&index_root, values) {
                Ok(()) => {
                    let formatted = format_index_exclusions(
                        &index_root,
                        &self.content_index.exclusions(&index_root),
                    );
                    let mut started = false;
                    if self.content_index.is_enabled(&index_root) {
                        let repaint = ctx.clone();
                        started = self.content_index.start_build(
                            index_root.clone(),
                            std::sync::Arc::new(move || repaint.request_repaint()),
                        );
                    }
                    if let Some(state) = self.find.as_mut() {
                        state.index_exclusions = formatted;
                        state.index_rerun_after_build = started;
                        state.error = None;
                    }
                }
                Err(error) => {
                    if let Some(state) = self.find.as_mut() {
                        state.error = Some(error);
                    }
                }
            }
        }
        if index_rebuild {
            let repaint = ctx.clone();
            let started = self.content_index.start_build(
                index_root.clone(),
                std::sync::Arc::new(move || repaint.request_repaint()),
            );
            if let Some(state) = self.find.as_mut() {
                state.index_rerun_after_build = started;
                if !started {
                    state.error = Some("Enable the content index before rebuilding".to_string());
                } else {
                    state.error = None;
                }
            }
        }
        if let Some(entry) = replay {
            let exclusions =
                format_index_exclusions(&entry.root, &self.content_index.exclusions(&entry.root));
            if let Some(state) = self.find.as_mut() {
                state.expression = entry.expression;
                state.mode = entry.mode;
                state.root = entry.root;
                state.index_exclusions = exclusions;
                state.index_rerun_after_build = false;
                state.history_open = false;
            }
            edited = true;
        }
        if let Some(path) = root_change {
            let exclusions = format_index_exclusions(&path, &self.content_index.exclusions(&path));
            if let Some(state) = self.find.as_mut() {
                state.root = path;
                state.index_exclusions = exclusions;
                state.index_rerun_after_build = false;
            }
            edited = true;
        }
        if let Some(index) = remove_predicate
            && let Some(state) = self.find.as_mut()
            && let Ok(mut query) = state.build_query()
            && index < query.predicates.len()
        {
            query.predicates.remove(index);
            state.expression = query.to_expression();
            edited = true;
        }
        if let Some(value) = explanation_change
            && let Some(state) = self.find.as_mut()
        {
            state.explanation_open = value;
        }
        if edited {
            self.mark_find_edited(ctx);
        }
        if run_now {
            self.begin_find_search(ctx);
        }
        if let Some(path) = reveal {
            self.ws.reveal(&path);
        }
        if save {
            let definition = self.find.as_ref().and_then(|state| {
                state
                    .build_query()
                    .ok()
                    .map(|query| crate::smart_folder::Definition {
                        name: state.save_name.trim().to_string(),
                        root: state.root.clone(),
                        query,
                    })
            });
            if let Some(definition) = definition {
                let name = definition.name.clone();
                self.smart_folders_mut().add(definition);
                let saved = crate::smart_folder::save(self.smart_folders_mut());
                let now = ctx.input(|input| input.time);
                let (message, kind) = if saved {
                    (
                        format!("Saved search \"{name}\""),
                        crate::toasts::ToastKind::Success,
                    )
                } else {
                    (
                        format!("Could not save search \"{name}\""),
                        crate::toasts::ToastKind::Error,
                    )
                };
                self.toasts
                    .push(crate::toasts::Toast::new(message, kind, false, now));
            }
        }
    }
}
