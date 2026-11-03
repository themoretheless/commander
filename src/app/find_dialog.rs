//! Streaming recursive search sheet. Query parsing and matching live in
//! `crate::query` and `crate::search`; this module owns only interaction state.

use super::*;
use crate::panel::format_size;
use std::time::SystemTime;

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
        let mut reveal = None;
        let mut open_path = None;
        let mut open_tab = None;
        let mut open_selected_tabs = None;
        let mut copy_result_paths = None;
        let tab_side = self.ws.active;
        let mut replay = None;
        let mut remove_predicate = None;
        let mut root_change = None;
        let mut explanation_change = None;
        let mut hide_result = None;
        let mut hide_selected_results = false;
        let mut select_result = None;
        let mut context_select_only = None;
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
                            open_tab = Some((tab_side, path));
                        }
                        response.request_focus();
                    } else if response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        if state.highlighted_result.is_some() {
                            reveal = state.highlighted_result_target(SystemTime::now());
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
                            copy_result_paths = state.prepare_result_copy(clock,false).map(|(text, count)| (text, count, state.result_copy_format));
                            ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor")));
                        }
                        let selected_count = state.visible_selected_result_count();
                        let copy_selected = ui.add_enabled(selected_count > 0,
                            egui::Button::new(format!("Copy selected ({selected_count})")).small());
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-copy-selected-paths"), copy_selected.rect));
                        if copy_selected.clicked() {
                            copy_result_paths = state.prepare_result_copy(clock,true).map(|(text, count)| (text, count, state.result_copy_format));
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
                            open_selected_tabs = Some((tab_side, state.selected_result_tab_targets(clock)));
                            ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor")));
                        }
                        let hide_selected = ui.add_enabled(selected_count > 0,
                            egui::Button::new("Hide selected").small())
                            .on_hover_text("Hide selected visible results; files are not changed. Show all results restores the rows.");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("find-hide-selected-results"), hide_selected.rect));
                        if hide_selected.clicked() {
                            hide_selected_results = true;
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
                    let indices = state.visible_result_order(clock);
                    #[cfg(feature="visual-qa")]
                    ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("find-rendered-order"),indices.clone()));
                    let pivot = state.time_pivot;
                    let row_height = result_row_height(pivot != crate::search::TimePivot::None);
                    let root = state.root.clone();
                    let open_explanation = state.explanation_open.clone();
                    state.highlighted_result =
                        state.highlighted_result.filter(|&row| row < indices.len());
                    let mut results_scroll = egui::ScrollArea::vertical()
                        .id_salt("search-results")
                        .auto_shrink([false, false])
                        .max_height((ui.available_height() - 90.0).max(140.0));
                    if state.highlight_scroll_pending {
                        let offset = state.highlighted_result.unwrap_or(0) as f32 * row_height;
                        results_scroll = results_scroll.vertical_scroll_offset(offset);
                        state.highlight_scroll_pending = false;
                    }
                    results_scroll.show_rows(ui, row_height, indices.len(), |ui, rows| {
                        for row in rows {
                            ui.allocate_ui_with_layout(
                                egui::vec2(ui.available_width(), row_height),
                                Layout::top_down(Align::Min),
                                |ui| {
                                    ui.set_min_height(row_height);
                                    let index = indices[row];
                                    let hit = &state.results[index];
                                    let key = explain_key(hit);
                                    let path = hit.reveal_path().to_path_buf();
                                    let relative = hit.relative_to(&root).display().to_string();
                                    let relative=match crate::display_name::visible_controls(&relative) {
                                        std::borrow::Cow::Borrowed(_)=>relative,
                                        std::borrow::Cow::Owned(visible)=>visible,
                                    };
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
                                        let mut selected = state.selected_results.contains(&hit.location);
                                        let checkbox = ui.checkbox(&mut selected, "").on_hover_text("Select result for Copy selected; Shift-click adds a visible range");
                                        checkbox.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, selected, format!("Select result: {relative}")));
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("find-select-result", relative.as_str())), checkbox.rect));
                                        if checkbox.changed() {
                                            select_result = Some((hit.location.clone(), ui.input(|input| input.modifiers.shift)));
                                        }
                                        let response = ui
                                            .add_sized(
                                                [(ui.available_width() - 190.0).max(0.0), 22.0],
                                                egui::Label::new(
                                                    egui::RichText::new(&relative)
                                                        .size(12.0)
                                                        .color(t.text_secondary)
                                                        .background_color(
                                                            if state.highlighted_result == Some(row) || state.selected_results.contains(&hit.location)
                                                            {
                                                                ui.visuals().selection.bg_fill
                                                            } else {
                                                                egui::Color32::TRANSPARENT
                                                            },
                                                        ),
                                                )
                                                .truncate()
                                                .sense(Sense::click()),
                                            )
                                            .on_hover_text(
                                                "Click to show in the panel, double-click to open; Cmd-click toggles selection, Shift-click adds a range; right-click for actions",
                                            );
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("find-result-row", relative.as_str())), response.rect));
                                        response.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::WidgetType::Button,
                                                true,
                                                format!("Reveal {relative}"),
                                            )
                                        });
                                        if response.secondary_clicked() && !state.selected_results.contains(&hit.location) {
                                            context_select_only = Some(hit.location.clone());
                                        }
                                        response.context_menu(|ui| {
                                            let mut menu_action = false;
                                            if ui.button("Reveal in panel").clicked() { reveal = Some(path.clone()); ui.close(); menu_action = true; }
                                            if ui.add_enabled(!hit.entry.is_dir, egui::Button::new("Open file")).clicked() { open_path = Some(path.clone()); ui.close(); menu_action = true; }
                                            if ui.button("Open background tab").clicked() { open_tab = Some((tab_side, path.clone())); ui.close(); menu_action = true; }
                                            if ui.button("Open selected tabs").clicked() {
                                                let targets = if state.selected_results.contains(&hit.location) { state.selected_result_tab_targets(clock) } else { vec![path.clone()] };
                                                open_selected_tabs = Some((tab_side, targets)); ui.close(); menu_action = true;
                                            }
                                            ui.separator();
                                            let copy_path = ui.button("Copy full path");
                                            #[cfg(feature = "visual-qa")]
                                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("find-context-copy-path", relative.as_str())), copy_path.rect));
                                            if copy_path.clicked() {
                                                copy_result_paths = Some((crate::clipboard::format(std::slice::from_ref(&hit.entry.path), crate::clipboard::PathStyle::FullPath, None), 1, FindCopyFormat::Paths));
                                                ui.close(); menu_action = true;
                                            }
                                            let copy_name = ui.button("Copy name");
                                            #[cfg(feature = "visual-qa")]
                                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("find-context-copy-name", relative.as_str())), copy_name.rect));
                                            if copy_name.clicked() {
                                                copy_result_paths = Some((crate::clipboard::format(std::slice::from_ref(&hit.entry.path), crate::clipboard::PathStyle::NameOnly, None), 1, FindCopyFormat::Names));
                                                ui.close(); menu_action = true;
                                            }
                                            ui.separator();
                                            if ui.button("Hide result").clicked() { hide_result = Some(hit.location.clone()); ui.close(); menu_action = true; }
                                            if ui.button("Hide selected").clicked() {
                                                if state.selected_results.contains(&hit.location) { hide_selected_results = true; } else { hide_result = Some(hit.location.clone()); }
                                                ui.close(); menu_action = true;
                                            }
                                            if ui.button("Why matched").clicked() { explanation_change = Some(Some(key.clone())); ui.close(); menu_action = true; }
                                            if menu_action { ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor"))); }
                                        });
                                        let modifiers = ui.input(|input| input.modifiers);
                                        if response.double_clicked() && !hit.entry.is_dir && !modifiers.command && !modifiers.shift {
                                            open_path = Some(path.clone());
                                        }
                                        if response.clicked() {
                                            if modifiers.command || modifiers.shift {
                                                select_result = Some((hit.location.clone(), modifiers.shift));
                                            } else {
                                                reveal = Some(path);
                                            }
                                        }
                                        ui.label(
                                            egui::RichText::new(size)
                                                .size(10.0)
                                                .color(t.text_muted),
                                        );
                                        let tab = ui.small_button("Tab").on_hover_text("Reveal in a new background tab; keep Search open (Cmd+Enter for keyboard selection)");
                                        tab.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Open background tab: {relative}")));
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("find-background-tab", relative.as_str())), tab.rect));
                                        if tab.clicked() { open_tab = Some((tab_side, hit.reveal_path().to_path_buf())); }
                                        let hide = ui.small_button("×").on_hover_text("Hide this search result; the file is not changed");
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("find-hide-result", relative.as_str())), hide.rect));
                                        hide.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Hide result: {relative}")));
                                        if hide.clicked() {
                                            hide_result = Some(hit.location.clone());
                                        }
                                        let explaining = open_explanation.as_ref() == Some(&key);
                                        let why = ui
                                            .add(egui::Button::selectable(explaining, "?").small())
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
                                            explanation_change = Some((!explaining).then_some(key));
                                        }
                                    });
                                },
                            );
                        }
                    });

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
        if let Some((text, count, format)) = copy_result_paths {
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
        if let Some(location) = context_select_only
            && let Some(state) = self.ui.modals.find.as_mut()
        {
            state.select_only_result(location, SystemTime::now());
        }
        if let Some((location, range)) = select_result
            && let Some(state) = self.ui.modals.find.as_mut()
        {
            state.select_result(location, range, SystemTime::now());
        }
        if hide_selected_results && let Some(state) = self.ui.modals.find.as_mut() {
            state.hide_selected_results();
            ctx.request_repaint();
        }
        if let Some(location) = hide_result
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
        if let Some(value) = explanation_change
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
        if let Some((side, targets)) = open_selected_tabs {
            let opened = self.ws.reveal_in_background_tabs(side, targets);
            if let Some(state) = self.ui.modals.find.as_mut() {
                state.result_action_error = (!opened).then(|| "No tabs opened: the group exceeds available tab slots, a target is invalid, or an operation blocks tab changes".to_string());
            }
        }
        if let Some((side, path)) = open_tab {
            let opened = self.ws.reveal_in_background_tab(side, path);
            if let Some(state) = self.ui.modals.find.as_mut() {
                state.result_action_error = (!opened)
                    .then(|| "Could not open a tab: tab limit or a pending operation".to_string());
            }
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

/// Field transitions the find dialog applies after its UI pass. They are
/// plain state edits, kept out of the frame code so they can be tested.
impl FindState {
    fn visible_result_count(&self) -> usize {
        self.results
            .iter()
            .filter(|hit| !self.hidden_results.contains(&hit.location))
            .count()
    }

    fn set_time_pivot(&mut self, pivot: crate::search::TimePivot, clock: SystemTime) {
        if self.time_pivot == pivot {
            return;
        }
        let highlighted = self
            .highlighted_result
            .and_then(|row| self.visible_result_order(clock).get(row).copied())
            .map(|index| self.results[index].location.clone());
        self.time_pivot = pivot;
        self.highlighted_result = highlighted.and_then(|location| {
            self.visible_result_order(clock)
                .iter()
                .position(|&index| self.results[index].location == location)
        });
        self.highlight_scroll_pending = self.highlighted_result.is_some();
    }

    fn set_result_sort(&mut self, sort: crate::search_order::Sort, clock: SystemTime) {
        if self.result_sort == sort {
            return;
        }
        let highlighted = self
            .highlighted_result
            .and_then(|row| self.visible_result_order(clock).get(row).copied())
            .map(|index| self.results[index].location.clone());
        self.result_sort = sort;
        self.highlighted_result = highlighted.and_then(|location| {
            self.visible_result_order(clock)
                .iter()
                .position(|&index| self.results[index].location == location)
        });
        self.highlight_scroll_pending = self.highlighted_result.is_some();
    }
    fn visible_result_order(&self, clock: SystemTime) -> Vec<usize> {
        let mut order =
            crate::search_order::order(&self.results, self.time_pivot, self.result_sort, clock);
        order.retain(|&index| !self.hidden_results.contains(&self.results[index].location));
        order
    }

    fn visible_selected_result_count(&self) -> usize {
        self.results
            .iter()
            .filter(|hit| {
                !self.hidden_results.contains(&hit.location)
                    && self.selected_results.contains(&hit.location)
            })
            .count()
    }

    fn selected_result_tab_targets(&self, clock: SystemTime) -> Vec<PathBuf> {
        self.visible_result_order(clock)
            .into_iter()
            .filter(|&index| {
                self.selected_results
                    .contains(&self.results[index].location)
            })
            .map(|index| self.results[index].reveal_path().to_path_buf())
            .collect()
    }

    fn prepare_result_copy(
        &mut self,
        clock: SystemTime,
        selected_only: bool,
    ) -> Option<(String, usize)> {
        self.result_action_error = None;
        if matches!(
            self.result_copy_format,
            FindCopyFormat::DetailsJson | FindCopyFormat::RelativePaths
        ) {
            let result = if self.result_copy_format == FindCopyFormat::RelativePaths {
                self.result_relative_paths_clipboard(clock, selected_only)
            } else {
                self.result_json(clock, selected_only)
            };
            match result {
                Ok(value) => value,
                Err(error) => {
                    self.result_action_error = Some(error);
                    None
                }
            }
        } else if selected_only {
            self.selected_result_paths_clipboard(clock)
        } else {
            self.visible_result_paths_clipboard(clock)
        }
    }
    fn result_relative_paths_clipboard(
        &self,
        clock: SystemTime,
        selected_only: bool,
    ) -> Result<Option<(String, usize)>, String> {
        let hits: Vec<_> = self
            .visible_result_order(clock)
            .into_iter()
            .filter(|&i| {
                !selected_only || self.selected_results.contains(&self.results[i].location)
            })
            .map(|i| &self.results[i])
            .collect();
        if hits.is_empty() {
            return Ok(None);
        }
        let Some((_, _, root)) = &self.last_query else {
            return Err("Relative paths need results bound to an applied search root".into());
        };
        let relative = |path: &std::path::Path| -> Result<String, String> {
            let part = path
                .strip_prefix(root)
                .map_err(|_| "A result is outside the applied search root".to_string())?;
            if !root.is_absolute()
                || part.components().any(|c| {
                    matches!(
                        c,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                })
            {
                return Err(
                    "Relative paths need an absolute root and contained lexical paths".into(),
                );
            }
            if part.as_os_str().is_empty() {
                return Ok(".".into());
            }
            part.to_str()
                .map(str::to_owned)
                .ok_or_else(|| "A relative path cannot be represented as UTF-8 text".into())
        };
        let lines: Result<Vec<_>, String> = hits
            .iter()
            .map(|hit| match &hit.location {
                crate::search::SearchLocation::Native(path) => relative(path),
                crate::search::SearchLocation::ArchiveMember {
                    archive, member, ..
                } => {
                    if member.is_absolute()
                        || member.components().any(|c| {
                            matches!(
                                c,
                                std::path::Component::ParentDir
                                    | std::path::Component::RootDir
                                    | std::path::Component::Prefix(_)
                            )
                        })
                    {
                        return Err("An archive member has no contained relative path".into());
                    }
                    let archive = relative(archive)?;
                    let member = member.to_str().ok_or_else(|| {
                        "An archive member cannot be represented as UTF-8 text".to_string()
                    })?;
                    Ok(format!("{archive}!/{member}"))
                }
            })
            .collect();
        let paths: Vec<_> = lines?.into_iter().map(PathBuf::from).collect();
        Ok(Some((
            crate::clipboard::format(&paths, crate::clipboard::PathStyle::FullPath, None),
            hits.len(),
        )))
    }

    fn result_json(
        &self,
        clock: SystemTime,
        selected_only: bool,
    ) -> Result<Option<(String, usize)>, String> {
        let Some((expression, mode, root)) = &self.last_query else {
            return Err("Search JSON needs results bound to an applied query".into());
        };
        let hits: Vec<_> = self
            .visible_result_order(clock)
            .into_iter()
            .filter(|&i| {
                !selected_only || self.selected_results.contains(&self.results[i].location)
            })
            .map(|i| &self.results[i])
            .collect();
        if selected_only && hits.is_empty() {
            return Ok(None);
        }
        let draft_differs = self.root != *root
            || self.mode != *mode
            || self
                .build_query()
                .map_or(true, |query| query.to_expression() != *expression);
        let context = crate::search_report::Context {
            expression,
            mode: *mode,
            root,
            generation: self.generation,
            source: &self.search_source,
            completed: self.result_complete,
            running: self.searching(),
            pending_rerun: self.pending_rerun,
            draft_differs,
            scanned: self.scanned,
            matched: self.matched,
            skipped: self.content_skipped,
            truncated: self.truncated,
            archive_error: self.archive_error.as_deref(),
            received_rows: self.results.len(),
            visible_rows: self.visible_result_count(),
            visible_selected_rows: self.visible_selected_result_count(),
            pivot: self.time_pivot,
            result_sort: self.result_sort,
            selected_only,
            ordered_at_utc: crate::listing_export::iso_8601_utc_precision(
                clock,
                chrono::SecondsFormat::AutoSi,
            ),
        };
        let count = hits.len();
        crate::search_report::json(&context, &hits).map(|text| Some((text, count)))
    }

    fn visible_result_paths_clipboard(&self, clock: SystemTime) -> Option<(String, usize)> {
        self.result_paths_clipboard(clock, false)
    }

    fn selected_result_paths_clipboard(&self, clock: SystemTime) -> Option<(String, usize)> {
        self.result_paths_clipboard(clock, true)
    }

    fn result_paths_clipboard(
        &self,
        clock: SystemTime,
        selected_only: bool,
    ) -> Option<(String, usize)> {
        let hits: Vec<_> = self
            .visible_result_order(clock)
            .into_iter()
            .filter(|&index| {
                !selected_only
                    || self
                        .selected_results
                        .contains(&self.results[index].location)
            })
            .map(|index| &self.results[index])
            .collect();
        if hits.is_empty() {
            return None;
        }
        let text = match self.result_copy_format {
            FindCopyFormat::RelativePaths => {
                return self
                    .result_relative_paths_clipboard(clock, selected_only)
                    .ok()
                    .flatten();
            }
            FindCopyFormat::DetailsJson => {
                return self.result_json(clock, selected_only).ok().flatten();
            }
            FindCopyFormat::ParentFolders => {
                let paths = hits
                    .iter()
                    .map(|hit| std::path::absolute(hit.reveal_path()).ok())
                    .collect::<Option<Vec<_>>>()?;
                let paths: Vec<_> = paths
                    .into_iter()
                    .filter(|path| path.parent().is_some())
                    .collect();
                if paths.is_empty() {
                    return None;
                }
                let text =
                    crate::clipboard::format(&paths, crate::clipboard::PathStyle::ParentPath, None);
                let count = text.lines().count();
                return Some((text, count));
            }
            FindCopyFormat::FileUrls => {
                let paths = hits
                    .iter()
                    .map(|hit| {
                        let path = hit.reveal_path();
                        if path.is_absolute() {
                            Some(path.to_path_buf())
                        } else {
                            std::path::absolute(path).ok()
                        }
                    })
                    .collect::<Option<Vec<_>>>()?;
                crate::clipboard::format(&paths, crate::clipboard::PathStyle::FileUrl, None)
            }
            FindCopyFormat::DetailsCsv => crate::listing_export::search_csv(
                &hits.iter().map(|hit| &hit.entry).collect::<Vec<_>>(),
            ),
            FindCopyFormat::ShellArguments => hits
                .iter()
                .map(|hit| crate::clipboard::shell_quote(&hit.reveal_path().to_string_lossy()))
                .collect::<Vec<_>>()
                .join(" "),
            FindCopyFormat::Paths
            | FindCopyFormat::Names
            | FindCopyFormat::NamesWithoutExtension => {
                let paths: Vec<_> = hits.iter().map(|hit| hit.entry.path.clone()).collect();
                let style = if self.result_copy_format == FindCopyFormat::Names {
                    crate::clipboard::PathStyle::NameOnly
                } else if self.result_copy_format == FindCopyFormat::NamesWithoutExtension {
                    crate::clipboard::PathStyle::NameWithoutExtension
                } else {
                    crate::clipboard::PathStyle::FullPath
                };
                crate::clipboard::format(&paths, style, None)
            }
        };
        Some((text, hits.len()))
    }

    fn select_all_visible_results(&mut self) {
        self.selected_results = self
            .results
            .iter()
            .filter(|hit| !self.hidden_results.contains(&hit.location))
            .map(|hit| hit.location.clone())
            .collect();
        self.result_selection_anchor = None;
    }

    fn invert_visible_result_selection(&mut self) {
        self.selected_results = self
            .results
            .iter()
            .filter(|hit| {
                !self.hidden_results.contains(&hit.location)
                    && !self.selected_results.contains(&hit.location)
            })
            .map(|hit| hit.location.clone())
            .collect();
        self.result_selection_anchor = None;
    }

    fn results_match_current_root(&self) -> bool {
        self.last_query
            .as_ref()
            .is_none_or(|(_, _, root)| root == &self.root)
    }

    fn remember_result_selection(&mut self) -> bool {
        if !self.results_match_current_root() {
            return false;
        }
        let locations: std::collections::HashSet<_> = self
            .results
            .iter()
            .filter(|hit| {
                !self.hidden_results.contains(&hit.location)
                    && self.selected_results.contains(&hit.location)
            })
            .map(|hit| hit.location.clone())
            .collect();
        if locations.is_empty() {
            return false;
        }
        self.remembered_result_selection = Some(FindResultSelection {
            root: self.root.clone(),
            locations,
        });
        true
    }

    fn restorable_result_selection_count(&self) -> usize {
        if !self.results_match_current_root() {
            return 0;
        }
        let Some(remembered) = self
            .remembered_result_selection
            .as_ref()
            .filter(|saved| saved.root == self.root)
        else {
            return 0;
        };
        self.results
            .iter()
            .filter(|hit| {
                !self.hidden_results.contains(&hit.location)
                    && remembered.locations.contains(&hit.location)
            })
            .count()
    }

    fn restore_result_selection(&mut self) -> bool {
        if !self.results_match_current_root() {
            return false;
        }
        let Some(remembered) = self
            .remembered_result_selection
            .as_ref()
            .filter(|saved| saved.root == self.root)
        else {
            return false;
        };
        let locations: std::collections::HashSet<_> = self
            .results
            .iter()
            .filter(|hit| {
                !self.hidden_results.contains(&hit.location)
                    && remembered.locations.contains(&hit.location)
            })
            .map(|hit| hit.location.clone())
            .collect();
        if locations.is_empty() {
            return false;
        }
        self.selected_results = locations;
        self.result_selection_anchor = None;
        true
    }

    fn clear_result_selection(&mut self) {
        self.selected_results.clear();
        self.result_selection_anchor = None;
    }

    fn select_only_result(
        &mut self,
        location: crate::search::SearchLocation,
        clock: SystemTime,
    ) -> bool {
        let order = self.visible_result_order(clock);
        let Some(row) = order
            .iter()
            .position(|&index| self.results[index].location == location)
        else {
            return false;
        };
        self.selected_results.clear();
        self.selected_results.insert(location.clone());
        self.result_selection_anchor = Some(location);
        self.highlighted_result = Some(row);
        self.highlight_scroll_pending = false;
        true
    }

    fn select_result(
        &mut self,
        location: crate::search::SearchLocation,
        range: bool,
        clock: SystemTime,
    ) {
        let order = self.visible_result_order(clock);
        let Some(target) = order
            .iter()
            .position(|&index| self.results[index].location == location)
        else {
            return;
        };
        if range
            && let Some(anchor) = self.result_selection_anchor.as_ref().and_then(|anchor| {
                order
                    .iter()
                    .position(|&index| &self.results[index].location == anchor)
            })
        {
            for &index in &order[anchor.min(target)..=anchor.max(target)] {
                self.selected_results
                    .insert(self.results[index].location.clone());
            }
        } else {
            if !self.selected_results.remove(&location) {
                self.selected_results.insert(location.clone());
            }
            self.result_selection_anchor = Some(location);
        }
    }

    fn highlighted_result_target(&self, clock: SystemTime) -> Option<PathBuf> {
        let index = *self
            .visible_result_order(clock)
            .get(self.highlighted_result?)?;
        self.results
            .get(index)
            .map(|hit| hit.reveal_path().to_path_buf())
    }

    fn hide_selected_results(&mut self) -> usize {
        let targets: std::collections::HashSet<_> = self
            .results
            .iter()
            .filter(|hit| {
                self.selected_results.contains(&hit.location)
                    && !self.hidden_results.contains(&hit.location)
            })
            .map(|hit| hit.location.clone())
            .collect();
        let count = targets.len();
        if count == 0 {
            return 0;
        }
        self.selected_results
            .retain(|location| !targets.contains(location));
        if self
            .result_selection_anchor
            .as_ref()
            .is_some_and(|anchor| targets.contains(anchor))
        {
            self.result_selection_anchor = None;
        }
        if self
            .explanation_open
            .as_ref()
            .is_some_and(|key| targets.contains(&key.1))
        {
            self.explanation_open = None;
        }
        self.hidden_results.extend(targets);
        self.highlighted_result = None;
        self.highlight_scroll_pending = false;
        count
    }

    fn hide_result(&mut self, location: crate::search::SearchLocation) {
        if !self.results.iter().any(|hit| hit.location == location) {
            return;
        }
        if self
            .explanation_open
            .as_ref()
            .is_some_and(|key| key.1 == location)
        {
            self.explanation_open = None;
        }
        self.selected_results.remove(&location);
        if self.result_selection_anchor.as_ref() == Some(&location) {
            self.result_selection_anchor = None;
        }
        self.hidden_results.insert(location);
        self.highlighted_result = None;
        self.highlight_scroll_pending = false;
    }

    fn restore_hidden_results(&mut self) {
        self.hidden_results.clear();
        self.highlighted_result = None;
        self.highlight_scroll_pending = false;
    }

    /// The exclusion editor's lines as paths, blank lines dropped.
    fn exclusion_paths(&self) -> Vec<PathBuf> {
        self.index_exclusions
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect()
    }

    /// Load a history entry's query and root, with `exclusions` formatted
    /// for that root, and close the history list.
    fn replay(&mut self, entry: crate::search::QueryHistoryEntry, exclusions: String) {
        self.expression = entry.expression;
        self.mode = entry.mode;
        self.root = entry.root;
        self.index_exclusions = exclusions;
        self.index_rerun_after_build = false;
        self.history_open = false;
    }

    /// Search under `root` instead, with `exclusions` formatted for it.
    fn change_root(&mut self, root: PathBuf, exclusions: String) {
        self.root = root;
        self.index_exclusions = exclusions;
        self.index_rerun_after_build = false;
    }

    /// Drop the query's `index`th predicate chip; false when the query does
    /// not parse or has no such predicate.
    fn remove_predicate(&mut self, index: usize) -> bool {
        let Ok(mut query) = self.build_query() else {
            return false;
        };
        if index >= query.predicates.len() {
            return false;
        }
        query.predicates.remove(index);
        self.expression = query.to_expression();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{FindCopyFormat, FindState};
    use super::{
        add_smart_folder, explain_key, explained_hit, fail_find_search_start, history_row_label,
        index_build_refusal, index_skipped_summary, move_result_highlight, pivot_order,
        result_row_height, search_root_error,
    };
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn json_state() -> FindState {
        let root = if cfg!(windows) {
            PathBuf::from(r"C:\x")
        } else {
            PathBuf::from("/x")
        };
        let mut state = FindState {
            root: root.clone(),
            expression: "name:report".into(),
            result_copy_format: FindCopyFormat::DetailsJson,
            result_complete: true,
            ran: true,
            scanned: 8,
            matched: 3,
            search_source: "Indexed snapshot".into(),
            results: vec![
                hit("report-a", None),
                hit("report-hidden", None),
                hit("bundle.zip!/report-a", None),
            ],
            ..Default::default()
        };
        for hit in &mut state.results {
            hit.entry.path = root.join(&hit.entry.name);
            hit.location = crate::search::SearchLocation::Native(hit.entry.path.clone());
        }
        state.results[2].location = crate::search::SearchLocation::ArchiveMember {
            archive: root.join("bundle.zip"),
            member: "report-a".into(),
            index: 7,
        };
        state.results[0].explanation = crate::search::MatchExplanation {
            total_score: 42,
            components: vec![crate::search::MatchComponent {
                field: "name",
                detail: "пример\n".into(),
                score: 42,
                matched_ranges: vec![(0, 3)],
            }],
        };
        state.last_query = Some(("name:report".into(), state.mode, state.root.clone()));
        state
    }
    #[test]
    fn created_report_preserves_exact_native_times_without_archive_fallback() {
        let mut state = json_state();
        for (value, expected) in [
            (None, None),
            (Some(SystemTime::UNIX_EPOCH), Some("1970-01-01T00:00:00Z")),
            (
                Some(SystemTime::UNIX_EPOCH - std::time::Duration::from_millis(500)),
                Some("1969-12-31T23:59:59.500Z"),
            ),
        ] {
            state.results[0].entry.created = value;
            state.results[2].entry.created = value;
            let (text, _) = state
                .result_json(SystemTime::now(), false)
                .unwrap()
                .unwrap();
            let r: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(r["rows"][0]["created_utc"].as_str(), expected);
            assert!(r["rows"][2]["created_utc"].is_null());
        }
    }

    #[test]
    fn changed_report_preserves_exact_native_times_without_archive_fallback() {
        let mut state = json_state();
        for (value, expected) in [
            (None, None),
            (Some(SystemTime::UNIX_EPOCH), Some("1970-01-01T00:00:00Z")),
            (
                Some(SystemTime::UNIX_EPOCH - std::time::Duration::from_millis(500)),
                Some("1969-12-31T23:59:59.500Z"),
            ),
        ] {
            state.results[0].entry.changed = value;
            state.results[2].entry.changed = value;
            let (text, _) = state
                .result_json(SystemTime::now(), false)
                .unwrap()
                .unwrap();
            let r: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(r["rows"][0]["changed_utc"].as_str(), expected);
            assert!(r["rows"][2]["changed_utc"].is_null());
        }
    }

    #[test]
    fn metric_query_report_preserves_zero_full_width_and_unknown_values() {
        let mut state = json_state();
        for value in [Some(0), Some(u64::MAX), None] {
            state.results[0].entry.allocated_size = value;
            state.results[0].entry.hard_links = value;
            let (text, _) = state
                .result_json(SystemTime::now(), false)
                .unwrap()
                .unwrap();
            let report: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(report["rows"][0]["allocated_bytes"].as_u64(), value);
            assert_eq!(report["rows"][0]["hard_links"].as_u64(), value);
        }
    }

    #[test]
    fn search_json_uses_applied_context_and_distinct_locations_in_visible_selected_order() {
        let now = SystemTime::now();
        let mut state = json_state();
        state.hide_result(state.results[1].location.clone());
        state.select_result(state.results[2].location.clone(), false, now);
        state.results[0].entry.modified =
            SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(1 << 43));
        state.expression = "type:invalid".into();
        state.root = "/another".into();
        state.mode = crate::query::MatchMode::Regex;
        state.pending_rerun = true;
        let (text, count) = state.result_json(now, false).unwrap().unwrap();
        let full: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(count, 2);
        assert_eq!(full["context"]["expression"], "name:report");
        assert_eq!(full["context"]["mode"], "Exact");
        assert_eq!(
            full["root"]["text"],
            state.last_query.as_ref().unwrap().2.to_str().unwrap()
        );
        assert_eq!(full["context"]["source"], "Indexed snapshot");
        assert_eq!(full["context"]["received_rows"], 3);
        assert_eq!(full["context"]["visible_rows"], 2);
        assert_eq!(full["context"]["visible_selected_rows"], 1);
        assert_eq!(full["context"]["draft_differs"], true);
        assert_eq!(full["context"]["pending_rerun"], true);
        assert_eq!(full["partial"], false);
        assert_eq!(full["rows"][0]["location"]["kind"], "native");
        assert!(full["rows"][0]["modified_utc"].is_null());
        assert_eq!(full["rows"][0]["score"], 42);
        assert_eq!(full["rows"][0]["explanation"][0]["detail"], "пример\n");
        assert_eq!(
            full["rows"][0]["explanation"][0]["matched_character_ranges"],
            serde_json::json!([[0, 3]])
        );
        assert_eq!(full["rows"][1]["location"]["kind"], "archive_member");
        assert_eq!(full["rows"][1]["location"]["member"]["text"], "report-a");
        assert_eq!(full["rows"][1]["location"]["index"], 7);
        assert!(full["rows"][1]["native_id"].is_null());
        let (text, count) = state.result_json(now, true).unwrap().unwrap();
        let selected: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(count, 1);
        assert_eq!(selected["scope"], "visible_selected_results");
        assert_eq!(selected["rows"][0]["location"], full["rows"][1]["location"]);
    }
    #[test]
    fn search_json_marks_partial_and_can_export_an_empty_completed_query() {
        let now = SystemTime::now();
        let mut state = json_state();
        state.results.clear();
        state.matched = 0;
        let (text, count) = state.prepare_result_copy(now, false).unwrap();
        let report: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(count, 0);
        assert_eq!(report["rows"], serde_json::json!([]));
        assert_eq!(report["partial"], false);
        assert!(state.result_json(now, true).unwrap().is_none());
        for reason in 0..4 {
            state.result_complete = reason != 0;
            state.truncated = reason == 1;
            state.content_skipped = usize::from(reason == 2);
            state.archive_error = (reason == 3).then(|| "bad archive".into());
            let (text, _) = state.result_json(now, false).unwrap().unwrap();
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&text).unwrap()["partial"],
                true
            );
        }
        state.last_query = None;
        assert!(state.prepare_result_copy(now, false).is_none());
        assert!(
            state
                .result_action_error
                .as_ref()
                .unwrap()
                .contains("applied query")
        );
    }
    #[test]
    fn search_json_is_bounded_and_reports_refusal_before_a_clipboard_value_exists() {
        let now = SystemTime::now();
        let mut state = json_state();
        state.results[0].entry.name = "x".repeat(1024 * 1024 + 1);
        assert!(state.prepare_result_copy(now, false).is_none());
        assert!(
            state
                .result_action_error
                .as_ref()
                .unwrap()
                .contains("1 MiB")
        );
        state.results[0].entry.name = "=formula,\"\n\t".into();
        let (text, count) = state.prepare_result_copy(now, false).unwrap();
        assert_eq!(count, 3);
        assert!(state.result_action_error.is_none());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["rows"][0]["display_name"],
            state.results[0].entry.name
        );
        state.results[2].location = crate::search::SearchLocation::ArchiveMember {
            archive: "/x/bundle.zip".into(),
            member: "../escape".into(),
            index: 7,
        };
        assert!(state.prepare_result_copy(now, false).is_none());
        assert!(
            state
                .result_action_error
                .as_ref()
                .unwrap()
                .contains("traversal")
        );
        state.results[2].location = crate::search::SearchLocation::Native("/different".into());
        assert!(state.prepare_result_copy(now, false).is_none());
    }
    #[cfg(unix)]
    #[test]
    fn search_json_preserves_non_utf8_path_bytes_without_replacement_and_ties_by_location() {
        use std::os::unix::ffi::OsStringExt;
        let mut state = json_state();
        let raw = PathBuf::from(std::ffi::OsString::from_vec(b"/x/raw\xff\n.txt".to_vec()));
        state.results[0].entry.path = raw.clone();
        state.results[0].location = crate::search::SearchLocation::Native(raw);
        state.results[0].identity = crate::search::FileIdentity::Native {
            volume: u64::MAX,
            file: u64::MAX - 1,
        };
        state.results[1].identity = state.results[0].identity.clone();
        let (text, _) = state
            .result_json(SystemTime::now(), false)
            .unwrap()
            .unwrap();
        let report: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            report["rows"][0]["native_id"]["device_id"].as_u64(),
            Some(u64::MAX)
        );
        assert_eq!(
            report["rows"][0]["native_id"]["file_id"].as_u64(),
            Some(u64::MAX - 1)
        );
        assert_eq!(
            report["rows"][0]["location"]["path"],
            serde_json::json!({"encoding":"unix_bytes_hex","hex":"2f782f726177ff0a2e747874"})
        );
        state.select_result(state.results[0].location.clone(), false, SystemTime::now());
        state.select_result(state.results[1].location.clone(), false, SystemTime::now());
        let (text, count) = state.result_json(SystemTime::now(), true).unwrap().unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&text).unwrap()["exported_rows"],
            2
        );
    }

    #[test]
    fn exclusion_editor_lines_become_trimmed_paths() {
        let state = FindState {
            index_exclusions: "  /a/node_modules \n\n/b\n   ".to_string(),
            ..FindState::default()
        };
        assert_eq!(
            state.exclusion_paths(),
            [PathBuf::from("/a/node_modules"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn replay_and_root_change_reset_the_index_rerun() {
        let mut state = FindState {
            history_open: true,
            index_rerun_after_build: true,
            ..FindState::default()
        };
        state.replay(
            crate::search::QueryHistoryEntry {
                expression: "report".to_string(),
                mode: crate::query::MatchMode::default(),
                root: PathBuf::from("/docs"),
                result_count: 3,
                scanned: 10,
                duration_ms: 5,
                last_run: 1,
            },
            "/docs/tmp".to_string(),
        );
        assert_eq!(state.expression, "report");
        assert_eq!(state.root, PathBuf::from("/docs"));
        assert_eq!(state.index_exclusions, "/docs/tmp");
        assert!(!state.history_open && !state.index_rerun_after_build);

        state.index_rerun_after_build = true;
        state.change_root(PathBuf::from("/other"), String::new());
        assert_eq!(state.root, PathBuf::from("/other"));
        assert!(!state.index_rerun_after_build);
    }

    #[test]
    fn failed_restart_retires_the_previous_search_run() {
        let root = crate::testutil::TempDir::new();
        let engine = crate::search::SearchEngine::default();
        let run = engine
            .start(
                root.path().to_path_buf(),
                crate::query::Query::default(),
                10,
                std::sync::Arc::new(|| {}),
            )
            .unwrap();
        let mut state = FindState {
            run: Some(run),
            pending_rerun: true,
            ..FindState::default()
        };

        fail_find_search_start(&mut state, "provider refused".to_string());

        assert!(!state.searching());
        assert!(!state.pending_rerun);
        assert_eq!(state.error.as_deref(), Some("provider refused"));
        engine.cancel();
    }

    #[test]
    fn removing_a_predicate_rewrites_the_expression() {
        let mut state = FindState::default();
        assert!(!state.remove_predicate(0), "an empty query has no chips");
        state.expression = "name:report path:docs".to_string();
        assert!(!state.remove_predicate(2));
        assert!(state.remove_predicate(0));
        let query = state.build_query().unwrap();
        assert_eq!(
            query.predicates,
            [crate::query::Predicate::PathContains("docs".to_string())]
        );
    }

    fn hit(name: &str, modified: Option<SystemTime>) -> crate::search::SearchHit {
        let path = PathBuf::from(format!("/x/{name}"));
        crate::search::SearchHit {
            entry: crate::panel::FileEntry {
                symlink: None,
                permissions: None,
                allocated_size: None,
                hard_links: None,
                owner_uid: None,
                owner_gid: None,
                accessed: None,
                created: None,
                accessed_str: "Unavailable".into(),
                created_str: "Unavailable".into(),
                changed: None,
                changed_str: "Unavailable".into(),
                name: name.to_string(),
                name_lower: name.to_lowercase(),
                sort_key: crate::panel::diacritic_sort_key(&name.to_lowercase()),
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
    fn highlighted_background_tab_target_uses_visible_order_and_archive_container() {
        let now = SystemTime::now();
        let mut archived = hit("member.txt", Some(now));
        let archive = PathBuf::from("/bundle.zip");
        archived.location = crate::search::SearchLocation::ArchiveMember {
            archive: archive.clone(),
            member: PathBuf::from("member.txt"),
            index: 0,
        };
        let mut state = FindState {
            results: vec![hit("hidden", None), archived],
            ..FindState::default()
        };
        assert!(state.highlighted_result_target(now).is_none());
        state.hide_result(state.results[0].location.clone());
        for pivot in crate::search::TimePivot::ALL {
            state.time_pivot = pivot;
            state.highlighted_result = Some(0);
            assert_eq!(state.highlighted_result_target(now), Some(archive.clone()));
        }
        state.highlighted_result = Some(1);
        assert!(state.highlighted_result_target(now).is_none());
        state.hide_result(state.results[1].location.clone());
        state.highlighted_result = Some(0);
        assert!(state.highlighted_result_target(now).is_none());
    }

    #[test]
    fn context_selection_tracks_visible_locations_and_ignores_hidden_targets() {
        let now = SystemTime::now();
        let mut state = FindState {
            results: vec![
                hit("one.txt", None),
                hit("hard.txt", None),
                hit("hidden.txt", None),
            ],
            ..FindState::default()
        };
        let one = state.results[0].location.clone();
        let other = state.results[1].location.clone();
        let hidden = state.results[2].location.clone();
        state.select_all_visible_results();
        state.hide_result(hidden.clone());
        assert!(state.select_only_result(other.clone(), now));
        assert_eq!(
            state.selected_results,
            std::collections::HashSet::from([other.clone()])
        );
        assert_eq!(state.highlighted_result, Some(1));
        assert_eq!(state.result_selection_anchor, Some(other.clone()));
        assert!(!state.select_only_result(hidden, now));
        assert!(!state.select_only_result(
            crate::search::SearchLocation::Native(PathBuf::from("/missing")),
            now
        ));
        assert_eq!(
            state.selected_results,
            std::collections::HashSet::from([other])
        );
        state.results.swap(0, 1);
        assert!(state.select_only_result(one.clone(), now));
        assert_eq!(
            state.selected_results,
            std::collections::HashSet::from([one])
        );
        assert_eq!(state.highlighted_result, Some(1));
    }

    #[test]
    fn find_relative_paths_bind_applied_root_and_preserve_members_view_and_selection() {
        let now = SystemTime::now();
        let mut state = FindState {
            root: "/draft/elsewhere".into(),
            last_query: Some(("".into(), crate::query::MatchMode::Exact, "/x".into())),
            result_copy_format: FindCopyFormat::RelativePaths,
            results: vec![
                hit("nested/файл.txt", None),
                hit("bundle.zip!/ignored", None),
                hit("hidden.txt", None),
            ],
            ..Default::default()
        };
        state.results[1].location = crate::search::SearchLocation::ArchiveMember {
            archive: "/x/packs/bundle.zip".into(),
            member: "deep/member.txt".into(),
            index: 7,
        };
        state
            .hidden_results
            .insert(state.results[2].location.clone());
        state
            .selected_results
            .insert(state.results[1].location.clone());
        state.result_sort = crate::search_order::Sort {
            field: crate::search_order::Field::Rank,
            descending: true,
        };
        assert_eq!(
            state.prepare_result_copy(now, false),
            Some((
                "packs/bundle.zip!/deep/member.txt\nnested/файл.txt".into(),
                2
            ))
        );
        assert_eq!(
            state.prepare_result_copy(now, true),
            Some(("packs/bundle.zip!/deep/member.txt".into(), 1))
        );
        assert_eq!(state.root, PathBuf::from("/draft/elsewhere"));
        assert_eq!(state.selected_results.len(), 1);
        state.results[0].location = crate::search::SearchLocation::Native("/x".into());
        assert_eq!(
            state.prepare_result_copy(now, false),
            Some(("packs/bundle.zip!/deep/member.txt\n.".into(), 2))
        );
        state.selected_results.clear();
        assert!(state.prepare_result_copy(now, true).is_none());
        state.results[0].location =
            crate::search::SearchLocation::Native("/x/line\nbreak.txt".into());
        assert_eq!(
            state.prepare_result_copy(now, false),
            Some((
                "packs/bundle.zip!/deep/member.txt\nline\\nbreak.txt".into(),
                2
            ))
        );
    }
    #[test]
    fn find_relative_paths_refuse_unbound_outside_and_traversing_rows_atomically() {
        let now = SystemTime::now();
        let mut state = FindState {
            result_copy_format: FindCopyFormat::RelativePaths,
            results: vec![hit("good.txt", None), hit("second.txt", None)],
            ..Default::default()
        };
        assert!(state.prepare_result_copy(now, false).is_none());
        assert!(
            state
                .result_action_error
                .as_ref()
                .unwrap()
                .contains("applied search root")
        );
        state.last_query = Some(("".into(), crate::query::MatchMode::Exact, "/x".into()));
        for path in ["/xy/other", "/x/../outside"] {
            state.results[1].location = crate::search::SearchLocation::Native(path.into());
            assert!(state.prepare_result_copy(now, false).is_none());
            assert!(state.result_action_error.is_some());
        }
        state.results[1].location = crate::search::SearchLocation::ArchiveMember {
            archive: "/x/a.zip".into(),
            member: "../bad".into(),
            index: 0,
        };
        assert!(state.prepare_result_copy(now, false).is_none());
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            state.results[1].location = crate::search::SearchLocation::Native(PathBuf::from(
                std::ffi::OsString::from_vec(b"/x/\xff".to_vec()),
            ));
            assert!(state.prepare_result_copy(now, false).is_none());
            assert!(
                state
                    .result_action_error
                    .as_ref()
                    .unwrap()
                    .contains("UTF-8")
            );
        }
        state.results[1].location = crate::search::SearchLocation::Native("/x/second.txt".into());
        assert_eq!(
            state.prepare_result_copy(now, false),
            Some(("good.txt\nsecond.txt".into(), 2))
        );
        assert!(state.result_action_error.is_none());
    }

    #[test]
    fn find_parent_folders_deduplicate_real_targets_in_visible_order() {
        let now = SystemTime::now();
        let mut state = FindState {
            result_copy_format: FindCopyFormat::ParentFolders,
            results: vec![
                hit("first/a", None),
                hit("first/b", None),
                hit("second/c", None),
                hit("hidden/d", None),
                hit("bundle.zip!/one", None),
                hit("bundle.zip!/two", None),
                hit("folder", None),
            ],
            ..Default::default()
        };
        for index in [4, 5] {
            state.results[index].location = crate::search::SearchLocation::ArchiveMember {
                archive: PathBuf::from("/packs/bundle.zip"),
                member: PathBuf::from(index.to_string()),
                index,
            };
        }
        state.results[6].entry.is_dir = true;
        state.hide_result(state.results[3].location.clone());
        assert_eq!(
            state.visible_result_paths_clipboard(now),
            Some(("/x/first\n/x/second\n/packs\n/x".into(), 4))
        );
        state.selected_results.extend([
            state.results[1].location.clone(),
            state.results[4].location.clone(),
        ]);
        assert_eq!(
            state.selected_result_paths_clipboard(now),
            Some(("/x/first\n/packs".into(), 2))
        );
        state.selected_results.clear();
        assert!(state.selected_result_paths_clipboard(now).is_none());
        state.results = vec![hit("root", None)];
        state.results[0].location = crate::search::SearchLocation::Native(PathBuf::from("/"));
        assert!(state.visible_result_paths_clipboard(now).is_none());
    }

    #[test]
    fn find_names_without_extensions_preserve_members_dotfiles_order_and_selection() {
        let now = SystemTime::now();
        let mut state = FindState {
            result_copy_format: FindCopyFormat::NamesWithoutExtension,
            results: vec![
                hit(".env", None),
                hit("archive.tar.gz", None),
                hit("hidden.txt", None),
                hit("folder.with.dots", None),
                hit("bundle.zip!/name.txt", None),
                hit("name.md", None),
                hit("line\nname.txt", None),
                hit("ёж.文.txt", None),
            ],
            ..Default::default()
        };
        state.results[3].entry.is_dir = true;
        state.results[4].location = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/bundles/container.zip"),
            member: PathBuf::from("name.txt"),
            index: 0,
        };
        state.hide_result(state.results[2].location.clone());
        assert_eq!(
            state.visible_result_paths_clipboard(now),
            Some((
                ".env\narchive.tar\nfolder.with\nname\nname\nline\\nname\nёж.文".into(),
                7
            ))
        );
        state.selected_results.extend([
            state.results[4].location.clone(),
            state.results[5].location.clone(),
        ]);
        assert_eq!(
            state.selected_result_paths_clipboard(now),
            Some(("name\nname".into(), 2))
        );
        state.selected_results.clear();
        assert!(state.selected_result_paths_clipboard(now).is_none());
        state.results.clear();
        assert!(state.visible_result_paths_clipboard(now).is_none());
    }

    #[test]
    fn find_file_urls_preserve_visible_selection_order_and_real_archive_containers() {
        let now = SystemTime::now();
        let mut state = FindState {
            result_copy_format: FindCopyFormat::FileUrls,
            results: vec![
                hit("a b#?%.txt", None),
                hit("hidden.txt", None),
                hit("bundle.zip!/one.txt", None),
                hit("bundle.zip!/two.txt", None),
            ],
            ..Default::default()
        };
        for (index, member) in [(2, "one.txt"), (3, "two.txt")] {
            state.results[index].location = crate::search::SearchLocation::ArchiveMember {
                archive: PathBuf::from("/x/a b.zip"),
                member: PathBuf::from(member),
                index,
            };
        }
        let hidden = state.results[1].location.clone();
        state.hide_result(hidden);
        assert_eq!(
            state.visible_result_paths_clipboard(now),
            Some((
                "file:///x/a%20b%23%3F%25.txt\nfile:///x/a%20b.zip\nfile:///x/a%20b.zip".into(),
                3
            ))
        );
        state
            .selected_results
            .insert(state.results[3].location.clone());
        assert_eq!(
            state.selected_result_paths_clipboard(now),
            Some(("file:///x/a%20b.zip".into(), 1))
        );
        state.selected_results.clear();
        assert!(state.selected_result_paths_clipboard(now).is_none());
        state.results.clear();
        assert!(state.visible_result_paths_clipboard(now).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn find_file_urls_preserve_raw_name_bytes_controls_and_relative_targets() {
        use std::os::unix::ffi::OsStringExt;
        let now = SystemTime::now();
        let mut raw = hit("raw", None);
        let path = PathBuf::from(std::ffi::OsString::from_vec(b"/x/raw\xff\n\t.txt".to_vec()));
        raw.entry.path = path.clone();
        raw.location = crate::search::SearchLocation::Native(path);
        let mut state = FindState {
            result_copy_format: FindCopyFormat::FileUrls,
            results: vec![raw],
            ..Default::default()
        };
        assert_eq!(
            state.visible_result_paths_clipboard(now),
            Some(("file:///x/raw%FF%0A%09.txt".into(), 1))
        );
        let relative = PathBuf::from("relative file.txt");
        state.results[0].entry.path = relative.clone();
        state.results[0].location = crate::search::SearchLocation::Native(relative);
        let (text, count) = state.visible_result_paths_clipboard(now).unwrap();
        assert!(text.starts_with("file:///"));
        assert!(text.ends_with("/relative%20file.txt"));
        assert_eq!(count, 1);
    }

    #[test]
    fn find_details_csv_copies_visible_or_selected_metadata_in_display_order() {
        let now = SystemTime::now();
        let mut state = FindState {
            result_copy_format: FindCopyFormat::DetailsCsv,
            results: vec![
                hit("same.txt", None),
                hit("hidden.txt", None),
                hit("bundle.zip!/same.txt", None),
            ],
            ..FindState::default()
        };
        state.results[2].entry.name = "same.txt".into();
        state.results[2].entry.name_lower = "same.txt".into();
        state.results[2].entry.sort_key = "same.txt".into();
        state.results[2].location = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/x/bundle.zip"),
            member: PathBuf::from("same.txt"),
            index: 0,
        };
        state.hide_result(state.results[1].location.clone());
        let (text, count) = state.visible_result_paths_clipboard(now).unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            text,
            "Path,Name,Kind,Size (bytes),Modified\n/x/same.txt,same.txt,file,1,\n/x/bundle.zip!/same.txt,same.txt,file,1,\n"
        );
        state.select_result(state.results[2].location.clone(), false, now);
        assert_eq!(
            state.selected_result_paths_clipboard(now),
            Some((
                "Path,Name,Kind,Size (bytes),Modified\n/x/bundle.zip!/same.txt,same.txt,file,1,\n"
                    .into(),
                1
            ))
        );
        state.results.swap(0, 2);
        assert!(
            state
                .visible_result_paths_clipboard(now)
                .unwrap()
                .0
                .starts_with("Path,Name,Kind,Size (bytes),Modified\n/x/bundle.zip!/same.txt,")
        );
        state.clear_result_selection();
        assert!(state.selected_result_paths_clipboard(now).is_none());
    }

    #[test]
    fn find_copy_formats_preserve_names_and_parse_as_literal_shell_arguments() {
        let now = SystemTime::now();
        let mut state = FindState {
            results: vec![
                hit("it's $(printf HACK)\nfile.txt", None),
                hit("hidden.txt", None),
                hit("bundle.zip!/a.txt", None),
            ],
            ..FindState::default()
        };
        state.results[2].location = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/x/archive space.zip"),
            member: PathBuf::from("a.txt"),
            index: 0,
        };
        state.select_all_visible_results();
        state.hide_result(state.results[1].location.clone());
        state.result_copy_format = FindCopyFormat::Names;
        assert_eq!(
            state.selected_result_paths_clipboard(now),
            Some(("it's $(printf HACK)\\nfile.txt\na.txt".into(), 2))
        );
        state.result_copy_format = FindCopyFormat::ShellArguments;
        let (text, count) = state.selected_result_paths_clipboard(now).unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            text,
            "'/x/it'\\''s $(printf HACK)\nfile.txt' '/x/archive space.zip'"
        );
        #[cfg(unix)]
        {
            let script = format!("set -- {text}; printf '%s\\0' \"$@\"");
            let output = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(script)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                output.stdout,
                b"/x/it's $(printf HACK)\nfile.txt\0/x/archive space.zip\0"
            );
        }
        state.result_copy_format = FindCopyFormat::Paths;
        assert!(
            state
                .selected_result_paths_clipboard(now)
                .unwrap()
                .0
                .ends_with("/x/bundle.zip!/a.txt")
        );
        state.clear_result_selection();
        assert!(state.selected_result_paths_clipboard(now).is_none());
        assert_eq!(state.visible_result_paths_clipboard(now).unwrap().1, 2);
    }

    #[test]
    fn remembered_find_selection_is_root_bound_and_restores_only_current_visible_locations() {
        let now = SystemTime::now();
        let mut state = FindState {
            root: PathBuf::from("/x"),
            results: vec![
                hit("a.txt", None),
                hit("hard.txt", None),
                hit("hidden.txt", None),
                hit("bundle.zip!/a.txt", None),
            ],
            ..FindState::default()
        };
        state.last_query = Some((String::new(), state.mode, state.root.clone()));
        let a = state.results[0].location.clone();
        let hard = state.results[1].location.clone();
        let archive = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/x/bundle.zip"),
            member: PathBuf::from("a.txt"),
            index: 0,
        };
        state.results[3].location = archive.clone();
        let mut duplicate = state.results[3].clone();
        duplicate.location = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/x/bundle.zip"),
            member: PathBuf::from("a.txt"),
            index: 1,
        };
        state.results.push(duplicate);
        assert!(!state.remember_result_selection());
        assert!(!state.restore_result_selection());
        state.select_result(a.clone(), false, now);
        state.select_result(archive.clone(), false, now);
        assert!(state.remember_result_selection());
        assert_eq!(state.restorable_result_selection_count(), 2);
        state.hide_result(archive.clone());
        state.clear_result_selection();
        state.select_result(hard.clone(), false, now);
        assert!(state.restore_result_selection());
        assert_eq!(
            state.selected_results,
            std::collections::HashSet::from([a.clone()])
        );
        assert!(state.result_selection_anchor.is_none());
        state.restore_hidden_results();
        assert_eq!(state.restorable_result_selection_count(), 2);
        assert!(state.restore_result_selection());
        assert_eq!(state.selected_results.len(), 2);
        // A refined query no longer returns the native remembered path.
        state.results.retain(|hit| hit.location != a);
        state.clear_result_selection();
        assert!(state.restore_result_selection());
        assert_eq!(
            state.selected_results,
            std::collections::HashSet::from([archive.clone()])
        );
        state.results.push(hit("a.txt", None));
        assert_eq!(
            state.selected_results.len(),
            1,
            "later hits are not auto-selected"
        );
        assert!(state.restore_result_selection());
        assert_eq!(state.selected_results.len(), 2);
        state.root = PathBuf::from("/other");
        assert_eq!(state.restorable_result_selection_count(), 0);
        let before = state.selected_results.clone();
        assert!(!state.restore_result_selection());
        assert_eq!(state.selected_results, before);
        // Remembering again replaces the one snapshot, including its root.
        state.clear_result_selection();
        state.select_result(hard, false, now);
        assert!(
            !state.remember_result_selection(),
            "old results cannot be rebound before the new-root search starts"
        );
        state.last_query = Some((String::new(), state.mode, state.root.clone()));
        assert!(state.remember_result_selection());
        state.root = PathBuf::from("/x");
        assert_eq!(state.restorable_result_selection_count(), 0);
        state.clear_result_selection();
        assert!(!state.remember_result_selection());
        assert_eq!(
            state.remembered_result_selection.as_ref().unwrap().root,
            PathBuf::from("/other")
        );
    }

    #[test]
    fn bulk_visible_result_selection_excludes_hidden_and_tracks_streamed_locations() {
        let now = SystemTime::now();
        let mut state = FindState {
            results: vec![
                hit("a.txt", None),
                hit("hard.txt", None),
                hit("hidden.txt", None),
                hit("bundle.zip!/a.txt", None),
            ],
            ..FindState::default()
        };
        let hidden = state.results[2].location.clone();
        state.hide_result(hidden.clone());
        state.results[3].location = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/x/bundle.zip"),
            member: PathBuf::from("a.txt"),
            index: 0,
        };
        let mut duplicate = state.results[3].clone();
        duplicate.location = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/x/bundle.zip"),
            member: PathBuf::from("a.txt"),
            index: 1,
        };
        state.results.push(duplicate);
        state.select_result(state.results[0].location.clone(), false, now);
        state.highlighted_result = Some(1);
        state.invert_visible_result_selection();
        assert_eq!(state.visible_selected_result_count(), 3);
        assert!(!state.selected_results.contains(&state.results[0].location));
        assert!(!state.selected_results.contains(&hidden));
        assert!(state.result_selection_anchor.is_none());
        assert_eq!(state.highlighted_result, Some(1));
        state.invert_visible_result_selection();
        assert_eq!(state.visible_selected_result_count(), 1);
        state.select_all_visible_results();
        assert_eq!(state.visible_selected_result_count(), 4);
        assert_eq!(state.selected_results.len(), 4);
        assert_eq!(state.selected_result_paths_clipboard(now).unwrap().1, 4);
        state.results.swap(0, 4);
        state.results.push(hit("streamed.txt", None));
        assert_eq!(
            state.visible_selected_result_count(),
            4,
            "selection is a snapshot, not a sticky mode"
        );
        state.invert_visible_result_selection();
        assert_eq!(
            state.selected_result_paths_clipboard(now),
            Some(("/x/streamed.txt".into(), 1))
        );
        state.restore_hidden_results();
        assert!(!state.selected_results.contains(&hidden));
        state.select_all_visible_results();
        assert_eq!(state.visible_selected_result_count(), 6);
        for hit in &state.results {
            state.hidden_results.insert(hit.location.clone());
        }
        state.select_all_visible_results();
        assert!(state.selected_results.is_empty());
        state.invert_visible_result_selection();
        assert!(state.selected_results.is_empty());
        state.results.clear();
        state.invert_visible_result_selection();
        assert!(state.selected_results.is_empty());
    }

    #[test]
    fn selected_search_tab_targets_preserve_visible_order_and_archive_containers() {
        let now = SystemTime::now();
        let mut state = FindState {
            results: vec![
                hit("a.txt", None),
                hit("bundle.zip!/a.txt", None),
                hit("bundle.zip!/b.txt", None),
                hit("hidden.txt", None),
            ],
            ..FindState::default()
        };
        let archive = PathBuf::from("/x/bundle.zip");
        for (index, name) in [(1, "a.txt"), (2, "b.txt")] {
            state.results[index].location = crate::search::SearchLocation::ArchiveMember {
                archive: archive.clone(),
                member: PathBuf::from(name),
                index,
            };
        }
        state
            .selected_results
            .extend(state.results.iter().map(|hit| hit.location.clone()));
        state.hide_result(state.results[3].location.clone());
        assert_eq!(
            state.selected_result_tab_targets(now),
            [PathBuf::from("/x/a.txt"), archive.clone(), archive.clone()]
        );
        state.results.swap(0, 2);
        assert_eq!(
            state.selected_result_tab_targets(now),
            [archive.clone(), archive, PathBuf::from("/x/a.txt")]
        );
        state.clear_result_selection();
        assert!(state.selected_result_tab_targets(now).is_empty());
    }

    #[test]
    fn hide_selected_search_results_is_atomic_by_location_and_restorable() {
        let now = SystemTime::now();
        let mut state = FindState {
            results: vec![
                hit("a.txt", None),
                hit("hard.txt", None),
                hit("bundle.zip!/a.txt", None),
            ],
            matched: 3,
            scanned: 5,
            ..FindState::default()
        };
        let a = state.results[0].location.clone();
        let unselected_hard_link = state.results[1].location.clone();
        let archive = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/x/bundle.zip"),
            member: PathBuf::from("a.txt"),
            index: 0,
        };
        state.results[2].location = archive.clone();
        let mut duplicate = state.results[2].clone();
        let duplicate_location = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/x/bundle.zip"),
            member: PathBuf::from("a.txt"),
            index: 1,
        };
        duplicate.location = duplicate_location.clone();
        state.results.push(duplicate);
        state.select_result(a.clone(), false, now);
        state.select_result(archive.clone(), false, now);
        state.explanation_open = Some(explain_key(&state.results[2]));
        state.highlighted_result = Some(2);
        state.highlight_scroll_pending = true;
        assert_eq!(state.hide_selected_results(), 2);
        assert_eq!(state.visible_result_order(now), [1, 3]);
        assert!(!state.hidden_results.contains(&unselected_hard_link));
        assert!(!state.hidden_results.contains(&duplicate_location));
        assert!(state.selected_results.is_empty());
        assert!(state.result_selection_anchor.is_none());
        assert!(state.explanation_open.is_none());
        assert!(state.highlighted_result.is_none());
        assert!(!state.highlight_scroll_pending);
        assert_eq!(
            (state.results.len(), state.matched, state.scanned),
            (4, 3, 5)
        );
        assert_eq!(state.hide_selected_results(), 0);
        state.results.swap(0, 3);
        state.results.push(hit("streamed.txt", None));
        assert_eq!(state.visible_result_count(), 3);
        state.restore_hidden_results();
        assert_eq!(state.visible_result_count(), 5);
        assert_eq!(state.visible_selected_result_count(), 0);
        assert_eq!(state.hide_selected_results(), 0);
    }

    #[test]
    fn search_result_selection_survives_streaming_sorting_ranges_and_hiding() {
        let now = SystemTime::now();
        let mut state = FindState {
            results: vec![
                hit("a.txt", None),
                hit("hidden.txt", None),
                hit("b.txt", Some(now)),
                hit("bundle.zip!/a.txt", None),
            ],
            ..FindState::default()
        };
        let a = state.results[0].location.clone();
        let b = state.results[2].location.clone();
        let hidden = state.results[1].location.clone();
        let archived = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/x/bundle.zip"),
            member: PathBuf::from("a.txt"),
            index: 0,
        };
        state.results[3].location = archived.clone();
        let mut duplicate = state.results[3].clone();
        let duplicate_location = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/x/bundle.zip"),
            member: PathBuf::from("a.txt"),
            index: 1,
        };
        duplicate.location = duplicate_location.clone();
        state.results.push(duplicate);
        state.hidden_results.insert(hidden.clone());
        state.select_result(a.clone(), false, now);
        state.select_result(b.clone(), true, now);
        assert_eq!(state.visible_selected_result_count(), 2);
        assert!(!state.selected_results.contains(&hidden));
        assert_eq!(
            state.selected_result_paths_clipboard(now),
            Some(("/x/a.txt\n/x/b.txt".into(), 2))
        );
        state.results.swap(0, 2);
        state.results.push(hit("streamed.txt", None));
        assert_eq!(
            state.selected_result_paths_clipboard(now),
            Some(("/x/b.txt\n/x/a.txt".into(), 2))
        );
        state.time_pivot = crate::search::TimePivot::Modified;
        state.select_result(archived.clone(), true, now);
        assert_eq!(state.visible_selected_result_count(), 3);
        assert!(!state.selected_results.contains(&duplicate_location));
        state.hide_result(a.clone());
        assert!(!state.selected_results.contains(&a));
        assert!(state.result_selection_anchor.is_none());
        state.restore_hidden_results();
        assert_eq!(state.visible_selected_result_count(), 2);
        state.select_result(duplicate_location.clone(), false, now);
        assert_eq!(state.visible_selected_result_count(), 3);
        let (text, count) = state.selected_result_paths_clipboard(now).unwrap();
        assert_eq!(count, 3);
        assert_eq!(text.matches("/x/bundle.zip!/a.txt").count(), 2);
        state.select_result(b.clone(), false, now);
        assert!(!state.selected_results.contains(&b));
        state.clear_result_selection();
        assert!(state.selected_result_paths_clipboard(now).is_none());
        assert!(state.result_selection_anchor.is_none());
        state.select_result(
            crate::search::SearchLocation::Native(PathBuf::from("/missing")),
            true,
            now,
        );
        assert!(state.selected_results.is_empty());
    }

    #[test]
    fn copied_search_paths_follow_visible_order_preserve_members_and_escape_controls() {
        let now = SystemTime::now();
        let mut state = FindState::default();
        assert!(state.visible_result_paths_clipboard(now).is_none());
        state.results = vec![
            hit("one\nline\t.txt", None),
            hit("hidden.txt", None),
            hit("bundle.zip!/a.txt", None),
            hit("hard.txt", None),
        ];
        state.results[2].location = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/x/bundle.zip"),
            member: PathBuf::from("a.txt"),
            index: 0,
        };
        state
            .hidden_results
            .insert(state.results[1].location.clone());
        let (text, count) = state.visible_result_paths_clipboard(now).unwrap();
        assert_eq!(count, 3);
        assert_eq!(
            text,
            "/x/one\\nline\\t.txt\n/x/bundle.zip!/a.txt\n/x/hard.txt"
        );
        assert_eq!(text.lines().count(), count);
        assert_eq!(state.results.len(), 4);
        state.results.swap(0, 3);
        assert_eq!(
            state.visible_result_paths_clipboard(now).unwrap().0,
            "/x/hard.txt\n/x/bundle.zip!/a.txt\n/x/one\\nline\\t.txt"
        );
        state.time_pivot = crate::search::TimePivot::Modified;
        state.results[3].entry.modified = Some(now);
        state.results[0].entry.modified =
            now.checked_sub(std::time::Duration::from_secs(400 * 86400));
        assert_eq!(
            state.visible_result_paths_clipboard(now).unwrap().0,
            "/x/one\\nline\\t.txt\n/x/hard.txt\n/x/bundle.zip!/a.txt"
        );
        for hit in &state.results {
            state.hidden_results.insert(hit.location.clone());
        }
        assert!(state.visible_result_paths_clipboard(now).is_none());
        state.restore_hidden_results();
        assert_eq!(state.visible_result_paths_clipboard(now).unwrap().1, 4);
    }

    #[test]
    fn hidden_search_results_follow_locations_across_sorting_streaming_and_restore() {
        let now = SystemTime::now();
        let mut state = FindState {
            results: vec![hit("a", None), hit("b", Some(now)), hit("c", None)],
            matched: 3,
            highlighted_result: Some(1),
            highlight_scroll_pending: true,
            ..FindState::default()
        };
        let hidden = state.results[1].location.clone();
        state.explanation_open = Some(explain_key(&state.results[1]));
        state.hide_result(hidden.clone());
        assert_eq!(state.visible_result_order(now), [0, 2]);
        assert_eq!(state.visible_result_count(), 2);
        assert_eq!(state.results.len(), 3);
        assert_eq!(state.matched, 3);
        assert!(state.explanation_open.is_none());
        assert!(state.highlighted_result.is_none() && !state.highlight_scroll_pending);
        // Hard links share the helper's native identity; only the chosen path is hidden.
        assert_eq!(state.results[0].identity, state.results[1].identity);
        state.results.reverse();
        state.results.push(hit("new", Some(now)));
        for pivot in crate::search::TimePivot::ALL {
            state.time_pivot = pivot;
            let visible = state.visible_result_order(now);
            assert_eq!(visible.len(), 3);
            assert!(
                visible
                    .iter()
                    .all(|&index| state.results[index].location != hidden)
            );
            let selected = move_result_highlight(None, state.visible_result_count(), true).unwrap();
            assert_ne!(state.results[visible[selected]].location, hidden);
        }
        state.restore_hidden_results();
        assert_eq!(
            state.visible_result_order(now),
            pivot_order(&state.results, state.time_pivot, now)
        );
        assert_eq!(state.visible_result_count(), 4);
        assert!(state.hidden_results.is_empty());
        for location in state
            .results
            .iter()
            .map(|hit| hit.location.clone())
            .collect::<Vec<_>>()
        {
            state.hide_result(location);
        }
        assert_eq!(state.visible_result_count(), 0);
        assert!(state.visible_result_order(now).is_empty());
        state.restore_hidden_results();
        assert_eq!(state.visible_result_count(), 4);
    }

    #[test]
    fn hidden_search_results_distinguish_duplicate_archive_member_positions() {
        let mut first = hit("same", None);
        first.location = crate::search::SearchLocation::ArchiveMember {
            archive: PathBuf::from("/bundle.zip"),
            member: PathBuf::from("same"),
            index: 0,
        };
        let mut second = first.clone();
        if let crate::search::SearchLocation::ArchiveMember { index, .. } = &mut second.location {
            *index = 1;
        }
        let mut state = FindState {
            results: vec![first, second],
            ..FindState::default()
        };
        state.hide_result(state.results[0].location.clone());
        assert_eq!(state.visible_result_order(SystemTime::now()), [1]);
        state.restore_hidden_results();
        assert_eq!(state.visible_result_count(), 2);
    }

    #[test]
    fn hard_links_keep_separate_explanations() {
        // Both helper hits share one native identity, like two hard links.
        let results = [hit("a", None), hit("b", None)];
        assert_eq!(results[0].identity, results[1].identity);
        let key = explain_key(&results[1]);
        let shown = explained_hit(&results, &key).expect("hit is listed");
        assert_eq!(shown.entry.name, "b");
        assert_ne!(explain_key(&results[0]), key);
    }

    #[test]
    fn result_sort_preserves_locations_hidden_selection_and_copied_view_context() {
        let now = SystemTime::now();
        let mut state = json_state();
        state.results[0].entry.size = 5;
        state.results[1].entry.size = 1;
        state.results[2].entry.size = 9;
        let original = state
            .results
            .iter()
            .map(|h| h.location.clone())
            .collect::<Vec<_>>();
        state.highlighted_result = Some(0);
        state
            .selected_results
            .insert(state.results[1].location.clone());
        state.set_result_sort(
            crate::search_order::Sort {
                field: crate::search_order::Field::Size,
                descending: false,
            },
            now,
        );
        assert_eq!(state.visible_result_order(now), [1, 0, 2]);
        assert_eq!(state.highlighted_result, Some(1));
        state
            .hidden_results
            .insert(state.results[1].location.clone());
        // The currently highlighted source row is now visible at index zero.
        state.highlighted_result = Some(0);
        let selected = state.selected_results.clone();
        state.set_result_sort(
            crate::search_order::Sort {
                field: crate::search_order::Field::Size,
                descending: true,
            },
            now,
        );
        assert_eq!(state.visible_result_order(now), [2, 0]);
        assert_eq!(state.highlighted_result, Some(1));
        assert_eq!(state.selected_results, selected);
        let (text, count) = state.result_json(now, false).unwrap().unwrap();
        let r: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            r["context"]["result_sort"],
            serde_json::json!({"field":"size","descending":true})
        );
        assert_eq!(r["rows"][0]["display_name"], state.results[2].entry.name);
        assert!(state.result_json(now, true).unwrap().is_none());
        state.results[0].entry.created = Some(now - std::time::Duration::from_secs(400 * 86400));
        state.set_time_pivot(crate::search::TimePivot::Created, now);
        assert_eq!(state.visible_result_order(now), [0, 2]);
        assert_eq!(state.highlighted_result, Some(0));
        assert_eq!(
            state
                .results
                .iter()
                .map(|h| h.location.clone())
                .collect::<Vec<_>>(),
            original
        );
    }

    #[test]
    fn new_time_pivots_preserve_location_highlight_hidden_selection_and_report_order() {
        let now = SystemTime::now();
        let old = now - std::time::Duration::from_secs(400 * 86400);
        let mut state = json_state();
        state.results[0].entry.created = Some(old);
        state.results[0].entry.changed = Some(now);
        state.results[1].entry.created = Some(now);
        state.results[1].entry.changed = Some(old);
        state.results[2].entry.created = Some(now);
        state.results[2].entry.changed = Some(now);
        state.highlighted_result = Some(0);
        state
            .selected_results
            .insert(state.results[1].location.clone());
        state.set_time_pivot(crate::search::TimePivot::Created, now);
        assert_eq!(state.visible_result_order(now), [1, 0, 2]);
        assert_eq!(state.highlighted_result, Some(1));
        assert!(state.highlight_scroll_pending);
        state.set_time_pivot(crate::search::TimePivot::Changed, now);
        assert_eq!(state.visible_result_order(now), [0, 1, 2]);
        assert_eq!(state.highlighted_result, Some(0));
        let selected = state.selected_results.clone();
        state
            .hidden_results
            .insert(state.results[1].location.clone());
        state.set_time_pivot(crate::search::TimePivot::Created, now);
        assert_eq!(state.visible_result_order(now), [0, 2]);
        assert_eq!(state.selected_results, selected);
        assert_eq!(state.highlighted_result, Some(0));
        let (text, count) = state.result_json(now, false).unwrap().unwrap();
        let r: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(count, 2);
        assert_eq!(r["context"]["pivot"], "Created");
        assert_eq!(r["rows"][0]["display_name"], state.results[0].entry.name);
        state.set_time_pivot(crate::search::TimePivot::None, now);
        assert_eq!(state.highlighted_result, Some(0));
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
    fn result_highlight_starts_at_an_edge_and_stays_in_bounds() {
        assert_eq!(move_result_highlight(None, 0, true), None);
        assert_eq!(move_result_highlight(None, 3, true), Some(0));
        assert_eq!(move_result_highlight(None, 3, false), Some(2));
        assert_eq!(move_result_highlight(Some(2), 3, true), Some(2));
        assert_eq!(move_result_highlight(Some(0), 3, false), Some(0));
        assert_eq!(move_result_highlight(Some(1), 3, true), Some(2));
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
