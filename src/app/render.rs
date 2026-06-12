use super::*;

impl App {
    pub(crate) fn render_panel(
        panel: &mut PanelState,
        ui: &mut egui::Ui,
        is_active: bool,
        t: &ThemeColors,
        image_cache: &mut crate::image_cache::ImageCache,
        panel_side: &str,
    ) {
        let panel_bg = t.bg_panel;

        Frame::NONE
            .fill(panel_bg)
            .inner_margin(Margin::same(0))
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::ZERO)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 10.0);

                // Path bar: back/forward + breadcrumb arrows
                Frame::NONE
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(Margin { left: 6, right: 6, top: 6, bottom: 0 })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.horizontal(|ui| {
                            // Back button
                            let btn_size = Vec2::new(28.0, 28.0);
                            let can_back = panel.can_go_back();
                            let (back_rect, back_resp) = ui.allocate_exact_size(btn_size, Sense::click());
                            let back_color = if can_back { t.text_primary } else { t.text_muted };
                            ui.painter().rect_stroke(back_rect, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                            ui.painter().text(
                                back_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "\u{25c0}",
                                egui::FontId::proportional(13.0),
                                back_color,
                            );
                            if back_resp.clicked() && can_back {
                                panel.go_back();
                            }

                            // Forward button
                            let can_fwd = panel.can_go_forward();
                            let (fwd_rect, fwd_resp) = ui.allocate_exact_size(btn_size, Sense::click());
                            let fwd_color = if can_fwd { t.text_primary } else { t.text_muted };
                            ui.painter().rect_stroke(fwd_rect, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                            ui.painter().text(
                                fwd_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "\u{25b6}",
                                egui::FontId::proportional(13.0),
                                fwd_color,
                            );
                            if fwd_resp.clicked() && can_fwd {
                                panel.go_forward();
                            }

                            // Tree toggle button
                            let tree_color = if panel.show_tree { t.accent } else { t.text_muted };
                            let (tree_rect, tree_resp) = ui.allocate_exact_size(btn_size, Sense::click());
                            ui.painter().rect_stroke(tree_rect, CornerRadius::ZERO, Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                            // Mini folder icon
                            {
                                let p = ui.painter();
                                let cx = tree_rect.center().x;
                                let cy = tree_rect.center().y;
                                let c = tree_color;
                                // Back
                                p.rect_filled(
                                    egui::Rect::from_center_size(egui::pos2(cx, cy + 1.0), egui::vec2(14.0, 10.0)),
                                    CornerRadius::same(2),
                                    c.linear_multiply(0.5),
                                );
                                // Tab
                                p.rect_filled(
                                    egui::Rect::from_min_size(egui::pos2(cx - 7.0, cy - 5.5), egui::vec2(6.0, 3.0)),
                                    CornerRadius { nw: 2, ne: 2, sw: 0, se: 0 },
                                    c.linear_multiply(0.7),
                                );
                                // Front
                                p.rect_filled(
                                    egui::Rect::from_center_size(egui::pos2(cx, cy + 2.0), egui::vec2(14.0, 8.0)),
                                    CornerRadius::same(1),
                                    c,
                                );
                            }
                            if tree_resp.clicked() {
                                // Toggle stored in panel, synced to App in update()
                                panel.show_tree = !panel.show_tree;
                            }

                            ui.add_space(6.0);

                            // Breadcrumbs as connected arrow-shaped buttons
                            let mut nav_to_crumb: Option<std::path::PathBuf> = None;
                            egui::ScrollArea::horizontal()
                                .max_width(ui.available_width())
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 0.0;
                                        let crumbs = panel.breadcrumbs();
                                        let len = crumbs.len();
                                        for (i, (name, path)) in crumbs.iter().enumerate() {
                                            let is_last = i == len - 1;
                                            let (bg, fg) = if is_last {
                                                (t.accent.linear_multiply(0.25), t.text_primary)
                                            } else {
                                                (t.bg_card, t.text_secondary)
                                            };

                                            let resp = Frame::NONE
                                                .fill(Color32::TRANSPARENT)
                                                .corner_radius(CornerRadius::ZERO)
                                                .stroke(Stroke::new(1.0, t.border))
                                                .inner_margin(Margin::symmetric(8, 3))
                                                .show(ui, |ui| {
                                                    ui.label(
                                                        egui::RichText::new(name)
                                                            .size(12.0)
                                                            .color(fg),
                                                    );
                                                })
                                                .response
                                                .interact(Sense::click());

                                            if resp.clicked() && !is_last {
                                                nav_to_crumb = Some(path.clone());
                                            }
                                            if resp.hovered() && !is_last {
                                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                            }

                                            // Arrow separator
                                            if !is_last {
                                                let next_bg = if i + 1 == len - 1 {
                                                    t.accent.linear_multiply(0.25)
                                                } else {
                                                    t.bg_card
                                                };
                                                // Draw a simple chevron
                                                Frame::NONE
                                                    .fill(Color32::TRANSPARENT)
                                                    .corner_radius(CornerRadius::ZERO)
                                                    .stroke(Stroke::new(1.0, t.border))
                                                    .inner_margin(Margin::symmetric(0, 3))
                                                    .show(ui, |ui| {
                                                        ui.label(
                                                            egui::RichText::new("\u{276f}")
                                                                .size(11.0)
                                                                .color(t.text_muted),
                                                        );
                                                    });
                                            }
                                        }
                                    });
                                });
                            if let Some(path) = nav_to_crumb {
                                panel.navigate_to(path);
                            }
                        });
                    });

                // Search bar
                Frame::NONE
                    .fill(panel_bg)
                    .inner_margin(Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut panel.search_query)
                                .hint_text("\u{1f50d} Filter\u{2026}")
                                .desired_width(ui.available_width())
                                .margin(egui::vec2(8.0, 4.0)),
                        );
                    });

                // Column headers
                let header_bg = if is_active {
                    Color32::from_rgb(
                        t.bg_panel.r().saturating_sub(10),
                        t.bg_panel.g().saturating_sub(10),
                        t.bg_panel.b().saturating_sub(10),
                    )
                } else {
                    Color32::TRANSPARENT
                };
                let header_text = if is_active {
                    t.text_primary
                } else {
                    t.text_muted
                };
                Frame::NONE
                    .fill(header_bg)
                    .inner_margin(Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let name_label = format!("Name{}", panel.sort_indicator(SortColumn::Name));
                            if ui
                                .label(
                                    egui::RichText::new(name_label)
                                        .size(11.0)
                                        .strong()
                                        .color(header_text),
                                )
                                .interact(Sense::click())
                                .clicked()
                            {
                                panel.set_sort(SortColumn::Name);
                            }

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add_space(8.0);

                                let mod_label =
                                    format!("Modified{}", panel.sort_indicator(SortColumn::Modified));
                                if ui
                                    .label(
                                        egui::RichText::new(mod_label)
                                            .size(11.0)
                                            .strong()
                                            .color(header_text),
                                    )
                                    .interact(Sense::click())
                                    .clicked()
                                {
                                    panel.set_sort(SortColumn::Modified);
                                }

                                ui.add_space(24.0);

                                let size_label =
                                    format!("Size{}", panel.sort_indicator(SortColumn::Size));
                                if ui
                                    .label(
                                        egui::RichText::new(size_label)
                                            .size(11.0)
                                            .strong()
                                            .color(header_text),
                                    )
                                    .interact(Sense::click())
                                    .clicked()
                                {
                                    panel.set_sort(SortColumn::Size);
                                }
                            });
                        });
                    });

                ui.add(egui::Separator::default().spacing(0.0));

                // Preview mode (image or text)
                if let Some(preview) = panel.preview.clone() {
                    use crate::panel::PreviewContent;
                    match &preview {
                        PreviewContent::Image(path) => {
                            let path = path.clone();
                            if let Some(texture) = image_cache.get_or_load_sync(ui.ctx(), &path) {
                                let tex_size = texture.size_vec2();
                                ui.centered_and_justified(|ui| {
                                    let avail = ui.available_size();
                                    let scale = (avail.x / tex_size.x).min(avail.y / tex_size.y).min(1.0);
                                    let display_size = egui::vec2(tex_size.x * scale, tex_size.y * scale);
                                    let resp = ui.add(
                                        egui::Image::from_texture(egui::load::SizedTexture::new(texture.id(), display_size))
                                    );
                                    if resp.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                        panel.preview = None;
                                    }
                                });
                            } else {
                                ui.centered_and_justified(|ui| {
                                    ui.spinner();
                                });
                            }
                        }
                        PreviewContent::Text { path, content } => {
                            // Text viewer
                            Frame::NONE
                                .fill(t.bg_panel)
                                .inner_margin(Margin::same(8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(
                                                path.file_name()
                                                    .map(|n| n.to_string_lossy().to_string())
                                                    .unwrap_or_default()
                                            )
                                            .size(12.0)
                                            .strong()
                                            .color(t.text_primary),
                                        );
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            if ui.small_button("✕").clicked()
                                                || ui.input(|i| i.key_pressed(egui::Key::Escape))
                                            {
                                                panel.preview = None;
                                            }
                                        });
                                    });
                                    ui.add(egui::Separator::default().spacing(4.0));

                                    egui::ScrollArea::both()
                                        .id_salt(format!("text_preview_{}", panel_side))
                                        .auto_shrink([false; 2])
                                        .show(ui, |ui| {
                                            ui.style_mut().interaction.selectable_labels = true;
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(content)
                                                        .size(12.0)
                                                        .font(egui::FontId::monospace(12.0))
                                                        .color(t.text_secondary),
                                                )
                                                .wrap(),
                                            );
                                        });
                                });
                        }
                    }
                    return;
                }

                // File list
                Self::render_file_list(ui, panel, is_active, t, panel_side);
            });
    }

    /// Draw a virtualized flat file list (only visible rows rendered).
    pub(crate) fn render_flat_list_virtual(
        ui: &mut egui::Ui,
        flat: &[crate::app::file_ops::FlatFileEntry],
        conflicts: &[String],
        t: &ThemeColors,
        max_height: f32,
        id_salt: &str,
    ) {
        let row_h = 20.0;
        let total_h = flat.len() as f32 * row_h;

        Frame::NONE
            .fill(t.bg_card.linear_multiply(0.3))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(max_height)
                    .id_salt(id_salt)
                    .show(ui, |ui| {
                        // Total count label
                        ui.label(egui::RichText::new(format!("{} items", flat.len())).size(10.0).color(t.text_muted));

                        let scroll_offset = ui.clip_rect().top() - ui.min_rect().top();
                        let viewport_h = max_height;
                        let first = ((scroll_offset / row_h).floor() as usize).min(flat.len());
                        let visible_count = ((viewport_h / row_h).ceil() as usize + 2).min(flat.len() - first);

                        // Spacer before visible rows
                        if first > 0 {
                            ui.allocate_space(Vec2::new(ui.available_width(), first as f32 * row_h));
                        }

                        // Render only visible rows
                        for (vi, fe) in flat[first..first + visible_count].iter().enumerate() {
                            let row_idx = first + vi;
                            let is_conflict = fe.depth == 0 && conflicts.contains(&fe.name);
                            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
                            let p = ui.painter();

                            // Zebra stripe
                            if row_idx % 2 == 1 {
                                p.rect_filled(rect, CornerRadius::ZERO, t.bg_card.linear_multiply(0.15));
                            }

                            let indent = fe.depth as f32 * 14.0;
                            let icon = if fe.is_dir { "📁" } else { "📄" };
                            let color = if is_conflict {
                                Color32::from_rgb(230, 160, 40)
                            } else if fe.depth > 0 {
                                t.text_secondary
                            } else {
                                t.text_primary
                            };

                            // Icon
                            p.text(
                                egui::pos2(rect.left() + indent, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                icon,
                                egui::FontId::proportional(11.0),
                                color,
                            );
                            // Name
                            p.text(
                                egui::pos2(rect.left() + indent + 18.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &fe.name,
                                egui::FontId::proportional(11.0),
                                color,
                            );
                            // Size
                            if !fe.is_dir && fe.size > 0 {
                                p.text(
                                    egui::pos2(rect.right() - 4.0, rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    format_size(fe.size),
                                    egui::FontId::proportional(10.0),
                                    t.text_muted,
                                );
                            }
                        }

                        // Spacer after visible rows
                        let after = flat.len() - first - visible_count;
                        if after > 0 {
                            ui.allocate_space(Vec2::new(ui.available_width(), after as f32 * row_h));
                        }
                    });
            });
    }

    /// Like render_flat_list_virtual but highlights incoming files (files being copied).
    pub(crate) fn render_flat_list_virtual_with_highlight(
        ui: &mut egui::Ui,
        flat: &[crate::app::file_ops::FlatFileEntry],
        conflicts: &[String],
        highlight_names: &std::collections::HashSet<String>,
        t: &ThemeColors,
        max_height: f32,
        id_salt: &str,
    ) {
        let row_h = 20.0;

        Frame::NONE
            .fill(t.bg_card.linear_multiply(0.3))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(max_height)
                    .id_salt(id_salt)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(format!("{} items", flat.len())).size(10.0).color(t.text_muted));

                        let scroll_offset = ui.clip_rect().top() - ui.min_rect().top();
                        let viewport_h = max_height;
                        let first = ((scroll_offset / row_h).floor() as usize).min(flat.len());
                        let visible_count = ((viewport_h / row_h).ceil() as usize + 2).min(flat.len().saturating_sub(first));

                        if first > 0 {
                            ui.allocate_space(Vec2::new(ui.available_width(), first as f32 * row_h));
                        }

                        for (vi, fe) in flat[first..first + visible_count].iter().enumerate() {
                            let row_idx = first + vi;
                            let is_conflict = fe.depth == 0 && conflicts.contains(&fe.name);
                            let is_incoming = fe.depth == 0 && highlight_names.contains(&fe.name);
                            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
                            let p = ui.painter();

                            // Background: incoming files get accent tint
                            if is_incoming {
                                p.rect_filled(rect, CornerRadius::ZERO, t.accent.linear_multiply(0.15));
                            } else if row_idx % 2 == 1 {
                                p.rect_filled(rect, CornerRadius::ZERO, t.bg_card.linear_multiply(0.15));
                            }

                            let indent = fe.depth as f32 * 14.0;
                            let icon = if fe.is_dir { "📁" } else { "📄" };
                            let color = if is_conflict {
                                Color32::from_rgb(230, 160, 40)
                            } else if is_incoming {
                                t.accent
                            } else if fe.depth > 0 {
                                t.text_secondary
                            } else {
                                t.text_primary
                            };

                            p.text(
                                egui::pos2(rect.left() + indent, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                icon,
                                egui::FontId::proportional(11.0),
                                color,
                            );
                            p.text(
                                egui::pos2(rect.left() + indent + 18.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &fe.name,
                                egui::FontId::proportional(11.0),
                                color,
                            );
                            if !fe.is_dir && fe.size > 0 {
                                p.text(
                                    egui::pos2(rect.right() - 4.0, rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    format_size(fe.size),
                                    egui::FontId::proportional(10.0),
                                    t.text_muted,
                                );
                            }
                        }

                        let after = flat.len().saturating_sub(first + visible_count);
                        if after > 0 {
                            ui.allocate_space(Vec2::new(ui.available_width(), after as f32 * row_h));
                        }
                    });
            });
    }

    /// Render file tree for copy/move dialog with flow visualization.
    /// `is_source` = true: files shown dimmed (leaving source)
    /// `is_source` = false: files shown highlighted (arriving at destination)
    pub(crate) fn render_flat_list_virtual_flow(
        ui: &mut egui::Ui,
        flat: &[crate::app::file_ops::FlatFileEntry],
        conflicts: &[String],
        t: &ThemeColors,
        max_height: f32,
        id_salt: &str,
        is_source: bool,
    ) {
        let row_h = 20.0;

        Frame::NONE
            .fill(t.bg_card.linear_multiply(0.3))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                // Header
                let header = if is_source { "Source" } else { "Destination" };
                let arrow = if is_source { "  ➜" } else { "➜  " };
                ui.label(egui::RichText::new(format!("{} {} ({} items)", arrow, header, flat.len())).size(10.0).color(t.text_muted));

                egui::ScrollArea::vertical()
                    .max_height(max_height)
                    .id_salt(id_salt)
                    .show(ui, |ui| {
                        let scroll_offset = ui.clip_rect().top() - ui.min_rect().top();
                        let viewport_h = max_height;
                        let first = ((scroll_offset / row_h).floor() as usize).min(flat.len());
                        let visible_count = ((viewport_h / row_h).ceil() as usize + 2).min(flat.len().saturating_sub(first));

                        if first > 0 {
                            ui.allocate_space(Vec2::new(ui.available_width(), first as f32 * row_h));
                        }

                        for (vi, fe) in flat[first..first + visible_count].iter().enumerate() {
                            let row_idx = first + vi;
                            let is_conflict = fe.depth == 0 && conflicts.contains(&fe.name);
                            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
                            let p = ui.painter();

                            if is_source {
                                // Source side: dimmed background, files are "leaving"
                                if row_idx % 2 == 1 {
                                    p.rect_filled(rect, CornerRadius::ZERO, t.accent_red.linear_multiply(0.05));
                                }
                            } else {
                                // Destination side: highlighted, files are "arriving"
                                p.rect_filled(rect, CornerRadius::ZERO, t.accent.linear_multiply(if row_idx % 2 == 0 { 0.08 } else { 0.12 }));
                            }

                            let indent = fe.depth as f32 * 14.0;
                            let icon = if fe.is_dir { "📁" } else { "📄" };

                            let color = if is_conflict {
                                Color32::from_rgb(230, 160, 40)
                            } else if is_source {
                                t.text_muted // dimmed — leaving
                            } else {
                                t.accent // highlighted — arriving
                            };

                            // Icon
                            p.text(
                                egui::pos2(rect.left() + indent, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                icon,
                                egui::FontId::proportional(11.0),
                                color,
                            );
                            // Name
                            p.text(
                                egui::pos2(rect.left() + indent + 18.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &fe.name,
                                egui::FontId::proportional(11.0),
                                color,
                            );

                            // Strikethrough on source side
                            if is_source && !is_conflict {
                                let text_start = rect.left() + indent + 18.0;
                                let text_end = text_start + fe.name.len() as f32 * 6.5;
                                let cy = rect.center().y;
                                p.line_segment(
                                    [egui::pos2(text_start, cy), egui::pos2(text_end.min(rect.right() - 4.0), cy)],
                                    Stroke::new(1.0, t.text_muted.linear_multiply(0.5)),
                                );
                            }

                            // Size
                            if !fe.is_dir && fe.size > 0 {
                                p.text(
                                    egui::pos2(rect.right() - 4.0, rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    format_size(fe.size),
                                    egui::FontId::proportional(10.0),
                                    if is_source { t.text_muted.linear_multiply(0.5) } else { t.text_muted },
                                );
                            }
                        }

                        let after = flat.len().saturating_sub(first + visible_count);
                        if after > 0 {
                            ui.allocate_space(Vec2::new(ui.available_width(), after as f32 * row_h));
                        }
                    });
            });
    }

    /// Animated file list for copy/move dialog.
    /// `transferred`: how many files have "moved" so far
    /// `is_source`: true = left side (files leaving), false = right side (files arriving)
    pub(crate) fn render_flat_list_animated(
        ui: &mut egui::Ui,
        flat: &[crate::app::file_ops::FlatFileEntry],
        conflicts: &[String],
        t: &ThemeColors,
        max_height: f32,
        id_salt: &str,
        transferred: usize,
        is_source: bool,
    ) {
        let row_h = 20.0;

        // On destination side, only show transferred files
        let visible_flat: &[crate::app::file_ops::FlatFileEntry] = if is_source {
            flat
        } else {
            &flat[..transferred]
        };

        Frame::NONE
            .fill(t.bg_card.linear_multiply(0.3))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                let header = if is_source {
                    format!("Source ({} items)", flat.len())
                } else {
                    format!("Destination ({}/{})", transferred, flat.len())
                };
                ui.label(egui::RichText::new(header).size(10.0).color(t.text_muted));

                egui::ScrollArea::vertical()
                    .max_height(max_height)
                    .id_salt(id_salt)
                    .show(ui, |ui| {
                        let scroll_offset = ui.clip_rect().top() - ui.min_rect().top();
                        let viewport_h = max_height;
                        let total = visible_flat.len();
                        let first = ((scroll_offset / row_h).floor() as usize).min(total);
                        let visible_count = ((viewport_h / row_h).ceil() as usize + 2).min(total.saturating_sub(first));

                        if first > 0 {
                            ui.allocate_space(Vec2::new(ui.available_width(), first as f32 * row_h));
                        }

                        for (vi, fe) in visible_flat[first..first + visible_count].iter().enumerate() {
                            let row_idx = first + vi;
                            let is_conflict = fe.depth == 0 && conflicts.contains(&fe.name);
                            let is_transferred = row_idx < transferred;
                            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
                            let p = ui.painter();

                            // Background
                            if is_source && is_transferred {
                                // Transferred on source side — faded red tint
                                p.rect_filled(rect, CornerRadius::ZERO, t.accent_red.linear_multiply(0.08));
                            } else if !is_source {
                                // Destination side — green/accent tint
                                let alpha = if row_idx + 1 == transferred { 0.2 } else { 0.1 };
                                p.rect_filled(rect, CornerRadius::ZERO, t.accent.linear_multiply(alpha));
                            } else if row_idx % 2 == 1 {
                                p.rect_filled(rect, CornerRadius::ZERO, t.bg_card.linear_multiply(0.15));
                            }

                            let indent = fe.depth as f32 * 14.0;
                            let icon = if fe.is_dir { "📁" } else { "📄" };

                            let color = if is_conflict {
                                Color32::from_rgb(230, 160, 40)
                            } else if is_source && is_transferred {
                                t.text_muted.linear_multiply(0.4) // very dim
                            } else if !is_source {
                                t.accent
                            } else {
                                t.text_primary
                            };

                            // Icon
                            p.text(
                                egui::pos2(rect.left() + indent, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                icon,
                                egui::FontId::proportional(11.0),
                                color,
                            );
                            // Name
                            p.text(
                                egui::pos2(rect.left() + indent + 18.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &fe.name,
                                egui::FontId::proportional(11.0),
                                color,
                            );

                            // Strikethrough on transferred source items
                            if is_source && is_transferred {
                                let text_start = rect.left() + indent + 18.0;
                                let text_end = text_start + fe.name.len() as f32 * 6.5;
                                p.line_segment(
                                    [egui::pos2(text_start, rect.center().y), egui::pos2(text_end.min(rect.right() - 4.0), rect.center().y)],
                                    Stroke::new(1.0, t.text_muted.linear_multiply(0.3)),
                                );
                            }

                            // Size
                            if !fe.is_dir && fe.size > 0 {
                                let size_color = if is_source && is_transferred {
                                    t.text_muted.linear_multiply(0.3)
                                } else {
                                    t.text_muted
                                };
                                p.text(
                                    egui::pos2(rect.right() - 4.0, rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    format_size(fe.size),
                                    egui::FontId::proportional(10.0),
                                    size_color,
                                );
                            }
                        }

                        let after = total.saturating_sub(first + visible_count);
                        if after > 0 {
                            ui.allocate_space(Vec2::new(ui.available_width(), after as f32 * row_h));
                        }
                    });
            });
    }

    /// Draw a flat progress bar without rounding.
    pub(crate) fn draw_progress_bar(
        ui: &mut egui::Ui,
        frac: f32,
        text: &str,
        fill_color: Color32,
        t: &ThemeColors,
    ) {
        let bar_h = 18.0;
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, bar_h), Sense::hover());
        let p = ui.painter();

        // Background
        p.rect_filled(rect, CornerRadius::ZERO, t.bg_card.linear_multiply(0.5));

        // Filled portion
        let filled_w = rect.width() * frac.clamp(0.0, 1.0);
        if filled_w > 0.0 {
            let filled_rect = egui::Rect::from_min_size(rect.min, Vec2::new(filled_w, bar_h));
            p.rect_filled(filled_rect, CornerRadius::ZERO, fill_color);
        }

        // Text centered
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(10.0),
            t.text_primary,
        );
    }
}
