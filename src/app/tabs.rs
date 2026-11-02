use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClosedTabAction {
    Reopen(usize),
    Forget(usize),
    Clear,
}

fn closed_tabs_menu(
    ui: &mut egui::Ui,
    snapshot: &crate::workspace::tabs::SavedTabs,
    side: ActivePanel,
) -> Option<ClosedTabAction> {
    if snapshot.closed.is_empty() {
        return None;
    }
    let mut action = None;
    let response = ui.menu_button(format!("Closed ({})", snapshot.closed.len()), |ui| {
        for (index, closed) in snapshot.closed.iter().enumerate().rev() {
            ui.horizontal(|ui| {
                let title = closed.tab.display_title();
                let entry = ui.add_enabled(
                    snapshot.entries.len() < crate::workspace::tabs::MAX_TABS,
                    egui::Button::new(format!(
                        "Reopen {}",
                        crate::display_name::truncate_preserving_extension(&title, 32)
                    )),
                );
                entry.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        entry.enabled(),
                        format!(
                            "Reopen closed folder tab: {title}, {}",
                            closed.tab.path.display()
                        ),
                    )
                });
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::new(("closed-tab-reopen", side, index)),
                        entry.rect,
                    )
                });
                if entry
                    .on_hover_text(closed.tab.path.display().to_string())
                    .clicked()
                {
                    action = Some(ClosedTabAction::Reopen(index));
                }
                let forget = ui
                    .button("Forget")
                    .on_hover_text("Remove this history entry; files are unchanged");
                forget.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        forget.enabled(),
                        format!("Forget closed folder tab: {title}"),
                    )
                });
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::new(("closed-tab-forget", side, index)),
                        forget.rect,
                    )
                });
                if forget.clicked() {
                    action = Some(ClosedTabAction::Forget(index));
                }
            });
        }
        ui.separator();
        let clear = ui.button("Clear history");
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new(("closed-tab-clear", side)), clear.rect));
        if clear.clicked() {
            action = Some(ClosedTabAction::Clear);
        }
        ui.label("Cmd+Shift+T reopens the most recent tab");
        if action.is_some() {
            ui.close();
        }
    });
    #[cfg(any(test, feature = "visual-qa"))]
    ui.ctx().data_mut(|d| {
        d.insert_temp(
            egui::Id::new(("closed-tab-menu", side)),
            response.response.rect,
        )
    });
    let _ = (side, response);
    action
}

impl App {
    /// Applies tab changes after rendering the strip so no row can retarget an action mid-loop.
    pub(super) fn render_folder_tabs(&mut self, ui: &mut egui::Ui, side: ActivePanel) -> bool {
        let snapshot = self.ws.tab_snapshot(side);
        let reveal_key = egui::Id::new(("folder_tab_reveal", side));
        let active_binding = (
            snapshot.active,
            snapshot.entries[snapshot.active].path.clone(),
            snapshot.entries[snapshot.active].full_path_label,
        );
        let reveal_active = ui.ctx().data_mut(|data| {
            let changed = data.get_temp::<(usize, PathBuf, bool)>(reveal_key).as_ref()
                != Some(&active_binding);
            data.insert_temp(reveal_key, active_binding);
            changed
        });
        let mut select = None;
        let mut close = false;
        let mut close_at = None;
        let mut duplicate_at = None;
        let mut copy_path_at = None;
        let mut rename_title = None;
        let mut protection_change = None;
        let mut path_label_change = None;
        let mut transfer_tab = None;
        let mut new_tab = false;
        let mut background_tab = None;
        let background_folder = match side {
            ActivePanel::Left => &self.ws.left,
            ActivePanel::Right => &self.ws.right,
        }
        .cursor_entry()
        .filter(|entry| entry.is_dir)
        .map(|entry| entry.path.clone());
        let mut move_tab = None;
        let mut deduplicate = false;
        let mut copy_tabs = false;
        let mut open_other = false;
        let can_copy_tabs = self.ws.can_copy_tabs_to_other(side);
        let mut close_group = None;
        let mut close_left = None;
        let mut reopen = None;
        let mut history_action = None;
        let duplicate_count = self.ws.duplicate_tab_count(side);
        egui::Frame::NONE
            .fill(self.colors.bg_card)
            .inner_margin(egui::Margin::symmetric(6, 3))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    let new = ui.add_enabled(
                        snapshot.entries.len() < crate::workspace::tabs::MAX_TABS,
                        egui::Button::new("+"),
                    );
                    new.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            new.enabled(),
                            "New folder tab",
                        )
                    });
                    #[cfg(feature = "visual-qa")]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tab-new", side)), new.rect));
                    new.context_menu(|ui| {
                        let background = ui.add_enabled(background_folder.is_some(), egui::Button::new("Open highlighted folder in background tab"));
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tab-open-background", side)), background.rect));
                        if background.clicked() { background_tab = background_folder.clone(); ui.close(); }
                    });
                    new_tab = new.on_hover_text("New folder tab (Cmd+T); right-click to open the highlighted folder in background").clicked();
                    if snapshot.entries.len() > 1 {
                        let close_button = ui.add_enabled(!snapshot.entries[snapshot.active].close_protected, egui::Button::new("×"));
                        close_button.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                close_button.enabled(),
                                "Close active folder tab",
                            )
                        });
                        close = close_button
                            .on_hover_text("Close active folder tab (Cmd+Shift+W)")
                            .clicked();
                    }
                    history_action = closed_tabs_menu(ui, &snapshot, side);
                    egui::ScrollArea::horizontal()
                        .id_salt(("folder_tabs", side))
                        .max_height(28.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (index, tab) in snapshot.entries.iter().enumerate() {
                                    let name = tab.display_title();
                                    let label = format!(
                                        "{}{} {}",
                                        if tab.close_protected { "*" } else { "" },
                                        index + 1,
                                        if tab.full_path_label { crate::display_name::truncate_path(&name, 40) } else { crate::display_name::truncate_preserving_extension(&name, 18) }
                                    );
                                    let button =
                                        ui.selectable_label(index == snapshot.active, label);
                                    button.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::WidgetType::SelectableLabel,
                                            button.enabled(),
                                            index == snapshot.active,
                                            format!(
                                                "Folder tab {}: {} ({})",
                                                index + 1,
                                                name,
                                                tab.path.display()
                                            ),
                                        )
                                    });
                                    #[cfg(feature = "visual-qa")]
                                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tab-anchor", side, index)), button.rect));
                                    if button.clicked_by(egui::PointerButton::Middle) && snapshot.entries.len() > 1 && !tab.close_protected { close_at = Some(index); }
                                    let title_id = egui::Id::new(("folder-tab-title-draft", side, index, &tab.path));
                                    if button.clicked_by(egui::PointerButton::Secondary) {
                                        ui.ctx().data_mut(|data| data.remove::<String>(title_id));
                                    }
                                    egui::Popup::context_menu(&button)
                                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                                        .show(|ui| {
                                        let mut protected = tab.close_protected;
                                        let protect = ui.checkbox(&mut protected, "Protect from closing");
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tab-protect", side)), protect.rect));
                                        if protect.changed() { protection_change = Some((index, protected)); }
                                        let mut full_path = tab.full_path_label;
                                        let path_label = ui.checkbox(&mut full_path, "Show full folder path")
                                            .on_hover_text("Use a compact path label on this tab; keep its custom title for switching back");
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tab-full-path", side)), path_label.rect));
                                        if path_label.changed() { path_label_change = Some((index, full_path)); ui.close(); }
                                        let mut title = ui.ctx().data(|data| data.get_temp::<String>(title_id))
                                            .unwrap_or_else(|| tab.title.clone().unwrap_or_default());
                                        ui.label("Tab title");
                                        let edit = ui.add(egui::TextEdit::singleline(&mut title).char_limit(64).hint_text("Use folder name").desired_width(240.0));
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tab-title-input", side)), edit.rect));
                                        ui.ctx().data_mut(|data| data.insert_temp(title_id, title.clone()));
                                        ui.horizontal(|ui| {
                                            let save = ui.add_enabled(crate::workspace::tabs::valid_tab_title(&title), egui::Button::new("Save title"));
                                            #[cfg(feature = "visual-qa")]
                                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tab-title-save", side)), save.rect));
                                            if save.clicked() || (edit.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) && crate::workspace::tabs::valid_tab_title(&title)) {
                                                rename_title = Some((index, title.clone()));
                                                ui.ctx().data_mut(|data| data.remove::<String>(title_id));
                                                ui.close();
                                            }
                                            if ui.add_enabled(tab.title.is_some(), egui::Button::new("Use folder name")).clicked() {
                                                rename_title = Some((index, String::new()));
                                                ui.ctx().data_mut(|data| data.remove::<String>(title_id));
                                                ui.close();
                                            }
                                        });
                                        ui.separator();
                                        if ui.button("Copy this folder path").clicked() {
                                            copy_path_at = Some(index);
                                            ui.close();
                                        }
                                        let transfer = ui.add_enabled(snapshot.entries.len() > 1 && self.ws.tab_count(side.other()) < crate::workspace::tabs::MAX_TABS, egui::Button::new("Move this folder tab to other pane"))
                                            .on_disabled_hover_text("Keep one tab here and leave room in the other pane (eight tabs maximum)")
                                            .on_hover_text("Move this tab with its selection, filter and history into a background tab in the other pane");
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tab-transfer", side)), transfer.rect));
                                        if transfer.clicked() { transfer_tab = Some(index); ui.close(); }

                                        let duplicate = ui.add_enabled(snapshot.entries.len() < crate::workspace::tabs::MAX_TABS, egui::Button::new("Duplicate this folder tab"))
                                            .on_disabled_hover_text("Each pane supports up to eight tabs")
                                            .on_hover_text("Copy this tab's folder and view settings beside it in the background; keep the current folder and focus");
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tab-duplicate", side)), duplicate.rect));
                                        if duplicate.clicked() { duplicate_at = Some(index); ui.close(); }

                                        if ui.add_enabled(snapshot.entries.len() > 1 && !tab.close_protected, egui::Button::new("Close this folder tab")).clicked() { close_at = Some(index); ui.close(); }
                                        ui.separator();

                                        let copy = ui.add_enabled(can_copy_tabs, egui::Button::new("Copy all folder tabs to other pane"))
                                            .on_hover_text(if can_copy_tabs { "Copy the whole group beside the other pane's current tab; both current folders stay open" } else { "The other pane needs room for the whole group (eight tabs maximum)" });
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tabs-copy-other", side)), copy.rect));
                                        if copy.clicked() { copy_tabs = true; ui.close(); }
                                        let open = ui.add_enabled(background_folder.is_some(), egui::Button::new("Open highlighted folder in other pane"))
                                            .on_hover_text("Open the current pane's highlighted folder in the other pane's active tab; keep focus here");
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-open-other", side)), open.rect));
                                        if open.clicked() { open_other = true; ui.close(); }

                                        ui.separator();
                                        let (closable_left, closable_right) = snapshot.closable_sides(index);
                                        let close_others = ui.add_enabled(closable_left + closable_right > 0, egui::Button::new("Close other folder tabs"));
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tab-close-others", side)), close_others.rect));
                                        if close_others.clicked() {
                                            close_group = Some((index, true));
                                            ui.close();
                                        }
                                        let close_prefix = ui.add_enabled(closable_left > 0, egui::Button::new("Close folder tabs to the left"));
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tab-close-left", side)), close_prefix.rect));
                                        if close_prefix.clicked() {
                                            close_left = Some(index);
                                            ui.close();
                                        }
                                        let close_right = ui.add_enabled(closable_right > 0, egui::Button::new("Close folder tabs to the right"));
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tab-close-right", side)), close_right.rect));
                                        if close_right.clicked() {
                                            close_group = Some((index, false));
                                            ui.close();
                                        }
                                        ui.separator();
                                        if ui.add_enabled(index > 0, egui::Button::new("Move tab left")).clicked() {
                                            move_tab = Some((index, index - 1));
                                            ui.close();
                                        }
                                        if ui.add_enabled(index + 1 < snapshot.entries.len(), egui::Button::new("Move tab right")).clicked() {
                                            move_tab = Some((index, index + 1));
                                            ui.close();
                                        }
                                        ui.separator();
                                        let dedup = ui.add_enabled(duplicate_count > 0, egui::Button::new("Close duplicate folder tabs")).on_hover_text("Keep protected tabs, then custom titles; among equal choices keep the active tab, then the first");
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("folder-tab-deduplicate", side)), dedup.rect));
                                        if dedup.clicked() {
                                            deduplicate = true;
                                            ui.close();
                                        }
                                    });
                                    if reveal_active && index == snapshot.active {
                                        button.scroll_to_me(Some(egui::Align::Center));
                                    }
                                    if button
                                        .on_hover_text(format!("{}\n{}\n{}", name, tab.path.display(), if tab.close_protected { "Protected from closing; change in this tab menu" } else { "Middle-click to close this tab" }))
                                        .clicked()
                                    {
                                        select = Some(index);
                                    }
                                }
                            });
                        });
                });
            });
        match history_action {
            Some(ClosedTabAction::Forget(index)) => {
                self.ws.forget_closed_tab_at(side, index);
                ui.ctx().request_repaint();
                return false;
            }
            Some(ClosedTabAction::Clear) => {
                self.ws.clear_closed_tabs(side);
                ui.ctx().request_repaint();
                return false;
            }
            Some(ClosedTabAction::Reopen(index)) => reopen = Some(index),
            None => {}
        }
        if let Some(index) = transfer_tab {
            let visible_changed = index == snapshot.active;
            if self.ws.move_tab_to_other(side, index) {
                self.ui.cancel_context_menu();
                if visible_changed && self.ws.active == side {
                    self.ui.type_ahead = None;
                    self.ui.chord = None;
                }
                self.compare_cache = None;
                ui.ctx().request_repaint();
                return visible_changed;
            }
            return false;
        }
        if let Some(index) = duplicate_at {
            if self.ws.duplicate_tab_at(side, index) {
                self.ui.cancel_context_menu();
                self.compare_cache = None;
                ui.ctx().request_repaint();
            }
            return false;
        }
        if let Some((index, full_path)) = path_label_change {
            if self.ws.set_tab_full_path_label(side, index, full_path) {
                self.ui.cancel_context_menu();
                ui.ctx().request_repaint();
            }
            return false;
        }
        if let Some((index, protected)) = protection_change {
            self.ws.set_tab_close_protection(side, index, protected);
            ui.ctx().request_repaint();
            return false;
        }
        if let Some((index, title)) = rename_title {
            if self.ws.set_tab_title(side, index, &title) {
                self.ui.cancel_context_menu();
                ui.ctx().request_repaint();
            }
            return false;
        }
        if let Some(index) = copy_path_at {
            if let Some(text) = self.ws.tab_path_for_copy(side, index) {
                self.write_clipboard(&text, "folder path", ui.ctx());
                self.ui.cancel_context_menu();
            }
            return false;
        }
        if let Some(index) = close_at {
            let visible_changed = index == snapshot.active;
            if self.ws.close_tab_at(side, index) {
                self.ui.cancel_context_menu();
                if visible_changed && self.ws.active == side {
                    self.ui.type_ahead = None;
                    self.ui.chord = None;
                }
                self.compare_cache = None;
                ui.ctx().request_repaint();
                return visible_changed;
            }
            return false;
        }
        if new_tab
            || background_tab.is_some()
            || close
            || select.is_some()
            || move_tab.is_some()
            || deduplicate
            || copy_tabs
            || open_other
            || close_group.is_some()
            || close_left.is_some()
            || reopen.is_some()
        {
            self.ws.active = side;
            self.ui.cancel_context_menu();
            self.ui.type_ahead = None;
            self.ui.chord = None;
            self.compare_cache = None;
        }
        if let Some(index) = reopen {
            return self.ws.reopen_closed_tab_at(side, index);
        }
        if open_other {
            self.ws
                .execute(crate::command::Command::OpenFolderOtherPane);
            ui.ctx().request_repaint();
            return false;
        }
        if copy_tabs {
            if self.ws.copy_tabs_to_other(side) {
                ui.ctx().request_repaint();
            }
            return false;
        }
        if let Some(path) = background_tab {
            if self.ws.new_background_tab(side, path) {
                ui.ctx().request_repaint();
            }
            // The visible panel has not changed: render it this frame too,
            // keeping its preview/search alive instead of briefly hiding it.
            return false;
        }
        if let Some(anchor) = close_left {
            return self.ws.close_tabs_left(side, anchor);
        }
        if let Some((anchor, others)) = close_group {
            return self.ws.close_tab_group(side, anchor, others);
        }
        if let Some((from, to)) = move_tab {
            return self.ws.move_tab(side, from, to);
        }
        if deduplicate {
            return self.ws.close_duplicate_tabs(side);
        }
        if new_tab {
            return self.ws.new_tab(side, None);
        }
        if close {
            return self.ws.close_tab(side);
        }
        select.is_some_and(|index| self.ws.select_tab(side, index))
    }
}

#[cfg(test)]
mod closed_menu_tests {
    use super::*;
    fn frame(
        ctx: &egui::Context,
        snapshot: &crate::workspace::tabs::SavedTabs,
        events: Vec<egui::Event>,
        time: &mut f64,
    ) -> Option<ClosedTabAction> {
        *time += 0.1;
        let mut action = None;
        crate::testutil::discard_egui_output(ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 700.0),
                )),
                time: Some(*time),
                events,
                ..Default::default()
            },
            |ui| {
                action = closed_tabs_menu(ui, snapshot, ActivePanel::Left);
            },
        ));
        action
    }
    fn click(
        ctx: &egui::Context,
        snapshot: &crate::workspace::tabs::SavedTabs,
        id: egui::Id,
        time: &mut f64,
    ) -> Option<ClosedTabAction> {
        let pos = ctx.data(|d| d.get_temp::<egui::Rect>(id)).unwrap().center();
        let mut result = None;
        for pressed in [true, false] {
            result = result.or(frame(
                ctx,
                snapshot,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                time,
            ));
        }
        result
    }
    #[test]
    fn history_menu_remains_actionable_at_capacity_without_reopening() {
        let root = crate::testutil::TempDir::new();
        let mut ws = crate::workspace::Workspace::new(root.path().into(), root.path().into());
        assert!(ws.new_tab(ActivePanel::Left, None));
        assert!(ws.set_tab_title(ActivePanel::Left, 1, "Saved project"));
        assert!(ws.close_tab(ActivePanel::Left));
        let mut snapshot = ws.tab_snapshot(ActivePanel::Left);
        let entry = snapshot.entries[0].clone();
        snapshot
            .entries
            .resize(crate::workspace::tabs::MAX_TABS, entry);
        for (action, id) in [
            (
                None,
                egui::Id::new(("closed-tab-reopen", ActivePanel::Left, 0usize)),
            ),
            (
                Some(ClosedTabAction::Forget(0)),
                egui::Id::new(("closed-tab-forget", ActivePanel::Left, 0usize)),
            ),
            (
                Some(ClosedTabAction::Clear),
                egui::Id::new(("closed-tab-clear", ActivePanel::Left)),
            ),
        ] {
            let ctx = egui::Context::default();
            let mut time = 0.0;
            frame(&ctx, &snapshot, vec![], &mut time);
            click(
                &ctx,
                &snapshot,
                egui::Id::new(("closed-tab-menu", ActivePanel::Left)),
                &mut time,
            );
            for _ in 0..2 {
                frame(&ctx, &snapshot, vec![], &mut time);
            }
            assert_eq!(click(&ctx, &snapshot, id, &mut time), action);
        }
        snapshot.entries.truncate(1);
        let ctx = egui::Context::default();
        let mut time = 0.0;
        frame(&ctx, &snapshot, vec![], &mut time);
        click(
            &ctx,
            &snapshot,
            egui::Id::new(("closed-tab-menu", ActivePanel::Left)),
            &mut time,
        );
        for _ in 0..2 {
            frame(&ctx, &snapshot, vec![], &mut time);
        }
        assert_eq!(
            click(
                &ctx,
                &snapshot,
                egui::Id::new(("closed-tab-reopen", ActivePanel::Left, 0usize)),
                &mut time
            ),
            Some(ClosedTabAction::Reopen(0))
        );
    }
}
