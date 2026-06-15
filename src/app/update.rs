//! Per-frame orchestration: each concern lives in its own method/module.

use super::*;

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
        self.show_palette_dialog(ctx);
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
            || self.ws.left.preview.is_some()
            || self.ws.right.preview.is_some();
        if has_animation {
            ctx.request_repaint();
        }

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
        // Drain the shelf (copy staged items into the active pane).
        if std::mem::take(&mut self.ws.drain_request) {
            let c = ctx.clone();
            self.ws.drain_shelf(move || c.request_repaint());
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
                Some(crate::workspace::build_compare_map(&self.ws.right.entries)),
                Some(crate::workspace::build_compare_map(&self.ws.left.entries)),
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
