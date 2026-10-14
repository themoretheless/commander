//! Streaming recursive search sheet. Query parsing and matching live in
//! `crate::query` and `crate::search`; this module owns only interaction state.

use super::*;
use crate::panel::format_size;
use std::time::SystemTime;

const AUTO_RUN_DELAY: f64 = 0.25;

/// Fixed height of one search result row. `show_rows` positions rows on this
/// stride, so every row is allocated exactly this tall: a time-bucket header
/// (or its gap) plus the 22px result line, or a 4px gap plus the line.
const fn result_row_height(pivot_active: bool) -> f32 {
    if pivot_active { 40.0 } else { 26.0 }
}

/// Display order of the results: grouped by time bucket under a pivot,
/// stable within a bucket. Each hit's bucket (a local-calendar conversion) is
/// computed once, not once per sort comparison.
fn pivot_order(
    results: &[crate::search::SearchHit],
    pivot: crate::search::TimePivot,
    clock: SystemTime,
) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..results.len()).collect();
    if pivot != crate::search::TimePivot::None {
        indices.sort_by_cached_key(|&index| {
            (
                crate::search::time_bucket(&results[index], pivot, clock),
                index,
            )
        });
    }
    indices
}

const fn index_phase_label(phase: crate::content_index::IndexPhase) -> &'static str {
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

/// Why a search cannot start in `root`: a saved search or history entry can
/// name a folder that was since moved or deleted, and walking it would just
/// report zero results.
fn search_root_error(root: &std::path::Path) -> Option<String> {
    match std::fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => None,
        Ok(_) => Some(format!(
            "Search folder {} is not a folder. Pick another folder above.",
            root.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(format!(
            "Search folder not found: {}. Pick another folder above.",
            root.display()
        )),
        Err(error) => Some(format!(
            "Could not open search folder {}: {error}",
            root.display()
        )),
    }
}

/// Message for an index build that did not start. The index keeps the real
/// reason (runtime control, battery/thermal pressure, a full queue).
fn index_build_refusal(enabled: bool, last_error: Option<String>) -> String {
    if enabled {
        last_error.unwrap_or_else(|| "Content index build did not start; try again later".into())
    } else {
        "Enable the content index before rebuilding".to_string()
    }
}

/// The nonzero skip counters of an index build, or `None` when nothing was
/// skipped.
fn index_skipped_summary(
    binary: usize,
    large: usize,
    unreadable: usize,
    quota: usize,
) -> Option<String> {
    let parts: Vec<String> = [
        (binary, "binary"),
        (large, "over the file size limit"),
        (unreadable, "unreadable"),
        (quota, "over the index size limit"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, label)| format!("{count} {label}"))
    .collect();
    (!parts.is_empty()).then(|| format!("Skipped: {}", parts.join(", ")))
}

/// One history row: what was searched, how, and where, so entries for
/// different folders are told apart.
fn history_row_label(entry: &crate::search::QueryHistoryEntry) -> String {
    let expression = if entry.expression.is_empty() {
        "All files"
    } else {
        &entry.expression
    };
    let folder = entry.root.file_name().map_or_else(
        || entry.root.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    format!(
        "{expression}  |  {}  |  in {folder}  |  {} results  |  {} ms",
        entry.mode.label(),
        entry.result_count,
        entry.duration_ms
    )
}

/// Add `definition` and persist the store; if saving fails the store is
/// rolled back so the UI never shows a search that is lost on restart.
fn add_smart_folder(
    store: &mut crate::smart_folder::SmartFolders,
    definition: crate::smart_folder::Definition,
    save: impl FnOnce(&crate::smart_folder::SmartFolders) -> bool,
) -> bool {
    let previous = store.clone();
    store.add(definition);
    let saved = save(store);
    if !saved {
        *store = previous;
    }
    saved
}

impl App {
    fn poll_find_events(
        &mut self,
    ) -> Option<(
        String,
        crate::query::MatchMode,
        PathBuf,
        crate::search::SearchSummary,
    )> {
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(state) = self.ui.modals.find.as_ref()
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
        if let Some(state) = self.ui.modals.find.as_mut() {
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
        history_record
    }

    pub(crate) fn begin_find_search(&mut self, ctx: &egui::Context) {
        let Some(state) = self.ui.modals.find.as_ref() else {
            return;
        };
        let query = match state.build_query() {
            Ok(query) => query,
            Err(error) => {
                if let Some(state) = self.ui.modals.find.as_mut() {
                    state.error = Some(error.to_string());
                    state.pending_rerun = false;
                }
                return;
            }
        };
        let root = state.root.clone();
        if let Some(error) = search_root_error(&root) {
            if let Some(state) = self.ui.modals.find.as_mut() {
                state.error = Some(error);
                state.pending_rerun = false;
            }
            return;
        }
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
            ) && let Some(state) = self.ui.modals.find.as_mut()
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
                if let Some(state) = self.ui.modals.find.as_mut() {
                    state.error = Some(error.to_string());
                    state.pending_rerun = false;
                }
                return;
            }
        };
        let search_source = run.provider.to_string();
        if let Some(state) = self.ui.modals.find.as_mut() {
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
        if let Some(state) = self.ui.modals.find.as_mut() {
            state.run = None;
            state.searching = false;
            state.pending_rerun = true;
            state.last_edit_at = ctx.input(|input| input.time);
            state.error = None;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }

    pub(crate) fn open_find(&mut self, ctx: &egui::Context) {
        self.search_engine.cancel();
        let root = self.ws.active_panel_ref().current_path.clone();
        let index_exclusions =
            format_index_exclusions(&root, &self.content_index.exclusions(&root));
        self.ui.modals.find = Some(FindState {
            root,
            index_exclusions,
            ..Default::default()
        });
        Self::mark_modal_opened(ctx, UiModal::Find);
    }

    pub(crate) fn show_find_dialog(&mut self, ctx: &egui::Context) {
        let escape_requested = self.take_modal_escape(crate::accessibility::ModalSurface::Find);
        if self.ui.modals.find.is_none() {
            return;
        }
        if super::ui_state::modal_close_requested(true, escape_requested) {
            self.search_engine.cancel();
            self.ui.modals.find = None;
            return;
        }
        let history_record = self.poll_find_events();

        let now_seconds = ctx.input(|input| input.time);
        let auto_run = self.ui.modals.find.as_ref().is_some_and(|state| {
            state.pending_rerun && now_seconds - state.last_edit_at >= AUTO_RUN_DELAY
        });
        if auto_run {
            self.begin_find_search(ctx);
        } else if self
            .ui
            .modals
            .find
            .as_ref()
            .is_some_and(|state| state.pending_rerun)
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

        let t = self.colors;
        let history = self.search_history.entries.clone();
        let index_root = self.ui.modals.find.as_ref().unwrap().root.clone();
        let index_status = self.content_index.status(&index_root);
        let rerun_on_fresh_index = self.ui.modals.find.as_ref().is_some_and(|state| {
            state.index_rerun_after_build
                && index_status.phase == crate::content_index::IndexPhase::Ready
        });
        let index_build_failed = self.ui.modals.find.as_ref().is_some_and(|state| {
            state.index_rerun_after_build
                && index_status.phase == crate::content_index::IndexPhase::Error
        });
        if rerun_on_fresh_index && let Some(state) = self.ui.modals.find.as_mut() {
            state.index_rerun_after_build = false;
        }
        if index_build_failed && let Some(state) = self.ui.modals.find.as_mut() {
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
        let mut open_path = None;
        let mut replay = None;
        let mut remove_predicate = None;
        let mut root_change = None;
        let mut explanation_change = None;
        let mut index_enabled_change = None;
        let mut index_rebuild = false;
        let mut index_apply_exclusions = false;

        {
            let state = self.ui.modals.find.as_mut().unwrap();
            egui::Window::new("Search")
                .open(&mut window_open)
                .collapsible(false)
                .resizable(true)
                .default_size([720.0, 620.0])
                .min_width(560.0)
                .min_height(420.0)
                .frame(super::dialog_frame(t, 14))
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
                        // Collapsed ancestors behind a "..." menu, as in the
                        // panel path bar.
                        if !layout.collapsed.is_empty() {
                            ui.label(egui::RichText::new("/").color(t.text_muted));
                            ui.menu_button("\u{2026}", |ui| {
                                for crumb in &layout.collapsed {
                                    if ui
                                        .button(&crumb.label)
                                        .on_hover_text(crumb.full_path.display().to_string())
                                        .clicked()
                                    {
                                        root_change = Some(crumb.full_path.clone());
                                        ui.close();
                                    }
                                }
                            })
                            .response
                            .on_hover_text("Choose a hidden parent folder");
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
                    if Self::take_modal_opened(ui.ctx(), UiModal::Find) {
                        response.request_focus();
                    }
                    edited |= response.changed();
                    if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        run_now = true;
                        // A single-line field gives up focus on Enter; keep
                        // it so the query can be refined right away.
                        response.request_focus();
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
                                .add(super::dialog_button(
                                    t,
                                    super::ButtonRole::Primary,
                                    egui::RichText::new(if state.searching {
                                        "Restart"
                                    } else {
                                        "Search"
                                    }),
                                ))
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
                                let chip_response = ui
                                    .add(
                                        egui::Button::new(format!("{chip}  x"))
                                            .fill(t.bg_card)
                                            .corner_radius(CornerRadius::same(2)),
                                    )
                                    .on_hover_text("Remove filter");
                                chip_response.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::WidgetType::Button,
                                        true,
                                        format!("Remove filter {chip}"),
                                    )
                                });
                                if chip_response.clicked() {
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
                                    let label = history_row_label(entry);
                                    let response =
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&label)
                                                    .size(11.0)
                                                    .color(t.text_secondary),
                                            )
                                            .sense(Sense::click()),
                                        )
                                        .on_hover_text(
                                            format!("Replay in {}", entry.root.display()),
                                        );
                                    response.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::WidgetType::Button,
                                            true,
                                            format!("Replay search {label}"),
                                        )
                                    });
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
                            if let Some(skipped) = index_skipped_summary(
                                index_status.skipped_binary,
                                index_status.skipped_large,
                                index_status.skipped_unreadable,
                                index_status.skipped_quota,
                            ) {
                                ui.label(
                                    egui::RichText::new(skipped).size(10.0).color(t.text_muted),
                                )
                                .on_hover_text(format!(
                                    "Text is indexed for files up to {} and up to {} in total",
                                    format_size(crate::content_index::MAX_FILE_BYTES),
                                    format_size(crate::content_index::MAX_CONTENT_BYTES as u64)
                                ));
                            }
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
                        let status =
                            ui.label(egui::RichText::new(status).size(11.0).color(t.text_muted));
                        // Announce progress and the finished count.
                        ui.ctx().accesskit_node_builder(status.id, |node| {
                            node.set_role(egui::accesskit::Role::Status);
                            node.set_live(egui::accesskit::Live::Polite);
                        });
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
                    let indices = pivot_order(&state.results, state.time_pivot, clock);
                    let pivot = state.time_pivot;
                    let row_height = result_row_height(pivot != crate::search::TimePivot::None);
                    let root = state.root.clone();
                    let open_explanation = state.explanation_open.clone();
                    egui::ScrollArea::vertical()
                        .id_salt("search-results")
                        .auto_shrink([false, false])
                        .max_height((ui.available_height() - 90.0).max(140.0))
                        .show_rows(ui, row_height, indices.len(), |ui, rows| {
                            for row in rows {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(ui.available_width(), row_height),
                                    Layout::top_down(Align::Min),
                                    |ui| {
                                        ui.set_min_height(row_height);
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
                                            let bucket =
                                                crate::search::time_bucket(hit, pivot, clock);
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
                                            let response = ui
                                                .add_sized(
                                                    [ui.available_width() - 92.0, 22.0],
                                                    egui::Label::new(
                                                        egui::RichText::new(&relative)
                                                            .size(12.0)
                                                            .color(t.text_secondary),
                                                    )
                                                    .truncate()
                                                    .sense(Sense::click()),
                                                )
                                                .on_hover_text(
                                                    "Click to show in the panel, double-click \
                                                     to open",
                                                );
                                            response.widget_info(|| {
                                                egui::WidgetInfo::labeled(
                                                    egui::WidgetType::Button,
                                                    true,
                                                    format!("Reveal {relative}"),
                                                )
                                            });
                                            if response.double_clicked() && !hit.entry.is_dir {
                                                open_path = Some(path.clone());
                                            }
                                            if response.clicked() {
                                                reveal = Some(path);
                                            }
                                            ui.label(
                                                egui::RichText::new(size)
                                                    .size(10.0)
                                                    .color(t.text_muted),
                                            );
                                            let explaining =
                                                open_explanation.as_ref() == Some(&identity);
                                            let why = ui
                                                .add(
                                                    egui::Button::selectable(explaining, "?")
                                                        .small(),
                                                )
                                                .on_hover_text(&explanation);
                                            why.widget_info(|| {
                                                egui::WidgetInfo::selected(
                                                    egui::WidgetType::Button,
                                                    true,
                                                    explaining,
                                                    format!("Why matched: {relative}"),
                                                )
                                            });
                                            if why.clicked() {
                                                explanation_change =
                                                    Some((!explaining).then_some(identity));
                                            }
                                        });
                                    },
                                );
                            }
                        });

                    if state.ran && !state.searching && state.results.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "No files match in {}. Try a broader query, another match \
                                 mode, or remove filters.",
                                state.root.display()
                            ))
                            .size(11.0)
                            .color(t.text_muted),
                        );
                    }
                    if let Some(identity) = &state.explanation_open
                        && let Some(hit) =
                            state.results.iter().find(|hit| &hit.identity == identity)
                    {
                        ui.separator();
                        ui.label(
                            egui::RichText::new(format!(
                                "{}: {}",
                                hit.relative_to(&state.root).display(),
                                hit.explanation.summary()
                            ))
                            .size(11.0)
                            .color(t.text_secondary),
                        );
                    }

                    ui.separator();
                    ui.horizontal(|ui| {
                        let name = ui.add(
                            egui::TextEdit::singleline(&mut state.save_name)
                                .desired_width(180.0)
                                .hint_text("Saved search name")
                                .margin(egui::vec2(6.0, 4.0)),
                        );
                        let can_save =
                            !state.save_name.trim().is_empty() && state.build_query().is_ok();
                        let enter = name.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if ui
                            .add_enabled(can_save, egui::Button::new("Save"))
                            .clicked()
                            || (enter && can_save)
                        {
                            save = true;
                        }
                    });
                });
        }

        if super::ui_state::modal_close_requested(window_open, false) {
            self.search_engine.cancel();
            self.ui.modals.find = None;
            return;
        }
        if let Some((expression, mode, root, summary)) = history_record {
            self.search_history.record(expression, mode, root, &summary);
        }
        if let Some(enabled) = index_enabled_change {
            if self.content_index.set_enabled(index_root.clone(), enabled) {
                if enabled {
                    let repaint = ctx.clone();
                    let started = self.content_index.start_build(
                        index_root.clone(),
                        std::sync::Arc::new(move || repaint.request_repaint()),
                    );
                    let refusal = (!started).then(|| {
                        index_build_refusal(true, self.content_index.status(&index_root).last_error)
                    });
                    if let Some(state) = self.ui.modals.find.as_mut() {
                        state.index_rerun_after_build = started;
                        state.error = refusal;
                    }
                } else if let Some(state) = self.ui.modals.find.as_mut() {
                    state.index_rerun_after_build = false;
                    state.error = None;
                }
                edited = true;
            } else if let Some(state) = self.ui.modals.find.as_mut() {
                state.error = Some("Could not save content index settings".to_string());
            }
        }
        if index_apply_exclusions {
            let values = self
                .ui
                .modals
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
                    let mut refusal = None;
                    if self.content_index.is_enabled(&index_root) {
                        let repaint = ctx.clone();
                        started = self.content_index.start_build(
                            index_root.clone(),
                            std::sync::Arc::new(move || repaint.request_repaint()),
                        );
                        if !started {
                            refusal = Some(index_build_refusal(
                                true,
                                self.content_index.status(&index_root).last_error,
                            ));
                        }
                    }
                    if let Some(state) = self.ui.modals.find.as_mut() {
                        state.index_exclusions = formatted;
                        state.index_rerun_after_build = started;
                        state.error = refusal;
                    }
                }
                Err(error) => {
                    if let Some(state) = self.ui.modals.find.as_mut() {
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
            let refusal = (!started).then(|| {
                index_build_refusal(
                    self.content_index.is_enabled(&index_root),
                    self.content_index.status(&index_root).last_error,
                )
            });
            if let Some(state) = self.ui.modals.find.as_mut() {
                state.index_rerun_after_build = started;
                state.error = refusal;
            }
        }
        if let Some(entry) = replay {
            let exclusions =
                format_index_exclusions(&entry.root, &self.content_index.exclusions(&entry.root));
            if let Some(state) = self.ui.modals.find.as_mut() {
                state.expression = entry.expression;
                state.mode = entry.mode;
                state.root = entry.root;
                state.index_exclusions = exclusions;
                state.index_rerun_after_build = false;
                state.history_open = false;
            }
            edited = true;
        }
        // Choosing the crumb that already is the root changes nothing, so it
        // must not cancel and restart a running search.
        if let Some(path) = root_change.filter(|path| *path != index_root) {
            let exclusions = format_index_exclusions(&path, &self.content_index.exclusions(&path));
            if let Some(state) = self.ui.modals.find.as_mut() {
                state.root = path;
                state.index_exclusions = exclusions;
                state.index_rerun_after_build = false;
            }
            edited = true;
        }
        if let Some(index) = remove_predicate
            && let Some(state) = self.ui.modals.find.as_mut()
            && let Ok(mut query) = state.build_query()
            && index < query.predicates.len()
        {
            query.predicates.remove(index);
            state.expression = query.to_expression();
            edited = true;
        }
        if let Some(value) = explanation_change
            && let Some(state) = self.ui.modals.find.as_mut()
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
        if let Some(path) = open_path {
            self.open_external(crate::ports::OpenRequest::OpenPath(path), ctx);
        }
        if save {
            let definition = self.ui.modals.find.as_ref().and_then(|state| {
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
                let saved = add_smart_folder(
                    self.smart_folders_mut(),
                    definition,
                    crate::smart_folder::save,
                );
                if saved && let Some(state) = self.ui.modals.find.as_mut() {
                    state.save_name.clear();
                }
                let (message, kind) = if saved {
                    (
                        format!("Saved search \"{name}\""),
                        crate::toasts::ToastKind::Success,
                    )
                } else {
                    (
                        format!(
                            "Could not save search \"{name}\"; it was not kept. Check that \
                             the settings folder is writable."
                        ),
                        crate::toasts::ToastKind::Error,
                    )
                };
                self.toast(ctx, kind, message, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        add_smart_folder, history_row_label, index_build_refusal, index_skipped_summary,
        pivot_order, result_row_height, search_root_error,
    };
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn hit(name: &str, modified: Option<SystemTime>) -> crate::search::SearchHit {
        let path = PathBuf::from(format!("/x/{name}"));
        crate::search::SearchHit {
            entry: crate::panel::FileEntry {
                name: name.to_string(),
                name_lower: name.to_lowercase(),
                path: path.clone(),
                identity: crate::panel::ListingIdentity::Unavailable,
                is_dir: false,
                size: 1,
                extension: String::new(),
                modified,
                modified_str: String::new(),
                size_str: String::new(),
            },
            identity: crate::search::FileIdentity::Native { volume: 1, file: 1 },
            accessed: None,
            explanation: crate::search::MatchExplanation::default(),
            location: crate::search::SearchLocation::Native(path),
        }
    }

    #[test]
    fn pivot_groups_results_by_bucket_and_keeps_rank_within_one() {
        let now = SystemTime::now();
        let old = now - Duration::from_secs(400 * 86_400);
        let results = [
            hit("old-a", Some(old)),
            hit("new-a", Some(now)),
            hit("unknown", None),
            hit("old-b", Some(old)),
            hit("new-b", Some(now)),
        ];
        assert_eq!(
            pivot_order(&results, crate::search::TimePivot::None, now),
            [0, 1, 2, 3, 4]
        );
        let order = pivot_order(&results, crate::search::TimePivot::Modified, now);
        let names: Vec<_> = order
            .iter()
            .map(|&index| results[index].entry.name.as_str())
            .collect();
        let bucket = |name: &str| {
            let index = results.iter().position(|h| h.entry.name == name).unwrap();
            crate::search::time_bucket(&results[index], crate::search::TimePivot::Modified, now)
        };
        // Sorted by bucket, original rank breaking ties.
        assert!(
            names
                .windows(2)
                .all(|pair| bucket(pair[0]) <= bucket(pair[1]))
        );
        let new_a = names.iter().position(|n| *n == "new-a").unwrap();
        let new_b = names.iter().position(|n| *n == "new-b").unwrap();
        let old_a = names.iter().position(|n| *n == "old-a").unwrap();
        let old_b = names.iter().position(|n| *n == "old-b").unwrap();
        assert_eq!(new_b, new_a + 1);
        assert_eq!(old_b, old_a + 1);
    }

    #[test]
    fn result_rows_are_allocated_at_the_show_rows_stride() {
        // Header line (or 12px gap) + 22px row fits the pivot height; a
        // 4px gap + 22px row is exactly the plain height.
        assert!(result_row_height(true) >= 12.0 + 22.0);
        assert_eq!(result_row_height(false), 4.0 + 22.0);
    }

    #[test]
    fn vanished_search_root_is_an_error_not_an_empty_result() {
        let temp = crate::testutil::TempDir::new();
        assert_eq!(search_root_error(temp.path()), None);
        let gone = temp.path().join("gone");
        let error = search_root_error(&gone).unwrap();
        assert!(error.starts_with("Search folder not found: "), "{error}");
        assert!(error.contains("gone"), "{error}");
        let file = temp.path().join("file.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(
            search_root_error(&file)
                .unwrap()
                .contains("is not a folder")
        );
    }

    #[test]
    fn index_build_refusal_reports_the_index_reason() {
        assert_eq!(
            index_build_refusal(true, Some("deferred under pressure".into())),
            "deferred under pressure"
        );
        assert!(index_build_refusal(true, None).contains("did not start"));
        assert_eq!(
            index_build_refusal(false, Some("ignored".into())),
            "Enable the content index before rebuilding"
        );
    }

    #[test]
    fn index_skipped_summary_lists_only_nonzero_counts() {
        assert_eq!(index_skipped_summary(0, 0, 0, 0), None);
        assert_eq!(
            index_skipped_summary(2, 0, 0, 1).as_deref(),
            Some("Skipped: 2 binary, 1 over the index size limit")
        );
    }

    #[test]
    fn history_rows_name_mode_and_folder() {
        let entry = crate::search::QueryHistoryEntry {
            expression: String::new(),
            mode: crate::query::MatchMode::Fuzzy,
            root: PathBuf::from("/home/me/projects"),
            result_count: 3,
            scanned: 10,
            duration_ms: 7,
            last_run: 0,
        };
        assert_eq!(
            history_row_label(&entry),
            "All files  |  Fuzzy  |  in projects  |  3 results  |  7 ms"
        );
    }

    #[test]
    fn failed_smart_folder_save_rolls_back_the_store() {
        let definition = |name: &str| crate::smart_folder::Definition {
            name: name.to_string(),
            root: PathBuf::from("/x"),
            query: crate::query::Query {
                predicates: vec![crate::query::Predicate::NameContains("a".into())],
                mode: crate::query::MatchMode::Exact,
            },
        };
        let mut store = crate::smart_folder::SmartFolders::default();
        assert!(add_smart_folder(&mut store, definition("kept"), |_| true));
        let before = store.clone();
        assert!(!add_smart_folder(&mut store, definition("lost"), |_| false));
        assert_eq!(store, before);
    }
}
