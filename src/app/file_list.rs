use super::*;
use crate::panel::FileColumn;

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
        metrics: crate::density::DensityMetrics,
        show_git: bool,
        column_config: &mut crate::panel::ColumnConfig,
        mut renaming: Option<&mut crate::app::RenameState>,
        grid: bool,
        user_tags: &std::collections::HashMap<std::path::PathBuf, String>,
        notes: &std::collections::HashMap<std::path::PathBuf, String>,
    ) {
        egui::ScrollArea::vertical()
            .id_salt(format!("file_list_{}", panel_side))
            .auto_shrink([false; 2])
            .animated(true)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;

                // Column header + grips - now respects order from config (more columns support).
                ui.horizontal(|ui| {
                    ui.set_min_width(ui.available_width());
                    for col in crate::panel::active_columns(column_config) {
                        let name = col.header();
                        let w = crate::panel::column_width(column_config, name).max(40.0);
                        ui.allocate_ui(egui::vec2(w, 18.0), |ui| {
                            crate::app::ui_common::primary_label(ui, name, t);
                        });
                        // simple grip after each except last (demo)
                        if name != "Modified" {  // rough
                            let grip_w = 6.0;
                            let (grip_r, grip_resp) = ui.allocate_exact_size(egui::vec2(grip_w, 18.0), Sense::drag());
                            crate::app::ui_common::paint_grip(ui, grip_r, t, true);
                            if grip_resp.dragged() {
                                let delta = ui.input(|i| i.pointer.delta().x);
                                // update corresponding width (simplified)
                                if name == "Name" { column_config.name_width = (column_config.name_width + delta).max(80.).min(600.); }
                                else if name == "Git" { column_config.git_width += delta; }
                            }
                        }
                    }
                });
                ui.add_space(2.0);

                // ".." row — go up one directory (cursor == 0)
                let can_go_up = panel.current_path().parent().is_some();
                if can_go_up {
                    let is_cursor_on_up = panel.cursor() == 0;
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
                        panel.set_cursor(0);
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
                let filtered = panel.filtered_indices();
                let query = panel.search_query().to_string();

                // Virtual list integration (main file_list now uses it for rows).
                crate::app::virtual_list::render_virtual_file_rows(
                    ui, panel, is_active, t, panel_side, size_bars, compare, opener, metrics, show_git,
                    user_tags, notes, column_config, None, grid,
                );

                if filtered.is_empty() {
                    use crate::panel::DirStatus;
                    // Distinguish a filtered-to-nothing list, a truly empty
                    // folder, and an unreadable/vanished one.
                    let (glyph, message, action): (&str, &str, Option<(&str, &str)>) =
                        if !panel.search_query().is_empty() {
                            ("\u{1f50d}", "No matches", None)
                        } else {
                            match panel.dir_status() {
                                DirStatus::Denied => (
                                    "\u{1f512}",
                                    "No permission to read this folder",
                                    Some(("Open in Finder", "finder")),
                                ),
                                DirStatus::Gone => (
                                    "\u{26a0}\u{fe0f}",
                                    "This folder no longer exists",
                                    Some(("Go up", "up")),
                                ),
                                _ => ("\u{1f4c2}", "Empty", None),
                            }
                        };
                    ui.add_space(40.0);
                    ui.with_layout(Layout::top_down(Align::Center), |ui| {
                        ui.label(egui::RichText::new(glyph).size(28.0));
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(message).size(13.0).color(t.text_muted));
                        if let Some((label, kind)) = action {
                            ui.add_space(8.0);
                            if ui.button(label).clicked() {
                                match kind {
                                    "finder" => opener(panel.current_path()),
                                    "up" => panel.go_up(),
                                    _ => {}
                                }
                            }
                        }
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

                let cursor = panel.cursor();
                let scroll_pending = panel.scroll_to_cursor();
                let dragging = !panel.drag_entries().is_empty();

                // Row sizing follows the density tier. `row_content` is the
                // allocated row height; `row_h` adds the 1px item spacing so the
                // virtualization stride matches (Comfortable == 28 + 1 == 29,
                // the pre-density default).
                let row_content = metrics.name_pt + metrics.row_pad_y * 2.0 + 7.0;
                let row_h = row_content + 1.0;
                // Dark theme? Derived from the panel background luminance, so the
                // semantic kind stub can pick the right toned palette.
                let dark =
                    (t.bg_panel.r() as u16 + t.bg_panel.g() as u16 + t.bg_panel.b() as u16) < 384;
                let total_rows = filtered.len();
                let viewport = ui.clip_rect();
                let scroll_top = viewport.top() - ui.min_rect().top();

                // Feed the visible-row count back to the core for PageUp/Down.
                panel.page_rows = ((viewport.height() / row_h).floor() as usize).max(1);

                if grid {
                    // Full grid with names, lazy cap, selectable, drag hint (idea #81/72/65).
                    // Cards: icon + truncated name, click to cursor/select, double open/nav.
                    let grid_item_w = 68.0f32;
                    let grid_item_h = 56.0f32;
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                        let max_show = filtered.len().min(96); // lazy cap for perf
                        for &entry_idx in &filtered[..max_show] {
                            let entry = &panel.entries()[entry_idx];
                            let row_cursor = entry_idx + 1;
                            let is_cur = row_cursor == cursor;
                            let is_sel = panel.selected.contains(&entry.path);
                            let bg = if is_cur && is_active {
                                t.bg_selected.linear_multiply(0.35)
                            } else if is_sel {
                                t.accent_purple.linear_multiply(0.2)
                            } else {
                                t.bg_card.linear_multiply(0.25)
                            };
                            let (item_r, item_resp) = ui.allocate_exact_size(
                                egui::vec2(grid_item_w, grid_item_h),
                                Sense::click_and_drag(),
                            );
                            if bg != Color32::TRANSPARENT {
                                ui.painter().rect_filled(item_r, CornerRadius::same(4), bg);
                            }
                            if item_resp.hovered() && !is_cur {
                                ui.painter().rect_filled(item_r, CornerRadius::same(4), t.bg_hover.linear_multiply(0.25));
                            }
                            // content centered
                            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(item_r.shrink2(egui::vec2(4.0, 2.0))));
                            c.vertical_centered(|ui| {
                                let icon = if entry.is_dir { "\u{1f4c1}" } else { &entry.icon() };
                                ui.label(egui::RichText::new(icon).size(22.0));
                                let short = if entry.name.len() > 9 {
                                    format!("{}…", &entry.name[..7])
                                } else {
                                    entry.name.clone()
                                };
                                ui.label(egui::RichText::new(short).size(9.0).color(t.text_primary));
                                if let Some(tag) = user_tags.get(&entry.path) {
                                    ui.label(egui::RichText::new(format!("[{}]", if tag.len()>2 {&tag[..2]} else {tag})).size(7.0).color(t.accent_purple));
                                }
                                if notes.contains_key(&entry.path) {
                                    ui.label(egui::RichText::new("📝").size(7.0));
                                }
                            });
                            if item_resp.hovered() {
                                let mut tip = format!("{} • {}", entry.name, entry.size_str);
                                if let Some(n) = notes.get(&entry.path) {
                                    tip.push_str(&format!(" | {}", n));
                                }
                                let _ = item_resp.clone().on_hover_text(tip);
                            }
                            if item_resp.double_clicked() {
                                pending_cursor = Some(row_cursor);
                                if entry.is_dir {
                                    navigate_to = Some(entry.path.clone());
                                } else {
                                    open_path = Some(entry.path.clone());
                                }
                            } else if item_resp.clicked() {
                                pending_cursor = Some(row_cursor);
                            }
                            if item_resp.drag_started() {
                                pending_cursor = Some(row_cursor);
                                drag_anchor = Some(entry.path.clone());
                            }
                            // subtle drag affordance
                            if dragging && entry.is_dir && item_resp.hovered() {
                                pending_drop_target = Some(entry.path.clone());
                            }
                        }
                    });
                    // grid does not use virtual spacer; fall to apply+status below
                } else {

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
                    let entry = &panel.entries()[entry_idx];
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
                        Vec2::new(ui.available_width(), row_content),
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
                            // Heatmap: intensity + color ramp (idea #16)
                            let intensity = (0.06 + frac * 0.22).min(0.28);
                            let tint = if frac > 0.75 {
                                t.accent_warning
                            } else if frac > 0.4 {
                                t.accent
                            } else {
                                t.text_muted
                            };
                            ui.painter().rect_filled(
                                bar,
                                CornerRadius::ZERO,
                                tint.linear_multiply(intensity),
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
                    } else {
                        // Semantic kind stub at the leading edge: a glanceable
                        // 3px accent by file kind (only when not comparing, so
                        // it never overlaps the compare stripe).
                        let (r, g, b) = crate::file_color::kind_color(
                            crate::selection_summary::kind_of(entry),
                            dark,
                        );
                        let stub = egui::Rect::from_min_size(
                            full_rect.min,
                            Vec2::new(3.0, full_rect.height()),
                        );
                        ui.painter().rect_filled(
                            stub,
                            CornerRadius {
                                nw: 0,
                                ne: 1,
                                sw: 0,
                                se: 1,
                            },
                            Color32::from_rgb(r, g, b),
                        );
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
                            ui.label(egui::RichText::new(entry.icon()).size(metrics.icon_pt));
                        }
                        ui.add_space(3.0);
                        // Color label / tag dot (idea #3,21,64): semantic + real user tags badge.
                        let (r, g, b) = crate::file_color::kind_color(
                            crate::selection_summary::kind_of(entry),
                            dark,
                        );
                        ui.painter().circle_filled(
                            ui.cursor().left_top() + egui::vec2(4.0, row_h / 2.0),
                            3.0,
                            Color32::from_rgb(r, g, b),
                        );
                        if let Some(tag) = user_tags.get(&entry.path) {
                            // small user tag badge/pill
                            let bx = ui.cursor().left_top() + egui::vec2(10.0, 2.0);
                            let bw = 18.0f32.min(tag.len() as f32 * 5.0 + 4.0);
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(bx, egui::vec2(bw, 10.0)),
                                CornerRadius::same(2),
                                t.accent_purple.linear_multiply(0.7),
                            );
                            ui.painter().text(
                                bx + egui::vec2(2.0, 0.0),
                                egui::Align2::LEFT_TOP,
                                if tag.len() > 3 { &tag[..3] } else { tag },
                                egui::FontId::proportional(8.0),
                                t.text_primary,
                            );
                            ui.add_space(16.0);
                        }
                        if let Some(note) = notes.get(&entry.path) {
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(ui.cursor().left_top() + egui::vec2(2.0, 3.0), egui::vec2(10.0, 8.0)),
                                CornerRadius::same(1),
                                t.accent.linear_multiply(0.6),
                            );
                            ui.add_space(12.0);
                        }
                        ui.add_space(8.0);

                        // Render cells according to active_columns order (supports Size, Modified etc + save).
                        for c in crate::panel::active_columns(column_config) {
                            let w = crate::panel::column_width(column_config, c.header()).max(20.0);
                            let val = c.cell(entry, panel);
                            ui.allocate_ui(egui::vec2(w, row_h), |ui| {
                                let color = if c.header() == "Git" {
                                    match val.chars().next().unwrap_or('?') {
                                        'M'|'m' => t.accent_red,
                                        'A'|'a' => egui::Color32::from_rgb(80,160,80),
                                        _ => t.text_muted,
                                    }
                                } else { t.text_primary };
                                ui.label(egui::RichText::new(val).size(metrics.meta_pt).color(color));
                            });
                        }

                        let name_color = if is_selected {
                            t.accent_purple
                        } else if entry.is_dir {
                            t.text_primary
                        } else {
                            t.text_secondary
                        };
                        if query.is_empty() {
                            ui.allocate_ui(egui::vec2(column_config.name_width.max(50.0), row_h), |ui| {
                                if renaming.as_ref().map_or(false, |r| r.path == entry.path) {
                                    if let Some(r) = renaming.as_mut() {
                                        let resp = ui.add(egui::TextEdit::singleline(&mut r.buffer).desired_width(f32::INFINITY));
                                        if !r.focused {
                                            resp.request_focus();
                                            r.focused = true;
                                        }
                                        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                            // commit handled outside for now
                                        }
                                        return;
                                    }
                                }
                                ui.label(
                                    egui::RichText::new(&entry.name)
                                        .size(metrics.name_pt)
                                        .color(name_color),
                                );
                            });
                        } else {
                            ui.allocate_ui(egui::vec2(column_config.name_width.max(50.0), row_h), |ui| {
                                ui.label(highlight_name_job(
                                    &entry.name,
                                    &query,
                                    name_color,
                                    t.accent,
                                    metrics.name_pt,
                                ));
                            });
                        }

                        // Hover card / quick info (idea #18) + notes (92)
                        if row_resp.hovered() {
                            let mut h = format!("{} • {} • {}", entry.name, entry.size_str, entry.modified_str);
                            if let Some(n) = notes.get(&entry.path) {
                                h.push_str(&format!(" | note: {}", n));
                            }
                            let _ = row_resp.clone().on_hover_text(h);
                        }

                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(entry.modified_display())
                                    .size(metrics.meta_pt)
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
                                    .size(metrics.meta_pt)
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
            } // end else list rendering

            // Drop locks before mutating panel (shared for grid + list paths)
            drop(dir_counts);
            drop(dir_sizes);

                // Apply the interactions recorded during the loop.
                if let Some(c) = pending_cursor {
                    panel.set_cursor(c);
                }
                if scrolled {
                    panel.set_scroll_to_cursor(false);
                }
                if let Some(anchor) = drag_anchor {
                    panel.drag_entries = if panel.selected().is_empty() {
                        vec![anchor]
                    } else {
                        panel
                            .filtered_entries()
                            .iter()
                            .filter(|e| panel.selected().contains(&e.path))
                            .map(|e| e.path.clone())
                            .collect()
                    };
                }
                if let Some(target) = pending_drop_target {
                    panel.set_drop_target(Some(target));
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

        // Status bar extracted to app/status_bar.rs (SRP, DRY)
        crate::app::status_bar::render_status_bar(ui, panel, t, compare);
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

/// Build a file name as a [`LayoutJob`], tinting the characters the fuzzy
/// filter matched in `accent` so the user sees why the row survived the filter.
/// Falls back to a flat `base`-coloured name when nothing matches.
fn highlight_name_job(
    name: &str,
    query: &str,
    base: Color32,
    accent: Color32,
    size: f32,
) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let ranges = crate::fuzzy::score(query, name)
        .map(|m| m.matched_ranges)
        .unwrap_or_default();
    let font = egui::FontId::proportional(size);
    let mut job = LayoutJob::default();
    let mut buf = [0u8; 4];
    for (idx, ch) in name.chars().enumerate() {
        let hit = ranges.iter().any(|&(s, e)| idx >= s && idx < e);
        let color = if hit { accent } else { base };
        job.append(
            ch.encode_utf8(&mut buf),
            0.0,
            TextFormat {
                font_id: font.clone(),
                color,
                ..Default::default()
            },
        );
    }
    job
}
