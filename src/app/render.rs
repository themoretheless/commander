use super::*;

impl App {
    /// Render one file panel. Returns `true` if the tree-sidebar toggle
    /// button was clicked (the tree itself is owned by [`App`]).
    // The arguments are mutable borrows of disjoint `self` fields (panel,
    // image_cache) plus the shared theme/opener; a bundling struct can't
    // hold them together without fighting the borrow checker.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_panel(
        panel: &mut PanelState,
        ui: &mut egui::Ui,
        is_active: bool,
        t: &ThemeColors,
        image_cache: &mut crate::image_cache::ImageCache,
        panel_side: &str,
        tree_open: bool,
        size_bars: bool,
        opener: &dyn Fn(&std::path::Path),
    ) -> bool {
        let panel_bg = t.bg_panel;
        let mut tree_toggle = false;

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
                    .inner_margin(Margin {
                        left: 6,
                        right: 6,
                        top: 6,
                        bottom: 0,
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.horizontal(|ui| {
                            // Back button
                            let btn_size = Vec2::new(28.0, 28.0);
                            let can_back = panel.can_go_back();
                            let (back_rect, back_resp) =
                                ui.allocate_exact_size(btn_size, Sense::click());
                            let back_color = if can_back {
                                t.text_primary
                            } else {
                                t.text_muted
                            };
                            ui.painter().rect_stroke(
                                back_rect,
                                CornerRadius::ZERO,
                                Stroke::new(1.0_f32, t.border),
                                egui::StrokeKind::Outside,
                            );
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
                            let (fwd_rect, fwd_resp) =
                                ui.allocate_exact_size(btn_size, Sense::click());
                            let fwd_color = if can_fwd {
                                t.text_primary
                            } else {
                                t.text_muted
                            };
                            ui.painter().rect_stroke(
                                fwd_rect,
                                CornerRadius::ZERO,
                                Stroke::new(1.0_f32, t.border),
                                egui::StrokeKind::Outside,
                            );
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
                            let tree_color = if tree_open { t.accent } else { t.text_muted };
                            let (tree_rect, tree_resp) =
                                ui.allocate_exact_size(btn_size, Sense::click());
                            ui.painter().rect_stroke(
                                tree_rect,
                                CornerRadius::ZERO,
                                Stroke::new(1.0_f32, t.border),
                                egui::StrokeKind::Outside,
                            );
                            // Mini folder icon
                            {
                                let p = ui.painter();
                                let cx = tree_rect.center().x;
                                let cy = tree_rect.center().y;
                                let c = tree_color;
                                // Back
                                p.rect_filled(
                                    egui::Rect::from_center_size(
                                        egui::pos2(cx, cy + 1.0),
                                        egui::vec2(14.0, 10.0),
                                    ),
                                    CornerRadius::same(2),
                                    c.linear_multiply(0.5),
                                );
                                // Tab
                                p.rect_filled(
                                    egui::Rect::from_min_size(
                                        egui::pos2(cx - 7.0, cy - 5.5),
                                        egui::vec2(6.0, 3.0),
                                    ),
                                    CornerRadius {
                                        nw: 2,
                                        ne: 2,
                                        sw: 0,
                                        se: 0,
                                    },
                                    c.linear_multiply(0.7),
                                );
                                // Front
                                p.rect_filled(
                                    egui::Rect::from_center_size(
                                        egui::pos2(cx, cy + 2.0),
                                        egui::vec2(14.0, 8.0),
                                    ),
                                    CornerRadius::same(1),
                                    c,
                                );
                            }
                            if tree_resp.clicked() {
                                tree_toggle = true;
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
                                            let fg = if is_last {
                                                t.text_primary
                                            } else {
                                                t.text_secondary
                                            };

                                            let resp = Frame::NONE
                                                .fill(Color32::TRANSPARENT)
                                                .corner_radius(CornerRadius::ZERO)
                                                .stroke(Stroke::new(1.0_f32, t.border))
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
                                                ui.ctx().set_cursor_icon(
                                                    egui::CursorIcon::PointingHand,
                                                );
                                            }

                                            // Arrow separator
                                            if !is_last {
                                                // Draw a simple chevron
                                                Frame::NONE
                                                    .fill(Color32::TRANSPARENT)
                                                    .corner_radius(CornerRadius::ZERO)
                                                    .stroke(Stroke::new(1.0_f32, t.border))
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
                            let name_label =
                                format!("Name{}", panel.sort_indicator(SortColumn::Name));
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

                                let mod_label = format!(
                                    "Modified{}",
                                    panel.sort_indicator(SortColumn::Modified)
                                );
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
                                    let scale =
                                        (avail.x / tex_size.x).min(avail.y / tex_size.y).min(1.0);
                                    let display_size =
                                        egui::vec2(tex_size.x * scale, tex_size.y * scale);
                                    let resp = ui.add(egui::Image::from_texture(
                                        egui::load::SizedTexture::new(texture.id(), display_size),
                                    ));
                                    if resp.clicked()
                                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                                    {
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
                                                    .unwrap_or_default(),
                                            )
                                            .size(12.0)
                                            .strong()
                                            .color(t.text_primary),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if ui.small_button("✕").clicked()
                                                    || ui
                                                        .input(|i| i.key_pressed(egui::Key::Escape))
                                                {
                                                    panel.preview = None;
                                                }
                                            },
                                        );
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
                Self::render_file_list(ui, panel, is_active, t, panel_side, size_bars, opener);
            });

        tree_toggle
    }
}
