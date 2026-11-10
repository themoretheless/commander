//! Streaming recursive search sheet. Query parsing and matching live in
//! `crate::query` and `crate::search`; this module owns only interaction state.

use super::*;
use crate::panel::format_size;
use std::time::SystemTime;

mod index_panel;
mod results;
mod state;

impl FindCopyFormat {
    fn label(self) -> &'static str {
        match self {
            Self::Paths => "Full paths",
            Self::Names => "Names",
            Self::ShellArguments => "Shell arguments",
            Self::DetailsCsv => "Details (CSV)",
            Self::DetailsJson => "Report (JSON)",
            Self::FileUrls => "File URLs",
            Self::NamesWithoutExtension => "Names (no extension)",
            Self::ParentFolders => "Parent folders",
            Self::RelativePaths => "Relative paths",
        }
    }
    fn noun(self) -> &'static str {
        match self {
            Self::Paths => "paths",
            Self::Names => "names",
            Self::ShellArguments => "shell arguments",
            Self::DetailsCsv => "CSV rows",
            Self::DetailsJson => "JSON report",
            Self::FileUrls => "file URLs",
            Self::NamesWithoutExtension => "names",
            Self::ParentFolders => "folders",
            Self::RelativePaths => "relative paths",
        }
    }
}

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
/// Which hit's explanation is open: identity plus where the hit was found.
pub type ExplainKey = (crate::search::FileIdentity, crate::search::SearchLocation);

fn explain_key(hit: &crate::search::SearchHit) -> ExplainKey {
    (hit.identity.clone(), hit.location.clone())
}

/// The hit `key` names, telling apart hard links that share an identity.
fn explained_hit<'a>(
    results: &'a [crate::search::SearchHit],
    key: &ExplainKey,
) -> Option<&'a crate::search::SearchHit> {
    results
        .iter()
        .find(|hit| hit.identity == key.0 && hit.location == key.1)
}

#[cfg(test)]
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

fn move_result_highlight(current: Option<usize>, len: usize, down: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(if down {
        current.map_or(0, |row| (row + 1).min(len - 1))
    } else {
        current.map_or(len - 1, |row| row.saturating_sub(1))
    })
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
///
/// This stats the root on the UI thread. It runs once per (debounced) search
/// start, and a single `metadata` call is accepted there even though a hung
/// network mount could stall it; the walk itself stays on the worker.
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

fn fail_find_search_start(state: &mut FindState, error: String) {
    if state.run.is_some() {
        state.result_complete = false;
    }
    state.run = None;
    state.error = Some(error);
    state.pending_rerun = false;
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
    /// Start building the content index for `root`, repainting as it
    /// progresses; false when the index refused to start.
    fn start_index_build(&mut self, ctx: &egui::Context, root: PathBuf) -> bool {
        let repaint = ctx.clone();
        self.content_index
            .start_build(root, std::sync::Arc::new(move || repaint.request_repaint()))
    }

    fn poll_find_events(
        &mut self,
    ) -> Option<(
        String,
        crate::query::MatchMode,
        PathBuf,
        crate::search::SearchSummary,
    )> {
        let (events, disconnected) = self
            .ui
            .modals
            .find
            .as_ref()
            .and_then(|state| state.run.as_ref())
            .map_or_else(Default::default, |run| {
                super::drain_events(|| run.try_recv())
            });

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
                        state.scanned = summary.scanned;
                        state.matched = summary.matched;
                        state.elapsed = summary.elapsed;
                        state.truncated = summary.truncated;
                        state.content_skipped = summary.content_skipped;
                        state.archive_error = summary.archive_error.clone();
                        state.result_complete = !summary.cancelled;
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
            if disconnected && state.searching() {
                state.result_complete = false;
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
        let with_archive_password = state.use_archive_password;
        if let Some(error) = search_root_error(&root) {
            if let Some(state) = self.ui.modals.find.as_mut() {
                // The old hits, counters and status belong to a different
                // root; showing them next to this error would contradict it.
                state.results.clear();
                state.hidden_results.clear();
                state.clear_result_selection();
                state.result_action_error = None;
                state.stable_order.clear();
                state.last_query = None;
                state.ran = false;
                state.result_complete = false;
                state.scanned = 0;
                state.matched = 0;
                state.elapsed = std::time::Duration::ZERO;
                state.truncated = false;
                state.content_skipped = 0;
                state.archive_error = None;
                state.explanation_open = None;
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
        // A refused index build is reported even though the search still runs
        // (as a plain walk), so starting the search must not hide it.
        let mut index_refusal = None;
        if index_status.enabled
            && matches!(
                index_status.phase,
                crate::content_index::IndexPhase::Missing | crate::content_index::IndexPhase::Error
            )
        {
            if self.start_index_build(ctx, root.clone()) {
                if let Some(state) = self.ui.modals.find.as_mut() {
                    state.index_rerun_after_build = !with_archive_password;
                }
            } else {
                index_refusal = Some(index_build_refusal(
                    true,
                    self.content_index.status(&root).last_error,
                ));
            }
        }
        let index = self
            .content_index
            .is_enabled(&root)
            .then(|| self.content_index.snapshot(&root))
            .flatten();
        let repaint = ctx.clone();
        let notify = std::sync::Arc::new(move || repaint.request_repaint());
        let password = self.ui.modals.find.as_mut().and_then(|state| {
            if !with_archive_password {
                return None;
            }
            state.use_archive_password = false;
            state.index_rerun_after_build = false;
            Some(std::mem::take(&mut state.archive_password))
        });
        let run_result = match (index, password) {
            (Some(index), Some(password)) => {
                self.search_engine.start_indexed_with_archive_password(
                    index,
                    query,
                    crate::search::DEFAULT_RESULT_CAP,
                    notify,
                    Some(password),
                )
            }
            (None, Some(password)) => self.search_engine.start_with_archive_password(
                root,
                query,
                crate::search::DEFAULT_RESULT_CAP,
                notify,
                Some(password),
            ),
            (Some(index), None) => self.search_engine.start_indexed(
                index,
                query,
                crate::search::DEFAULT_RESULT_CAP,
                notify,
            ),
            (None, None) => {
                self.search_engine
                    .start(root, query, crate::search::DEFAULT_RESULT_CAP, notify)
            }
        };
        let run = match run_result {
            Ok(run) => run,
            Err(error) => {
                // Refusal happens before SearchEngine publishes a new active
                // generation, so explicitly invalidate the previous worker.
                self.search_engine.cancel();
                if let Some(state) = self.ui.modals.find.as_mut() {
                    fail_find_search_start(state, error.to_string());
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
            state.hidden_results.clear();
            state.clear_result_selection();
            state.result_action_error = None;
            state.stable_order = previous;
            state.last_query = Some(query_key);
            state.ran = true;
            state.result_complete = false;
            state.scanned = 0;
            state.matched = 0;
            state.elapsed = std::time::Duration::ZERO;
            state.truncated = false;
            state.content_skipped = 0;
            state.archive_error = None;
            state.error = index_refusal;
            state.pending_rerun = false;
            state.explanation_open = None;
            state.highlighted_result = None;
            state.highlight_scroll_pending = false;
        }
    }

    fn mark_find_edited(&mut self, ctx: &egui::Context) {
        self.search_engine.cancel();
        if let Some(state) = self.ui.modals.find.as_mut() {
            if state.run.is_some() {
                state.result_complete = false;
            }
            state.run = None;
            state.pending_rerun = true;
            state.last_edit_at = ctx.input(|input| input.time);
            state.error = None;
            state.highlighted_result = None;
            state.highlight_scroll_pending = false;
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

    pub(crate) fn open_identity_find(
        &mut self,
        ctx: &egui::Context,
        root: std::path::PathBuf,
        device: u64,
        inode: u64,
    ) {
        self.open_find(ctx);
        let exclusions = format_index_exclusions(&root, &self.content_index.exclusions(&root));
        if let Some(state) = self.ui.modals.find.as_mut() {
            state.root = root;
            state.index_exclusions = exclusions;
            state.expression = format!("device:{device} inode:{inode} type:regular archives:off");
        }
        self.begin_find_search(ctx);
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
            state.pending_rerun
                && !state.use_archive_password
                && now_seconds - state.last_edit_at >= AUTO_RUN_DELAY
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
                && !state.use_archive_password
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
        let tab_side = self.ws.active;
        let mut result_actions = results::ResultActions::default();
        let mut replay = None;
        let mut remove_predicate = None;
        let mut root_change = None;
        let mut index_enabled_change = None;
        let mut index_rebuild = false;
        let mut index_apply_exclusions = false;

        {
            let state = self.ui.modals.find.as_mut().unwrap();
            let dialog_response = egui::Window::new("Search")
                .open(&mut window_open)
                .collapsible(false)
                .resizable(true)
                .default_pos(ctx.content_rect().min + egui::vec2((ctx.content_rect().width() - 720.0).max(0.0) * 0.5, (ctx.content_rect().height() - 620.0).max(0.0) * 0.5))
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
                            .id(egui::Id::new("find-query-editor"))
                            .desired_width(f32::INFINITY)
                            .hint_text("Search files...")
                            .margin(egui::vec2(8.0, 7.0)),
                    ).on_hover_text("Use case:sensitive to distinguish letter case. Find empty files: type:file size:=0. Find links with missing targets: type:broken-link. Numeric ownership: uid:501 gid:!20. Filesystem identity: device:16777230 inode:12345. Immediate parent: parent:\"/absolute/folder\"\nMetadata/status change time (Unix ctime): changed-after:2026-01-01 changed-before:2026-02-01. Creation time: created-after:2026-01-01 created-before:2026-02-01. Access time: accessed-after:2026-01-01 accessed-before:2026-02-01 (strict boundaries; UTC dates or RFC 3339 with offset). Hard-link counts: links:>1 or links:2..4 (regular files only). Allocated disk space: allocated:>1MB. Combine conditions: glob:report-*.pdf ext:pdf archives:off exclude:*.bak depth:<=2 path:reports content:revenue notcontent:draft. glob: matches the whole name using * and ?; quote values containing spaces.");
                    #[cfg(feature = "visual-qa")]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-query-input"), response.rect));
                    #[cfg(feature = "visual-qa")]
                    {
                        let query_focused = response.has_focus();
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-query-focused"), query_focused));
                    }
                    if Self::take_modal_opened(ui.ctx(), UiModal::Find) {
                        response.request_focus();
                    }
                    if response.changed() {
                        state.highlighted_result = None;
                        edited = true;
                    }
                    if response.has_focus()
                        && ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                        })
                        && state.visible_result_count() > 0
                    {
                        state.highlighted_result = move_result_highlight(
                            state.highlighted_result,
                            state.visible_result_count(),
                            true,
                        );
                        state.highlight_scroll_pending = true;
                        // Key consumption alone leaves egui's queued arrow focus move intact.
                        response.request_focus();
                        ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
                    }
                    if response.has_focus()
                        && ui.input_mut(|input| {
                            input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                        })
                        && state.visible_result_count() > 0
                    {
                        state.highlighted_result = move_result_highlight(
                            state.highlighted_result,
                            state.visible_result_count(),
                            false,
                        );
                        state.highlight_scroll_pending = true;
                        response.request_focus();
                        ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
                    }
                    let background_enter = response.has_focus()
                        && ui.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter));
                    if background_enter {
                        if let Some(path) = state.highlighted_result_target(SystemTime::now()) {
                            result_actions.open_tab = Some((tab_side, path));
                        }
                        response.request_focus();
                    } else if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        if state.highlighted_result.is_some() {
                            result_actions.reveal = state.highlighted_result_target(SystemTime::now());
                        } else {
                            run_now = true;
                        }
                        // Keep the query available for refinement after Enter.
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
                            let search_button = ui
                                .add(super::dialog_button(
                                    t,
                                    super::ButtonRole::Primary,
                                    egui::RichText::new(if state.searching() {
                                        "Restart"
                                    } else {
                                        "Search"
                                    }),
                                ))
;
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("find-search-button"), search_button.rect));
                            if search_button.clicked() {
                                run_now = true;
                            }
                        });
                    });

                    ui.horizontal_wrapped(|ui| {
                        let toggle = ui.checkbox(&mut state.use_archive_password, "ZIP password for next search");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("find-password-toggle"), toggle.rect));
                        if toggle.changed() {
                            state.pending_rerun = false;
                            if !state.use_archive_password { state.archive_password = zeroize::Zeroizing::new(String::new()); }
                        }
                        if state.use_archive_password {
                            let mut output = egui::TextEdit::singleline(&mut *state.archive_password)
                                .id_salt("find-archive-password").password(true).desired_width(180.0)
                                .hint_text("For this search only").show(ui);
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("find-password-input"), (output.response.rect, output.response.id)));
                            output.state.clear_undoer();
                            output.state.store(ui.ctx(), output.response.id);
                            ui.label("Press Search to use it");
                        }
                    }).response.on_hover_text("One password for encrypted ZIP/ZIPX content in this search. Empty passwords are allowed. Cleared on launch or close; never saved in history or the content index. Automatic search is paused while enabled.");

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
                        let index_actions = index_panel::index_details_ui(ui, state, t, &index_status);
                        index_rebuild |= index_actions.rebuild;
                        index_apply_exclusions |= index_actions.apply_exclusions;
                    }

                    ui.add_space(6.0);
                    let clock = SystemTime::now();
                    ui.horizontal_wrapped(|ui| {
                        let status = if state.searching() {
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
                                    "{} skipped",
                                    state.content_skipped
                                ))
                                .size(11.0)
                                .color(t.accent_warning),
                            ).on_hover_text("Unreadable paths, unavailable or binary content, or unknown metadata. Cached observations may require an index rebuild; a link target access or loop error does not prove it is missing.");
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Group by");
                        for pivot in crate::search::TimePivot::ALL {
                            let response=ui.selectable_label(state.time_pivot==pivot,pivot.label());
                            #[cfg(feature="visual-qa")]
                            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("find-time-pivot",pivot.label())),response.rect));
                            if response.clicked(){state.set_time_pivot(pivot,clock);}
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        let mut sort=state.result_sort;
                        ui.label("Sort by");
                        let picker=egui::ComboBox::from_id_salt("find-result-sort").width(180.0).height(320.0).selected_text(sort.field.label()).show_ui(ui,|ui| {
                            for field in crate::search_order::Field::ALL {
                                let choice=ui.selectable_value(&mut sort.field,field,field.label());
                                #[cfg(feature="visual-qa")]
                                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("find-result-sort-choice",field.label())),choice.rect));
                                if choice.clicked() {
                                    ui.close();
                                    ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor")));
                                }
                            }
                        });
                        #[cfg(feature="visual-qa")]
                        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("find-result-sort-picker"),picker.response.rect));
                        picker.response.on_hover_text("Sort retained rows within each date group. Name uses natural folded ordering; full paths use native lexical order. Unknown observations remain last. No rescan or metadata reads.");
                        let _reverse=ui.checkbox(&mut sort.descending,"Descending");
                        #[cfg(feature="visual-qa")]
                        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("find-result-sort-descending"),_reverse.rect));
                        if sort!=state.result_sort {state.set_result_sort(sort,clock);}
                    });
                    if let Some(error) = &state.archive_error {
                        // Why the skipped count includes an archive.
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(error)
                                    .size(11.0)
                                    .color(t.accent_warning),
                            )
                            .truncate(),
                        )
                        .on_hover_text(error);
                    }
                    if let Some(error) = &state.error {
                        ui.label(egui::RichText::new(error).size(11.0).color(t.accent_red));
                    }
                    if let Some(error) = &state.result_action_error {
                        ui.label(egui::RichText::new(error).size(11.0).color(t.accent_red));
                    }
                    ui.add_space(4.0);
                    ui.separator();

                    ui.horizontal_wrapped(|ui| {
                        let select_all = ui.add_enabled(state.visible_result_count() > 0,
                            egui::Button::new("Select all visible").small());
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-select-all-visible"), select_all.rect));
                        if select_all.clicked() {
                            state.select_all_visible_results();
                            ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor")));
                        }
                        let invert = ui.add_enabled(state.visible_result_count() > 0,
                            egui::Button::new("Invert selection").small());
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-invert-selection"), invert.rect));
                        if invert.clicked() {
                            state.invert_visible_result_selection();
                            ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor")));
                        }
                        ui.label("Copy as");
                        let picker = egui::ComboBox::from_id_salt("find-copy-format")
                            .height(280.0)
                            .selected_text(state.result_copy_format.label())
                            .show_ui(ui, |ui| {
                                for format in [FindCopyFormat::Paths, FindCopyFormat::Names, FindCopyFormat::ShellArguments, FindCopyFormat::DetailsCsv, FindCopyFormat::DetailsJson, FindCopyFormat::FileUrls, FindCopyFormat::NamesWithoutExtension, FindCopyFormat::ParentFolders, FindCopyFormat::RelativePaths] {
                                    let choice = ui.selectable_value(&mut state.result_copy_format, format, format.label());
                                    #[cfg(feature = "visual-qa")]
                                    ui.ctx().data_mut(|data| {
                                        data.insert_temp(egui::Id::new(("find-copy-format-choice", format.label())), choice.rect);
                                        data.insert_temp(egui::Id::new(("find-copy-format-choice-visible", format.label())),choice.rect.intersect(ui.clip_rect()));
                                    });
                                    if choice.clicked() {
                                        ui.close();
                                        ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor")));
                                    }
                                }
                            });
                        if state.result_copy_format == FindCopyFormat::RelativePaths {
                            picker.response.clone().on_hover_text("Paths relative to the applied search root, preserving archive !/member references. Draft root changes do not retarget existing results. Plain lines, not shell arguments.");
                        }
                        if state.result_copy_format == FindCopyFormat::FileUrls {
                            picker.response.clone().on_hover_text("File URLs refer to native files. Archive members use the URL of their container.");
                        }
                        if state.result_copy_format == FindCopyFormat::ParentFolders {
                            picker.response.clone().on_hover_text("Copies distinct containing folders in displayed order. Archive members use the folder containing their archive; directories use their parent.");
                        }
                        if state.result_copy_format == FindCopyFormat::NamesWithoutExtension {
                            picker.response.clone().on_hover_text("Removes the last dot suffix lexically: archive.tar.gz becomes archive.tar; .env stays .env. Archive hits use member names.");
                        }
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-copy-format"), picker.response.rect));
                        picker.response.on_hover_text("Full paths retain archive members; names omit directories; shell arguments quote real file/archive-container paths and join with spaces");
                        let copy = ui.add_enabled(state.visible_result_count() > 0 || (state.result_copy_format==FindCopyFormat::DetailsJson && state.last_query.is_some()),
                            egui::Button::new(format!("Copy {}", state.result_copy_format.noun())).small())
                            .on_hover_text("Copy all visible results in displayed order using the chosen format");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-copy-result-paths"), copy.rect));
                        if copy.clicked() {
                            result_actions.copy_result_paths = state.prepare_result_copy(clock,false).map(|(text, count)| (text, count, state.result_copy_format));
                            ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor")));
                        }
                        let selected_count = state.visible_selected_result_count();
                        let copy_selected = ui.add_enabled(selected_count > 0,
                            egui::Button::new(format!("Copy selected ({selected_count})")).small());
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-copy-selected-paths"), copy_selected.rect));
                        if copy_selected.clicked() {
                            result_actions.copy_result_paths = state.prepare_result_copy(clock,true).map(|(text, count)| (text, count, state.result_copy_format));
                            ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor")));
                        }
                        let remember = ui.add_enabled(selected_count > 0 && state.results_match_current_root(),
                            egui::Button::new("Remember selection").small())
                            .on_hover_text("Remember the selected visible results for this search root until Search closes");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-remember-selection"), remember.rect));
                        if remember.clicked() {
                            state.remember_result_selection();
                            ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor")));
                        }
                        let restorable = state.restorable_result_selection_count();
                        let restore_selection = ui.add_enabled(restorable > 0,
                            egui::Button::new(format!("Restore selection ({restorable})")).small())
                            .on_hover_text("Replace selection with remembered results currently visible under this same search root");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-restore-selection"), restore_selection.rect));
                        if restore_selection.clicked() {
                            state.restore_result_selection();
                            ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor")));
                        }
                        let open_selected = ui.add_enabled(selected_count > 0,
                            egui::Button::new("Open selected tabs").small())
                            .on_hover_text("Open every selected visible result in a background tab, in displayed order; the entire group must fit the tab limit");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-open-selected-tabs"), open_selected.rect));
                        if open_selected.clicked() {
                            result_actions.open_selected_tabs = Some((tab_side, state.selected_result_tab_targets(clock)));
                            ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor")));
                        }
                        let hide_selected = ui.add_enabled(selected_count > 0,
                            egui::Button::new("Hide selected").small())
                            .on_hover_text("Hide selected visible results; files are not changed. Show all results restores the rows.");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-hide-selected-results"), hide_selected.rect));
                        if hide_selected.clicked() {
                            result_actions.hide_selected_results = true;
                            ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor")));
                        }
                        if selected_count > 0 && ui.small_button("Clear selection").clicked() {
                            state.clear_result_selection();
                        }
                        if !state.hidden_results.is_empty() {
                            ui.label(format!("{} hidden results", state.results.len() - state.visible_result_count()));
                            let restore = ui.small_button("Show all results");
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-show-all-results"), restore.rect));
                            if restore.clicked() {
                                state.restore_hidden_results();
                                ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor")));
                            }
                        }
                    });
                    results::results_ui(ui, state, t, clock, tab_side, &mut result_actions);

                    if state.ran
                        && !state.searching()
                        && state.results.is_empty()
                        && state.error.is_none()
                    {
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
                    if let Some(key) = &state.explanation_open
                        && let Some(hit) = explained_hit(&state.results, key)
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
        }

        if super::ui_state::modal_close_requested(window_open, false) {
            self.search_engine.cancel();
            self.ui.modals.find = None;
            return;
        }
        if let Some((text, count, format)) = result_actions.copy_result_paths {
            self.write_clipboard(
                &text,
                &if format == FindCopyFormat::DetailsJson {
                    format!("Search JSON report ({count} results)")
                } else {
                    format!("{count} search result {}", format.noun())
                },
                ctx,
            );
        }
        if let Some(location) = result_actions.context_select_only
            && let Some(state) = self.ui.modals.find.as_mut()
        {
            state.select_only_result(location, SystemTime::now());
        }
        if let Some((location, range)) = result_actions.select_result
            && let Some(state) = self.ui.modals.find.as_mut()
        {
            state.select_result(location, range, SystemTime::now());
        }
        if result_actions.hide_selected_results
            && let Some(state) = self.ui.modals.find.as_mut()
        {
            state.hide_selected_results();
            ctx.request_repaint();
        }
        if let Some(location) = result_actions.hide_result
            && let Some(state) = self.ui.modals.find.as_mut()
        {
            state.hide_result(location);
            ctx.request_repaint();
        }
        if let Some((expression, mode, root, summary)) = history_record {
            self.search_history.record(expression, mode, root, &summary);
        }
        let mut index_refusal = None;
        if let Some(enabled) = index_enabled_change {
            if self.content_index.set_enabled(index_root.clone(), enabled) {
                if enabled {
                    let started = self.start_index_build(ctx, index_root.clone());
                    let refusal = (!started).then(|| {
                        index_build_refusal(true, self.content_index.status(&index_root).last_error)
                    });
                    if let Some(state) = self.ui.modals.find.as_mut() {
                        state.index_rerun_after_build = started;
                    }
                    // Applied after mark_find_edited below, which clears the
                    // error for the edit this toggle also counts as.
                    index_refusal = refusal;
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
                .map(FindState::exclusion_paths)
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
                        started = self.start_index_build(ctx, index_root.clone());
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
            let started = self.start_index_build(ctx, index_root.clone());
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
                state.replay(entry, exclusions);
            }
            edited = true;
        }
        // Choosing the crumb that already is the root changes nothing, so it
        // must not cancel and restart a running search.
        if let Some(path) = root_change.filter(|path| *path != index_root) {
            let exclusions = format_index_exclusions(&path, &self.content_index.exclusions(&path));
            if let Some(state) = self.ui.modals.find.as_mut() {
                state.change_root(path, exclusions);
            }
            edited = true;
        }
        if let Some(index) = remove_predicate
            && let Some(state) = self.ui.modals.find.as_mut()
        {
            edited |= state.remove_predicate(index);
        }
        if let Some(value) = result_actions.explanation_change
            && let Some(state) = self.ui.modals.find.as_mut()
        {
            state.explanation_open = value;
        }
        if edited {
            self.mark_find_edited(ctx);
        }
        if let Some(refusal) = index_refusal
            && let Some(state) = self.ui.modals.find.as_mut()
        {
            state.error = Some(refusal);
        }
        if run_now {
            self.begin_find_search(ctx);
        }
        if let Some((side, targets)) = result_actions.open_selected_tabs {
            let opened = self.ws.reveal_in_background_tabs(side, targets);
            if let Some(state) = self.ui.modals.find.as_mut() {
                state.result_action_error = (!opened).then(|| "No tabs opened: the group exceeds available tab slots, a target is invalid, or an operation blocks tab changes".to_string());
            }
        }
        if let Some((side, path)) = result_actions.open_tab {
            let opened = self.ws.reveal_in_background_tab(side, path);
            if let Some(state) = self.ui.modals.find.as_mut() {
                state.result_action_error = (!opened)
                    .then(|| "Could not open a tab: tab limit or a pending operation".to_string());
            }
        }
        if let Some(path) = result_actions.reveal {
            self.ws.reveal(&path);
        }
        if let Some(path) = result_actions.open_path {
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
                // A failed save gets no toast here: the persistence layer
                // records the failure with its cause and `begin_frame`
                // toasts that, so a second, vaguer error would repeat it.
                // `add_smart_folder` already dropped the unsaved entry.
                if saved {
                    if let Some(state) = self.ui.modals.find.as_mut() {
                        state.save_name.clear();
                    }
                    self.toast(
                        ctx,
                        crate::toasts::ToastKind::Success,
                        format!("Saved search \"{name}\""),
                        false,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
