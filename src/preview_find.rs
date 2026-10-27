//! Unicode literal and background regex search over displayed preview text.
use egui::{Color32, Galley, Id, Rect, Ui, text::CCursor};
use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_MATCHES: usize = 4096;
const MAX_QUERY_CHARS: usize = 256;
const MAX_REGEX_BYTES: usize = 256 * 1024;
type RegexResult = Result<(Vec<Range<usize>>, bool), String>;

struct RegexJob {
    receiver: std::sync::Mutex<std::sync::mpsc::Receiver<RegexResult>>,
    task: crate::workload::TaskHandle,
    finished: std::sync::atomic::AtomicBool,
}
impl Drop for RegexJob {
    fn drop(&mut self) {
        if !self.finished.load(std::sync::atomic::Ordering::Acquire) {
            self.task.cancel();
        }
    }
}

/// Unicode regex word boundaries; grouping keeps alternation inside both boundaries.
fn word_pattern(query: &str, whole_words: bool) -> String {
    if whole_words {
        format!(r"\b(?:{query})\b")
    } else {
        query.to_owned()
    }
}

#[cfg(test)]
fn regex_matches(
    text: &str,
    query: &str,
    case_sensitive: bool,
    cancelled: &dyn Fn() -> bool,
) -> RegexResult {
    regex_matches_options(text, query, case_sensitive, false, cancelled)
}

#[cfg(test)]
fn matches(text: &str, query: &str, case_sensitive: bool) -> (Vec<Range<usize>>, bool) {
    matches_options(text, query, case_sensitive, false)
}

fn regex_matches_options(
    text: &str,
    query: &str,
    case_sensitive: bool,
    whole_words: bool,
    cancelled: &dyn Fn() -> bool,
) -> RegexResult {
    if cancelled() {
        return Err("Search cancelled".into());
    }
    if text.len() > MAX_REGEX_BYTES {
        return Err("Regex search is limited to 256 KiB of displayed text.".into());
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err("Regex patterns are limited to 256 characters.".into());
    }
    if query.is_empty() {
        return Ok((Vec::new(), false));
    }
    let pattern = word_pattern(query, whole_words);
    let build = |pattern: &str| {
        regex::RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .size_limit(1024 * 1024)
            .dfa_size_limit(1024 * 1024)
            .nest_limit(32)
            .build()
    };
    let regex = build(&pattern)
        .or_else(|error| {
            // A trailing comment in (?x) mode can consume the generated closing
            // group. Only retry an originally valid expression; a newline ends
            // that comment and is itself ignored by free-spacing mode.
            if whole_words && build(query).is_ok() {
                build(&word_pattern(&format!("{query}\n"), true))
            } else {
                Err(error)
            }
        })
        .map_err(|error| error.to_string())?;
    let mut ranges = Vec::new();
    let mut previous_byte = 0;
    let mut previous_char = 0;
    if cancelled() {
        return Err("Search cancelled".into());
    }
    for found in regex.find_iter(text) {
        if cancelled() {
            return Err("Search cancelled".into());
        }
        if ranges.len() == MAX_MATCHES {
            return Ok((ranges, true));
        }
        let start = previous_char + text[previous_byte..found.start()].chars().count();
        let end = start + found.as_str().chars().count();
        ranges.push(start..end);
        previous_byte = found.end();
        previous_char = end;
    }
    if cancelled() {
        return Err("Search cancelled".into());
    }
    Ok((ranges, false))
}

#[derive(Clone, Default)]
pub struct PreviewFind {
    query: String,
    case_sensitive: bool,
    smart_case: bool,
    regex: bool,
    whole_words: bool,
    stop_at_edges: bool,
    error: Option<String>,
    pending: Option<Arc<RegexJob>>,
    ready_at: Option<std::time::Instant>,
    last_used: u64,
    matches: Vec<Range<usize>>,
    current: usize,
    limited: bool,
    galley: Option<Arc<Galley>>,
    path: PathBuf,
    reveal: bool,
}

fn matches_options(
    text: &str,
    query: &str,
    case_sensitive: bool,
    whole_words: bool,
) -> (Vec<Range<usize>>, bool) {
    if query.is_empty() {
        return (Vec::new(), false);
    }
    let Ok(pattern) = regex::RegexBuilder::new(&word_pattern(&regex::escape(query), whole_words))
        .case_insensitive(!case_sensitive)
        .build()
    else {
        return (Vec::new(), false);
    };
    let mut result = Vec::new();
    let mut previous_byte = 0;
    let mut previous_char = 0;
    for found in pattern.find_iter(text) {
        if result.len() == MAX_MATCHES {
            return (result, true);
        }
        let start = previous_char + text[previous_byte..found.start()].chars().count();
        let end = start + found.as_str().chars().count();
        result.push(start..end);
        previous_byte = found.end();
        previous_char = end;
    }
    (result, false)
}

impl PreviewFind {
    #[cfg(feature = "visual-qa")]
    pub fn regex_ready(&self) -> bool {
        self.regex && self.pending.is_none() && self.ready_at.is_none() && self.error.is_none()
    }

    #[cfg(any(test, feature = "visual-qa"))]
    pub fn current_match(&self) -> Option<usize> {
        (!self.matches.is_empty()).then_some(self.current)
    }
    #[cfg(any(test, feature = "visual-qa"))]
    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    #[cfg(any(test, feature = "visual-qa"))]
    pub fn with_query(query: &str) -> Self {
        Self {
            query: query.chars().take(MAX_QUERY_CHARS).collect(),
            ..Default::default()
        }
    }

    fn cancel_pending(&mut self) {
        if let Some(job) = self.pending.take() {
            job.task.cancel();
            job.finished
                .store(true, std::sync::atomic::Ordering::Release);
        }
    }
    fn clear_query(&mut self) {
        self.query.clear();
        self.reindex();
    }

    fn effective_case_sensitive(&self) -> bool {
        self.case_sensitive || (self.smart_case && self.query.chars().any(char::is_uppercase))
    }
    #[cfg(feature = "visual-qa")]
    pub fn smart_case_enabled(&self) -> bool {
        self.smart_case
    }

    fn reindex(&mut self) {
        self.cancel_pending();
        self.error = None;
        self.ready_at = None;
        self.matches.clear();
        self.limited = false;
        self.current = 0;
        self.reveal = false;
        if self.regex {
            if !self.query.is_empty() {
                self.ready_at =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(150));
            }
            return;
        }
        let text = self.galley.as_ref().map_or("", |galley| galley.text());
        (self.matches, self.limited) = matches_options(
            text,
            &self.query,
            self.effective_case_sensitive(),
            self.whole_words,
        );
        self.current = 0;
        self.reveal = !self.matches.is_empty();
    }

    fn start_regex(
        &mut self,
        ctx: &egui::Context,
        workload: &crate::workload::WorkloadHandle,
        now: std::time::Instant,
    ) {
        let Some(ready) = self.ready_at else {
            return;
        };
        if now < ready {
            ctx.request_repaint_after(ready - now);
            return;
        }
        self.ready_at = None;
        let text = self.galley.as_ref().map_or("", |galley| galley.text());
        if text.len() > MAX_REGEX_BYTES {
            self.error = Some("Regex search is limited to 256 KiB of displayed text.".into());
            return;
        }
        let text: Arc<str> = Arc::from(text);
        let query = self.query.clone();
        let case_sensitive = self.effective_case_sensitive();
        let whole_words = self.whole_words;
        let repaint = ctx.clone();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let spec = crate::workload::TaskSpec::new(
            crate::workload::TaskKind::Search,
            self.path.parent().unwrap_or(Path::new(".")),
            0,
        )
        .priority(crate::workload::Priority::Interactive)
        .estimated_bytes(text.len() as u64 + 2 * 1024 * 1024);
        match workload.submit(spec, move |cancelled| {
            let result = regex_matches_options(&text, &query, case_sensitive, whole_words, &|| {
                cancelled.is_cancelled()
            });
            let _ = sender.send(result);
            repaint.request_repaint();
        }) {
            Ok(task) => {
                self.pending = Some(Arc::new(RegexJob {
                    receiver: std::sync::Mutex::new(receiver),
                    task,
                    finished: std::sync::atomic::AtomicBool::new(false),
                }))
            }
            Err(error) => self.error = Some(format!("Could not start regex search: {error}")),
        }
    }
    fn poll_regex(&mut self) {
        let Some(job) = self.pending.as_ref() else {
            return;
        };
        let result = match crate::lock_util::recover(&job.receiver).try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err("Search stopped before returning results. Retry to search again.".into())
            }
        };
        job.finished
            .store(true, std::sync::atomic::Ordering::Release);
        self.pending = None;
        match result {
            Ok((ranges, limited)) => {
                self.matches = ranges;
                self.limited = limited;
                self.current = 0;
                self.reveal = !self.matches.is_empty();
                self.error = None;
            }
            Err(error) => {
                self.matches.clear();
                self.error = Some(error);
            }
        }
    }
    /// Called after both panes render: hidden/closed views must not retain jobs.
    pub fn retire_unused(ctx: &egui::Context) {
        for side in ["left", "right"] {
            for kind in ["text_preview", "pdf-page-text"] {
                let id = Id::new((kind, side)).with("find");
                if let Some(mut state) = ctx.data(|data| data.get_temp::<Self>(id))
                    && state.last_used != ctx.cumulative_frame_nr()
                    && (state.pending.is_some() || state.ready_at.is_some())
                {
                    state.cancel_pending();
                    state.ready_at = None;
                    state.error = Some(
                        "Search paused while this preview is hidden. Retry to search again.".into(),
                    );
                    ctx.data_mut(|data| data.insert_temp(id, state));
                }
            }
        }
    }

    fn update(&mut self, path: &Path, galley: &Arc<Galley>) {
        let same_allocation = self
            .galley
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, galley));
        let changed = self.path != path
            || (!same_allocation
                && self
                    .galley
                    .as_ref()
                    .is_none_or(|old| old.text() != galley.text()));
        self.path = path.to_path_buf();
        self.galley = Some(galley.clone());
        if changed {
            self.reindex();
        }
    }

    fn can_step(&self, forward: bool) -> bool {
        !self.matches.is_empty()
            && (!self.stop_at_edges
                || if forward {
                    self.current + 1 < self.matches.len()
                } else {
                    self.current > 0
                })
    }

    fn step(&mut self, forward: bool) {
        if !self.can_step(forward) {
            return;
        }
        self.current = if forward {
            (self.current + 1) % self.matches.len()
        } else {
            (self.current + self.matches.len() - 1) % self.matches.len()
        };
        self.reveal = true;
    }

    pub fn toolbar(ui: &mut Ui, id: Id, path: &Path, galley: &Arc<Galley>) -> Self {
        let mut state = ui
            .ctx()
            .data(|data| data.get_temp::<Self>(id).unwrap_or_default());
        state.update(path, galley);
        state.poll_regex();
        state.last_used = ui.ctx().cumulative_frame_nr();
        let field_id = id.with("query");
        let navigation = if ui.memory(|memory| memory.has_focus(field_id)) {
            ui.input_mut(|input| {
                if input.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter)
                    || input.consume_key(egui::Modifiers::SHIFT, egui::Key::F3)
                {
                    Some(false)
                } else if input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                    || input.consume_key(egui::Modifiers::NONE, egui::Key::F3)
                {
                    Some(true)
                } else {
                    None
                }
            })
        } else {
            None
        };
        if let Some(forward) = navigation {
            state.step(forward);
            ui.memory_mut(|memory| memory.request_focus(field_id));
        }
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut state.query)
                    .id(field_id)
                    .hint_text("Find in preview")
                    .char_limit(MAX_QUERY_CHARS)
                    .desired_width((ui.available_width() - 56.0).max(40.0)),
            );
            #[cfg(feature = "visual-qa")]
            ui.ctx()
                .data_mut(|data| data.insert_temp(id.with("query-control"), response.rect));
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::TextEdit,
                    response.enabled(),
                    "Find in preview",
                )
            });
            if response.changed() {
                state.reindex();
            }
            let clear = ui
                .add_enabled(!state.query.is_empty(), egui::Button::new("Clear"))
                .on_hover_text("Clear the preview query and matches; keep search options");
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx()
                .data_mut(|data| data.insert_temp(id.with("clear-query"), clear.rect));
            if clear.clicked() {
                state.clear_query();
                ui.memory_mut(|memory| memory.request_focus(field_id));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui
                .checkbox(&mut state.case_sensitive, "Match case")
                .changed()
            {
                state.reindex();
            }
            let smart = ui.checkbox(&mut state.smart_case, "Smart case")
                .on_hover_text("When Match case is off, uppercase characters anywhere in the query make it case-sensitive. Includes regex syntax; inline regex flags still apply.");
            #[cfg(feature = "visual-qa")]
            ui.ctx().data_mut(|data| data.insert_temp(id.with("smart-case-toggle"), smart.rect));
            if smart.changed() { state.reindex(); }
            let words = ui.checkbox(&mut state.whole_words, "Whole words")
                .on_hover_text("Unicode word boundaries: letters, numbers, marks and underscore belong to words. Applies to literal and regex search.");
            #[cfg(feature = "visual-qa")]
            ui.ctx().data_mut(|data| data.insert_temp(id.with("whole-words-toggle"), words.rect));
            if words.changed() {
                state.reindex();
            }
            let regex = ui
                .checkbox(&mut state.regex, "Regex")
                .on_hover_text("Rust regex syntax; searches only displayed text");
            #[cfg(feature = "visual-qa")]
            ui.ctx()
                .data_mut(|data| data.insert_temp(id.with("regex-toggle"), regex.rect));
            if regex.changed() {
                state.reindex();
            }
            let mut wrap = !state.stop_at_edges;
            let wrap_control = ui.checkbox(&mut wrap, "Wrap search")
                .on_hover_text("Continue at the opposite end of the collected matches. Disable to stop at the first or last result.");
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| data.insert_temp(id.with("wrap-toggle"), wrap_control.rect));
            #[cfg(not(any(test, feature = "visual-qa")))]
            let _ = wrap_control;
            // Navigation preference only: keep the selected match and in-flight search.
            state.stop_at_edges = !wrap;
            if state.pending.is_some() || state.ready_at.is_some() {
                if ui.button("Cancel search").clicked() {
                    state.cancel_pending();
                    state.ready_at = None;
                    state.error = Some("Search cancelled".into());
                }
            } else if state.regex && state.error.is_some() && ui.button("Retry").clicked() {
                state.reindex();
            }
            let previous = ui.add_enabled(state.can_step(false), egui::Button::new("Prev"));
            previous.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    previous.enabled(),
                    "Previous preview match",
                )
            });
            #[cfg(any(test, feature = "visual-qa"))]
            let previous_enabled = previous.enabled();
            if previous
                .on_hover_text("Previous match (Shift+Enter or Shift+F3 in search)")
                .clicked()
            {
                state.step(false);
            }
            let next = ui.add_enabled(state.can_step(true), egui::Button::new("Next"));
            next.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    next.enabled(),
                    "Next preview match",
                )
            });
            #[cfg(any(test, feature = "visual-qa"))]
            let next_enabled = next.enabled();
            if next.on_hover_text("Next match (Enter or F3 in search)").clicked() {
                state.step(true);
            }
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| data.insert_temp(id.with("navigation-enabled"), [previous_enabled, next_enabled]));
            let count = if state.pending.is_some() || state.ready_at.is_some() {
                "Searching…".to_string()
            } else if state.error.is_some() {
                "Search unavailable".to_string()
            } else if state.query.is_empty() {
                "Find text".to_string()
            } else if state.matches.is_empty() {
                "No matches".to_string()
            } else {
                format!(
                    "{} / {}{}",
                    state.current + 1,
                    state.matches.len(),
                    if state.limited { "+" } else { "" }
                )
            };
            let status = ui.label(&count);
            ui.ctx().accesskit_node_builder(status.id, |node| {
                node.set_role(egui::accesskit::Role::Status);
                node.set_live(egui::accesskit::Live::Polite);
                node.set_label(format!("Preview search: {count}"));
            });
        });
        if let Some(error) = &state.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        state.start_regex(
            ui.ctx(),
            &crate::workload::global_handle(),
            std::time::Instant::now(),
        );
        if state.pending.is_some() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        state
    }

    /// Underline the current match across wrapped rows without changing syntax colors.
    pub fn decorate(&mut self, ui: &Ui, origin: egui::Pos2, color: Color32) {
        let Some(range) = self.matches.get(self.current) else {
            return;
        };
        let Some(galley) = &self.galley else {
            return;
        };
        let start_cursor = CCursor {
            prefer_next_row: true,
            ..CCursor::new(range.start)
        };
        let start = galley.layout_from_cursor(start_cursor);
        let end = galley.layout_from_cursor(CCursor::new(range.end));
        let start_rect = galley.pos_from_cursor(start_cursor);
        if self.reveal {
            ui.scroll_to_rect_animation(
                start_rect.translate(origin.to_vec2()).expand(4.0),
                Some(egui::Align::Center),
                egui::style::ScrollAnimation::none(),
            );
            self.reveal = false;
        }
        for row_index in start.row..=end.row {
            let Some(row) = galley.rows.get(row_index) else {
                break;
            };
            let left = if row_index == start.row {
                start_rect.left()
            } else {
                row.rect().left()
            };
            let right = if row_index == end.row {
                galley.pos_from_cursor(CCursor::new(range.end)).left()
            } else {
                row.rect().right()
            };
            let rect = Rect::from_min_max(
                egui::pos2(left, row.rect().bottom() - 2.0),
                egui::pos2(right.max(left + 2.0), row.rect().bottom()),
            );
            if ui.clip_rect().intersects(rect.translate(origin.to_vec2())) {
                ui.painter()
                    .rect_filled(rect.translate(origin.to_vec2()), 0.0, color);
            }
        }
    }

    pub fn save(self, ui: &Ui, id: Id) {
        ui.ctx().data_mut(|data| data.insert_temp(id, self));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn whole_words_exclude_unicode_word_interiors_and_keep_character_offsets() {
        assert_eq!(
            matches_options(
                "cat catalog cat_name Cat 猫cat cat2 cat!",
                "cat",
                false,
                true
            )
            .0,
            [0..3, 21..24, 35..38]
        );
        assert_eq!(
            matches_options("ёж ёжик ЁЖ", "ёж", false, true).0,
            [0..2, 8..10]
        );
        assert!(
            matches_options("cafe\u{301}", "cafe", true, true)
                .0
                .is_empty()
        );
        assert_eq!(
            matches_options("[cat]", "cat", true, true).0,
            [Range { start: 1, end: 4 }]
        );
        assert!(matches_options("catalog", "cat", true, true).0.is_empty());
        assert_eq!(
            matches_options("catalog", "cat", true, false).0,
            [Range { start: 0, end: 3 }]
        );
    }

    #[test]
    fn whole_word_regex_groups_alternation_and_retains_limits() {
        let found = regex_matches_options(
            "catalog cat dog dogma",
            "cat|catalog|dog",
            true,
            true,
            &|| false,
        )
        .unwrap()
        .0;
        assert_eq!(found, [0..7, 8..11, 12..15]);
        assert_eq!(
            regex_matches_options(
                "item-12 xitem-34 ITEM-56",
                r"item-\d+",
                false,
                true,
                &|| false
            )
            .unwrap()
            .0,
            [0..7, 17..24]
        );
        assert!(regex_matches_options("abc", "[", true, true, &|| false).is_err());
        assert!(regex_matches_options("abc", "a", true, true, &|| true).is_err());
        assert!(
            regex_matches_options("abc", "", true, true, &|| false)
                .unwrap()
                .0
                .is_empty()
        );
    }

    #[test]
    fn whole_word_regex_keeps_trailing_free_spacing_comments_valid() {
        assert_eq!(
            regex_matches_options("cat catalog", "(?x)cat # comment", true, true, &|| false)
                .unwrap()
                .0,
            [Range { start: 0, end: 3 }]
        );
        assert!(regex_matches_options("cat", "(?x)[ # broken", true, true, &|| false).is_err());
    }

    #[test]
    fn whole_word_toggle_discards_pending_regex_results() {
        let ctx = egui::Context::default();
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let mut state = regex_state(&ctx, "item-12 xitem-34");
        state.start_regex(
            &ctx,
            &runtime.handle(),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        assert!(runtime.run_next());
        state.whole_words = true;
        state.reindex();
        state.poll_regex();
        assert!(state.matches.is_empty());
        state.start_regex(
            &ctx,
            &runtime.handle(),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        assert!(runtime.run_next());
        state.poll_regex();
        assert_eq!(state.matches, [Range { start: 0, end: 7 }]);
    }

    #[test]
    fn regex_unicode_anchors_capture_groups_and_zero_width_are_character_ranges() {
        let indexed =
            |text, query, sensitive| regex_matches(text, query, sensitive, &|| false).unwrap().0;
        assert_eq!(indexed("Ёж ёж", "ёж", false), [0..2, 3..5]);
        assert_eq!(indexed("Ёж ёж", "ёж", true), [Range { start: 3, end: 5 }]);
        assert_eq!(
            indexed("item-12\nITEM-34", r"item-(\d+)", false),
            [0..7, 8..15]
        );
        assert_eq!(indexed("ёж\né É", r"(?m)^", true), [0..0, 3..3]);
        assert_eq!(indexed("ёж\né É", r"$", true), [Range { start: 6, end: 6 }]);
        assert_eq!(
            indexed("a[x]", r"\[x\]", true),
            [Range { start: 1, end: 4 }]
        );
        assert!(indexed("abc", "", true).is_empty());
    }

    #[test]
    fn regex_errors_limits_and_cancellation_are_explicit() {
        for query in ["[", "(?<=a)b", "a{1000000000}"] {
            assert!(
                regex_matches("abc", query, true, &|| false).is_err(),
                "{query}"
            );
        }
        let nested = "(".repeat(40) + "a" + &")".repeat(40);
        assert!(regex_matches("a", &nested, true, &|| false).is_err());
        assert!(
            regex_matches(&"a".repeat(MAX_REGEX_BYTES + 1), "a", true, &|| false)
                .unwrap_err()
                .contains("256 KiB")
        );
        assert!(regex_matches("a", &"a".repeat(257), true, &|| false).is_err());
        assert!(
            regex_matches("a", "a", true, &|| true)
                .unwrap_err()
                .contains("cancelled")
        );
        let ticks = std::cell::Cell::new(0);
        assert!(
            regex_matches("a a a", "a", true, &|| {
                ticks.set(ticks.get() + 1);
                ticks.get() >= 4
            })
            .unwrap_err()
            .contains("cancelled")
        );
        let (ranges, limited) =
            regex_matches(&"я".repeat(MAX_MATCHES + 1), "(?:)", true, &|| false).unwrap();
        assert!(limited);
        assert_eq!(ranges.len(), MAX_MATCHES);
        assert_eq!(ranges.last(), Some(&(MAX_MATCHES - 1..MAX_MATCHES - 1)));
    }

    fn regex_state(ctx: &egui::Context, text: &str) -> PreviewFind {
        let mut state = PreviewFind {
            query: r"item-\d+".into(),
            regex: true,
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            let galley = ui.ctx().fonts_mut(|fonts| {
                fonts.layout_job(egui::text::LayoutJob::simple(
                    text.into(),
                    egui::FontId::monospace(12.0),
                    Color32::WHITE,
                    400.0,
                ))
            });
            state.update(Path::new("/regex.txt"), &galley);
        }));
        state
    }

    #[test]
    fn regex_jobs_debounce_discard_old_results_and_preserve_normal_completion_counts() {
        let ctx = egui::Context::default();
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let mut state = regex_state(&ctx, "item-12 ITEM-34");
        state.start_regex(
            &ctx,
            &runtime.handle(),
            state.ready_at.unwrap() - std::time::Duration::from_millis(1),
        );
        assert_eq!(runtime.stats().queued, 0);
        state.start_regex(
            &ctx,
            &runtime.handle(),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        assert!(runtime.run_next());
        state.query = "ITEM-34".into();
        state.reindex(); // an old result is waiting but must not be accepted
        state.poll_regex();
        assert!(state.matches.is_empty());
        state.start_regex(
            &ctx,
            &runtime.handle(),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        assert!(runtime.run_next());
        state.poll_regex();
        assert_eq!(state.matches, [Range { start: 8, end: 15 }]);
        assert!(state.pending.is_none() && state.error.is_none());
        assert_eq!(runtime.stats().cancelled, 0);
    }

    #[test]
    fn hidden_previews_cancel_jobs_and_admission_failure_is_visible() {
        let ctx = egui::Context::default();
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let mut state = regex_state(&ctx, "item-12");
        state.start_regex(
            &ctx,
            &runtime.handle(),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        state.last_used = u64::MAX;
        let id = Id::new(("text_preview", "right")).with("find");
        ctx.data_mut(|data| data.insert_temp(id, state));
        PreviewFind::retire_unused(&ctx);
        assert_eq!(runtime.stats().cancelled, 1);
        assert!(!runtime.run_next());
        let state = ctx.data(|data| data.get_temp::<PreviewFind>(id)).unwrap();
        assert!(state.pending.is_none() && state.error.as_deref().unwrap().contains("hidden"));
        let denied =
            crate::workload::DeterministicWorkload::new(crate::workload::SchedulerLimits {
                max_queued: 0,
                ..Default::default()
            });
        let mut state = regex_state(&ctx, "item-12");
        state.start_regex(
            &ctx,
            &denied.handle(),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        assert!(state.pending.is_none() && state.ready_at.is_none());
        assert!(state.error.as_deref().unwrap().contains("queue is full"));
    }

    #[test]
    fn retiring_one_pane_keeps_the_other_job_and_abandonment_can_be_retried() {
        let ctx = egui::Context::default();
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let mut left = regex_state(&ctx, "item-12");
        let mut right = regex_state(&ctx, "item-34");
        left.start_regex(
            &ctx,
            &runtime.handle(),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        right.start_regex(
            &ctx,
            &runtime.handle(),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        left.last_used = ctx.cumulative_frame_nr();
        right.last_used = u64::MAX;
        let left_id = Id::new(("text_preview", "left")).with("find");
        let right_id = Id::new(("pdf-page-text", "right")).with("find");
        ctx.data_mut(|data| {
            data.insert_temp(left_id, left);
            data.insert_temp(right_id, right);
        });
        PreviewFind::retire_unused(&ctx);
        assert_eq!(runtime.stats().cancelled, 1);
        assert_eq!(runtime.stats().queued, 1);
        assert!(runtime.run_next());
        let mut left = ctx
            .data(|data| data.get_temp::<PreviewFind>(left_id))
            .unwrap();
        left.poll_regex();
        assert_eq!(left.matches.len(), 1);
        left.reindex();
        left.start_regex(
            &ctx,
            &runtime.handle(),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        assert!(left.pending.as_ref().unwrap().task.cancel());
        left.poll_regex();
        assert!(left.pending.is_none());
        assert!(left.error.as_deref().unwrap().contains("stopped"));
        left.reindex();
        left.start_regex(
            &ctx,
            &runtime.handle(),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        assert!(runtime.run_next());
        left.poll_regex();
        assert!(left.error.is_none());
        assert_eq!(left.matches.len(), 1);
    }

    #[test]
    fn clear_query_cancels_regex_and_preserves_options() {
        let ctx = egui::Context::default();
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let mut state = regex_state(&ctx, "item-12 item-34");
        state.smart_case = true;
        state.whole_words = true;
        state.stop_at_edges = true;
        state.start_regex(
            &ctx,
            &runtime.handle(),
            std::time::Instant::now() + std::time::Duration::from_secs(1),
        );
        assert!(state.pending.is_some());
        state.error = Some("stale error".into());
        state.clear_query();
        assert!(state.query.is_empty() && state.matches.is_empty());
        assert!(state.pending.is_none() && state.ready_at.is_none() && state.error.is_none());
        assert!(state.regex && state.smart_case && state.whole_words && state.stop_at_edges);
        assert_eq!(state.current, 0);
        assert!(!state.reveal && !state.limited);
        runtime.run_next();
        state.poll_regex();
        assert!(state.matches.is_empty());
    }

    #[test]
    fn clear_button_removes_matches_and_returns_focus_to_query() {
        let ctx = egui::Context::default();
        let id = Id::new("clear-preview-find-test");
        ctx.data_mut(|data| data.insert_temp(id, PreviewFind::with_query("one")));
        for frame in 0..4 {
            let mut input = egui::RawInput::default();
            if frame == 1 || frame == 2 {
                let rect = ctx
                    .data(|data| data.get_temp::<Rect>(id.with("clear-query")))
                    .unwrap();
                input.events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: frame == 1,
                        modifiers: Default::default(),
                    },
                ];
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                let galley = ui.ctx().fonts_mut(|fonts| {
                    fonts.layout_job(egui::text::LayoutJob::simple(
                        "one one".into(),
                        egui::FontId::monospace(12.0),
                        Color32::WHITE,
                        400.0,
                    ))
                });
                let state = PreviewFind::toolbar(ui, id, Path::new("/a"), &galley);
                if frame >= 2 {
                    assert!(state.query.is_empty() && state.matches.is_empty());
                    assert!(!state.can_step(true) && !state.can_step(false));
                    assert!(ui.memory(|memory| memory.has_focus(id.with("query"))));
                }
                state.save(ui, id);
            }));
        }
    }

    #[test]
    fn zero_width_regex_matches_draw_a_marker_without_modifying_text() {
        let ctx = egui::Context::default();
        let mut state = PreviewFind::default();
        let output =
            crate::testutil::take_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
                let galley = ui.ctx().fonts_mut(|fonts| {
                    fonts.layout_job(egui::text::LayoutJob::simple(
                        "ёж".into(),
                        egui::FontId::monospace(12.0),
                        Color32::WHITE,
                        400.0,
                    ))
                });
                state.update(Path::new("/a"), &galley);
                state.matches = vec![Range { start: 2, end: 2 }];
                state.decorate(ui, egui::pos2(20.0, 20.0), Color32::RED);
                assert_eq!(galley.text(), "ёж");
            }));
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Rect(rect) if rect.fill == Color32::RED && rect.rect.width() >= 2.0)));
    }

    #[test]
    fn literal_unicode_search_uses_character_offsets() {
        assert_eq!(matches("ёж É é [x]", "é", false).0, [3..4, 5..6]);
        let sensitive = matches("ёж É é [x]", "é", true).0;
        assert_eq!(sensitive.len(), 1);
        assert_eq!(sensitive[0], 5..6);
        let literal = matches("ёж É é [x]", "[x]", false).0;
        assert_eq!(literal.len(), 1);
        assert_eq!(literal[0], 7..10);
        assert!(matches("anything", "", false).0.is_empty());
        assert!(matches("anything", "missing", false).0.is_empty());
        let non_overlapping = matches("aaa", "aa", true).0;
        assert_eq!(non_overlapping.len(), 1);
        assert_eq!(non_overlapping[0], 0..2);
    }
    #[test]
    fn match_budget_is_explicit_and_navigation_wraps() {
        let (found, limited) = matches(&"я".repeat(MAX_MATCHES + 1), "я", false);
        assert_eq!(found.len(), MAX_MATCHES);
        assert!(limited);
        let mut state = PreviewFind {
            matches: vec![0..1, 2..3],
            ..Default::default()
        };
        state.step(false);
        assert_eq!(state.current, 1);
        state.step(true);
        assert_eq!(state.current, 0);
        state.matches.clear();
        state.step(false);
    }
    #[test]
    fn finding_a_distant_match_scrolls_the_preview_into_view() {
        let ctx = egui::Context::default();
        let mut state = PreviewFind::with_query("needle");
        let text = format!("{}needle", "line\n".repeat(200));
        let mut greatest_offset = 0.0_f32;
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 400.0),
                )),
                ..Default::default()
            };
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                let galley = ui.ctx().fonts_mut(|fonts| {
                    fonts.layout_job(egui::text::LayoutJob::simple(
                        text.clone(),
                        egui::FontId::monospace(12.0),
                        Color32::WHITE,
                        400.0,
                    ))
                });
                state.update(Path::new("/long.txt"), &galley);
                let scroll = egui::ScrollArea::vertical()
                    .id_salt("find-scroll")
                    .max_height(100.0)
                    .show(ui, |ui| {
                        let response = ui.add(egui::Label::new(galley));
                        state.decorate(ui, response.rect.min, Color32::WHITE);
                    });
                greatest_offset = greatest_offset.max(scroll.state.offset.y);
            }));
        }
        assert!(
            greatest_offset > 100.0,
            "the distant match must become visible"
        );
        assert!(!state.reveal);
    }

    #[test]
    fn formatted_search_indexes_visible_text_and_skips_hidden_html() {
        let ctx = egui::Context::default();
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            let job = crate::document_preview::layout(
                "<script>hidden</script><p>Visible Ёж</p>",
                crate::document_preview::DocumentKind::Html,
                Color32::WHITE,
                300.0,
            );
            let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
            let mut state = PreviewFind::with_query("hidden");
            state.update(Path::new("/a.html"), &galley);
            assert!(state.matches.is_empty());
            state.query = "ёж".into();
            state.reindex();
            assert_eq!(state.matches.len(), 1);
        }));
    }

    #[test]
    fn smart_case_uses_unicode_uppercase_and_manual_match_case_overrides_it() {
        let ctx = egui::Context::default();
        crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
            let galley = ui.painter().layout_no_wrap(
                "foo Foo FOO ёж Ёж ЁЖ".into(),
                egui::FontId::monospace(12.0),
                Color32::WHITE,
            );
            for (query, smart_case, case_sensitive, expected) in [
                ("foo", false, false, 3),
                ("Foo", false, false, 3),
                ("foo", true, false, 3),
                ("Foo", true, false, 1),
                ("ёж", true, false, 3),
                ("Ёж", true, false, 1),
                ("foo", true, true, 1),
            ] {
                let mut state = PreviewFind {
                    query: query.into(),
                    smart_case,
                    case_sensitive,
                    ..Default::default()
                };
                state.update(Path::new("/a.txt"), &galley);
                assert_eq!(
                    state.matches.len(),
                    expected,
                    "{query}, smart={smart_case}, manual={case_sensitive}"
                );
            }
        }));
    }

    #[test]
    fn smart_case_changes_cancel_stale_regex_and_keep_inline_flags() {
        let ctx = egui::Context::default();
        let runtime = crate::workload::DeterministicWorkload::new(Default::default());
        let mut state = regex_state(&ctx, "foo Foo FOO foobar");
        state.query = "Foo".into();
        state.whole_words = true;
        state.reindex();
        let now = || std::time::Instant::now() + std::time::Duration::from_secs(1);
        state.start_regex(&ctx, &runtime.handle(), now());
        assert!(state.pending.is_some());
        state.smart_case = true;
        state.reindex();
        assert!(state.pending.is_none());
        assert!(!runtime.run_next());
        assert_eq!(runtime.stats().cancelled, 1);
        state.start_regex(&ctx, &runtime.handle(), now());
        assert!(runtime.run_next());
        state.poll_regex();
        assert_eq!(state.matches.len(), 1);
        assert_eq!(
            &state.galley.as_ref().unwrap().text()[state.matches[0].clone()],
            "Foo"
        );
        state.query = "(?i)Foo".into();
        state.reindex();
        state.start_regex(&ctx, &runtime.handle(), now());
        assert!(runtime.run_next());
        state.poll_regex();
        assert_eq!(state.matches.len(), 3);
    }

    #[test]
    fn wrap_toggle_keeps_current_result_and_disables_edges_for_buttons_and_f3() {
        let ctx = egui::Context::default();
        let id = Id::new("nowrap-find-test");
        ctx.data_mut(|data| data.insert_temp(id, PreviewFind::with_query("one")));
        for frame in 0..6 {
            let mut events = Vec::new();
            if frame == 1 || frame == 2 {
                let rect = ctx
                    .data(|data| data.get_temp::<Rect>(id.with("wrap-toggle")))
                    .unwrap();
                events = vec![
                    egui::Event::PointerMoved(rect.center()),
                    egui::Event::PointerButton {
                        pos: rect.center(),
                        button: egui::PointerButton::Primary,
                        pressed: frame == 1,
                        modifiers: Default::default(),
                    },
                ];
            } else if frame >= 3 {
                ctx.memory_mut(|memory| memory.request_focus(id.with("query")));
                let modifiers = egui::Modifiers {
                    shift: frame >= 4,
                    ..Default::default()
                };
                events = vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key: egui::Key::F3,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ];
            }
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 500.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let galley = ui.painter().layout_no_wrap(
                        "one two one".into(),
                        egui::FontId::monospace(12.0),
                        Color32::WHITE,
                    );
                    let mut state = PreviewFind::toolbar(ui, id, Path::new("/a.txt"), &galley);
                    if frame == 0 {
                        state.current = 1;
                    }
                    if frame >= 2 {
                        assert!(state.stop_at_edges);
                        assert_eq!(state.current, if frame < 4 { 1 } else { 0 });
                        assert_eq!(
                            ctx.data(
                                |data| data.get_temp::<[bool; 2]>(id.with("navigation-enabled"))
                            ),
                            Some(if frame < 4 {
                                [true, false]
                            } else {
                                [false, true]
                            })
                        );
                    }
                    assert_eq!(state.matches.len(), 2);
                    state.save(ui, id);
                },
            ));
        }
        let mut empty = PreviewFind {
            stop_at_edges: true,
            ..Default::default()
        };
        empty.step(true);
        empty.step(false);
        assert!(!empty.can_step(true) && !empty.can_step(false));
        empty.matches.push(0..1);
        assert!(!empty.can_step(true) && !empty.can_step(false));
    }

    #[test]
    fn f3_navigates_both_directions_wraps_and_does_not_consume_without_focus() {
        let ctx = egui::Context::default();
        let id = Id::new("f3-find-test");
        ctx.data_mut(|data| data.insert_temp(id, PreviewFind::with_query("one")));
        for (frame, focused, shift, expected) in [
            (0, false, false, 0),
            (1, true, false, 1),
            (2, true, false, 0),
            (3, true, true, 1),
            (4, true, true, 0),
            (5, false, false, 0),
        ] {
            if focused {
                ctx.memory_mut(|memory| memory.request_focus(id.with("query")));
            } else {
                ctx.memory_mut(|memory| memory.surrender_focus(id.with("query")));
            }
            let modifiers = egui::Modifiers {
                shift,
                ..Default::default()
            };
            let events = if frame == 0 {
                vec![]
            } else {
                vec![
                    egui::Event::ModifiersChanged(modifiers),
                    egui::Event::Key {
                        key: egui::Key::F3,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    },
                ]
            };
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    let galley = ui.painter().layout_no_wrap(
                        "one two one".into(),
                        egui::FontId::monospace(12.0),
                        Color32::WHITE,
                    );
                    let state = PreviewFind::toolbar(ui, id, Path::new("/a.txt"), &galley);
                    assert_eq!(state.current_match(), Some(expected));
                    if frame == 5 {
                        assert!(ui.input(|input| input.key_pressed(egui::Key::F3)));
                    }
                    assert_eq!(galley.text(), "one two one");
                    state.save(ui, id);
                },
            ));
        }
    }

    #[test]
    fn enter_and_shift_enter_navigate_only_the_focused_search_field() {
        let ctx = egui::Context::default();
        let id = Id::new("find-test");
        ctx.data_mut(|data| data.insert_temp(id, PreviewFind::with_query("one")));
        for (step, shift) in [false, false, true].into_iter().enumerate() {
            let modifiers = egui::Modifiers {
                shift,
                ..Default::default()
            };
            let mut input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 400.0),
                )),
                ..Default::default()
            };
            if step > 0 {
                ctx.memory_mut(|memory| memory.request_focus(id.with("query")));
                input.events.push(egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                });
            }
            crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
                let galley = ui.ctx().fonts_mut(|fonts| {
                    fonts.layout_job(egui::text::LayoutJob::simple(
                        "one two one".into(),
                        egui::FontId::monospace(12.0),
                        Color32::WHITE,
                        400.0,
                    ))
                });
                let state = PreviewFind::toolbar(ui, id, Path::new("/a"), &galley);
                assert_eq!(state.current, usize::from(step == 1));
                state.save(ui, id);
            }));
        }
    }

    #[test]
    fn query_edits_and_content_changes_reset_results_but_reflow_keeps_current() {
        let ctx = egui::Context::default();
        let mut state = PreviewFind {
            query: "one".into(),
            ..Default::default()
        };
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            let layout = |text: &str, width| {
                ui.ctx().fonts_mut(|fonts| {
                    fonts.layout_job(egui::text::LayoutJob::simple(
                        text.into(),
                        egui::FontId::monospace(12.0),
                        Color32::WHITE,
                        width,
                    ))
                })
            };
            let original = layout("one two one", 400.0);
            state.update(Path::new("/a"), &original);
            state.step(true);
            assert_eq!(state.current, 1);
            state.update(Path::new("/a"), &layout("one two one", 40.0));
            assert_eq!(state.current, 1);
            state.update(Path::new("/a"), &layout("nothing", 40.0));
            assert!(state.matches.is_empty());
            state.update(Path::new("/b"), &original);
            assert_eq!(state.current, 0);
            assert_eq!(state.matches.len(), 2);
        }));
    }
}
