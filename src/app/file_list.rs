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
        compare: Option<&crate::compare::CompareMap>,
        context_menu: &dyn crate::ports::ContextMenuPort,
        opener: &dyn Fn(crate::ports::OpenRequest),
        dragging: bool,
        metrics: crate::density::DensityMetrics,
        reduced_motion: bool,
    ) -> Option<crate::provider_runtime::ContextMenuUiEffect> {
        let mut context_menu_effect = None;
        egui::ScrollArea::vertical()
            .id_salt(format!("file_list_{}", panel_side))
            .auto_shrink([false; 2])
            .animated(!reduced_motion)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;

                // ".." row — go up one directory (cursor == 0)
                let can_go_up = panel.current_path.parent().is_some();
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
                                crate::app::glyphs::parent_up(ui, t.accent);
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
                    up_row.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            ui.is_enabled(),
                            "Parent folder",
                        )
                    });

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
                // No FileEntry is cloned per frame; row interactions
                // are recorded and applied after the loop, so the
                // loop body only borrows the panel immutably.
                let filtered = panel.filtered_indices();
                // Active filter query, for highlighting matched characters in
                // each visible row. Trimmed to match panel filtering semantics.
                let query = panel.search_query().trim().to_string();

                if filtered.is_empty() {
                    use crate::panel::DirStatus;
                    // Distinguish a filtered-to-nothing list, a truly empty
                    // folder, and an unreadable/vanished one.
                    let (glyph, message, action): (&str, &str, Option<(&str, &str)>) =
                        if crate::panel::filter_is_active(panel.search_query(), &panel.facets()) {
                            (
                                "\u{1f50d}",
                                "No matches",
                                Some(("Clear filters", "clear_filters")),
                            )
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
                                DirStatus::Partial => {
                                    ("\u{21bb}", "Folder changed while reading; retrying", None)
                                }
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
                                    "finder" => opener(crate::ports::OpenRequest::Reveal(
                                        panel.current_path.clone(),
                                    )),
                                    "up" => panel.go_up(),
                                    "clear_filters" => {
                                        panel.clear_filters();
                                    }
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
                let mut scrolled = false;

                // One immutable metrics snapshot for the frame. Warm frames
                // reuse the same Arc; workers never expose mutable maps here.
                let size_snapshot = panel.size_snapshot();

                let cursor = panel.cursor();
                let scroll_pending = panel.scroll_to_cursor();
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

                if dragging
                    && panel.current_path.is_dir()
                    && !crate::volume_profile::profile(&panel.current_path).read_only
                    && let Some(pointer) = ui.ctx().input(|input| input.pointer.hover_pos())
                {
                    let velocity = crate::operation_view::edge_autoscroll_velocity(
                        pointer.y,
                        viewport.top(),
                        viewport.bottom(),
                        44.0,
                        640.0,
                    );
                    if velocity != 0.0 {
                        let dt = ui
                            .ctx()
                            .input(|input| input.stable_dt)
                            .clamp(1.0 / 120.0, 1.0 / 20.0);
                        ui.scroll_with_delta(egui::vec2(0.0, velocity * dt));
                        ui.ctx().request_repaint();
                    }
                }

                // One "now" for the whole frame, so every visible row's
                // relative Modified date is measured from the same instant.
                let now = std::time::SystemTime::now();

                // Feed the visible-row count back to the core for PageUp/Down.
                panel.set_page_rows(((viewport.height() / row_h).floor() as usize).max(1));

                // Largest entry size in the listing, used to scale occupancy
                // bars.
                let size_max: u64 = if size_bars {
                    panel.max_display_size(&filtered)
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
                panel.set_scroll_anchor(first_visible);

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
                    let is_selected = panel.is_selected(&entry.path);
                    let is_marked = panel.is_marked(&entry.path);

                    let zebra = if idx % 2 == 1 {
                        t.bg_card.linear_multiply(0.3)
                    } else {
                        Color32::TRANSPARENT
                    };

                    let bg = if is_cursor {
                        t.bg_selected
                            .linear_multiply(if is_active { 0.22 } else { 0.08 })
                    } else if is_selected {
                        t.accent_purple.linear_multiply(0.15)
                    } else {
                        zebra
                    };

                    let (row_rect, row_resp) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), row_content),
                        Sense::click_and_drag(),
                    );
                    let row_resp = if let Some(map) = compare {
                        if let Some(hint) = crate::compare::compare_hint(entry, map) {
                            row_resp.on_hover_text(hint)
                        } else {
                            row_resp
                        }
                    } else {
                        row_resp
                    };
                    let modified_label = match entry.modified {
                        Some(modified) => crate::reldate::relative_date(modified, now),
                        None => entry.modified_display().to_string(),
                    };
                    let size_text = if entry.is_dir {
                        size_snapshot
                            .size_of(&entry.path)
                            .map(crate::panel::format_size)
                            .unwrap_or_else(|| "\u{2026}".to_string())
                    } else {
                        entry.size_display().to_string()
                    };
                    let semantics = crate::accessibility::file_row_semantics(
                        &entry.name,
                        crate::selection_summary::kind_of(entry).label(),
                        &size_text,
                        &modified_label,
                        is_selected,
                        is_marked,
                        is_cursor,
                        entry.is_dir,
                    );
                    row_resp.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::SelectableLabel,
                            ui.is_enabled(),
                            semantics.selected,
                            &semantics.label,
                        )
                    });
                    ui.ctx().accesskit_node_builder(row_resp.id, |node| {
                        node.set_role(egui::accesskit::Role::Row);
                        node.set_selected(semantics.selected);
                        if let Some(expanded) = semantics.expanded {
                            node.set_expanded(expanded);
                        }
                    });
                    #[cfg(feature = "visual-qa")]
                    if panel_side == "left" && row_resp.hovered() {
                        crate::visual_qa::record_response(
                            ui.ctx(),
                            crate::visual_qa::ProbeId::LeftRow,
                            &row_resp,
                        );
                    }

                    // Scroll to cursor row when navigating with keyboard
                    if is_cursor && scroll_pending {
                        ui.scroll_to_rect(row_rect, Some(Align::Center));
                        scrolled = true;
                    }

                    // Paint background
                    let full_rect =
                        egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), row_rect.y_range());
                    if is_cursor && is_active && ui.is_enabled() {
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(egui::Id::new("current_focus_indicator"), full_rect);
                        });
                    }
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
                    if is_cursor {
                        ui.painter().rect_stroke(
                            full_rect.shrink(1.0),
                            CornerRadius::ZERO,
                            Stroke::new(1.0, t.accent),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().text(
                            egui::pos2(full_rect.left() + 7.0, full_rect.center().y),
                            egui::Align2::CENTER_CENTER,
                            "\u{203a}",
                            egui::FontId::proportional(13.0),
                            t.accent,
                        );
                    }
                    if row_resp.has_focus() {
                        ui.painter().rect_stroke(
                            full_rect.shrink(2.0),
                            CornerRadius::ZERO,
                            Stroke::new(2.0, t.text_primary),
                            egui::StrokeKind::Inside,
                        );
                        ui.painter().rect_stroke(
                            full_rect.shrink(4.0),
                            CornerRadius::ZERO,
                            Stroke::new(1.0, t.bg_panel),
                            egui::StrokeKind::Inside,
                        );
                    }

                    // Occupancy bar: width proportional to this entry's share
                    // of the largest entry, ramping to a warning tint when it
                    // dominates the directory.
                    if size_max > 0 {
                        let size = size_snapshot.display_size(entry);
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
                        use crate::compare::CompareStatus;
                        let stripe = match crate::compare::classify_entry(entry, map) {
                            CompareStatus::Unique => Some(t.accent),
                            CompareStatus::Differs => Some(t.accent_warning),
                            CompareStatus::TypeConflict => Some(t.accent_red),
                            CompareStatus::Identical | CompareStatus::DirectoryPair => None,
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

                    // Marked indicator: a right-edge stripe, independent of
                    // selection/cursor and of the left-edge kind/compare stripe.
                    if is_marked {
                        let edge = egui::Rect::from_min_size(
                            egui::pos2(full_rect.right() - 3.0, full_rect.top()),
                            Vec2::new(3.0, full_rect.height()),
                        );
                        ui.painter()
                            .rect_filled(edge, CornerRadius::ZERO, t.accent_warning);
                        ui.painter().text(
                            egui::pos2(full_rect.right() - 10.0, full_rect.center().y),
                            egui::Align2::CENTER_CENTER,
                            "\u{25c6}",
                            egui::FontId::proportional(metrics.meta_pt),
                            t.accent_warning,
                        );
                    }

                    // Content
                    let mut child_ui = ui.new_child(
                        egui::UiBuilder::new().max_rect(row_rect.shrink2(egui::vec2(6.0, 3.0))),
                    );
                    child_ui.style_mut().interaction.selectable_labels = false;
                    child_ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;

                        if is_selected {
                            ui.label(
                                egui::RichText::new("\u{2713}")
                                    .size(metrics.meta_pt)
                                    .strong()
                                    .color(t.accent_purple),
                            );
                        } else {
                            ui.add_space(9.0);
                        }

                        if entry.is_dir {
                            let count = size_snapshot.count_of(&entry.path);
                            Self::paint_folder_icon(ui, count);
                        } else {
                            ui.add_space(3.0);
                            let (red, green, blue) = crate::file_color::kind_color(
                                crate::selection_summary::kind_of(entry),
                                dark,
                            );
                            crate::app::glyphs::file_document(
                                ui,
                                &entry.extension,
                                Color32::from_rgb(red, green, blue),
                                t.bg_panel,
                            );
                        }
                        ui.add_space(3.0);

                        let name_color = if is_selected {
                            t.accent_purple
                        } else if entry.is_dir {
                            t.text_primary
                        } else {
                            t.text_secondary
                        };
                        let responsive = crate::accessibility::file_row_layout(
                            ui.available_width() - if is_marked { 16.0 } else { 0.0 },
                        );
                        let max_name_chars =
                            (responsive.name_width / metrics.name_pt).floor() as usize;
                        let display_name = crate::display_name::truncate_preserving_extension(
                            &entry.name,
                            max_name_chars.max(1),
                        );
                        let shortened = matches!(&display_name, std::borrow::Cow::Owned(_));
                        let name_response = if query.is_empty() {
                            ui.add_sized(
                                [responsive.name_width, row_content],
                                egui::Label::new(
                                    egui::RichText::new(display_name.as_ref())
                                        .size(metrics.name_pt)
                                        .color(name_color),
                                )
                                .truncate(),
                            )
                        } else {
                            ui.add_sized(
                                [responsive.name_width, row_content],
                                egui::Label::new(highlight_name_job(
                                    display_name.as_ref(),
                                    &query,
                                    name_color,
                                    t.accent,
                                    metrics.name_pt,
                                ))
                                .truncate(),
                            )
                        };
                        if shortened {
                            name_response.on_hover_text(&entry.name);
                        }

                        if responsive.show_metadata {
                            ui.allocate_ui_with_layout(
                                egui::vec2(responsive.metadata_width, row_content),
                                Layout::right_to_left(Align::Center),
                                |ui| {
                                    ui.add_space(4.0);
                                    let modified_resp = ui.label(
                                        egui::RichText::new(&modified_label)
                                            .size(metrics.meta_pt)
                                            .color(t.text_muted),
                                    );
                                    if entry.modified.is_some() {
                                        modified_resp.on_hover_text(&entry.modified_str);
                                    }
                                    ui.add_space(10.0);
                                    ui.label(
                                        egui::RichText::new(size_text)
                                            .size(metrics.meta_pt)
                                            .color(t.text_muted),
                                    );
                                    if let Some(map) = compare {
                                        use crate::compare::CompareStatus;
                                        let (symbol, hint) =
                                            match crate::compare::classify_entry(entry, map) {
                                                CompareStatus::Unique => ("+", "Only in this pane"),
                                                CompareStatus::Differs => ("\u{2260}", "Differs"),
                                                CompareStatus::Identical => ("=", "Identical"),
                                                CompareStatus::DirectoryPair => {
                                                    ("?", "Folder pair; contents not compared")
                                                }
                                                CompareStatus::TypeConflict => {
                                                    ("!", "File/folder type conflict")
                                                }
                                            };
                                        ui.label(
                                            egui::RichText::new(symbol)
                                                .size(metrics.meta_pt)
                                                .strong()
                                                .color(t.text_secondary),
                                        )
                                        .on_hover_text(hint);
                                    }
                                },
                            );
                        }
                    });

                    if row_resp.secondary_clicked() {
                        context_menu_effect = crate::provider_runtime::request_context_menu(
                            context_menu,
                            &entry.path,
                        );
                    }

                    if row_resp.double_clicked() {
                        pending_cursor = Some(row_cursor);
                        if entry.is_dir {
                            navigate_to = Some(entry.path.clone());
                        } else {
                            open_path = Some(entry.path.clone());
                        }
                    } else if row_resp.clicked() {
                        row_resp.request_focus();
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

                // Apply the interactions recorded during the loop.
                if let Some(c) = pending_cursor {
                    panel.set_cursor(c);
                }
                if scrolled {
                    panel.set_scroll_to_cursor(false);
                }
                if let Some(anchor) = drag_anchor {
                    panel.begin_drag(anchor);
                }
                if let Some(target) = pending_drop_target {
                    panel.drop_target = Some(target);
                }
                if let Some(path) = open_path {
                    opener(crate::ports::OpenRequest::OpenPath(path));
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
                    let shown = panel.filtered_count();
                    let total = panel.entries().len();
                    let selected_total = panel.selected_count();
                    let selected_visible = panel.visible_selected_count();
                    // One pass for the folder total plus its largest/oldest entry.
                    let overview = panel.folder_overview();
                    let filters_active =
                        crate::panel::filter_is_active(panel.search_query(), &panel.facets());
                    let count_prefix = if filters_active {
                        format!("{shown} of {total} items")
                    } else {
                        format!("{shown} items")
                    };
                    let size_str = match overview.total {
                        Some(s) => format!("{count_prefix} ({})", format_size(s)),
                        None => format!("{count_prefix} (\u{2026})"),
                    };
                    ui.label(egui::RichText::new(size_str).size(11.0).color(t.text_muted));
                    if filters_active {
                        let active_count = panel.facets().active_count();
                        let filter_label = if active_count > 0 {
                            format!("  |  Filters {active_count}")
                        } else {
                            "  |  Filter text".to_string()
                        };
                        ui.label(egui::RichText::new(filter_label).size(11.0).color(t.accent));
                    }
                    if selected_total > 0 {
                        let selected_size = format_size(panel.visible_selected_size());
                        let selection_text = if selected_visible == selected_total {
                            format!("{selected_total} selected ({selected_size})")
                        } else {
                            format!(
                                "{selected_total} selected ({selected_visible} visible, \
                                 {selected_size})"
                            )
                        };
                        ui.label(
                            egui::RichText::new(format!("  |  {selection_text}"))
                                .size(11.0)
                                .color(t.accent_purple),
                        );
                    }

                    // Folder largest/oldest: the always-on complement to the
                    // selection HUD (which shows the same for the selection).
                    // Hidden once a selection is active, so the two don't clash.
                    if selected_total == 0 && total >= 2 {
                        let short = |name: &str| -> String {
                            const MAX: usize = 16;
                            if name.chars().count() > MAX {
                                let head: String = name.chars().take(MAX - 1).collect();
                                format!("{head}\u{2026}")
                            } else {
                                name.to_string()
                            }
                        };
                        let mut extra = String::new();
                        if let Some((name, sz)) = &overview.largest
                            && *sz > 0
                        {
                            extra.push_str(&format!(
                                "  |  largest {} ({})",
                                short(name),
                                format_size(*sz)
                            ));
                        }
                        if let Some((name, _)) = &overview.oldest {
                            extra.push_str(&format!("  |  oldest {}", short(name)));
                        }
                        if !extra.is_empty() {
                            ui.label(egui::RichText::new(extra).size(11.0).color(t.text_muted));
                        }
                    }

                    // Compare mode: chips to turn the diff into a selection.
                    if let Some(map) = compare {
                        use crate::compare::CompareCriterion;
                        ui.label(
                            egui::RichText::new("  |  Select:")
                                .size(11.0)
                                .color(t.text_muted),
                        );
                        for (label, crit) in [
                            ("Newer", CompareCriterion::Newer),
                            ("Differing", CompareCriterion::Differing),
                            ("Unique", CompareCriterion::Unique),
                        ] {
                            let clicked = ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new(label).size(11.0).color(t.accent),
                                    )
                                    .sense(Sense::click()),
                                )
                                .clicked();
                            if clicked {
                                panel.replace_selection(crate::compare::select_by_compare(
                                    panel
                                        .filtered_indices()
                                        .iter()
                                        .filter_map(|&i| panel.entries().get(i)),
                                    map,
                                    crit,
                                ));
                            }
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let hidden_label = if panel.show_hidden() {
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
        context_menu_effect
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
