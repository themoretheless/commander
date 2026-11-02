//! Read-only unified diff sheet. The diff is computed by `crate::textdiff`;
//! this file only reads the files and renders the result.

use super::*;
use crate::textdiff::{DiffKind, change_counts};
use std::path::Path;

/// Largest file we will read into memory for a diff.
const DIFF_SIZE_CAP: u64 = 2 * 1024 * 1024;

/// Why a file could not be read for a diff.
#[derive(Debug, PartialEq, Eq)]
enum ReadError {
    /// A folder, FIFO, device or other non-regular file.
    NotFile,
    TooLarge,
    /// Not valid UTF-8, most likely a binary file.
    NotText,
    /// The OS refused the read (permission denied, gone, ...).
    Io(String),
}

/// Read a regular file as UTF-8, never more than `DIFF_SIZE_CAP` bytes.
/// FIFOs and devices are rejected (they would block or stream forever on the
/// UI thread), and the read itself is bounded in case the file grows.
fn read_text(path: &Path) -> Result<String, ReadError> {
    use std::io::Read;
    let io = |error: std::io::Error| ReadError::Io(error.to_string());
    let meta = std::fs::metadata(path).map_err(io)?;
    if !meta.is_file() {
        return Err(ReadError::NotFile);
    }
    if meta.len() > DIFF_SIZE_CAP {
        return Err(ReadError::TooLarge);
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(io)?
        .take(DIFF_SIZE_CAP + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() as u64 > DIFF_SIZE_CAP {
        return Err(ReadError::TooLarge);
    }
    String::from_utf8(bytes).map_err(|_| ReadError::NotText)
}

/// The sheet's message for a file that could not be read: names the file and
/// the reason, and the next step where there is one.
fn read_error_message(path: &Path, error: &ReadError) -> String {
    let name = path.display();
    match error {
        ReadError::NotFile => format!("{name} is not a regular file, so it cannot be diffed."),
        ReadError::TooLarge => format!(
            "{name} is larger than {}, too large to diff.",
            crate::panel::format_size(DIFF_SIZE_CAP)
        ),
        ReadError::NotText => {
            format!("{name} is not UTF-8 text (binary?), so it cannot be diffed.")
        }
        ReadError::Io(error) => {
            format!("Could not read {name}: {error}. Check that it exists and is readable.")
        }
    }
}

/// Header labels for the two sides. Same-named files (the cross-pane pair)
/// carry their folder too, so the user can tell which side is `-` and `+`.
fn side_labels(a: &Path, b: &Path) -> (String, String) {
    let name = |path: &Path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default()
    };
    let (name_a, name_b) = (name(a), name(b));
    if !name_a.eq_ignore_ascii_case(&name_b) {
        return (name_a, name_b);
    }
    let parent = |path: &Path| path.parent().map(Path::to_path_buf).unwrap_or_default();
    let (parent_a, parent_b) = (parent(a), parent(b));
    // The folder name alone, unless both folders share it too.
    let folder = |path: &Path| match path.file_name() {
        Some(folder) if parent_a.file_name() != parent_b.file_name() => {
            folder.to_string_lossy().to_string()
        }
        _ => path.display().to_string(),
    };
    (
        format!("{name_a} ({})", folder(&parent_a)),
        format!("{name_b} ({})", folder(&parent_b)),
    )
}

/// Both sides hold the same text. `diff_lines` emits `Equal` rows, so an
/// identical non-empty pair has lines but no insertions or deletions.
fn files_identical(lines: &[crate::textdiff::DiffLine]) -> bool {
    change_counts(lines) == (0, 0)
}

/// One unwrapped monospace row per line, so only the visible rows are laid
/// out: an empty-vs-large diff can hold about two million lines. Returns how
/// many lines were laid out.
struct DiffRender {
    laid_out: usize,
    offset: egui::Vec2,
    visible: Vec<usize>,
    expanded: Option<std::ops::Range<usize>>,
    control_ids: Vec<egui::Id>,
}
#[cfg(test)]
fn render_diff_lines(
    ui: &mut egui::Ui,
    lines: &[crate::textdiff::DiffLine],
    t: &ThemeColors,
    offset: egui::Vec2,
    active: Option<std::ops::Range<usize>>,
    reveal: Option<usize>,
) -> DiffRender {
    let mut view = crate::diff_context::Context::default();
    view.rebuild(lines.len(), &[]);
    render_diff_view(ui, lines, t, offset, active, reveal, (&view, None, None))
}
fn render_diff_view(
    ui: &mut egui::Ui,
    lines: &[crate::textdiff::DiffLine],
    t: &ThemeColors,
    mut scroll_offset: egui::Vec2,
    active: Option<std::ops::Range<usize>>,
    reveal: Option<usize>,
    presentation: (
        &crate::diff_context::Context,
        Option<&crate::diff_navigation::Navigation>,
        Option<&crate::diff_intraline::Intraline>,
    ),
) -> DiffRender {
    let (view, numbers, intraline) = presentation;
    let font = egui::FontId::monospace(12.0);
    let row_h = ui.fonts_mut(|fonts| fonts.row_height(&font));
    if let Some(row) = reveal.and_then(|row| view.display_index(row)) {
        scroll_offset.y = row as f32 * row_h;
    }
    let mut result = DiffRender {
        laid_out: 0,
        offset: scroll_offset,
        visible: Vec::new(),
        expanded: None,
        control_ids: Vec::new(),
    };
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.spacing_mut().interact_size.y = row_h;
        let output = egui::ScrollArea::both()
            .id_salt("diff-lines")
            .max_height((ui.available_height() - 100.0).clamp(row_h, 420.0))
            .auto_shrink([false, false])
            .scroll_offset(scroll_offset)
            .show_rows(ui, row_h, view.row_count(), |ui, range| {
                result.laid_out = range.len();
                for index in range {
                    match view.row(index) {
                        Some(crate::diff_context::Row::Line(original)) => {
                            let line = &lines[original];
                            result.visible.push(original);
                            let (mark, color) = match line.kind {
                                DiffKind::Equal => (" ", t.text_muted),
                                DiffKind::Insert => ("+", t.accent),
                                DiffKind::Delete => ("-", t.accent_red),
                            };
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                if let Some(nav) = numbers {
                                    let digits = lines.len().max(1).ilog10() as usize + 1;
                                    let (minus, plus) = nav.source_numbers[original];
                                    let number = |n: Option<usize>| {
                                        n.map(|n| n.to_string()).unwrap_or_default()
                                    };
                                    let gutter = format!(
                                        "{:>digits$} {:>digits$} |",
                                        number(minus),
                                        number(plus)
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(gutter)
                                                .font(font.clone())
                                                .color(t.text_muted),
                                        )
                                        .selectable(false),
                                    );
                                }
                                let background =
                                    if active.as_ref().is_some_and(|r| r.contains(&original)) {
                                        t.bg_hover
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    };
                                if let Some(intraline) =
                                    intraline.filter(|_| line.kind != DiffKind::Equal)
                                {
                                    let whole = 0..line.text.len();
                                    let spans = intraline
                                        .rows
                                        .get(&original)
                                        .map_or(std::slice::from_ref(&whole), Vec::as_slice);
                                    let job = intraline_job(
                                        mark,
                                        &line.text,
                                        spans,
                                        font.clone(),
                                        color,
                                        background,
                                    );
                                    ui.add(egui::Label::new(job).extend());
                                } else {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!("{mark} {}", line.text))
                                                .background_color(background)
                                                .font(font.clone())
                                                .color(color),
                                        )
                                        .extend(),
                                    );
                                }
                            });
                        }
                        Some(crate::diff_context::Row::Gap(hidden)) => {
                            let label = format!("{} unchanged lines hidden - Show", hidden.len());
                            let (rect, response) = ui.allocate_exact_size(
                                egui::vec2(
                                    ui.available_width().min(ui.clip_rect().width()).max(1.0),
                                    row_h,
                                ),
                                egui::Sense::click(),
                            );
                            response.widget_info(|| {
                                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label)
                            });
                            ui.painter().text(
                                rect.left_center(),
                                egui::Align2::LEFT_CENTER,
                                &label,
                                egui::FontId::monospace(10.0),
                                t.accent,
                            );
                            result.control_ids.push(response.id);
                            #[cfg(any(test, feature = "visual-qa"))]
                            ui.ctx().data_mut(|data| {
                                data.insert_temp(
                                    egui::Id::new(("diff-gap", hidden.start, hidden.end)),
                                    rect,
                                )
                            });
                            if response.clicked() {
                                result.expanded = Some(hidden);
                            }
                        }
                        None => {}
                    }
                }
            });
        ui.ctx().data_mut(|data| {
            data.insert_temp(
                egui::Id::new("diff-viewport-height"),
                output.inner_rect.height(),
            )
        });
        result.offset = output.state.offset;
    });
    result
}

fn intraline_job(
    mark: &str,
    text: &str,
    spans: &[std::ops::Range<usize>],
    font: egui::FontId,
    color: egui::Color32,
    background: egui::Color32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let format = egui::TextFormat {
        font_id: font,
        color,
        background,
        ..Default::default()
    };
    job.append(&format!("{mark} "), 0.0, format.clone());
    let mut offset = 0;
    for span in spans {
        job.append(&text[offset..span.start], 0.0, format.clone());
        job.append(
            &text[span.clone()],
            0.0,
            egui::TextFormat {
                background: color.gamma_multiply(0.35),
                underline: egui::Stroke::new(1.0, color),
                ..format.clone()
            },
        );
        offset = span.end;
    }
    job.append(&text[offset..], 0.0, format);
    job
}

fn load_sources(a: &Path, b: &Path) -> Result<(String, String), String> {
    match (read_text(a), read_text(b)) {
        (Ok(ta), Ok(tb)) => Ok((ta, tb)),
        (Err(error), _) => Err(read_error_message(a, &error)),
        (_, Err(error)) => Err(read_error_message(b, &error)),
    }
}

/// Re-read the retained operands, independently of current pane selection.
pub(super) fn reload_comparison(state: &mut DiffState) {
    match load_sources(&state.paths.0, &state.paths.1) {
        Ok(source) => {
            state.source = Some(source);
            state.message = None;
        }
        Err(message) => {
            state.source = None;
            state.message = Some(message);
        }
    }
    recompare(state);
}

/// Reverse the retained comparison without touching the files or panel state.
pub(super) fn swap_sides(state: &mut DiffState) {
    std::mem::swap(&mut state.paths.0, &mut state.paths.1);
    std::mem::swap(&mut state.name_a, &mut state.name_b);
    if let Some((a, b)) = &mut state.source {
        std::mem::swap(a, b);
    }
    recompare(state);
    state.keyboard_focused = true;
}

/// Rebuild against the retained bounded snapshot, with no filesystem I/O.
fn recompare(state: &mut DiffState) {
    let lines = match &state.source {
        Some((a, b)) => match if state.options == crate::textdiff::Options::default() {
            crate::textdiff::diff_lines(a, b)
        } else {
            crate::textdiff::diff_lines_with_options(a, b, state.options)
        } {
            Ok(lines) => {
                state.message = None;
                lines
            }
            Err(_) => {
                state.message = Some(
                    "These files contain too many line combinations to compare safely.".into(),
                );
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    let numbers = state.navigation.show_numbers;
    state.navigation = crate::diff_navigation::Navigation::new(&lines);
    state.navigation.show_numbers = numbers;
    let (compact, context) = (state.context_view.compact, state.context_view.context);
    state.context_view = Default::default();
    state.context_view.compact = compact;
    state.context_view.context = context;
    state
        .context_view
        .rebuild(lines.len(), &state.navigation.hunks);
    let (query, case, regex, words) = (
        state.find.query.clone(),
        state.find.case,
        state.find.regex,
        state.find.words,
    );
    state.find = Default::default();
    state.find.query = query;
    state.find.case = case;
    state.find.regex = regex;
    state.find.words = words;
    state.find.refresh(&lines);
    state.find.focused = false;
    state.jump = Default::default();
    state.scroll_offset = egui::Vec2::ZERO;
    state.intraline.rebuild(&lines);
    state.lines = lines;
}

impl App {
    pub(crate) fn open_diff(&mut self, ctx: &egui::Context) {
        match self.ws.diff_targets() {
            None => {
                self.toast(
                    ctx,
                    crate::toasts::ToastKind::Info,
                    "Select two files, or put the cursor on a file with the same name in both panes",
                    false,
                );
            }
            Some((a, b)) => {
                self.ui.tools.replace_on(self.ws.active.other());
                let (name_a, name_b) = side_labels(&a, &b);
                let mut state = DiffState {
                    intraline: Default::default(),
                    source: None,
                    options: Default::default(),
                    paths: (a, b),
                    jump: Default::default(),
                    find: Default::default(),
                    context_view: Default::default(),
                    navigation: Default::default(),
                    name_a,
                    name_b,
                    lines: Vec::new(),
                    message: None,
                    keyboard_focused: false,
                    scroll_offset: egui::Vec2::ZERO,
                };
                reload_comparison(&mut state);
                self.ui.tools.diff = Some(state);
            }
        }
    }

    pub(crate) fn show_diff_tool(&mut self, ui: &mut egui::Ui) {
        let escape_requested =
            self.take_escape_request(crate::accessibility::EscapeRoute::PaneTool);
        let Some(state) = &mut self.ui.tools.diff else {
            return;
        };
        let t = self.colors;
        let mut close = false;

        ui.vertical(|ui| {
            let mut navigation_ids=Vec::new();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("- {}  vs  + {}", state.name_a, state.name_b))
                        .size(13.0)
                        .strong()
                        .color(t.text_primary),
                );
                if state.message.is_none() {
                    let (ins, del) = change_counts(&state.lines);
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(format!("+{ins}"))
                            .size(11.0)
                            .color(t.accent),
                    );
                    ui.label(
                        egui::RichText::new(format!("-{del}"))
                            .size(11.0)
                            .color(t.accent_red),
                    );
                }
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                let response=ui.button("Reload compared files").on_hover_text("Cmd/Ctrl+R · reread the same two paths");
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("diff-reload"),response.rect));
                navigation_ids.push(response.id);
                if response.clicked() {reload_comparison(state);}
                let response=ui.button("Swap compared sides").on_hover_text("Ctrl+U while comparison keyboard navigation is active · reverse - / + using the loaded text; Reload follows the reversed paths");
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("diff-swap"),response.rect));
                navigation_ids.push(response.id);
                if response.clicked() {swap_sides(state);}
            });
            ui.horizontal_wrapped(|ui| {
                let spaces=ui.add_enabled(state.source.is_some() && !state.options.all_spaces,egui::Checkbox::new(&mut state.options.space_changes,"Ignore space changes"))
                    .on_hover_text("Compare each run of ASCII spaces/tabs as one space; keep absent spaces and line endings distinct");
                let all=ui.add_enabled(state.source.is_some(),egui::Checkbox::new(&mut state.options.all_spaces,"Ignore all spaces"))
                    .on_hover_text("Ignore ASCII spaces and tabs, including their absence; preserve line boundaries, endings and original display text. Overrides Ignore space changes while enabled.");
                let tabs=ui.add_enabled(state.source.is_some() && !state.options.all_spaces && !state.options.space_changes,egui::Checkbox::new(&mut state.options.expand_tabs,"Expand tabs for comparison"))
                    .on_hover_text("Compare tabs as spaces up to the next 8-character tab stop. Columns count Unicode characters, not screen width. Other spacing remains significant; space-ignore modes override this setting.");
                let endings=ui.add_enabled(state.source.is_some(),egui::Checkbox::new(&mut state.options.ignore_crlf,"Ignore CRLF/LF differences"))
                    .on_hover_text("Compare CRLF and LF line endings as equal; keep missing final newlines and CR characters inside text distinct. Does not modify files.");
                let case=ui.add_enabled(state.source.is_some(),egui::Checkbox::new(&mut state.options.ignore_case,"Ignore case"))
                    .on_hover_text("Compare Unicode lowercase text; keep original text for display");
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|d| {d.insert_temp(egui::Id::new("diff-expand-tabs"),tabs.rect);d.insert_temp(egui::Id::new("diff-ignore-crlf"),endings.rect);d.insert_temp(egui::Id::new("diff-ignore-all-spaces"),all.rect);d.insert_temp(egui::Id::new("diff-ignore-spaces"),spaces.rect);d.insert_temp(egui::Id::new("diff-ignore-case"),case.rect);});
                #[cfg(feature="visual-qa")]
                if ui.input(|i|i.pointer.any_pressed() || i.pointer.any_released()) {
                    log::info!("Tab checkbox input: pressed={} released={} clicked={} changed={} enabled={} value={}",ui.input(|i|i.pointer.any_pressed()),ui.input(|i|i.pointer.any_released()),tabs.clicked(),tabs.changed(),tabs.enabled(),state.options.expand_tabs);
                }
                navigation_ids.extend([spaces.id,all.id,tabs.id,endings.id,case.id]);
                if spaces.changed() || all.changed() || tabs.changed() || endings.changed() || case.changed() { recompare(state); state.keyboard_focused=true; }
            let response=ui.checkbox(&mut state.intraline.enabled,"Highlight changes within lines")
                .on_hover_text("Original character differences within ordinally paired changed lines; unpaired lines are highlighted in full");
            navigation_ids.push(response.id);
            #[cfg(any(test,feature="visual-qa"))]
            ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("diff-intraline"),response.rect));
            if response.changed() {state.intraline.rebuild(&state.lines);state.keyboard_focused=true;}
            });
            ui.horizontal_wrapped(|ui| {
                let trailing=ui.add_enabled(state.source.is_some() && !state.options.all_spaces,egui::Checkbox::new(&mut state.options.trailing_spaces,"Ignore trailing spaces"))
                    .on_hover_text("Ignore ASCII spaces/tabs only at line ends. Leading/interior spaces, line endings and original display text remain significant. Combines with other comparison options.");
                navigation_ids.push(trailing.id);
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new("diff-ignore-trailing-spaces"),trailing.rect));
                if trailing.changed() { recompare(state); state.keyboard_focused=true; }
            });
            if state.intraline.enabled && state.intraline.simplified_pairs>0 {
                ui.small(format!("{} line pairs use a simplified changed span (highlight limit)",state.intraline.simplified_pairs));
            }
            if state.options.trailing_spaces || state.options.space_changes || state.options.all_spaces || state.options.expand_tabs || state.options.ignore_crlf || state.options.ignore_case {
                ui.small("Comparison options active · shared rows show the - file's original text");
            }

            if let Some(msg) = &state.message {
                ui.label(egui::RichText::new(msg).size(12.0).color(t.text_muted));
            } else if files_identical(&state.lines) {
                ui.label(
                    egui::RichText::new(if state.options == crate::textdiff::Options::default() { "The files are identical." } else { "No differences under the selected comparison options." })
                        .size(12.0)
                        .color(t.text_muted),
                );
            } else {
                ui.horizontal_wrapped(|ui| {
                    for step in crate::diff_navigation::Step::ALL {
                        let response=ui.add_enabled(state.navigation.enabled(step),egui::Button::new(step.label()));
                        navigation_ids.push(response.id);
                        #[cfg(any(test,feature="visual-qa"))]
                        ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new(("diff-hunk",step.key())),response.rect));
                        if response.clicked() {state.navigation.go(step);state.find.focused=false;state.jump.selected=None;state.keyboard_focused=true;}
                    }
                });
                let (changed,ids)=state.context_view.toolbar(ui,state.lines.len(),&state.navigation.hunks);navigation_ids.extend(ids);
                if changed {state.navigation.reveal_current();state.find.focused=false;state.jump.selected=None;}
                let response=ui.checkbox(&mut state.navigation.show_numbers,"Source line numbers (- / +)");
                navigation_ids.push(response.id);
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new("diff-line-numbers"),response.rect));
                ui.small(state.navigation.position_label());
                let (found,ids)=state.find.toolbar(ui,&state.lines);navigation_ids.extend(ids);
                if let Some(row)=found {state.context_view.reveal(row,&state.navigation.hunks);state.jump.selected=None;}
                let (jumped,ids)=state.jump.toolbar(ui,&state.navigation.source_numbers);navigation_ids.extend(ids);
                if let Some(row)=jumped {state.context_view.reveal(row,&state.navigation.hunks);state.find.focused=false;}
                let active=state.navigation.selected().map(|h|h.rows.clone());let reveal=state.navigation.take_reveal();
                let active=if state.find.focused {state.find.selected().map(|row|row..row+1)}else{active};
                let active=state.jump.selected.map(|row|row..row+1).or(active);
                let reveal=jumped.or(found).or(reveal);
                let rendered=render_diff_view(ui,&state.lines,&t,state.scroll_offset,active,reveal,(&state.context_view,state.navigation.show_numbers.then_some(&state.navigation),state.intraline.enabled.then_some(&state.intraline)));
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new("diff-visible-rows"),rendered.visible.clone()));
                let _=(rendered.laid_out,&rendered.visible);
                navigation_ids.extend(rendered.control_ids);
                state.scroll_offset=rendered.offset;
                if let Some(range)=rendered.expanded {state.context_view.expand(range,&state.navigation.hunks);state.keyboard_focused=true;ui.ctx().request_repaint();}

            }

            ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new("diff-navigation-widget-ids"),navigation_ids));
            if state.keyboard_focused {
                ui.label(
                    egui::RichText::new("Keyboard active · Ctrl+U swap · Cmd/Ctrl+F or F7 find · F3 / Shift+F3 matches · Alt+Up/Down differences · Alt+Home/End first/last · Tab or Esc returns to files")
                        .size(11.0)
                        .color(t.accent),
                );
            }

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add(super::dialog_button(
                        t,
                        super::ButtonRole::Secondary,
                        egui::RichText::new("Close").size(13.0),
                    ))
                    .clicked()
                    || escape_requested
                {
                    close = true;
                }
            });
        });

        if close {
            self.ui.tools.diff = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ReadError, files_identical, read_error_message, read_text, render_diff_lines, side_labels,
    };
    use std::path::Path;

    fn retained_pair(a: std::path::PathBuf, b: std::path::PathBuf) -> super::DiffState {
        super::DiffState {
            intraline: Default::default(),
            source: None,
            options: Default::default(),
            paths: (a, b),
            jump: Default::default(),
            find: Default::default(),
            context_view: Default::default(),
            navigation: Default::default(),
            name_a: "a".into(),
            name_b: "b".into(),
            lines: vec![],
            message: None,
            keyboard_focused: true,
            scroll_offset: egui::Vec2::ZERO,
        }
    }
    #[test]
    fn expand_tabs_recomputes_retained_text_and_survives_reload_swap() {
        let dir = crate::testutil::TempDir::new();
        let a = dir.file("a", "x\tz\n");
        let b = dir.file("b", "x       z\n");
        let mut state = retained_pair(a.clone(), b.clone());
        super::reload_comparison(&mut state);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (1, 1));
        state.options.expand_tabs = true;
        super::recompare(&mut state);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (0, 0));
        assert_eq!(state.lines[0].text, "x\tz");
        super::swap_sides(&mut state);
        super::reload_comparison(&mut state);
        assert!(state.options.expand_tabs);
        assert_eq!(state.lines[0].text, "x       z");
        std::fs::remove_file(a).unwrap();
        std::fs::remove_file(b).unwrap();
        state.options.expand_tabs = false;
        super::recompare(&mut state);
        assert!(state.message.is_none());
        assert_eq!(crate::textdiff::change_counts(&state.lines), (1, 1));
    }

    #[test]
    fn ignore_crlf_rebuilds_retained_rows_coordinates_and_survives_reload_swap() {
        let dir = crate::testutil::TempDir::new();
        let a = dir.file("a", "head\r\nold\r\nend\r\n");
        let b = dir.file("b", "head\nnew\nend\n");
        let mut state = retained_pair(a.clone(), b.clone());
        super::reload_comparison(&mut state);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (3, 3));
        state.options.ignore_crlf = true;
        state.intraline.enabled = true;
        state.navigation.show_numbers = true;
        super::recompare(&mut state);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (1, 1));
        assert_eq!(state.navigation.hunks.len(), 1);
        assert_eq!(state.navigation.hunks[0].left_start, 2);
        assert_eq!(state.navigation.hunks[0].right_start, 2);
        super::swap_sides(&mut state);
        super::reload_comparison(&mut state);
        assert!(
            state.options.ignore_crlf && state.navigation.show_numbers && state.intraline.enabled
        );
        assert_eq!(crate::textdiff::change_counts(&state.lines), (1, 1));
        std::fs::remove_file(a).unwrap();
        std::fs::remove_file(b).unwrap();
        state.options.ignore_crlf = false;
        super::recompare(&mut state);
        assert!(state.message.is_none());
        assert_eq!(crate::textdiff::change_counts(&state.lines), (3, 3));
    }

    #[test]
    fn trailing_spaces_recompare_swap_reload_preserve_original_owners() {
        let dir = crate::testutil::TempDir::new();
        let a = dir.file("a", "row \t\n");
        let b = dir.file("b", "row\n");
        let mut state = retained_pair(a.clone(), b.clone());
        super::reload_comparison(&mut state);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (1, 1));
        state.options.trailing_spaces = true;
        super::recompare(&mut state);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (0, 0));
        assert_eq!(state.lines[0].text, "row \t");
        super::swap_sides(&mut state);
        assert!(state.options.trailing_spaces);
        assert_eq!(state.lines[0].text, "row");
        std::fs::write(&a, " row\n").unwrap();
        super::recompare(&mut state);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (0, 0));
        super::reload_comparison(&mut state);
        assert!(state.options.trailing_spaces);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (1, 1));
    }

    #[test]
    fn all_spaces_rebuilds_retained_sources_and_survives_swap_reload() {
        let dir = crate::testutil::TempDir::new();
        let a = dir.file("a", " a + b\n");
        let b = dir.file("b", "a+b\n");
        let mut state = retained_pair(a.clone(), b.clone());
        super::reload_comparison(&mut state);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (1, 1));
        state.options.all_spaces = true;
        state.navigation.show_numbers = true;
        state.intraline.enabled = true;
        state.find.query = "a".into();
        super::recompare(&mut state);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (0, 0));
        assert!(state.navigation.hunks.is_empty() && state.intraline.rows.is_empty());
        super::swap_sides(&mut state);
        assert!(
            state.options.all_spaces && state.navigation.show_numbers && state.intraline.enabled
        );
        assert_eq!(state.lines[0].text, "a+b");
        std::fs::write(&b, "a-b\n").unwrap();
        super::recompare(&mut state);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (0, 0));
        super::reload_comparison(&mut state);
        assert!(state.options.all_spaces);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (1, 1));
        std::fs::remove_file(a).unwrap();
        std::fs::remove_file(b).unwrap();
        state.options.all_spaces = false;
        super::recompare(&mut state);
        assert!(state.message.is_none());
    }

    #[test]
    fn swap_rebuilds_direction_from_retained_sources_and_preserves_view_policies() {
        let dir = crate::testutil::TempDir::new();
        let a = dir.file(
            "minus/report.txt",
            "HEAD  a\r\nsame\nleft one\nleft two\nend",
        );
        let b = dir.file("plus/report.txt", "head a\r\nsame\nright\nend\n");
        let mut state = retained_pair(a.clone(), b.clone());
        (state.name_a, state.name_b) = side_labels(&a, &b);
        state.options = crate::textdiff::Options {
            trailing_spaces: false,
            space_changes: true,
            all_spaces: false,
            expand_tabs: false,
            ignore_crlf: false,
            ignore_case: true,
        };
        state.intraline.enabled = true;
        state.navigation.show_numbers = true;
        state.context_view.compact = true;
        state.context_view.context = 0;
        state.find.query = "left|right".into();
        state.find.regex = true;
        super::reload_comparison(&mut state);
        let source = state.source.clone().unwrap();
        let lines = state.lines.clone();
        let labels = (state.name_a.clone(), state.name_b.clone());
        let numbers = state.navigation.source_numbers.clone();
        assert_eq!(crate::textdiff::change_counts(&lines), (2, 3));
        std::fs::remove_file(&a).unwrap();
        std::fs::remove_file(&b).unwrap();
        state.scroll_offset = egui::vec2(20., 300.);
        state.jump.query = "7".into();
        state.jump.selected = Some(7);
        state.find.pending_reveal = Some(7);
        super::swap_sides(&mut state);
        assert_eq!(state.paths, (b.clone(), a.clone()));
        assert_eq!((&state.name_a, &state.name_b), (&labels.1, &labels.0));
        assert_eq!(state.source, Some((source.1.clone(), source.0.clone())));
        assert_eq!(
            state.lines,
            crate::textdiff::diff_lines_with_options(&source.1, &source.0, state.options).unwrap()
        );
        assert_eq!(crate::textdiff::change_counts(&state.lines), (3, 2));
        assert_eq!(state.lines[0].text, "head a");
        assert!(
            state.message.is_none() && state.intraline.enabled && !state.intraline.rows.is_empty()
        );
        assert!(
            state.navigation.show_numbers && state.context_view.compact && state.keyboard_focused
        );
        assert_eq!(state.context_view.context, 0);
        assert_eq!(state.find.query, "left|right");
        assert!(state.find.regex && !state.find.focused && state.find.pending_reveal.is_none());
        let expected: Vec<_> = state
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.text.contains("left") || l.text.contains("right"))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(state.find.rows, expected);
        assert_eq!(state.jump.selected, None);
        assert!(state.jump.query.is_empty());
        assert_eq!(state.scroll_offset, egui::Vec2::ZERO);
        super::swap_sides(&mut state);
        assert_eq!(state.paths, (a.clone(), b.clone()));
        assert_eq!(state.source, Some(source.clone()));
        assert_eq!(state.lines, lines);
        assert_eq!(state.navigation.source_numbers, numbers);
        assert_eq!((state.name_a.clone(), state.name_b.clone()), labels);
        super::swap_sides(&mut state);
        std::fs::write(&a, "new minus\n").unwrap();
        std::fs::write(&b, "new plus\n").unwrap();
        super::reload_comparison(&mut state);
        assert_eq!(
            state.source,
            Some(("new plus\n".into(), "new minus\n".into()))
        );
        assert_eq!(state.paths, (b, a));
    }

    #[test]
    fn swap_keeps_failed_read_diagnostic_and_reload_recovers_in_reversed_order() {
        let dir = crate::testutil::TempDir::new();
        let a = dir.path().join("gone-a");
        let b = dir.file("b", "same\n");
        let mut state = retained_pair(a.clone(), b.clone());
        super::reload_comparison(&mut state);
        let message = state.message.clone().unwrap();
        super::swap_sides(&mut state);
        assert_eq!(state.paths, (b.clone(), a.clone()));
        assert_eq!(state.message.as_deref(), Some(message.as_str()));
        assert!(state.source.is_none() && state.lines.is_empty());
        std::fs::write(&a, "same\n").unwrap();
        super::reload_comparison(&mut state);
        assert!(state.message.is_none() && super::files_identical(&state.lines));
        super::swap_sides(&mut state);
        assert_eq!(state.paths, (a, b));
        assert!(super::files_identical(&state.lines));
    }

    #[test]
    fn intraline_layout_keeps_exact_text_and_rebuilds_when_comparison_is_replaced() {
        let dir = crate::testutil::TempDir::new();
        let mut state = retained_pair(dir.path().join("a"), dir.path().join("b"));
        state.source = Some(("pre old 12 tail\n".into(), "pre new 13 tail\n".into()));
        state.intraline.enabled = true;
        super::recompare(&mut state);
        assert_eq!(state.intraline.rows[&0], vec![4..7, 9..10]);
        let job = super::intraline_job(
            "-",
            &state.lines[0].text,
            &state.intraline.rows[&0],
            egui::FontId::monospace(12.0),
            egui::Color32::RED,
            egui::Color32::TRANSPARENT,
        );
        assert_eq!(job.text, "- pre old 12 tail");
        let marked: String = job
            .sections
            .iter()
            .filter(|s| s.format.underline.width > 0.0)
            .map(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0])
            .collect();
        assert_eq!(marked, "old2");
        state.source = Some(("SAME\n".into(), "same\n".into()));
        state.options.ignore_case = true;
        super::recompare(&mut state);
        assert!(state.intraline.enabled && state.intraline.rows.is_empty());
        state.options.ignore_case = false;
        super::recompare(&mut state);
        assert_eq!(state.intraline.rows.len(), 2);
        super::reload_comparison(&mut state);
        assert!(
            state.message.is_some() && state.intraline.enabled && state.intraline.rows.is_empty()
        );
    }

    #[test]
    fn options_rebuild_cached_sources_after_files_disappear_and_reload_preserves_policy() {
        let dir = crate::testutil::TempDir::new();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        let ta = "alpha  beta\nSAME\nkept\nold\nend\n";
        let tb = "alpha beta\nsame\nkept\nnew\nend\n";
        std::fs::write(&a, ta).unwrap();
        std::fs::write(&b, tb).unwrap();
        let mut state = retained_pair(a.clone(), b.clone());
        state.navigation.show_numbers = true;
        state.context_view.compact = true;
        state.context_view.context = 0;
        state.find.query = "old".into();
        super::reload_comparison(&mut state);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (3, 3));
        std::fs::remove_file(&a).unwrap();
        std::fs::remove_file(&b).unwrap();
        state.jump.selected = Some(7);
        state.scroll_offset = egui::vec2(7.0, 300.0);
        state.options = crate::textdiff::Options {
            trailing_spaces: false,
            space_changes: true,
            all_spaces: false,
            expand_tabs: false,
            ignore_crlf: false,
            ignore_case: true,
        };
        super::recompare(&mut state);
        assert!(state.message.is_none());
        assert_eq!(crate::textdiff::change_counts(&state.lines), (1, 1));
        assert_eq!(state.navigation.hunks.len(), 1);
        assert_eq!(state.navigation.hunks[0].left_start, 4);
        assert_eq!(state.find.rows, [3]);
        assert_eq!(state.jump.selected, None);
        assert_eq!(state.scroll_offset, egui::Vec2::ZERO);
        assert!(state.navigation.show_numbers && state.context_view.compact);
        assert_eq!(state.context_view.context, 0);
        assert_eq!(
            state.source.as_ref().unwrap(),
            &(ta.to_owned(), tb.to_owned())
        );
        state.options = Default::default();
        super::recompare(&mut state);
        assert_eq!(crate::textdiff::change_counts(&state.lines), (3, 3));
        state.options.ignore_case = true;
        super::reload_comparison(&mut state);
        assert!(state.source.is_none() && state.lines.is_empty() && state.message.is_some());
        assert!(state.navigation.hunks.is_empty() && state.find.rows.is_empty());
        assert!(state.options.ignore_case);
        std::fs::write(&a, "NEW\n").unwrap();
        std::fs::write(&b, "new\n").unwrap();
        super::reload_comparison(&mut state);
        assert!(state.source.is_some() && state.message.is_none());
        assert!(super::files_identical(&state.lines));
    }

    #[test]
    fn options_can_recover_matrix_limit_from_retained_sources_without_reloading() {
        let mut state = retained_pair("unread-a".into(), "unread-b".into());
        state.source = Some((
            (0..3000).map(|i| format!("row {i} A\n")).collect(),
            (0..3000).map(|i| format!("row {i} a\n")).collect(),
        ));
        super::recompare(&mut state);
        assert!(state.message.is_some() && state.lines.is_empty());
        state.options.ignore_case = true;
        super::recompare(&mut state);
        assert!(state.message.is_none() && super::files_identical(&state.lines));
        state.options.ignore_case = false;
        super::recompare(&mut state);
        assert!(state.message.is_some() && state.lines.is_empty());
    }

    #[test]
    fn reload_uses_retained_paths_and_rebuilds_all_coordinates_search_and_context() {
        let dir = crate::testutil::TempDir::new();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, "a\nold\ntail\n").unwrap();
        std::fs::write(&b, "a\nnew\ntail\n").unwrap();
        let mut state = retained_pair(a.clone(), b.clone());
        state.navigation.show_numbers = true;
        state.context_view.compact = true;
        state.context_view.context = 0;
        state.find.query = "tail".into();
        state.find.case = true;
        super::reload_comparison(&mut state);
        assert_eq!(state.find.rows, [3]);
        assert!(state.context_view.hidden > 0);
        assert!(state.context_view.reveal(3, &state.navigation.hunks));
        state.jump.selected = Some(3);
        state.scroll_offset = egui::vec2(30.0, 200.0);
        std::fs::write(&a, "tail\nleft\n").unwrap();
        std::fs::write(&b, "tail\nright\nextra\n").unwrap();
        super::reload_comparison(&mut state);
        assert_eq!(state.paths, (a, b));
        assert!(state.message.is_none());
        assert_eq!(state.find.rows, [0]);
        assert!(!state.find.focused);
        assert!(state.find.case);
        assert_eq!(state.jump.selected, None);
        assert_eq!(state.scroll_offset, egui::Vec2::ZERO);
        assert!(state.navigation.show_numbers);
        assert!(state.context_view.compact);
        assert_eq!(state.context_view.context, 0);
        assert_eq!(state.navigation.current, Some(0));
        assert_eq!(state.navigation.source_numbers.last().unwrap().1, Some(3));
        assert!(matches!(
            state.context_view.row(0),
            Some(crate::diff_context::Row::Gap(_))
        ));
    }
    #[test]
    fn reload_error_clears_stale_success_and_recovers_after_repair() {
        let dir = crate::testutil::TempDir::new();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, "old\n").unwrap();
        std::fs::write(&b, "new\n").unwrap();
        let mut state = retained_pair(a, b.clone());
        super::reload_comparison(&mut state);
        assert_eq!(state.navigation.hunks.len(), 1);
        std::fs::remove_file(&b).unwrap();
        super::reload_comparison(&mut state);
        assert!(state.message.is_some());
        assert!(state.lines.is_empty());
        assert!(state.navigation.hunks.is_empty());
        assert!(state.navigation.source_numbers.is_empty());
        assert!(state.find.rows.is_empty());
        std::fs::write(&b, [0xff]).unwrap();
        super::reload_comparison(&mut state);
        assert!(state.message.as_ref().unwrap().contains("UTF-8"));
        std::fs::write(&b, "old\n").unwrap();
        super::reload_comparison(&mut state);
        assert!(state.message.is_none());
        assert!(super::files_identical(&state.lines));
        assert_eq!(state.lines.len(), 1);
    }
    #[test]
    fn compact_renderer_maps_original_hunks_before_and_after_expanding_a_gap() {
        let lines: Vec<_> = (0..1000)
            .map(|i| crate::textdiff::DiffLine {
                kind: match i {
                    400 | 800 => crate::textdiff::DiffKind::Delete,
                    401 | 801 => crate::textdiff::DiffKind::Insert,
                    _ => crate::textdiff::DiffKind::Equal,
                },
                text: format!("line {i}"),
            })
            .collect();
        let navigation = crate::diff_navigation::Navigation::new(&lines);
        let mut view = crate::diff_context::Context::default();
        view.compact = true;
        view.rebuild(lines.len(), &navigation.hunks);
        let ctx = egui::Context::default();
        let mut visible = Vec::new();
        crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
            let rendered = super::render_diff_view(
                ui,
                &lines,
                &crate::theme::ThemeColors::dark(),
                egui::Vec2::ZERO,
                Some(800..802),
                Some(800),
                (&view, Some(&navigation), None),
            );
            assert!(rendered.laid_out <= view.row_count());
            visible = rendered.visible;
        }));
        assert!(visible.contains(&800));
        assert!(visible.contains(&400));
        assert!(!visible.contains(&500));
        assert!(view.expand(405..797, &navigation.hunks));
        crate::testutil::discard_egui_output(ctx.run_ui(Default::default(), |ui| {
            let rendered = super::render_diff_view(
                ui,
                &lines,
                &crate::theme::ThemeColors::dark(),
                egui::Vec2::ZERO,
                Some(800..802),
                Some(800),
                (&view, Some(&navigation), None),
            );
            assert!(rendered.laid_out < 100);
            assert!(rendered.visible.contains(&800));
            assert!(rendered.offset.y > 0.0);
        }));
    }
    #[test]
    fn distant_hunk_reveal_scrolls_to_original_rows_without_laying_out_the_whole_diff() {
        let lines: Vec<_> = (0..1000)
            .map(|i| crate::textdiff::DiffLine {
                kind: match i {
                    400 | 800 => crate::textdiff::DiffKind::Delete,
                    401 | 801 => crate::textdiff::DiffKind::Insert,
                    _ => crate::textdiff::DiffKind::Equal,
                },
                text: format!("line {i}"),
            })
            .collect();
        let original = lines.clone();
        let mut navigation = crate::diff_navigation::Navigation::new(&lines);
        navigation.go(crate::diff_navigation::Step::Last);
        let hunk = navigation.selected().unwrap().rows.clone();
        let ctx = egui::Context::default();
        let mut offset = egui::Vec2::ZERO;
        let mut visible = Vec::new();
        let mut laid = 0;
        for _ in 0..2 {
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    let rendered = super::render_diff_lines(
                        ui,
                        &lines,
                        &crate::theme::ThemeColors::dark(),
                        offset,
                        Some(hunk.clone()),
                        navigation.take_reveal(),
                    );
                    laid = rendered.laid_out;
                    offset = rendered.offset;
                    visible = rendered.visible;
                },
            ));
        }
        assert!(offset.y > 5000.0);
        assert!(visible.contains(&hunk.start));
        assert!(laid > 0 && laid < 100);
        assert_eq!(lines, original);
    }
    #[test]
    fn large_diff_lays_out_only_visible_lines() {
        let lines: Vec<_> = (0..50_000)
            .map(|index| crate::textdiff::DiffLine {
                kind: crate::textdiff::DiffKind::Insert,
                text: format!("line {index}"),
            })
            .collect();
        let ctx = egui::Context::default();
        let t = crate::theme::ThemeColors::light();
        let mut laid_out = 0;
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            laid_out = render_diff_lines(ui, &lines, &t, egui::Vec2::ZERO, None, None).laid_out;
        }));
        assert!(laid_out > 0);
        assert!(
            laid_out < 100,
            "{laid_out} lines laid out for a 420 px viewport"
        );
    }

    #[test]
    fn reads_regular_text_files() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.file("a.txt", "one\ntwo\n");
        assert_eq!(read_text(&path), Ok("one\ntwo\n".to_string()));
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert_eq!(read_text(&path), Err(ReadError::NotText));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_devices_instead_of_streaming_them() {
        // A device reports length 0, so only the file-type check stops it.
        assert_eq!(
            read_text(std::path::Path::new("/dev/null")),
            Err(ReadError::NotFile)
        );
    }

    #[test]
    fn read_errors_name_the_file_and_the_reason() {
        let dir = crate::testutil::TempDir::new();
        let missing = dir.path().join("gone.txt");
        let Err(error) = read_text(&missing) else {
            panic!("a missing file cannot be read");
        };
        assert!(matches!(error, ReadError::Io(_)), "{error:?}");
        let message = read_error_message(&missing, &error);
        assert!(message.contains("gone.txt"), "{message}");
        assert!(message.starts_with("Could not read"), "{message}");

        assert_eq!(read_text(dir.path()), Err(ReadError::NotFile));
        let big = dir.path().join("big.txt");
        std::fs::write(&big, vec![b'a'; super::DIFF_SIZE_CAP as usize + 1]).unwrap();
        assert_eq!(read_text(&big), Err(ReadError::TooLarge));
        let message = read_error_message(&big, &ReadError::TooLarge);
        assert!(
            message.contains("big.txt") && message.contains("too large"),
            "{message}"
        );
    }

    #[test]
    fn identical_non_empty_files_are_reported_identical() {
        let same = crate::textdiff::diff_lines("one\ntwo\n", "one\ntwo\n").unwrap();
        assert!(!same.is_empty(), "equal rows are still emitted");
        assert!(files_identical(&same));
        assert!(files_identical(&[]));
        let changed = crate::textdiff::diff_lines("one\n", "two\n").unwrap();
        assert!(!files_identical(&changed));
    }

    #[test]
    fn same_named_sides_show_their_folder() {
        assert_eq!(
            side_labels(Path::new("/x/a.txt"), Path::new("/x/b.txt")),
            ("a.txt".to_string(), "b.txt".to_string())
        );
        assert_eq!(
            side_labels(Path::new("/left/a.txt"), Path::new("/right/A.txt")),
            ("a.txt (left)".to_string(), "A.txt (right)".to_string())
        );
        // Folders with the same name fall back to their full path.
        let (a, b) = side_labels(Path::new("/one/src/a.txt"), Path::new("/two/src/a.txt"));
        assert_eq!(a, format!("a.txt ({})", Path::new("/one/src").display()));
        assert_eq!(b, format!("a.txt ({})", Path::new("/two/src").display()));
    }
}
