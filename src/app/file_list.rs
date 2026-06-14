use super::*;

impl App {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_file_list(
        ui: &mut egui::Ui,
        panel: &mut PanelState,
        is_active: bool,
        t: &ThemeColors,
        panel_side: &str,
        size_bars: bool,
        compare: Option<&crate::workspace::CompareMap>,
        opener: &dyn Fn(&std::path::Path),
    ) {
        egui::ScrollArea::vertical()
            .id_salt(format!("file_list_{}", panel_side))
            .auto_shrink([false; 2])
            .animated(true)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;

                // ".." row — go up one directory (cursor == 0)
                let can_go_up = panel.current_path.parent().is_some();
                if can_go_up {
                    let is_cursor_on_up = panel.cursor == 0;
                    let up_bg = if is_cursor_on_up && is_active {
                        t.bg_selected.linear_multiply(0.25)
                    } else {
                        t.bg_card.linear_multiply(0.3)
                    };
                    let up_row = Frame::NONE
                        .fill(up_bg)
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::ZERO)
                        .inner_margin(Margin::symmetric(10, 4))
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new("\u{2ba4}").size(14.0).color(t.accent),
                                );
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new("..")
                                        .size(13.0)
                                        .strong()
                                        .color(t.text_primary),
                                );
                            });
                        })
                        .response
                        .interact(Sense::click());

                    if up_row.double_clicked() {
                        panel.go_up();
                    } else if up_row.clicked() {
                        panel.cursor = 0;
                    }
                    if up_row.hovered() && !is_cursor_on_up {
                        ui.painter().rect_filled(
                            up_row.rect,
                            CornerRadius::same(3),
                            t.bg_hover.linear_multiply(0.3),
                        );
                    }
                }

                // Cached filtered view: indices into panel.entries.
                // No FileEntry is cloned per frame; row interactions
                // are recorded and applied after the loop, so the
                // loop body only borrows the panel immutably.
                let filtered = panel.filtered_indices();

                if filtered.is_empty() {
                    ui.add_space(40.0);
                    ui.with_layout(Layout::top_down(Align::Center), |ui| {
                        ui.label(egui::RichText::new("Empty").size(14.0).color(t.text_muted));
                    });
                    return;
                }

                // Deferred row interactions (applied after the loop).
                let mut pending_cursor: Option<usize> = None;
                let mut navigate_to: Option<std::path::PathBuf> = None;
                let mut open_path: Option<std::path::PathBuf> = None;
                let mut drag_anchor: Option<std::path::PathBuf> = None;
                let mut pending_drop_target: Option<std::path::PathBuf> = None;
                let mut ctx_refresh = false;
                let mut scrolled = false;

                // Lock shared data once for all rows (clone Arc to avoid borrowing panel)
                let counts_arc = std::sync::Arc::clone(&panel.dir_counts);
                let sizes_arc = std::sync::Arc::clone(&panel.dir_sizes);
                let dir_counts = counts_arc.lock().ok();
                let dir_sizes = sizes_arc.lock().ok();

                let cursor = panel.cursor;
                let scroll_pending = panel.scroll_to_cursor;
                let dragging = !panel.drag_entries.is_empty();

                let row_h = 29.0; // 28 + 1 spacing
                let total_rows = filtered.len();
                let viewport = ui.clip_rect();
                let scroll_top = viewport.top() - ui.min_rect().top();

                // Feed the visible-row count back to the core for PageUp/Down.
                panel.page_rows = ((viewport.height() / row_h).floor() as usize).max(1);

                // Largest entry size in the listing, used to scale occupancy
                // bars. Computed once with the size map already locked above.
                let size_max: u64 = if size_bars {
                    dir_sizes
                        .as_ref()
                        .map(|sizes| {
                            filtered
                                .iter()
                                .map(|&i| {
                                    crate::panel::entry_display_size(&panel.entries[i], sizes)
                                })
                                .max()
                                .unwrap_or(0)
                        })
                        .unwrap_or(0)
                } else {
                    0
                };

                // Which rows are visible
                let mut first_visible = ((scroll_top / row_h).floor() as usize).min(total_rows);
                let mut last_visible =
                    ((scroll_top + viewport.height()) / row_h).ceil() as usize + 1;
                last_visible = last_visible.min(total_rows);

                // Ensure cursor row is in visible range when keyboard scrolling
                let cursor_file_idx = if cursor > 0 { cursor - 1 } else { 0 };
                if scroll_pending && total_rows > 0 {
                    if cursor_file_idx < first_visible {
                        first_visible = cursor_file_idx;
                        last_visible =
                            (first_visible + ((viewport.height() / row_h).ceil() as usize) + 2)
                                .min(total_rows);
                    } else if cursor_file_idx >= last_visible {
                        last_visible = (cursor_file_idx + 1).min(total_rows);
                        let visible_count = ((viewport.height() / row_h).ceil() as usize) + 2;
                        first_visible = last_visible.saturating_sub(visible_count);
                    }
                }

                // Space before visible rows
                if first_visible > 0 {
                    ui.allocate_space(Vec2::new(
                        ui.available_width(),
                        first_visible as f32 * row_h,
                    ));
                }

                // Render only visible rows
                for (offset, &entry_idx) in filtered[first_visible..last_visible].iter().enumerate()
                {
                    let idx = first_visible + offset;
                    let entry = &panel.entries[entry_idx];
                    let row_cursor = idx + 1;
                    let is_cursor = row_cursor == cursor;
                    let is_selected = panel.selected.contains(&entry.path);

                    let zebra = if idx % 2 == 1 {
                        t.bg_card.linear_multiply(0.3)
                    } else {
                        Color32::TRANSPARENT
                    };

                    let bg = if is_cursor && is_active {
                        t.bg_selected.linear_multiply(0.25)
                    } else if is_selected {
                        t.accent_purple.linear_multiply(0.15)
                    } else {
                        zebra
                    };

                    let (row_rect, row_resp) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 28.0),
                        Sense::click_and_drag(),
                    );

                    // Scroll to cursor row when navigating with keyboard
                    if is_cursor && scroll_pending {
                        ui.scroll_to_rect(row_rect, Some(Align::Center));
                        scrolled = true;
                    }

                    // Paint background
                    let full_rect =
                        egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), row_rect.y_range());
                    if bg != Color32::TRANSPARENT {
                        ui.painter().rect_filled(full_rect, CornerRadius::ZERO, bg);
                    }
                    if row_resp.hovered() && !is_cursor {
                        ui.painter().rect_filled(
                            full_rect,
                            CornerRadius::ZERO,
                            t.bg_hover.linear_multiply(0.3),
                        );
                    }

                    // Occupancy bar: width proportional to this entry's share
                    // of the largest entry, ramping to a warning tint when it
                    // dominates the directory.
                    if size_max > 0 {
                        let size = dir_sizes
                            .as_ref()
                            .map(|s| crate::panel::entry_display_size(entry, s))
                            .unwrap_or(0);
                        if size > 0 {
                            let frac = (size as f32 / size_max as f32).clamp(0.0, 1.0);
                            let bar = egui::Rect::from_min_size(
                                full_rect.min,
                                Vec2::new(full_rect.width() * frac, full_rect.height()),
                            );
                            let tint = if frac > 0.66 {
                                t.accent_warning
                            } else {
                                t.accent
                            };
                            ui.painter().rect_filled(
                                bar,
                                CornerRadius::ZERO,
                                tint.linear_multiply(0.12),
                            );
                        }
                    }

                    // Compare mode: a left-edge stripe showing how this entry
                    // relates to the other panel.
                    if let Some(map) = compare {
                        use crate::workspace::CompareStatus;
                        let stripe = match crate::workspace::classify_entry(entry, map) {
                            CompareStatus::Unique => Some(t.accent),
                            CompareStatus::Differs => Some(t.accent_warning),
                            CompareStatus::Identical => None,
                        };
                        if let Some(color) = stripe {
                            let edge = egui::Rect::from_min_size(
                                full_rect.min,
                                Vec2::new(3.0, full_rect.height()),
                            );
                            ui.painter().rect_filled(edge, CornerRadius::ZERO, color);
                        }
                    }

                    // Content
                    let mut child_ui = ui.new_child(
                        egui::UiBuilder::new().max_rect(row_rect.shrink2(egui::vec2(6.0, 3.0))),
                    );
                    child_ui.style_mut().interaction.selectable_labels = false;
                    child_ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;

                        if entry.is_dir {
                            let count = dir_counts
                                .as_ref()
                                .and_then(|c| c.get(&entry.path).copied());
                            Self::paint_folder_icon(ui, count);
                        } else {
                            ui.add_space(3.0);
                            ui.label(egui::RichText::new(entry.icon()).size(14.0));
                        }
                        ui.add_space(3.0);

                        let name_color = if is_selected {
                            t.accent_purple
                        } else if entry.is_dir {
                            t.text_primary
                        } else {
                            t.text_secondary
                        };
                        ui.label(
                            egui::RichText::new(&entry.name)
                                .size(13.0)
                                .color(name_color),
                        );

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(entry.modified_display())
                                    .size(11.0)
                                    .color(t.text_muted),
                            );
                            ui.add_space(16.0);
                            let size_text = if let Some(ref sizes) = dir_sizes {
                                entry.size_display_with_dir_size(sizes)
                            } else {
                                entry.size_display().to_string()
                            };
                            ui.label(
                                egui::RichText::new(size_text)
                                    .size(11.0)
                                    .color(t.text_muted),
                            );
                        });
                    });

                    if row_resp.secondary_clicked() && crate::native_menu::show(&entry.path) {
                        ctx_refresh = true;
                    }

                    if row_resp.double_clicked() {
                        pending_cursor = Some(row_cursor);
                        if entry.is_dir {
                            navigate_to = Some(entry.path.clone());
                        } else {
                            open_path = Some(entry.path.clone());
                        }
                    } else if row_resp.clicked() {
                        pending_cursor = Some(row_cursor);
                    }

                    // Drag start — selection resolved after the loop
                    if row_resp.drag_started() {
                        pending_cursor = Some(row_cursor);
                        drag_anchor = Some(entry.path.clone());
                    }

                    // Drop target highlight — show when dragging over a directory
                    if row_resp.hovered() && dragging && entry.is_dir {
                        pending_drop_target = Some(entry.path.clone());
                        ui.painter().rect_stroke(
                            full_rect,
                            CornerRadius::ZERO,
                            Stroke::new(2.0_f32, t.accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                }

                // Space after visible rows
                let after = total_rows.saturating_sub(last_visible);
                if after > 0 {
                    ui.allocate_space(Vec2::new(ui.available_width(), after as f32 * row_h));
                }

                // Drop locks before mutating panel
                drop(dir_counts);
                drop(dir_sizes);

                // Apply the interactions recorded during the loop.
                if let Some(c) = pending_cursor {
                    panel.cursor = c;
                }
                if scrolled {
                    panel.scroll_to_cursor = false;
                }
                if let Some(anchor) = drag_anchor {
                    panel.drag_entries = if panel.selected.is_empty() {
                        vec![anchor]
                    } else {
                        panel
                            .filtered_entries()
                            .iter()
                            .filter(|e| panel.selected.contains(&e.path))
                            .map(|e| e.path.clone())
                            .collect()
                    };
                }
                if let Some(target) = pending_drop_target {
                    panel.drop_target = Some(target);
                }
                if ctx_refresh {
                    panel.refresh();
                }
                if let Some(path) = open_path {
                    opener(&path);
                }
                if let Some(path) = navigate_to {
                    panel.navigate_to(path);
                }
            });

        // Status bar
        Frame::NONE
            .fill(Color32::TRANSPARENT)
            .inner_margin(Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let total = panel.filtered_count();
                    let sel = panel.selected.len();
                    let dir_total = panel.total_dir_size();
                    let size_str = match dir_total {
                        Some(s) => format!("{} items ({})", total, format_size(s)),
                        None => format!("{} items (\u{2026})", total),
                    };
                    ui.label(egui::RichText::new(size_str).size(11.0).color(t.text_muted));
                    if sel > 0 {
                        ui.label(
                            egui::RichText::new(format!(
                                "  |  {} selected ({})",
                                sel,
                                format_size(panel.total_size_selected())
                            ))
                            .size(11.0)
                            .color(t.accent_purple),
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let hidden_label = if panel.show_hidden {
                            "Hidden: ON"
                        } else {
                            "Hidden: OFF"
                        };
                        ui.label(
                            egui::RichText::new(hidden_label)
                                .size(11.0)
                                .color(t.text_muted),
                        );
                    });
                });
            });
    }

    pub(crate) fn paint_folder_icon(ui: &mut egui::Ui, count: Option<usize>) {
        let size = Vec2::new(24.0, 18.0);
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        let p = ui.painter();
        let x = rect.left();
        let y = rect.top();
        let w = size.x;
        let h = size.y;

        let color = Color32::from_rgb(200, 175, 100);
        let s = Stroke::new(1.2_f32, color);

        let tab_w = w * 0.35;
        let tab_h = 3.5;
        let body_top = tab_h;

        // Tab (outline only)
        let tab = [
            egui::pos2(x + 0.5, y + body_top),
            egui::pos2(x + 0.5, y + 0.5),
            egui::pos2(x + tab_w, y + 0.5),
            egui::pos2(x + tab_w + 2.5, y + body_top),
        ];
        for i in 0..tab.len() - 1 {
            p.line_segment([tab[i], tab[i + 1]], s);
        }

        // Body (outline only)
        let body_rect = egui::Rect::from_min_size(
            egui::pos2(x + 0.5, y + body_top),
            egui::vec2(w - 1.0, h - body_top - 0.5),
        );
        p.rect_stroke(body_rect, CornerRadius::ZERO, s, egui::StrokeKind::Outside);

        if let Some(c) = count {
            if c >= 999 {
                // Many items — fill the body
                p.rect_filled(body_rect, CornerRadius::ZERO, color.linear_multiply(0.3));
            } else if c > 0 {
                p.text(
                    body_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{}", c),
                    egui::FontId::proportional(8.5),
                    color,
                );
            }
        }
    }
}
