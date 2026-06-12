use super::*;

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
        self.left.poll_fs_changes();
        self.right.poll_fs_changes();
        self.handle_keys(ctx);
        self.preload_images(ctx);
        self.poll_transfer();
        let t = self.colors;

        // Transfer progress dialog
        if let Some(ref state) = self.active_transfer {
            let s = state.lock().unwrap();
            let progress_frac = if s.total_bytes > 0 {
                s.copied_bytes as f32 / s.total_bytes as f32
            } else {
                0.0
            };
            let file_frac = if s.current_file_size > 0 {
                s.current_file_copied as f32 / s.current_file_size as f32
            } else {
                0.0
            };
            let speed = s.speed_bps();
            let eta = s.eta_secs();
            let current_file = s.current_file.clone();
            let current_file_copied = s.current_file_copied;
            let current_file_size = s.current_file_size;
            let files_done = s.files_done;
            let files_total = s.files_total;
            let copied = s.copied_bytes;
            let total = s.total_bytes;
            let samples: Vec<(f64, f64)> = s.speed_samples.clone();
            let finished = s.finished;
            drop(s);

            egui::Window::new(if finished { "Transfer Complete" } else { "Transferring..." })
                .collapsible(false)
                .resizable(false)
                .default_width(450.0)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    // Current file name
                    ui.label(
                        egui::RichText::new(format!("File: {}", current_file))
                            .size(12.0).color(t.text_primary),
                    );

                    // Current file progress bar (no rounding)
                    ui.add_space(4.0);
                    Self::draw_progress_bar(ui, file_frac, &format!(
                        "{} / {}",
                        format_size(current_file_copied),
                        format_size(current_file_size),
                    ), t.accent, &t);

                    // Total progress bar (no rounding)
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new("Total:")
                            .size(11.0).color(t.text_muted),
                    );
                    ui.add_space(2.0);
                    Self::draw_progress_bar(ui, progress_frac, &format!(
                        "{} / {} ({:.0}%)",
                        format_size(copied),
                        format_size(total),
                        progress_frac * 100.0,
                    ), t.accent.linear_multiply(0.7), &t);

                    // Speed + ETA + files
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!(
                            "{}/s", format_size(speed as u64)
                        )).size(11.0).color(t.text_muted));

                        ui.add_space(16.0);
                        if eta > 0.0 && !finished {
                            let mins = (eta / 60.0) as u64;
                            let secs = (eta % 60.0) as u64;
                            ui.label(egui::RichText::new(format!(
                                "ETA: {}:{:02}", mins, secs
                            )).size(11.0).color(t.text_muted));
                        }

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(egui::RichText::new(format!(
                                "{}/{} files", files_done, files_total
                            )).size(11.0).color(t.text_muted));
                        });
                    });

                    // Speed graph
                    if samples.len() > 2 {
                        ui.add_space(8.0);
                        let graph_h = 60.0;
                        let (rect, _) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), graph_h),
                            Sense::hover(),
                        );

                        // Compute per-sample speed
                        let mut speeds: Vec<f64> = Vec::new();
                        for i in 1..samples.len() {
                            let dt = samples[i].0 - samples[i - 1].0;
                            let db = samples[i].1 - samples[i - 1].1;
                            if dt > 0.01 {
                                speeds.push(db / dt);
                            } else {
                                speeds.push(0.0);
                            }
                        }

                        let max_speed = speeds.iter().cloned().fold(1.0f64, f64::max);
                        let p = ui.painter();

                        // Background
                        p.rect_filled(rect, CornerRadius::same(3), t.bg_card.linear_multiply(0.3));

                        // Draw speed line
                        if speeds.len() >= 2 {
                            let n = speeds.len();
                            let points: Vec<egui::Pos2> = speeds.iter().enumerate().map(|(i, &s)| {
                                let x = rect.left() + (i as f32 / (n - 1) as f32) * rect.width();
                                let y = rect.bottom() - (s as f32 / max_speed as f32) * rect.height() * 0.9;
                                egui::pos2(x, y)
                            }).collect();

                            for w in points.windows(2) {
                                p.line_segment([w[0], w[1]], Stroke::new(1.5, t.accent));
                            }
                        }

                        // Max speed label
                        p.text(
                            egui::pos2(rect.left() + 4.0, rect.top() + 2.0),
                            egui::Align2::LEFT_TOP,
                            format!("{}/s", format_size(max_speed as u64)),
                            egui::FontId::proportional(9.0),
                            t.text_muted,
                        );
                    }

                    ui.add_space(8.0);
                    if finished {
                        if ui.add(
                            egui::Button::new(egui::RichText::new("OK").size(13.0).color(Color32::WHITE))
                                .fill(t.accent).corner_radius(CornerRadius::ZERO),
                        ).clicked() {
                            self.active_transfer = None;
                            self.left.refresh();
                            self.right.refresh();
                        }
                    } else {
                        if ui.add(
                            egui::Button::new(egui::RichText::new("Cancel").size(13.0).color(Color32::WHITE))
                                .fill(t.accent_red).corner_radius(CornerRadius::ZERO),
                        ).clicked() {
                            self.cancel_transfer();
                        }
                    }
                });

            // Keep repainting during transfer
            if !finished {
                ctx.request_repaint();
            }
        }

        // Universal confirmation dialog for file operations
        if self.pending_op.is_some() {
            let op = self.pending_op.clone().unwrap();
            let (title, action_label, action_color, entries, target_pb, conflicts, flat_arc) = match &op {
                PendingOp::Copy { entries, target, conflicts, flat, .. } => (
                    "Copy", "Copy", t.accent, entries.clone(), Some(target.clone()), conflicts.clone(), flat.clone(),
                ),
                PendingOp::Move { entries, target, conflicts, flat, .. } => (
                    "Move", "Move", Color32::from_rgb(230, 160, 40), entries.clone(), Some(target.clone()), conflicts.clone(), flat.clone(),
                ),
                PendingOp::Delete { entries, flat } => (
                    "Delete", "Move to Trash", t.accent_red, entries.clone(), None, vec![], flat.clone(),
                ),
            };
            let flat_opt = flat_arc.lock().unwrap().clone();
            let flat_ready = flat_opt.is_some();
            let flat = flat_opt.unwrap_or_default();

            let has_conflicts = !conflicts.is_empty();
            let is_delete = target_pb.is_none();
            let win_title = format!("{} — {} item(s)", title, entries.len());
            let screen = ctx.screen_rect();
            let pad = 120.0;
            let avail_w = (screen.width() - pad * 2.0).max(300.0);
            let avail_h = (screen.height() - pad * 2.0).max(200.0);
            let win_w = 1000.0f32.min(avail_w);
            let win_h = 800.0f32.min(avail_h);

            egui::Window::new(win_title)
                .collapsible(false)
                .resizable(false)
                .fixed_size(Vec2::new(win_w, win_h))
                .title_bar(false)
                .frame(Frame::NONE
                    .fill(t.bg_panel)
                    .inner_margin(Margin::same(6))
                    .stroke(Stroke::new(1.0, t.border))
                )
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    // Title + tabs on one line
                    if is_delete {
                        ui.label(egui::RichText::new(format!("Delete — {} item(s)", entries.len())).size(14.0).strong().color(t.text_primary));
                        ui.add_space(6.0);
                    } else {
                        let cur_method = match &self.pending_op {
                            Some(PendingOp::Copy { method, .. }) => *method,
                            Some(PendingOp::Move { method, .. }) => *method,
                            _ => CopyMethod::Native,
                        };

                        let row_h = 28.0;
                        let full_w = ui.available_width();
                        let (row_rect, _) = ui.allocate_exact_size(Vec2::new(full_w, row_h), Sense::hover());
                        let p = ui.painter();

                        // Bottom line across full width
                        p.line_segment(
                            [egui::pos2(row_rect.left(), row_rect.bottom()),
                             egui::pos2(row_rect.right(), row_rect.bottom())],
                            Stroke::new(1.0, t.border),
                        );

                        // Title on the left
                        p.text(
                            egui::pos2(row_rect.left() + 4.0, row_rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            format!("{} — {} item(s)", title, entries.len()),
                            egui::FontId::proportional(13.0),
                            t.text_primary,
                        );

                        // Tabs on the right
                        let tabs: &[(&str, CopyMethod)] = &[
                            ("Native", CopyMethod::Native),
                            ("Buffered", CopyMethod::Buffered),
                        ];
                        let tab_w = 90.0;
                        let tabs_total_w = tab_w * tabs.len() as f32;
                        let tabs_left = row_rect.right() - tabs_total_w;

                        let mut clicked_method: Option<CopyMethod> = None;

                        for (i, &(label, method)) in tabs.iter().enumerate() {
                            let active = cur_method == method;
                            let tab_rect = egui::Rect::from_min_size(
                                egui::pos2(tabs_left + i as f32 * tab_w, row_rect.top()),
                                Vec2::new(tab_w, row_h),
                            );

                            if active {
                                p.rect_filled(
                                    tab_rect,
                                    CornerRadius::ZERO,
                                    t.bg_panel,
                                );
                                // Left border
                                p.line_segment(
                                    [egui::pos2(tab_rect.left(), tab_rect.bottom()),
                                     egui::pos2(tab_rect.left(), tab_rect.top())],
                                    Stroke::new(1.0, t.border),
                                );
                                // Top border
                                p.line_segment(
                                    [egui::pos2(tab_rect.left(), tab_rect.top()),
                                     egui::pos2(tab_rect.right(), tab_rect.top())],
                                    Stroke::new(1.0, t.border),
                                );
                                // Right border
                                p.line_segment(
                                    [egui::pos2(tab_rect.right(), tab_rect.top()),
                                     egui::pos2(tab_rect.right(), tab_rect.bottom())],
                                    Stroke::new(1.0, t.border),
                                );
                                // Cover bottom line
                                p.line_segment(
                                    [egui::pos2(tab_rect.left() + 1.0, tab_rect.bottom()),
                                     egui::pos2(tab_rect.right() - 1.0, tab_rect.bottom())],
                                    Stroke::new(2.0, t.bg_panel),
                                );
                            }

                            let fg = if active { t.text_primary } else { t.text_muted };
                            p.text(
                                tab_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                label,
                                egui::FontId::proportional(12.0),
                                fg,
                            );

                            // Click detection
                            let tab_resp = ui.interact(tab_rect, ui.id().with(format!("tab_{}", i)), Sense::click());
                            if tab_resp.clicked() {
                                clicked_method = Some(method);
                            }
                            if tab_resp.hovered() && !active {
                                p.rect_filled(
                                    tab_rect,
                                    CornerRadius::ZERO,
                                    t.bg_hover.linear_multiply(0.2),
                                );
                            }
                        }

                        if let Some(method) = clicked_method {
                            match &mut self.pending_op {
                                Some(PendingOp::Copy { method: m, .. }) => *m = method,
                                Some(PendingOp::Move { method: m, .. }) => *m = method,
                                _ => {}
                            }
                        }

                        ui.add_space(8.0);
                    }

                    if !flat_ready {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new("Scanning files...").size(12.0).color(t.text_muted));
                        });
                        ctx.request_repaint();
                    } else if is_delete {
                        // ── Delete: single list (virtualized) ──
                        let list_h = (ui.available_height() - 80.0).max(60.0);
                        Self::render_flat_list_virtual(ui, &flat, &[], &t, list_h, "pending_op_files");
                    } else {
                        // ── Copy/Move: two columns (source → destination) ──
                        let target_path = target_pb.as_ref().unwrap();
                        let source_dir = entries.first()
                            .and_then(|e| e.path.parent())
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();

                        // Headers
                        ui.columns(2, |cols| {
                            cols[0].label(egui::RichText::new(format!("Source: {}", source_dir)).size(11.0).color(t.text_muted));
                            cols[1].label(egui::RichText::new(format!("Destination: {}", target_path.display())).size(11.0).color(t.text_muted));
                        });
                        ui.add_space(4.0);

                        // Animated flow: files gradually transfer from left to right
                        let anim_id = egui::Id::new("pending_flow_start");
                        let start_time: f64 = ctx.data_mut(|d| {
                            *d.get_temp_mut_or_insert_with(anim_id, || ctx.input(|i| i.time))
                        });
                        let elapsed = ctx.input(|i| i.time) - start_time;
                        // Transfer one file every 80ms, all done in ~N*80ms
                        let transferred = ((elapsed / 0.08) as usize).min(flat.len());

                        // Request repaint while animation is running
                        if transferred < flat.len() {
                            ctx.request_repaint();
                        }

                        ui.columns(2, |cols| {
                            let list_h = (cols[0].available_height() - 80.0).max(60.0);

                            // Left: source tree — transferred files dimmed+strikethrough, rest normal
                            Self::render_flat_list_animated(&mut cols[0], &flat, &conflicts, &t, list_h, "pending_src", transferred, true);

                            // Right: only transferred files shown, highlighted
                            let list_h = (cols[1].available_height() - 80.0).max(60.0);
                            Self::render_flat_list_animated(&mut cols[1], &flat, &conflicts, &t, list_h, "pending_dst", transferred, false);
                        });
                    }

                    // Total size (from flat list, files only)
                    let total: u64 = flat.iter().filter(|f| !f.is_dir).map(|f| f.size).sum();
                    if total > 0 {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(format!("Total: {}", format_size(total))).size(11.0).color(t.text_muted));
                    }

                    // Conflict warning + overwrite policy buttons
                    if has_conflicts {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(format!("  {} file(s) already exist at destination", conflicts.len()))
                                .size(12.0).color(Color32::from_rgb(230, 160, 40)),
                        );
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.add(
                                egui::Button::new(egui::RichText::new("Overwrite All").size(12.0).color(Color32::WHITE))
                                    .fill(Color32::from_rgb(200, 120, 30))
                                    .corner_radius(CornerRadius::ZERO),
                            ).clicked() {
                                self.set_pending_policy(OverwritePolicy::OverwriteAll);
                                self.confirm_pending_op(ctx);
                            }
                            ui.add_space(4.0);
                            if ui.add(
                                egui::Button::new(egui::RichText::new("Skip Existing").size(12.0).color(t.text_primary))
                                    .fill(t.bg_card)
                                    .corner_radius(CornerRadius::ZERO),
                            ).clicked() {
                                self.set_pending_policy(OverwritePolicy::SkipAll);
                                self.confirm_pending_op(ctx);
                            }
                        });
                    }

                    ui.add_space(12.0);

                    // Action buttons
                    ui.horizontal(|ui| {
                        if ui.add(
                            egui::Button::new(egui::RichText::new("Cancel").size(13.0).color(t.text_primary))
                                .fill(t.bg_card).corner_radius(CornerRadius::ZERO),
                        ).clicked() {
                            self.pending_op = None;
                            ctx.data_mut(|d| {
                                d.remove::<Vec<crate::app::file_ops::FlatFileEntry>>(egui::Id::new("dst_flat_cache"));
                                d.remove::<Vec<crate::app::file_ops::FlatFileEntry>>(egui::Id::new("src_remaining_cache"));
                                d.remove::<f64>(egui::Id::new("pending_flow_start"));
                            });
                        }
                        if !has_conflicts {
                            ui.add_space(8.0);
                            if ui.add(
                                egui::Button::new(egui::RichText::new(action_label).size(13.0).color(Color32::WHITE))
                                    .fill(action_color).corner_radius(CornerRadius::ZERO),
                            ).clicked() {
                                self.confirm_pending_op(ctx);
                            }
                        }
                    });

                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        self.pending_op = None;
                            ctx.data_mut(|d| {
                                d.remove::<Vec<crate::app::file_ops::FlatFileEntry>>(egui::Id::new("dst_flat_cache"));
                                d.remove::<Vec<crate::app::file_ops::FlatFileEntry>>(egui::Id::new("src_remaining_cache"));
                                d.remove::<f64>(egui::Id::new("pending_flow_start"));
                            });
                    }
                    if !has_conflicts && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.confirm_pending_op(ctx);
                    }
                });
        }

        // Toolbar
        egui::TopBottomPanel::top("toolbar")
            .frame(Frame::NONE.fill(t.bg_toolbar))
            .show(ctx, |ui| {
                self.toolbar(ui, ctx);
            });

        // Bottom shortcut bar
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

        let window_width = ctx.screen_rect().width();
        let panel_id = egui::Id::new("left_panel");

        // Sync tree toggle from panels
        if self.left.show_tree != self.show_tree || self.right.show_tree != self.show_tree {
            if self.left.show_tree != self.show_tree {
                self.show_tree = self.left.show_tree;
            } else {
                self.show_tree = self.right.show_tree;
            }
            self.left.show_tree = self.show_tree;
            self.right.show_tree = self.show_tree;
            if self.show_tree {
                let path = match self.active {
                    ActivePanel::Left => self.left.current_path.clone(),
                    ActivePanel::Right => self.right.current_path.clone(),
                };
                self.tree_expand_to_path(&path);
            }
        }

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
                Self::render_panel(&mut self.left, ui, self.active == ActivePanel::Left, &t, &mut self.image_cache, "left");
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
                Self::render_panel(&mut self.right, ui, self.active == ActivePanel::Right, &t, &mut self.image_cache, "right");
            });

        // Drag overlay — show floating label with dragged file count
        let dragging = !self.left.drag_entries.is_empty() || !self.right.drag_entries.is_empty();
        if dragging {
            let drag_entries = if !self.left.drag_entries.is_empty() {
                &self.left.drag_entries
            } else {
                &self.right.drag_entries
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

        // Handle drop — move files to target directory
        if ctx.input(|i| i.pointer.any_released()) {
            // Check left panel drop
            if !self.left.drag_entries.is_empty() {
                let target = self.right.drop_target.take()
                    .unwrap_or_else(|| self.right.current_path.clone());
                for src in &self.left.drag_entries {
                    if let Some(name) = src.file_name() {
                        let dest = target.join(name);
                        let _ = std::fs::rename(src, &dest);
                    }
                }
                self.left.drag_entries.clear();
                self.left.refresh();
                self.right.refresh();
            }
            // Check right panel drop
            if !self.right.drag_entries.is_empty() {
                let target = self.left.drop_target.take()
                    .unwrap_or_else(|| self.left.current_path.clone());
                for src in &self.right.drag_entries {
                    if let Some(name) = src.file_name() {
                        let dest = target.join(name);
                        let _ = std::fs::rename(src, &dest);
                    }
                }
                self.right.drag_entries.clear();
                self.left.refresh();
                self.right.refresh();
            }
            self.left.drop_target = None;
            self.right.drop_target = None;
        }

        // Handle pending context menu actions
        let action = match self.active {
            ActivePanel::Left => self.left.pending_action.take(),
            ActivePanel::Right => self.right.pending_action.take(),
        };
        if let Some(action) = action {
            use crate::panel::ContextAction;
            match action {
                ContextAction::Open(path) => { let _ = open::that(&path); }
                ContextAction::RevealInFinder(path) => {
                    let _ = std::process::Command::new("open").arg("-R").arg(&path).spawn();
                }
                ContextAction::CopySelected => self.request_copy(),
                ContextAction::MoveSelected => self.request_move(),
                ContextAction::DeleteSelected => self.request_delete(),
            }
        }
    }
}
