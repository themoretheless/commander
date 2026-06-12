//! Per-frame orchestration: each concern lives in its own method/module.

use super::*;

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.begin_frame(ctx);
        self.show_transfer_dialog(ctx);
        self.show_confirm_dialog(ctx);
        self.show_toolbar_panel(ctx);
        self.show_shortcut_bar(ctx);
        self.show_main_area(ctx);
        self.show_drag_overlay(ctx);
        self.handle_drop(ctx);
    }
}

impl App {
    /// Frame bookkeeping: repaint heuristics, context wiring, fs polling,
    /// input handling and background-task polling.
    fn begin_frame(&mut self, ctx: &egui::Context) {
        // Repaint only when there's activity (scroll animation, background loads)
        // egui will auto-repaint on user input (mouse, keyboard)
        let has_animation = ctx.is_using_pointer()
            || ctx.input(|i| i.smooth_scroll_delta.length() > 0.0)
            || self.left.preview.is_some()
            || self.right.preview.is_some();
        if has_animation {
            ctx.request_repaint();
        }

        if self.left.ctx.is_none() {
            self.left.set_ctx(ctx.clone());
            self.left.refresh();
        }
        if self.right.ctx.is_none() {
            self.right.set_ctx(ctx.clone());
            self.right.refresh();
        }

        let fs_changed = self.left.poll_fs_changes() | self.right.poll_fs_changes();
        if fs_changed {
            self.tree_children_cache.clear();
        }

        // Drop targets are only valid for the frame that set them
        // (rows re-assert them while hovered during render).
        self.left.drop_target = None;
        self.right.drop_target = None;

        self.handle_keys(ctx);
        self.preload_images(ctx);
        self.poll_transfer();
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
                                    egui::RichText::new(key)
                                        .size(11.0)
                                        .strong()
                                        .color(t.accent),
                                );
                                ui.label(
                                    egui::RichText::new(action)
                                        .size(11.0)
                                        .color(t.text_muted),
                                );
                                ui.add_space(8.0);
                            }

                            // Scale slider on the right
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add_space(12.0);
                                ui.label(
                                    egui::RichText::new(format!("{}%", (self.ui_scale * 100.0) as u32))
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

    /// Tree sidebar plus the two file panels with the resizable divider.
    fn show_main_area(&mut self, ctx: &egui::Context) {
        let t = self.colors;
        let window_width = ctx.screen_rect().width();
        let panel_id = egui::Id::new("left_panel");

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
                                self.active_panel().navigate_to(path);
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
            let prev_half: f32 = ctx.data_mut(|d| d.get_temp(egui::Id::new("prev_half")).unwrap_or(0.0));
            if self.prev_window_width > 0.0 && ((window_width - self.prev_window_width).abs() > 1.0 || (half - prev_half).abs() > 1.0) {
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
                if ui.rect_contains_pointer(ui.max_rect()) && ctx.input(|i| i.pointer.any_pressed()) {
                    self.active = ActivePanel::Left;
                }
                tree_toggle |= Self::render_panel(
                    &mut self.left, ui, self.active == ActivePanel::Left,
                    &t, &mut self.image_cache, "left", self.show_tree,
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
                        && i.pointer.button_double_clicked(egui::PointerButton::Primary)
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
                if ui.rect_contains_pointer(ui.max_rect()) && ctx.input(|i| i.pointer.any_pressed()) {
                    self.active = ActivePanel::Right;
                }
                tree_toggle |= Self::render_panel(
                    &mut self.right, ui, self.active == ActivePanel::Right,
                    &t, &mut self.image_cache, "right", self.show_tree,
                );
            });

        if tree_toggle {
            self.show_tree = !self.show_tree;
            if self.show_tree {
                let path = self.active_panel().current_path.clone();
                self.tree_expand_to_path(&path);
            }
        }
    }

    /// Floating label with the dragged file count next to the pointer.
    fn show_drag_overlay(&mut self, ctx: &egui::Context) {
        let t = self.colors;
        let drag_entries = if !self.left.drag_entries.is_empty() {
            &self.left.drag_entries
        } else if !self.right.drag_entries.is_empty() {
            &self.right.drag_entries
        } else {
            return;
        };
        let count = drag_entries.len();
        if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
            let label = if count == 1 {
                drag_entries[0].file_name()
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
                            ui.label(
                                egui::RichText::new(label)
                                    .size(12.0)
                                    .color(t.text_primary),
                            );
                        });
                });
        }
        ctx.request_repaint();
    }

    /// On mouse release, move dragged files into the hovered directory.
    fn handle_drop(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.pointer.any_released()) {
            return;
        }
        Self::drop_into(&mut self.left, &mut self.right);
        Self::drop_into(&mut self.right, &mut self.left);
        self.left.drop_target = None;
        self.right.drop_target = None;
    }

    /// Drop `source`'s dragged entries. A target hovered in the source panel
    /// itself (drag onto own subdirectory) takes priority over the other panel.
    fn drop_into(source: &mut PanelState, other: &mut PanelState) {
        if source.drag_entries.is_empty() {
            return;
        }
        let target = source.drop_target.take()
            .or_else(|| other.drop_target.take())
            .unwrap_or_else(|| other.current_path.clone());
        for src in &source.drag_entries {
            if let Some(name) = src.file_name() {
                let _ = std::fs::rename(src, target.join(name));
            }
        }
        source.drag_entries.clear();
        source.refresh();
        other.refresh();
    }
}
