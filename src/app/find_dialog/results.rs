//! The virtualized result list with per-row actions and context menus.

use super::*;

/// What the user asked for in the query field, result toolbar and list; applied after the frame.
#[derive(Default)]
pub(super) struct ResultActions {
    pub(super) reveal: Option<PathBuf>,
    pub(super) open_path: Option<PathBuf>,
    pub(super) open_tab: Option<(crate::workspace::ActivePanel, PathBuf)>,
    pub(super) open_selected_tabs: Option<(crate::workspace::ActivePanel, Vec<PathBuf>)>,
    pub(super) copy_result_paths: Option<(String, usize, FindCopyFormat)>,
    pub(super) explanation_change: Option<Option<ExplainKey>>,
    pub(super) hide_result: Option<crate::search::SearchLocation>,
    pub(super) hide_selected_results: bool,
    pub(super) select_result: Option<(crate::search::SearchLocation, bool)>,
    pub(super) context_select_only: Option<crate::search::SearchLocation>,
}

pub(super) fn results_ui(
    ui: &mut egui::Ui,
    state: &mut FindState,
    t: crate::theme::ThemeColors,
    clock: SystemTime,
    tab_side: crate::workspace::ActivePanel,
    actions: &mut ResultActions,
) {
    let indices = state.visible_result_order(clock);
    #[cfg(feature = "visual-qa")]
    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new("find-rendered-order"), indices.clone()));
    let pivot = state.time_pivot;
    let row_height = result_row_height(pivot != crate::search::TimePivot::None);
    let root = state.root.clone();
    let open_explanation = state.explanation_open.clone();
    state.highlighted_result = state.highlighted_result.filter(|&row| row < indices.len());
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
                            actions.select_result = Some((hit.location.clone(), ui.input(|input| input.modifiers.shift)));
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
                            actions.context_select_only = Some(hit.location.clone());
                        }
                        response.context_menu(|ui| {
                            let mut menu_action = false;
                            if ui.button("Reveal in panel").clicked() { actions.reveal = Some(path.clone()); ui.close(); menu_action = true; }
                            if ui.add_enabled(!hit.entry.is_dir, egui::Button::new("Open file")).clicked() { actions.open_path = Some(path.clone()); ui.close(); menu_action = true; }
                            if ui.button("Open background tab").clicked() { actions.open_tab = Some((tab_side, path.clone())); ui.close(); menu_action = true; }
                            if ui.button("Open selected tabs").clicked() {
                                let targets = if state.selected_results.contains(&hit.location) { state.selected_result_tab_targets(clock) } else { vec![path.clone()] };
                                actions.open_selected_tabs = Some((tab_side, targets)); ui.close(); menu_action = true;
                            }
                            ui.separator();
                            let copy_path = ui.button("Copy full path");
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("find-context-copy-path", relative.as_str())), copy_path.rect));
                            if copy_path.clicked() {
                                actions.copy_result_paths = Some((crate::clipboard::format(std::slice::from_ref(&hit.entry.path), crate::clipboard::PathStyle::FullPath, None), 1, FindCopyFormat::Paths));
                                ui.close(); menu_action = true;
                            }
                            let copy_name = ui.button("Copy name");
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("find-context-copy-name", relative.as_str())), copy_name.rect));
                            if copy_name.clicked() {
                                actions.copy_result_paths = Some((crate::clipboard::format(std::slice::from_ref(&hit.entry.path), crate::clipboard::PathStyle::NameOnly, None), 1, FindCopyFormat::Names));
                                ui.close(); menu_action = true;
                            }
                            ui.separator();
                            if ui.button("Hide result").clicked() { actions.hide_result = Some(hit.location.clone()); ui.close(); menu_action = true; }
                            if ui.button("Hide selected").clicked() {
                                if state.selected_results.contains(&hit.location) { actions.hide_selected_results = true; } else { actions.hide_result = Some(hit.location.clone()); }
                                ui.close(); menu_action = true;
                            }
                            if ui.button("Why matched").clicked() { actions.explanation_change = Some(Some(key.clone())); ui.close(); menu_action = true; }
                            if menu_action { ui.memory_mut(|memory| memory.request_focus(egui::Id::new("find-query-editor"))); }
                        });
                        let modifiers = ui.input(|input| input.modifiers);
                        if response.double_clicked() && !hit.entry.is_dir && !modifiers.command && !modifiers.shift {
                            actions.open_path = Some(path.clone());
                        }
                        if response.clicked() {
                            if modifiers.command || modifiers.shift {
                                actions.select_result = Some((hit.location.clone(), modifiers.shift));
                            } else {
                                actions.reveal = Some(path);
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
                        if tab.clicked() { actions.open_tab = Some((tab_side, hit.reveal_path().to_path_buf())); }
                        let hide = ui.small_button("×").on_hover_text("Hide this search result; the file is not changed");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("find-hide-result", relative.as_str())), hide.rect));
                        hide.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Hide result: {relative}")));
                        if hide.clicked() {
                            actions.hide_result = Some(hit.location.clone());
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
                            actions.explanation_change = Some((!explaining).then_some(key));
                        }
                    });
                },
            );
        }
    });
}
