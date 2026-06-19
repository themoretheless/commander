//! Per-frame orchestration: each concern lives in its own method/module.

use super::*;

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.begin_frame(ctx);
        self.show_transfer_dialog(ctx);
        self.show_confirm_dialog(ctx);
        self.show_rename_dialog(ctx);
        // Wire inline rename commit/cancel from list TextEdit (Enter/Esc) for quick F2 inline.
        if let Some(r) = &mut self.renaming {
            let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
            let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
            if enter {
                let path = r.path.clone();
                let buffer = r.buffer.clone();
                match self.ws.commit_rename(&path, &buffer) {
                    Ok(()) => self.renaming = None,
                    Err(msg) => r.error = Some(msg),
                }
            } else if esc {
                self.renaming = None;
            }
        }
        self.show_batch_rename_dialog(ctx);
        self.show_sync_dialog(ctx);
        self.show_duplicates_dialog(ctx);
        self.show_diff_dialog(ctx);
        self.show_treemap_dialog(ctx);
        self.show_find_dialog(ctx);
        self.show_saved_search_dialog(ctx);
        self.show_mask_dialog(ctx);
        self.show_path_dialog(ctx);
        self.show_recent_dialog(ctx);
        self.show_palette_dialog(ctx);
        self.show_bookmarks_dialog(ctx);
        self.show_column_config_dialog(ctx);
        self.show_user_tag_editor(ctx);
        self.show_permissions_dialog(ctx);
        self.show_archive_dialog(ctx);
        self.show_notes_dialog(ctx);
        // Terminal now bottom pane in show_main_area (toggle with T).
        self.show_toolbar_panel(ctx);
        self.show_shortcut_bar(ctx);
        self.show_shelf_tray(ctx);
        self.show_selection_hud(ctx);
        self.show_main_area(ctx);
        self.show_drag_overlay(ctx);
        self.show_type_ahead_overlay(ctx);
        self.show_toasts(ctx);
        self.handle_drop(ctx);
    }

    /// eframe calls this on exit and on its auto-save interval; persist our
    /// own session snapshot (panel paths, layout, view toggles).
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        crate::session::save(&self.to_session());
    }
}

impl App {
    /// Frame bookkeeping: repaint heuristics, notify wiring, fs polling,
    /// input handling and background-task polling.
    fn begin_frame(&mut self, ctx: &egui::Context) {
        // Repaint only when there's activity (scroll animation, background loads)
        // egui will auto-repaint on user input (mouse, keyboard)
        let has_animation = ctx.is_using_pointer()
            || ctx.input(|i| i.smooth_scroll_delta.length() > 0.0)
            || self.ws.left_active_tab().state.preview().is_some()
            || self.ws.right_active_tab().state.preview().is_some();
        if has_animation {
            ctx.request_repaint();
        }

        // Wire/refresh active tabs only. Share one notify closure (less clone).
        // Use active indices. Full all-tabs wiring lazy later.
        let notify = std::sync::Arc::new({
            let c = ctx.clone();
            move || c.request_repaint()
        });
        {
            let left = self.ws.left_active_tab_mut();
            if !left.state.has_notify() {
                left.state.set_notify(notify.clone());
                left.state.refresh();
            }
        }
        {
            let right = self.ws.right_active_tab_mut();
            if !right.state.has_notify() {
                right.state.set_notify(notify);
                right.state.refresh();
            }
        }

        {
            let left = self.ws.left_active_tab_mut();
            let fs_left = left.state.poll_fs_changes();
            left.state.set_drop_target(None);
            if fs_left {
                self.tree_children_cache.clear();
            }
        }
        {
            let right = self.ws.right_active_tab_mut();
            let fs_right = right.state.poll_fs_changes();
            right.state.set_drop_target(None);
            if fs_right {
                self.tree_children_cache.clear();
            }
        }

        // Delegate git message apply to ws (thin App wiring only).
        self.ws.poll_git();

        self.handle_keys(ctx);

        // Macro record stub (idea #37/63).
        if self.macro_recording {
            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.macro_steps.push("Enter".into());
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.macro_steps.push("Esc".into());
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                self.macro_steps.push("Up".into());
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                self.macro_steps.push("Down".into());
            }
        }
        if !self.macro_recording && !self.macro_steps.is_empty() {
            if ctx.input(|i| i.key_pressed(egui::Key::P)) {
                self.playback_current_macro_steps();
            }
        }

        // User tag toggle 'T' for cursor (idea #64/21): assign/remove demo tag label.
        if ctx.input(|i| i.key_pressed(egui::Key::T)) && !self.macro_recording {
            if let Some(e) = self.ws.active_panel_ref().filtered_get(self.ws.active_panel_ref().cursor().saturating_sub(1)) {
                let p = e.path.clone();
                if self.ws.user_tags.contains_key(&p) {
                    self.ws.user_tags.remove(&p);
                } else {
                    self.ws.user_tags.insert(p, "★".to_string());
                }
            }
        }

        // File note toggle 'N' (idea #92): assign/remove simple note.
        if ctx.input(|i| i.key_pressed(egui::Key::N)) && !self.macro_recording {
            if let Some(e) = self.ws.active_panel_ref().filtered_get(self.ws.active_panel_ref().cursor().saturating_sub(1)) {
                let p = e.path.clone();
                if self.ws.file_notes.contains_key(&p) {
                    self.ws.file_notes.remove(&p);
                } else {
                    self.ws.file_notes.insert(p, "note".to_string());
                }
            }
        }

        // Linked scroll sync (idea #13): when on, keep cursors in sync for "master" feel (scroll/cursor moves affect other).
        if self.ws.linked_scroll {
            self.sync_linked_scroll();
        }

        // Bookmarks now handled by dedicated dialog (show_bookmarks_dialog) + full UI in bookmarks_ui.
        // Assign is inside dialog too; request flag consumed there. Basic assign logic kept in ws for compat.
        if self.ws.requests.assign_bookmark_request {
            // Fallback direct assign if dialog not used.
            let p = self.ws.active_panel_ref().current_path().clone();
            if !self.ws.bookmarks.iter().any(|b| b.path == p) {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string());
                self.ws.bookmarks.push(crate::session::Bookmark { name, path: p });
            }
            self.ws.requests.assign_bookmark_request = false;
        }

        // Git actions toasts + refresh already done in ws
        if let Some(msg) = std::mem::take(&mut self.ws.requests.git_toast) {
            self.toasts.push(crate::toasts::Toast::new(
                msg,
                crate::toasts::ToastKind::Success,
                false,
                ctx.input(|i| i.time),
            ));
        }

        // Drain centralized errors (from git/report_error etc) into error toasts.
        for e in crate::error::drain_errors() {
            self.toasts.push(crate::toasts::Toast::new(
                e,
                crate::toasts::ToastKind::Error,
                false,
                ctx.input(|i| i.time),
            ));
        }

        if std::mem::take(&mut self.ws.requests.toggle_show_git_request) {
            self.ws.show_git_status = !self.ws.show_git_status;
        }

        self.preload_images(ctx);
        if self.ws.poll_transfer() {
            // A clean move just finished: raise an undoable toast.
            let now = ctx.input(|i| i.time);
            if let Some(a) = self.ws.stack.peek_undo() {
                self.toasts.push(crate::toasts::Toast::new(
                    format!("{} {} item(s)", a.verb(), a.item_count()),
                    crate::toasts::ToastKind::Success,
                    true,
                    now,
                ));
            }
        }
        self.toasts.prune(ctx.input(|i| i.time));
        // A two-way sync runs in two passes; start the queued second one once
        // the first finishes.
        if self.ws.has_sync_followup() {
            let c = ctx.clone();
            self.ws.start_sync_followup(move || c.request_repaint());
        }
        // Run a requested undo / redo with a repaint callback.
        if std::mem::take(&mut self.ws.requests.undo_request) {
            let c = ctx.clone();
            self.ws.perform_undo(move || c.request_repaint());
            // The offered Undo is spent; drop the undoable toast(s).
            self.toasts.dismiss_undoable();
        }
        if std::mem::take(&mut self.ws.requests.redo_request) {
            let c = ctx.clone();
            self.ws.perform_redo(move || c.request_repaint());
        }
        // Drain the shelf (copy staged items into the active pane).
        if std::mem::take(&mut self.ws.requests.drain_request) {
            let c = ctx.clone();
            self.ws.drain_shelf(move || c.request_repaint());
        }
        // Cycle the list density (toward Spacious; wraps).
        if std::mem::take(&mut self.ws.requests.cycle_density_request) {
            self.density = crate::density::cycle(self.density, 1);
        }
        // Copy the selection's path(s) to the clipboard in the requested style.
        if let Some(style) = self.ws.requests.clipboard_request.take() {
            let paths: Vec<std::path::PathBuf> = self
                .ws
                .active_panel_ref()
                .selected_or_cursor()
                .into_iter()
                .map(|e| e.path)
                .collect();
            if !paths.is_empty() {
                let other_root = self.ws.inactive_panel().current_path().clone();
                let text = crate::clipboard::format(&paths, style, Some(&other_root));
                ctx.copy_text(text);
                let now = ctx.input(|i| i.time);
                self.toasts.push(crate::toasts::Toast::new(
                    format!(
                        "Copied {} ({})",
                        crate::clipboard::style_label(style),
                        paths.len()
                    ),
                    crate::toasts::ToastKind::Success,
                    false,
                    now,
                ));
            }
        }
    }

    fn show_toolbar_panel(&mut self, ctx: &egui::Context) {
        let t = self.colors;
        egui::TopBottomPanel::top("toolbar")
            .frame(Frame::NONE.fill(t.bg_toolbar))
            .show(ctx, |ui| {
                self.toolbar(ui, ctx);
            });
    }

    fn show_shortcut_bar(&mut self, ctx: &egui::Context) {
        let t = self.colors;
        egui::TopBottomPanel::bottom("shortcuts")
            .frame(Frame::NONE.fill(t.bg_toolbar))
            .show(ctx, |ui| {
                Frame::NONE
                    .inner_margin(Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let keys = [
                                ("Tab", "Switch"),
                                ("Enter", "Open"),
                                ("Space", "Select"),
                                ("F5", "Copy"),
                                ("F6", "Move"),
                                ("F7", "MkDir"),
                                ("F8", "Delete"),
                                ("\u{2318}H", "Hidden"),
                            ];
                            for (key, action) in keys {
                                ui.label(
                                    egui::RichText::new(key).size(11.0).strong().color(t.accent),
                                );
                                ui.label(
                                    egui::RichText::new(action).size(11.0).color(t.text_muted),
                                );
                                ui.add_space(8.0);
                            }

                            // Scale slider on the right
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add_space(12.0);
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{}%",
                                        (self.ui_scale * 100.0) as u32
                                    ))
                                    .size(11.0)
                                    .color(t.text_muted),
                                );
                                let mut preview = self.ui_scale;
                                let slider = egui::Slider::new(&mut preview, 0.8..=1.2)
                                    .step_by(0.05)
                                    .show_value(false)
                                    .trailing_fill(true);
                                let resp = ui.add_sized(egui::vec2(120.0, 16.0), slider);
                                self.ui_scale = preview;
                                // Apply only when released
                                if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                                    if (self.ui_scale - 1.0).abs() < 0.03 {
                                        self.ui_scale = 1.0;
                                    }
                                    ctx.set_zoom_factor(self.ui_scale);
                                }
                                ui.label(
                                    egui::RichText::new("\u{1f50d}")
                                        .size(12.0)
                                        .color(t.text_muted),
                                );
                            });
                        });
                    });
            });
    }

    /// Bottom shelf (drop stack) tray, shown only when something is staged:
    /// count + total size, removable chips, Drain-here and Clear.
    fn show_shelf_tray(&mut self, ctx: &egui::Context) {
        if self.ws.shelf.is_empty() {
            return;
        }
        let t = self.colors;
        let count = self.ws.shelf.len();
        let total = self
            .ws
            .shelf
            .total_size(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0));
        let items: Vec<std::path::PathBuf> = self.ws.shelf.items().to_vec();

        let mut remove: Option<std::path::PathBuf> = None;
        let mut clear = false;
        let mut drain = false;

        egui::TopBottomPanel::bottom("shelf_tray")
            .frame(Frame::NONE.fill(t.bg_card))
            .show(ctx, |ui| {
                Frame::NONE
                    .inner_margin(Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "\u{1f4cb} Shelf {count} \u{00b7} {}",
                                    crate::panel::format_size(total)
                                ))
                                .size(11.0)
                                .strong()
                                .color(t.accent),
                            );
                            ui.add_space(8.0);
                            for p in &items {
                                let name = p
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                let chip = Frame::NONE
                                    .fill(t.bg_panel)
                                    .stroke(Stroke::new(1.0_f32, t.border))
                                    .inner_margin(Margin::symmetric(6, 2))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(format!("{name}  \u{00d7}"))
                                                .size(11.0)
                                                .color(t.text_secondary),
                                        );
                                    })
                                    .response
                                    .interact(Sense::click());
                                if chip.clicked() {
                                    remove = Some(p.clone());
                                }
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("Clear")
                                                .size(11.0)
                                                .color(t.text_primary),
                                        )
                                        .fill(t.bg_panel)
                                        .corner_radius(CornerRadius::ZERO),
                                    )
                                    .clicked()
                                {
                                    clear = true;
                                }
                                ui.add_space(6.0);
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("Drain here \u{2318}\u{21e7}V")
                                                .size(11.0)
                                                .color(Color32::WHITE),
                                        )
                                        .fill(t.accent)
                                        .corner_radius(CornerRadius::ZERO),
                                    )
                                    .clicked()
                                {
                                    drain = true;
                                }
                            });
                        });
                    });
            });

        if let Some(p) = remove {
            self.ws.shelf.remove(&p);
        }
        if clear {
            self.ws.shelf.clear();
        }
        if drain {
            self.ws.requests.drain_request = true;
        }
    }

    /// A quiet pill near the bottom of the central area summarizing the active
    /// panel's selection (count, folders, size, kind breakdown). Shown only
    /// when something is selected; complements the per-panel status bar.
    fn show_selection_hud(&mut self, ctx: &egui::Context) {
        let selected = self.ws.active_panel_ref().selected_entries();
        if selected.is_empty() {
            return;
        }
        let s = crate::selection_summary::summarize(&selected);
        let t = self.colors;

        let mut head = format!("{} item(s)", s.count);
        if s.dir_count > 0 {
            head.push_str(&format!(" \u{00b7} {} folder(s)", s.dir_count));
        }
        if s.total_bytes > 0 {
            head.push_str(&format!(
                " \u{00b7} {}",
                crate::panel::format_size(s.total_bytes)
            ));
        }
        let breakdown: String = s
            .kinds
            .iter()
            .take(4)
            .map(|(k, n)| format!("{n} {}", k.label()))
            .collect::<Vec<_>>()
            .join("  \u{00b7}  ");

        let area = ctx.available_rect();
        egui::Area::new(egui::Id::new("selection_hud"))
            .anchor(
                egui::Align2::CENTER_BOTTOM,
                [
                    area.center().x - ctx.screen_rect().center().x,
                    -(ctx.screen_rect().bottom() - area.bottom()) - 14.0,
                ],
            )
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(t.bg_card)
                    .stroke(Stroke::new(1.0_f32, t.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(12, 7))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(head)
                                    .size(11.0)
                                    .strong()
                                    .color(t.accent),
                            );
                            if !breakdown.is_empty() {
                                ui.add_space(10.0);
                                ui.label(
                                    egui::RichText::new(breakdown)
                                        .size(11.0)
                                        .color(t.text_muted),
                                );
                            }
                        });
                    });
            });
    }

    /// Tree sidebar plus the two file panels with the resizable divider.
    fn show_main_area(&mut self, ctx: &egui::Context) {
        let t = self.colors;
        let metrics = crate::density::metrics(self.density);
        let window_width = ctx.screen_rect().width();
        let panel_id = egui::Id::new("left_panel");

        // Build cross-panel comparison maps before borrowing panels mutably:
        // each panel is tinted against the OTHER panel's entries.
        let (left_compare, right_compare) = if self.show_compare {
            (
                Some(crate::workspace::build_compare_map(&self.ws.right_active_tab().state.entries)),
                Some(crate::workspace::build_compare_map(&self.ws.left_active_tab().state.entries)),
            )
        } else {
            (None, None)
        };

        // Global tree sidebar
        let mut tree_actual_width: f32 = 0.0;
        if self.show_tree {
            let tree_resp = egui::SidePanel::left("global_tree")
                .resizable(true)
                .default_width(self.tree_width)
                .min_width(100.0)
                .max_width(400.0)
                .frame(Frame::NONE.fill(t.bg_deep).inner_margin(Margin::same(0)))
                .show(ctx, |ui| {
                    egui::ScrollArea::both()
                        .id_salt("global_tree_scroll")
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.style_mut().interaction.selectable_labels = false;
                            let tree_nav = self.render_global_tree(ui, &t);
                            crate::app::virtual_tree::render_virtual_tree(ui, (), &t);
                            if let Some(path) = tree_nav {
                                self.tree_expand_to_path(&path);
                                self.ws.active_panel().navigate_to(path);
                            }
                        });
                });
            tree_actual_width = tree_resp.response.rect.width();
        }

        // Compute half from remaining space (after tree) - delegated to layout module for SRP
        let (half, should_reset) = crate::app::layout::compute_panel_half(
            ctx,
            window_width,
            tree_actual_width,
            self.prev_window_width,
            self.show_tree,
            ctx.data_mut(|d| d.get_temp(egui::Id::new("prev_half")).unwrap_or(0.0)),
        );
        if should_reset {
            ctx.data_mut(|d| {
                d.remove::<egui::containers::panel::PanelState>(panel_id);
            });
        }
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("prev_half"), half));
        self.prev_window_width = window_width;

        let mut tree_toggle = false;

        // Left panel
        let left_resp = egui::SidePanel::left(panel_id)
            .resizable(true)
            .default_width(half)
            .min_width(300.0)
            .frame(Frame::NONE.fill(t.bg_deep).inner_margin(Margin::same(0)))
            .show(ctx, |ui| {
                if ui.rect_contains_pointer(ui.max_rect()) && ctx.input(|i| i.pointer.any_pressed())
                {
                    self.ws.active = ActivePanel::Left;
                }

                // Tab bar for left (DRY extracted)
                self.render_tab_bar(ui, true, &t, ctx, metrics);

                let is_left_active = self.ws.active == ActivePanel::Left;
                let left_opener = self.ws.opener.clone();
                let left_show_git = self.ws.show_git_status;
                let left_user_tags = self.ws.user_tags.clone();
                let left_file_notes = self.ws.file_notes.clone();
                let left_grid = self.grid_view;
                let left_size_bars = self.show_size_bars;
                let left_tree = self.show_tree;
                tree_toggle |= Self::render_panel(
                    &mut self.ws.left_active_tab_mut().state,
                    ui,
                    is_left_active,
                    &t,
                    &mut self.image_cache,
                    "left",
                    left_tree,
                    left_size_bars,
                    left_compare.as_ref(),
                    left_opener.as_ref(),
                    metrics,
                    left_show_git,
                    &mut self.config.column_config,
                    self.renaming.as_mut(),
                    left_grid,
                    &left_user_tags,
                    &left_file_notes
                );
            });

        // Double-click on panel divider → reset to 50/50 - delegated to layout
        crate::app::layout::handle_panel_divider_double_click(ctx, left_resp.response.rect, panel_id);

        // Right panel (takes remaining space)
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(t.bg_deep).inner_margin(Margin::same(0)))
            .show(ctx, |ui| {
                if ui.rect_contains_pointer(ui.max_rect()) && ctx.input(|i| i.pointer.any_pressed())
                {
                    self.ws.active = ActivePanel::Right;
                }

                // Tab bar for right (DRY extracted)
                self.render_tab_bar(ui, false, &t, ctx, metrics);

                let is_right_active = self.ws.active == ActivePanel::Right;
                let right_opener = self.ws.opener.clone();
                let right_show_git = self.ws.show_git_status;
                let right_user_tags = self.ws.user_tags.clone();
                let right_file_notes = self.ws.file_notes.clone();
                let right_grid = self.grid_view;
                let right_size_bars = self.show_size_bars;
                let right_tree = self.show_tree;
                tree_toggle |= Self::render_panel(
                    &mut self.ws.right_active_tab_mut().state,
                    ui,
                    is_right_active,
                    &t,
                    &mut self.image_cache,
                    "right",
                    right_tree,
                    right_size_bars,
                    right_compare.as_ref(),
                    right_opener.as_ref(),
                    metrics,
                    right_show_git,
                    &mut self.config.column_config,
                    self.renaming.as_mut(),
                    right_grid,
                    &right_user_tags,
                    &right_file_notes
                );
            });

        // Terminal as bottom pane (idea #11, like preview grip area).
        if self.terminal_open {
            egui::TopBottomPanel::bottom("terminal")
                .default_height(90.0)
                .min_height(50.0)
                .show(ctx, |ui| {
                    let t = self.colors;
                    crate::app::ui_common::section_frame(&t).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            crate::app::ui_common::primary_label(ui, "Terminal", &t);
                            let dir = self.ws.active_panel_ref().current_path().display().to_string();
                            crate::app::ui_common::muted_label(ui, &format!("@ {}", dir), &t);
                            if ui.small_button("Open native").clicked() {
                                let d = self.ws.active_panel_ref().current_path().clone();
                                let _ = std::process::Command::new("open")
                                    .arg("-a")
                                    .arg("Terminal")
                                    .arg(&d)
                                    .spawn();
                            }
                            if ui.small_button("X").clicked() {
                                self.terminal_open = false;
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("$ ").color(t.accent));
                            let mut cmd = String::new();
                            let resp = ui.add(egui::TextEdit::singleline(&mut cmd).desired_width(300.0));
                            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                if !cmd.trim().is_empty() {
                                    self.terminal_history.push(cmd.clone());
                                    if self.terminal_history.len() > 5 { self.terminal_history.remove(0); }
                                }
                                // stub run
                                crate::app::ui_common::muted_label(ui, &format!("(stub ran: {})", cmd), &t);
                            }
                        });
                        if !self.terminal_history.is_empty() {
                            crate::app::ui_common::muted_label(ui, "History:", &t);
                            for h in self.terminal_history.iter().rev().take(3) {
                                crate::app::ui_common::muted_label(ui, h, &t);
                            }
                        }
                    });
                });
        }

        if tree_toggle {
            self.show_tree = !self.show_tree;
            if self.show_tree {
                let path = self.ws.active_panel().current_path().clone();
                self.tree_expand_to_path(&path);
            }
        }
    }

    /// Floating label with the dragged file count next to the pointer.
    fn show_drag_overlay(&mut self, ctx: &egui::Context) {
        let t = self.colors;
        let left_state = &self.ws.left_active_tab().state;
        let right_state = &self.ws.right_active_tab().state;
        let drag_entries = if !left_state.drag_entries().is_empty() {
            left_state.drag_entries()
        } else if !right_state.drag_entries().is_empty() {
            right_state.drag_entries()
        } else {
            return;
        };
        let count = drag_entries.len();
        if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
            let label = if count == 1 {
                drag_entries[0]
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "1 item".into())
            } else {
                format!("{} items", count)
            };
            egui::Area::new(egui::Id::new("drag_overlay"))
                .fixed_pos(pos + egui::vec2(12.0, 12.0))
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .inner_margin(Margin::symmetric(8, 4))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(label).size(12.0).color(t.text_primary));
                        });
                });
        }
        ctx.request_repaint();
    }

    /// Floating capsule showing the current type-ahead buffer.
    fn show_type_ahead_overlay(&mut self, ctx: &egui::Context) {
        let Some((buffer, last)) = &self.type_ahead else {
            return;
        };
        let now = ctx.input(|i| i.time);
        if now - last > 1.5 {
            self.type_ahead = None;
            return;
        }
        let t = self.colors;
        let label = format!("\u{2192} {buffer}");
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("type_ahead_overlay"))
            .fixed_pos(egui::pos2(screen.center().x - 60.0, screen.bottom() - 80.0))
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(t.bg_card)
                    .inner_margin(Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(label)
                                .size(13.0)
                                .strong()
                                .color(t.accent),
                        );
                    });
            });
        // Keep repainting so the capsule fades out on idle.
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    /// Bottom-right stack of operation toasts, each with a hairline countdown
    /// and an inline Undo on undoable ops.
    fn show_toasts(&mut self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        let t = self.colors;
        let now = ctx.input(|i| i.time);
        let screen = ctx.screen_rect();
        let mut undo = false;

        // Newest on top: stack upward from the bottom-right corner.
        for (i, toast) in self.toasts.active().iter().enumerate().rev() {
            let y = screen.bottom() - 70.0 - (i as f32) * 44.0;
            let accent = match toast.kind {
                crate::toasts::ToastKind::Success => t.accent,
                crate::toasts::ToastKind::Error => t.accent_red,
            };
            let frac = crate::toasts::remaining_fraction(toast, now);
            egui::Area::new(egui::Id::new(("toast", i)))
                .fixed_pos(egui::pos2(screen.right() - 280.0, y))
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(t.bg_card)
                        .stroke(Stroke::new(1.0_f32, t.border))
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_width(236.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&toast.message)
                                        .size(12.0)
                                        .color(t.text_primary),
                                );
                                if toast.undoable {
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new("Undo \u{2318}Z")
                                                        .size(11.0)
                                                        .color(Color32::WHITE),
                                                )
                                                .fill(accent)
                                                .corner_radius(CornerRadius::ZERO),
                                            )
                                            .clicked()
                                        {
                                            undo = true;
                                        }
                                    });
                                }
                            });
                            // Hairline countdown.
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 2.0),
                                Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    rect.min,
                                    Vec2::new(rect.width() * frac, 2.0),
                                ),
                                CornerRadius::ZERO,
                                accent,
                            );
                        });
                });
        }
        if undo {
            self.ws.requests.undo_request = true;
        }
        // Keep animating the countdown.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// On mouse release, move dragged files into the hovered directory.
    fn handle_drop(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.pointer.any_released()) {
            return;
        }
        let ctx2 = ctx.clone();
        self.ws.drop_dragged(move || ctx2.request_repaint());
    }

    /// DRY extracted tab bar for left or right side (solid single responsibility for tab UI).
    /// Full drag reorder with live visual insert marker (idea from cycle: drag tabs complete).
    fn render_tab_bar(&mut self, ui: &mut egui::Ui, is_left: bool, t: &ThemeColors, ctx: &egui::Context, _metrics: crate::density::DensityMetrics) {
        let (len, side_active) = if is_left {
            (self.ws.left.len(), ActivePanel::Left)
        } else {
            (self.ws.right.len(), ActivePanel::Right)
        };
        let mut tab_rects: Vec<(usize, egui::Rect)> = Vec::new();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(1.0, 0.0);
            let mut to_close: Option<usize> = None;
            for i in 0..len {
                let tab = if is_left { &self.ws.left.tabs[i] } else { &self.ws.right.tabs[i] };
                let title = Self::tab_title(tab);
                let active = if is_left { self.ws.left.active_index() } else { self.ws.right.active_index() };
                let is_active = i == active;
                let btn = if is_active {
                    egui::Button::new(egui::RichText::new(title).strong().size(11.0))
                        .fill(t.accent.linear_multiply(0.25))
                        .corner_radius(egui::CornerRadius::same(2))
                } else {
                    egui::Button::new(egui::RichText::new(title).size(11.0))
                        .fill(Color32::TRANSPARENT)
                        .corner_radius(egui::CornerRadius::same(2))
                };
                let resp = ui.add(btn.sense(egui::Sense::click_and_drag()));
                tab_rects.push((i, resp.rect));
                if resp.clicked() {
                    if is_left { self.ws.left.set_active(i); } else { self.ws.right.set_active(i); }
                    self.ws.active = side_active;
                }
                if resp.drag_started() {
                    self.dragged_tab = Some((is_left, i));
                }
                // Tab context menu (ideas #2,14)
                resp.context_menu(|ui| {
                    if ui.button("Close others").clicked() {
                        if is_left {
                            self.ws.left.keep_only(i);
                        } else {
                            self.ws.right.keep_only(i);
                        }
                        ui.close_menu();
                    }
                    if ui.button("Duplicate tab").clicked() {
                        self.ws.active = side_active;
                        if is_left { self.ws.left.active = i; } else { self.ws.right.active = i; }
                        self.ws.duplicate_active_tab();
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.label("Tab menu (more in future)");
                });
                if len > 1 {
                    if ui.small_button("×").clicked() {
                        to_close = Some(i);
                    }
                }
            }
            // Live drag reorder with visual marker (full impl of proposed idea)
            let mut insert_target: Option<usize> = None;
            if let Some((side, _)) = self.dragged_tab {
                if side == is_left {
                    let pointer = ui.input(|i| i.pointer.interact_pos());
                    if let Some(pos) = pointer {
                        // Find closest tab slot for insert (before the hovered tab)
                        let mut best = len; // append at end
                        let mut best_dist = f32::MAX;
                        for (idx, r) in &tab_rects {
                            let center_x = r.center().x;
                            let d = (center_x - pos.x).abs();
                            if d < best_dist {
                                best_dist = d;
                                best = *idx;
                            }
                        }
                        // If pointer past last, append; else insert before best
                        if pos.x > tab_rects.last().map(|(_,r)| r.right()).unwrap_or(0.0) {
                            best = len;
                        }
                        insert_target = Some(best);
                        // Paint thin insert marker
                        if let Some(target) = insert_target {
                            let x = if target < tab_rects.len() {
                                tab_rects[target].1.left() - 1.0
                            } else if let Some((_, last)) = tab_rects.last() {
                                last.right() + 1.0
                            } else { 0.0 };
                            let y0 = ui.min_rect().top();
                            let y1 = ui.min_rect().bottom();
                            ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(x-1.0, y0), egui::pos2(x+1.0, y1)), 0.0, t.accent);
                        }
                    }
                }
            }
            if ctx.input(|i| i.pointer.any_released()) {
                if let Some((s, di)) = self.dragged_tab.take() {
                    if s == is_left && di < len {
                        if let Some(target) = insert_target {
                            let tab = if is_left {
                                self.ws.left.tabs.remove(di)
                            } else {
                                self.ws.right.tabs.remove(di)
                            };
                            let adj_target = if target > di { target - 1 } else { target };
                            let final_idx = adj_target.min(if is_left { self.ws.left.tabs.len() } else { self.ws.right.tabs.len() });
                            if is_left {
                                self.ws.left.tabs.insert(final_idx, tab);
                                self.ws.left.set_active(final_idx);
                            } else {
                                self.ws.right.tabs.insert(final_idx, tab);
                                self.ws.right.set_active(final_idx);
                            }
                        }
                    }
                }
            }
            if let Some(i) = to_close {
                if is_left {
                    self.ws.left.close_tab(i);
                } else {
                    self.ws.right.close_tab(i);
                }
                self.ws.active = side_active;
            }
            if ui.small_button("+").on_hover_text("New tab").clicked() {
                self.ws.active = side_active;
                self.ws.duplicate_active_tab();
                self.ws.active = side_active;
            }
            if ui.small_button("C").on_hover_text("Columns config").clicked() {
                self.column_config_open = true;
            }
            // Linked scroll toggle (idea #13)
            if crate::app::ui_common::small_toggle(ui, "L", self.ws.linked_scroll, "Toggle linked scroll") {
                self.ws.linked_scroll = !self.ws.linked_scroll;
            }
            // Mini terminal toggle (idea #11)
            if crate::app::ui_common::small_toggle(ui, "T", self.terminal_open, "Toggle terminal pane") {
                self.terminal_open = !self.terminal_open;
            }
            // Macro record/play/save (idea #82/71/37): named last + playback.
            if crate::app::ui_common::small_toggle(ui, "M", self.macro_recording, "Toggle macro record (stops auto-saves 'last')") {
                self.macro_recording = !self.macro_recording;
                if !self.macro_recording && !self.macro_steps.is_empty() {
                    self.saved_macros.insert("last".to_string(), self.macro_steps.clone());
                }
            }
            if !self.macro_recording && !self.macro_steps.is_empty() {
                if ui.small_button("P").on_hover_text("Playback current steps").clicked() {
                    self.playback_current_macro_steps();
                }
                if ui.small_button("save").on_hover_text("Save current as 'last' macro").clicked() {
                    self.saved_macros.insert("last".to_string(), self.macro_steps.clone());
                }
            }
            if let Some(st) = self.saved_macros.get("last") {
                if !self.macro_recording && ui.small_button("last").on_hover_text(format!("Play saved last ({} steps)", st.len())).clicked() {
                    self.macro_steps = st.clone();
                    self.playback_current_macro_steps();
                }
            }
            // Grid toggle (idea #65/72)
            if crate::app::ui_common::small_toggle(ui, "G", self.grid_view, "Toggle grid/list") {
                self.grid_view = !self.grid_view;
            }
            ui.add_space(6.0);
            // Action buttons (less frequent, separated for UX clarity)
            // Tag editor (idea #73)
            if ui.small_button("tag").on_hover_text("User tags editor").clicked() {
                self.user_tag_editor_open = true;
            }
            // Notes (idea #92)
            if ui.small_button("note").on_hover_text("File notes editor").clicked() {
                self.notes_open = true;
            }
            // Permissions (idea #83)
            if ui.small_button("perm").on_hover_text("Permissions/chmod stub").clicked() {
                self.permissions_open = true;
            }
            // Archive (idea #84)
            if ui.small_button("zip").on_hover_text("Browse archive stub").clicked() {
                self.archive_open = true;
            }
        });
        ui.add_space(2.0);
    }

    /// Column config dialog (TC style): full functional toggle + widths + reset. (idea #10)
    pub(crate) fn show_column_config_dialog(&mut self, ctx: &egui::Context) {
        if !self.column_config_open {
            return;
        }
        let t = self.colors;
        let mut close = false;
        egui::Window::new("Columns")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Columns (from plugin registry + config)");
                ui.separator();
                // Always on Name
                ui.horizontal(|ui| {
                    ui.label("Name (always)");
                    ui.add(egui::Slider::new(&mut self.config.column_config.name_width, 100.0..=500.0).text("w"));
                });
                // Git toggle + width
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut self.config.column_config.show_git, "Git").changed() {
                        self.ws.show_git_status = self.config.column_config.show_git;
                    }
                    if self.config.column_config.show_git {
                        ui.add(egui::Slider::new(&mut self.config.column_config.git_width, 20.0..=80.0).text("w"));
                    }
                });
                ui.separator();
                let cols: Vec<_> = crate::panel::active_columns(&self.config.column_config).into_iter().map(|c| c.header().to_string()).collect();
                ui.label(format!("Active: {}", cols.join(", ")));
                ui.horizontal(|ui| {
                    if ui.button("Reset defaults").clicked() {
                        self.config.column_config = crate::panel::ColumnConfig::default();
                        self.ws.show_git_status = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        if close {
            self.column_config_open = false;
        }
    }

    /// User tag editor stub dialog (idea #73/83/64). Lists current, allows assign/remove for active cursor.
    pub(crate) fn show_user_tag_editor(&mut self, ctx: &egui::Context) {
        if !self.user_tag_editor_open {
            return;
        }
        let t = self.colors;
        let mut close = false;
        egui::Window::new("Tags (color labels)")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("User tags (★ demo). Toggle with T on row. Full editor later.");
                ui.separator();
                let cur = self.ws.active_panel_ref().cursor();
                if let Some(e) = self.ws.active_panel_ref().filtered_get(cur.saturating_sub(1)) {
                    let p = e.path.clone();
                    ui.label(format!("Current: {}", e.name));
                    if self.ws.user_tags.contains_key(&p) {
                        if ui.button("Remove tag").clicked() {
                            self.ws.user_tags.remove(&p);
                        }
                    } else if ui.button("Assign ★ tag").clicked() {
                        self.ws.user_tags.insert(p.clone(), "★".into());
                    }
                }
                ui.label(format!("Tagged items: {}", self.ws.user_tags.len()));
                if !self.ws.user_tags.is_empty() {
                    for (pth, tag) in self.ws.user_tags.iter().take(5) {
                        ui.label(format!("{}  {}", tag, pth.display()));
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button("Close").clicked() { close = true; }
                    if ui.button("Clear all").clicked() { self.ws.user_tags.clear(); }
                });
            });
        if close {
            self.user_tag_editor_open = false;
        }
    }

    /// Permissions stub dialog (idea #83, modeled on Finder Get Info + mc chmod).
    pub(crate) fn show_permissions_dialog(&mut self, ctx: &egui::Context) {
        if !self.permissions_open { return; }
        let t = self.colors;
        let mut close = false;
        egui::Window::new("Permissions")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Permissions (stub). Full chmod + recursive later.");
                ui.separator();
                if let Some(e) = self.ws.active_panel_ref().filtered_get(self.ws.active_panel_ref().cursor().saturating_sub(1)) {
                    ui.label(format!("File: {}", e.name));
                    ui.label("Owner: rwx  Group: r-x  Other: r--  (demo)");
                    if ui.button("Apply (stub)").clicked() {
                        self.toasts.push(crate::toasts::Toast::new(
                            format!("chmod stub on {}", e.name),
                            crate::toasts::ToastKind::Success,
                            false,
                            ctx.input(|i| i.time),
                        ));
                    }
                }
                if ui.button("Close").clicked() { close = true; }
            });
        if close {
            self.permissions_open = false;
        }
    }

    /// Basic archive stub (idea #84): detect + list contents stub, extract action stub.
    pub(crate) fn show_archive_dialog(&mut self, ctx: &egui::Context) {
        if !self.archive_open { return; }
        let t = self.colors;
        let mut close = false;
        egui::Window::new("Archive")
            .collapsible(false)
            .resizable(true)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Zip/tar browser stub (like mc F3 on archive).");
                if let Some(e) = self.ws.active_panel_ref().filtered_get(self.ws.active_panel_ref().cursor().saturating_sub(1)) {
                    let n = e.name.to_lowercase();
                    ui.label(format!("Archive: {}", e.name));
                    if n.ends_with(".zip") || n.ends_with(".tar") || n.contains(".tar.") {
                        ui.label("Contents (stub): file1.txt\n dir/\n file2.rs");
                        if ui.button("Extract here (stub)").clicked() {
                            self.toasts.push(crate::toasts::Toast::new(
                                "Extract stub done",
                                crate::toasts::ToastKind::Success,
                                false,
                                ctx.input(|i| i.time),
                            ));
                        }
                    } else {
                        ui.label("Select a .zip/.tar to browse.");
                    }
                }
                if ui.button("Close").clicked() { close = true; }
            });
        if close {
            self.archive_open = false;
        }
    }

    /// Notes editor stub (idea #92): view/edit note for current file, list some.
    pub(crate) fn show_notes_dialog(&mut self, ctx: &egui::Context) {
        if !self.notes_open { return; }
        let t = self.colors;
        let mut close = false;
        egui::Window::new("Notes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Attach notes to files (shown on hover). N key toggles demo.");
                ui.separator();
                let panel = self.ws.active_panel_ref();
                if let Some(e) = panel.filtered_get(panel.cursor.saturating_sub(1)) {
                    let p = e.path.clone();
                    let mut note = self.ws.file_notes.get(&p).cloned().unwrap_or_default();
                    ui.label(format!("Note for {}", e.name));
                    if ui.add(egui::TextEdit::singleline(&mut note).desired_width(200.0)).changed() {
                        if note.trim().is_empty() {
                            self.ws.file_notes.remove(&p);
                        } else {
                            self.ws.file_notes.insert(p.clone(), note);
                        }
                    }
                }
                ui.label(format!("Notes: {}", self.ws.file_notes.len()));
                if ui.button("Close").clicked() { close = true; }
            });
        if close {
            self.notes_open = false;
        }
    }

    /// Centralized macro playback (nav + enter simulation). Deduped from begin_frame + button.
    fn playback_current_macro_steps(&mut self) {
        let panel = self.ws.active_panel();
        for step in &self.macro_steps {
            match step.as_str() {
                "Up" if panel.cursor() > 0 => {
                    panel.set_cursor(panel.cursor() - 1);
                    panel.set_scroll_to_cursor(true);
                }
                "Down" => {
                    let max = panel.filtered_count();
                    if panel.cursor() < max {
                        panel.set_cursor(panel.cursor() + 1);
                        panel.set_scroll_to_cursor(true);
                    }
                }
                "Enter" => {
                    if let Some(e) = panel.filtered_get(panel.cursor().saturating_sub(1)) {
                        if e.is_dir {
                            let p = e.path.clone();
                            panel.navigate_to(p);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Extracted for separation: keeps begin_frame smaller, logic for linked scroll (idea #13) in one place.
    fn sync_linked_scroll(&mut self) {
        let active = self.ws.active_panel_ref();
        let ac = active.cursor();
        let other = self.ws.inactive_panel_mut();
        let omax = other.filtered_count();
        if ac <= omax && other.cursor() != ac {
            other.set_cursor_and_scroll(ac);
        }
    }

    /// Helper to avoid string allocation duplication in tab rendering (perf + DRY).
    fn tab_title(tab: &crate::workspace::PanelTab) -> String {
        let base = tab.state.current_path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "root".to_string());
        let has_filter = !tab.state.search_query().is_empty() || !tab.state.facets().is_empty();
        let has_git = !tab.state.git_status().is_empty();
        let suffix = if has_filter { " •" } else { "" };
        let git_suffix = if has_git { " G" } else { "" };
        format!("{}{}{}", base, suffix, git_suffix)
    }

}
