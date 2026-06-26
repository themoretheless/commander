//! Per-frame orchestration: each concern lives in its own method/module.

use super::*;

fn clipped_label(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let mut out: String = text.chars().take(keep).collect();
    out.push_str("...");
    out
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.begin_frame(ctx);
        self.show_transfer_dialog(ctx);
        self.show_confirm_dialog(ctx);
        self.show_rename_dialog(ctx);
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
        self.show_run_command_dialog(ctx);
        self.show_palette_dialog(ctx);
        if !self.focus_mode {
            self.show_toolbar_panel(ctx);
            self.show_shortcut_bar(ctx);
            self.show_shelf_tray(ctx);
            self.show_selection_hud(ctx);
        }
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
            || self.ws.left.preview.is_some()
            || self.ws.right.preview.is_some();
        if has_animation {
            ctx.request_repaint();
        }

        self.update_focus_mode(ctx);

        // First frame: wire the repaint callback into both panels and do
        // the initial directory read.
        if !self.ws.left.has_notify() {
            let c = ctx.clone();
            self.ws
                .left
                .set_notify(std::sync::Arc::new(move || c.request_repaint()));
            self.ws.left.refresh();
        }
        if !self.ws.right.has_notify() {
            let c = ctx.clone();
            self.ws
                .right
                .set_notify(std::sync::Arc::new(move || c.request_repaint()));
            self.ws.right.refresh();
        }

        let fs_changed = self.ws.left.poll_fs_changes() | self.ws.right.poll_fs_changes();
        if fs_changed {
            self.tree_children_cache.clear();
        }

        // Drop targets are only valid for the frame that set them
        // (rows re-assert them while hovered during render).
        self.ws.left.drop_target = None;
        self.ws.right.drop_target = None;

        self.handle_keys(ctx);
        self.preload_images(ctx);
        {
            let c = ctx.clone();
            // poll_transfer drains the next queued transfer (two-way sync second
            // pass, or any op queued behind the active one) via this notify.
            if self.ws.poll_transfer(move || c.request_repaint()) {
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
        }
        self.toasts.prune(ctx.input(|i| i.time));
        // Run a requested undo / redo with a repaint callback.
        if std::mem::take(&mut self.ws.undo_request) {
            let c = ctx.clone();
            self.ws.perform_undo(move || c.request_repaint());
            // The offered Undo is spent; drop the undoable toast(s).
            self.toasts.dismiss_undoable();
        }
        if std::mem::take(&mut self.ws.redo_request) {
            let c = ctx.clone();
            self.ws.perform_redo(move || c.request_repaint());
        }
        // Gather the selection into a new subfolder (queues an undoable Move).
        if std::mem::take(&mut self.ws.gather_request) {
            let c = ctx.clone();
            self.ws.gather_into_folder(move || c.request_repaint());
        }
        // Drain the shelf (copy staged items into the active pane).
        if std::mem::take(&mut self.ws.drain_request) {
            let c = ctx.clone();
            let outcome = self.ws.drain_shelf(move || c.request_repaint());
            if outcome.unavailable > 0 {
                let now = ctx.input(|i| i.time);
                let item = |n: usize| if n == 1 { "item" } else { "items" };
                self.toasts.push(crate::toasts::Toast::new(
                    format!(
                        "{} shelf {} unavailable, kept on the shelf",
                        outcome.unavailable,
                        item(outcome.unavailable)
                    ),
                    crate::toasts::ToastKind::Error,
                    false,
                    now,
                ));
            }
        }
        // Cycle the list density (toward Spacious; wraps).
        if std::mem::take(&mut self.ws.cycle_density_request) {
            self.density = crate::density::cycle(self.density, 1);
        }
        // Copy the selection's path(s) to the clipboard in the requested style.
        if let Some(style) = self.ws.clipboard_request.take() {
            let paths: Vec<std::path::PathBuf> = self
                .ws
                .active_panel_ref()
                .selected_or_cursor()
                .into_iter()
                .map(|e| e.path)
                .collect();
            if !paths.is_empty() {
                let other_root = self.ws.inactive_panel().current_path.clone();
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
        // Copy an arbitrary text payload (e.g. an exported listing).
        if let Some((text, label)) = self.ws.clipboard_text_request.take() {
            ctx.copy_text(text);
            let now = ctx.input(|i| i.time);
            self.toasts.push(crate::toasts::Toast::new(
                format!("Copied {label}"),
                crate::toasts::ToastKind::Success,
                false,
                now,
            ));
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
        let quick_context = self.quick_action_context();
        let quick_actions = crate::quick_actions::actions(quick_context);
        let next_hint = crate::quick_actions::next_hint(quick_context);
        let max_quick_actions = if ctx.available_rect().width() < 1120.0 {
            2
        } else {
            4
        };
        let mut quick_action: Option<crate::quick_actions::QuickAction> = None;
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
                                let chip = |ui: &mut egui::Ui, label: String, active: bool| {
                                    let fill = if active {
                                        t.accent.linear_multiply(0.14)
                                    } else {
                                        t.bg_card
                                    };
                                    let color = if active { t.accent } else { t.text_muted };
                                    Frame::NONE
                                        .fill(fill)
                                        .corner_radius(crate::theme::ROUNDING_SM)
                                        .inner_margin(Margin::symmetric(7, 2))
                                        .show(ui, |ui| {
                                            ui.label(
                                                egui::RichText::new(label).size(10.0).color(color),
                                            );
                                        });
                                };
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
                                ui.add_space(10.0);
                                chip(
                                    ui,
                                    format!("Rows {}", crate::density::short_label(self.density)),
                                    true,
                                );
                                chip(ui, "Tree".to_string(), self.show_tree);
                                chip(ui, "Compare".to_string(), self.show_compare);
                                chip(
                                    ui,
                                    "Hidden".to_string(),
                                    self.ws.active_panel_ref().show_hidden,
                                );
                                if !self.ws.shelf.is_empty() {
                                    chip(ui, format!("Shelf {}", self.ws.shelf.len()), true);
                                }
                                if !quick_actions.is_empty() {
                                    ui.add_space(8.0);
                                    for spec in quick_actions.iter().take(max_quick_actions).rev() {
                                        let fill = match spec.action {
                                            crate::quick_actions::QuickAction::DrainShelf
                                            | crate::quick_actions::QuickAction::AddToShelf => {
                                                t.accent.linear_multiply(0.22)
                                            }
                                            crate::quick_actions::QuickAction::ClearFilters
                                            | crate::quick_actions::QuickAction::ClearSelection => {
                                                t.accent_red.linear_multiply(0.12)
                                            }
                                            crate::quick_actions::QuickAction::FocusMode => {
                                                t.accent_purple.linear_multiply(0.14)
                                            }
                                            _ => t.bg_card,
                                        };
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(&spec.label)
                                                        .size(11.0)
                                                        .color(t.text_primary),
                                                )
                                                .fill(fill)
                                                .corner_radius(crate::theme::ROUNDING_SM),
                                            )
                                            .on_hover_text(spec.hint)
                                            .clicked()
                                        {
                                            quick_action = Some(spec.action);
                                        }
                                    }
                                }
                                if ctx.available_rect().width() >= 1280.0 {
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new(format!("Next: {next_hint}"))
                                            .size(11.0)
                                            .color(t.text_muted),
                                    );
                                }
                            });
                        });
                    });
            });
        if let Some(action) = quick_action {
            self.run_quick_action(action, ctx);
        }
    }

    fn quick_action_context(&self) -> crate::quick_actions::QuickActionContext {
        let active = self.ws.active_panel_ref();
        crate::quick_actions::QuickActionContext {
            selected_count: active.selected.len(),
            shelf_count: self.ws.shelf.len(),
            has_filters: crate::panel::filter_is_active(&active.search_query, &active.facets),
        }
    }

    fn update_focus_mode(&mut self, ctx: &egui::Context) {
        if !self.focus_mode {
            return;
        }
        let exit_focus = ctx.input(|i| {
            crate::focus_mode::should_exit(
                self.focus_started_at,
                i.time,
                i.pointer.delta().length_sq(),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if exit_focus {
            self.focus_mode = false;
        }
    }

    fn run_quick_action(&mut self, action: crate::quick_actions::QuickAction, ctx: &egui::Context) {
        use crate::quick_actions::QuickAction;
        match action {
            QuickAction::AddToShelf => self.ws.execute(crate::command::Command::ShelfAdd),
            QuickAction::DrainShelf => self.ws.execute(crate::command::Command::ShelfDrain),
            QuickAction::CopyNames => {
                self.ws.clipboard_request = Some(crate::clipboard::PathStyle::NameOnly);
            }
            QuickAction::BatchRename => {
                self.ws.execute(crate::command::Command::BeginBatchRename);
            }
            QuickAction::ClearSelection => {
                self.ws.active_panel().selected.clear();
            }
            QuickAction::ClearFilters => {
                let panel = self.ws.active_panel();
                panel.search_query.clear();
                panel.facets = crate::panel::FacetSet::default();
            }
            QuickAction::SaveFilter => self.save_active_filter_as_smart_folder(ctx),
            QuickAction::OpenPalette => self.ws.palette_request = true,
            QuickAction::FindFiles => self.ws.execute(crate::command::Command::BeginFind),
            QuickAction::RecentFolders => self.ws.execute(crate::command::Command::BeginRecent),
            QuickAction::FocusMode => {
                self.focus_mode = true;
                self.focus_started_at = ctx.input(|i| i.time);
            }
        }
    }

    fn save_active_filter_as_smart_folder(&mut self, ctx: &egui::Context) {
        let (name, root, query) = {
            let active = self.ws.active_panel_ref();
            let query = crate::query::from_panel_filter(&active.search_query, &active.facets);
            if query.predicates.is_empty() {
                return;
            }
            let folder = active
                .current_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| active.current_path.display().to_string());
            let descriptor = if active.search_query.trim().is_empty() {
                format!("{} facet(s)", active.facets.active_count())
            } else {
                active.search_query.trim().to_string()
            };
            (
                format!(
                    "Filter: {} - {}",
                    clipped_label(&folder, 24),
                    clipped_label(&descriptor, 32)
                ),
                active.current_path.clone(),
                query,
            )
        };
        self.smart_folders_mut()
            .add(crate::smart_folder::Definition {
                name: name.clone(),
                root,
                query,
            });
        crate::smart_folder::save(self.smart_folders_mut());
        let now = ctx.input(|i| i.time);
        self.toasts.push(crate::toasts::Toast::new(
            format!("Saved smart folder \"{name}\""),
            crate::toasts::ToastKind::Success,
            false,
            now,
        ));
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
            self.ws.drain_request = true;
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
        // Average file size, once more than one item is picked.
        if s.count >= 2 && s.total_bytes > 0 {
            head.push_str(&format!(
                " \u{00b7} avg {}",
                crate::panel::format_size(s.total_bytes / s.count as u64)
            ));
        }
        // Largest/oldest are only meaningful once more than one item is picked.
        if s.count >= 2 {
            let short = |name: &str| -> String {
                const MAX: usize = 18;
                if name.chars().count() > MAX {
                    let head: String = name.chars().take(MAX - 1).collect();
                    format!("{head}\u{2026}")
                } else {
                    name.to_string()
                }
            };
            if let Some((name, sz)) = &s.largest
                && *sz > 0
            {
                head.push_str(&format!(
                    " \u{00b7} largest {} ({})",
                    short(name),
                    crate::panel::format_size(*sz)
                ));
            }
            if let Some((name, _)) = &s.oldest {
                head.push_str(&format!(" \u{00b7} oldest {}", short(name)));
            }
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
        // each panel is tinted against the OTHER panel's entries. Reuse the
        // cached maps while neither panel's entries changed, so compare mode does
        // not rebuild two HashMaps (cloning every name) on every painted frame.
        let cmp_right_gen = self.ws.right.entries_gen();
        let cmp_left_gen = self.ws.left.entries_gen();
        let (left_compare, right_compare) = if self.show_compare {
            match self.compare_cache.take() {
                Some((rg, lg, lmap, rmap)) if rg == cmp_right_gen && lg == cmp_left_gen => {
                    (Some(lmap), Some(rmap))
                }
                _ => (
                    Some(crate::compare::build_compare_map(&self.ws.right.entries)),
                    Some(crate::compare::build_compare_map(&self.ws.left.entries)),
                ),
            }
        } else {
            self.compare_cache = None;
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
                            if let Some(path) = tree_nav {
                                self.tree_expand_to_path(&path);
                                self.ws.active_panel().navigate_to(path);
                            }
                        });
                });
            tree_actual_width = tree_resp.response.rect.width();
        }

        // Compute half from remaining space (after tree)
        let remaining = window_width - tree_actual_width - 6.0;
        let half = remaining / 2.0;
        // Reset panel width when window resizes or tree toggled
        {
            let prev_half: f32 =
                ctx.data_mut(|d| d.get_temp(egui::Id::new("prev_half")).unwrap_or(0.0));
            if self.prev_window_width > 0.0
                && ((window_width - self.prev_window_width).abs() > 1.0
                    || (half - prev_half).abs() > 1.0)
            {
                ctx.data_mut(|d| {
                    d.remove::<egui::containers::panel::PanelState>(panel_id);
                });
            }
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("prev_half"), half));
        }
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
                tree_toggle |= Self::render_panel(
                    &mut self.ws.left,
                    ui,
                    self.ws.active == ActivePanel::Left,
                    &t,
                    &mut self.image_cache,
                    "left",
                    self.show_tree,
                    self.show_size_bars,
                    left_compare.as_ref(),
                    self.ws.opener.as_ref(),
                    metrics,
                );
            });

        // Double-click on panel divider → reset to 50/50
        {
            let panel_rect = left_resp.response.rect;
            let divider_rect = egui::Rect::from_min_max(
                egui::pos2(panel_rect.right() - 4.0, panel_rect.top()),
                egui::pos2(panel_rect.right() + 4.0, panel_rect.bottom()),
            );
            let double_clicked = ctx.input(|i| {
                if let Some(pos) = i.pointer.latest_pos() {
                    divider_rect.contains(pos)
                        && i.pointer
                            .button_double_clicked(egui::PointerButton::Primary)
                } else {
                    false
                }
            });
            if double_clicked {
                ctx.data_mut(|d| {
                    d.remove::<egui::containers::panel::PanelState>(panel_id);
                });
            }
        }

        // Right panel (takes remaining space)
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(t.bg_deep).inner_margin(Margin::same(0)))
            .show(ctx, |ui| {
                if ui.rect_contains_pointer(ui.max_rect()) && ctx.input(|i| i.pointer.any_pressed())
                {
                    self.ws.active = ActivePanel::Right;
                }
                tree_toggle |= Self::render_panel(
                    &mut self.ws.right,
                    ui,
                    self.ws.active == ActivePanel::Right,
                    &t,
                    &mut self.image_cache,
                    "right",
                    self.show_tree,
                    self.show_size_bars,
                    right_compare.as_ref(),
                    self.ws.opener.as_ref(),
                    metrics,
                );
            });

        if tree_toggle {
            self.show_tree = !self.show_tree;
            if self.show_tree {
                let path = self.ws.active_panel().current_path.clone();
                self.tree_expand_to_path(&path);
            }
        }

        // Stash the freshly-used compare maps (keyed by the generations they were
        // built from) so the next frame reuses them while the entries are
        // unchanged.
        if let (Some(lmap), Some(rmap)) = (left_compare, right_compare) {
            self.compare_cache = Some((cmp_right_gen, cmp_left_gen, lmap, rmap));
        }
    }

    /// Floating label with the dragged file count next to the pointer.
    fn show_drag_overlay(&mut self, ctx: &egui::Context) {
        let t = self.colors;
        let drag_entries = if !self.ws.left.drag_entries.is_empty() {
            &self.ws.left.drag_entries
        } else if !self.ws.right.drag_entries.is_empty() {
            &self.ws.right.drag_entries
        } else {
            return;
        };
        let (source, other) = if !self.ws.left.drag_entries.is_empty() {
            (&self.ws.left, &self.ws.right)
        } else {
            (&self.ws.right, &self.ws.left)
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
            let explicit_target = source.drop_target.as_ref().or(other.drop_target.as_ref());
            let target = explicit_target.unwrap_or(&other.current_path);
            let target_name = target
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| target.display().to_string());
            let target_label = if explicit_target.is_some() {
                format!("Drop into {target_name}")
            } else {
                format!("Drop to other panel: {target_name}")
            };
            egui::Area::new(egui::Id::new("drag_overlay"))
                .fixed_pos(pos + egui::vec2(12.0, 12.0))
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .inner_margin(Margin::symmetric(9, 6))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(label)
                                    .size(12.0)
                                    .strong()
                                    .color(t.text_primary),
                            );
                            ui.label(
                                egui::RichText::new(target_label)
                                    .size(10.0)
                                    .color(t.text_muted),
                            );
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
                crate::toasts::ToastKind::Info => t.text_muted,
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
            self.ws.undo_request = true;
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
}
