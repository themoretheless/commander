use super::*;
use crate::panel::{FacetSet, KindFacet};

impl App {
    /// A row of toggleable quick-filter chips under the filter box.
    fn facet_chips(ui: &mut egui::Ui, panel: &mut PanelState, t: &ThemeColors) -> bool {
        let before = panel.facets;
        Frame::NONE
            .fill(Color32::TRANSPARENT)
            .inner_margin(Margin {
                left: 10,
                right: 10,
                top: 0,
                bottom: 2,
            })
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);

                    let chip = |ui: &mut egui::Ui, label: &str, active: bool| -> bool {
                        let fill = if active {
                            t.accent.linear_multiply(0.3)
                        } else {
                            t.bg_card
                        };
                        ui.add(
                            egui::Button::new(
                                egui::RichText::new(label).size(10.0).color(t.text_primary),
                            )
                            .fill(fill)
                            .corner_radius(CornerRadius::same(2)),
                        )
                        .clicked()
                    };

                    let f = &mut panel.facets;
                    // Kind chips (mutually exclusive: clicking the active one clears it).
                    for (label, kind) in [
                        ("Folders", KindFacet::Folders),
                        ("Images", KindFacet::Images),
                        ("Docs", KindFacet::Docs),
                        ("Archives", KindFacet::Archives),
                        ("Code", KindFacet::Code),
                    ] {
                        if chip(ui, label, f.kind == Some(kind)) {
                            f.kind = if f.kind == Some(kind) {
                                None
                            } else {
                                Some(kind)
                            };
                        }
                    }
                    ui.add_space(6.0);
                    if chip(ui, ">1MB", f.min_size == Some(1 << 20)) {
                        f.min_size = if f.min_size == Some(1 << 20) {
                            None
                        } else {
                            Some(1 << 20)
                        };
                    }
                    if chip(ui, ">100MB", f.min_size == Some(100 << 20)) {
                        f.min_size = if f.min_size == Some(100 << 20) {
                            None
                        } else {
                            Some(100 << 20)
                        };
                    }
                    ui.add_space(6.0);
                    if chip(ui, "Today", f.max_age_days == Some(1)) {
                        f.max_age_days = if f.max_age_days == Some(1) {
                            None
                        } else {
                            Some(1)
                        };
                    }
                    if chip(ui, "Week", f.max_age_days == Some(7)) {
                        f.max_age_days = if f.max_age_days == Some(7) {
                            None
                        } else {
                            Some(7)
                        };
                    }
                    if chip(ui, "Month", f.max_age_days == Some(30)) {
                        f.max_age_days = if f.max_age_days == Some(30) {
                            None
                        } else {
                            Some(30)
                        };
                    }
                    // Older than a month (min-age): the stale-files bucket.
                    if chip(ui, "Older", f.min_age_days == Some(30)) {
                        f.min_age_days = if f.min_age_days == Some(30) {
                            None
                        } else {
                            Some(30)
                        };
                    }
                    let active_count = f.active_count();
                    if active_count > 0 {
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(format!("{active_count} active"))
                                .size(10.0)
                                .color(t.text_muted),
                        );
                    }
                    if !f.is_empty() && chip(ui, "\u{2715} Clear", false) {
                        *f = FacetSet::default();
                    }
                });
            });
        panel.facets != before
    }

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
        compare: Option<&crate::compare::CompareMap>,
        opener: &dyn Fn(&std::path::Path),
        dragging: bool,
        metrics: crate::density::DensityMetrics,
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
                let mut filter_changed = false;
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

                            // Breadcrumbs: elide deep paths behind a "..." menu
                            // so the root and the current folder stay visible.
                            let mut nav_to_crumb: Option<std::path::PathBuf> = None;
                            let all = crate::crumbs::crumbs(&panel.current_path);
                            // Roughly one chip per ~130px; always keep two.
                            let max_visible = ((ui.available_width() / 130.0) as usize).max(2);
                            let layout = crate::crumbs::elide_crumbs(&all, max_visible);

                            // Draw one clickable crumb chip; returns its response.
                            let chip = |ui: &mut egui::Ui, label: &str, fg: Color32| {
                                Frame::NONE
                                    .fill(Color32::TRANSPARENT)
                                    .corner_radius(CornerRadius::ZERO)
                                    .stroke(Stroke::new(1.0_f32, t.border))
                                    .inner_margin(Margin::symmetric(8, 3))
                                    .show(ui, |ui| {
                                        ui.label(egui::RichText::new(label).size(12.0).color(fg));
                                    })
                                    .response
                                    .interact(Sense::click())
                            };
                            let sep = |ui: &mut egui::Ui| {
                                Frame::NONE
                                    .inner_margin(Margin::symmetric(3, 3))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new("\u{276f}")
                                                .size(11.0)
                                                .color(t.text_muted),
                                        );
                                    });
                            };

                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;

                                // Head (root). It is itself the current dir only
                                // when the path has a single segment.
                                let head_is_current = layout.tail.is_empty();
                                let fg = if head_is_current {
                                    t.text_primary
                                } else {
                                    t.text_secondary
                                };
                                let resp = chip(ui, &layout.head.label, fg);
                                if resp.clicked() && !head_is_current {
                                    nav_to_crumb = Some(layout.head.full_path.clone());
                                }
                                if resp.hovered() && !head_is_current {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                if !head_is_current {
                                    sep(ui);
                                }

                                // Collapsed ancestors behind a "..." menu.
                                if layout.is_elided() {
                                    ui.menu_button("\u{2026}", |ui| {
                                        for c in &layout.collapsed {
                                            if ui.button(&c.label).clicked() {
                                                nav_to_crumb = Some(c.full_path.clone());
                                                ui.close();
                                            }
                                        }
                                    });
                                    sep(ui);
                                }

                                // Tail, ending at the current directory.
                                let tlen = layout.tail.len();
                                for (i, c) in layout.tail.iter().enumerate() {
                                    let is_last = i == tlen - 1;
                                    let fg = if is_last {
                                        t.text_primary
                                    } else {
                                        t.text_secondary
                                    };
                                    let resp = chip(ui, &c.label, fg);
                                    if resp.clicked() && !is_last {
                                        nav_to_crumb = Some(c.full_path.clone());
                                    }
                                    if resp.hovered() && !is_last {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    if !is_last {
                                        sep(ui);
                                    }
                                }
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
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            let text_filter_active = !panel.search_query.trim().is_empty();
                            let clear_width = if text_filter_active { 30.0 } else { 0.0 };
                            let input_width = (ui.available_width() - clear_width).max(80.0);
                            filter_changed |= ui
                                .add_sized(
                                    Vec2::new(input_width, 26.0),
                                    egui::TextEdit::singleline(&mut panel.search_query)
                                        .hint_text("\u{1f50d} Filter\u{2026}")
                                        .desired_width(f32::INFINITY)
                                        .margin(egui::vec2(8.0, 4.0)),
                                )
                                .changed();
                            if text_filter_active
                                && ui
                                    .add_sized(
                                        Vec2::new(26.0, 24.0),
                                        egui::Button::new(
                                            egui::RichText::new("\u{00d7}")
                                                .size(12.0)
                                                .color(t.text_secondary),
                                        )
                                        .fill(t.bg_card)
                                        .corner_radius(crate::theme::ROUNDING_SM),
                                    )
                                    .on_hover_text("Clear filter")
                                    .clicked()
                            {
                                panel.search_query.clear();
                                filter_changed = true;
                            }
                        });
                    });

                // Quick-filter facet chips.
                let facets_changed = Self::facet_chips(ui, panel, t);
                if filter_changed || facets_changed {
                    panel.ensure_cursor_valid();
                }

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
                let header = Frame::NONE
                    .fill(header_bg)
                    .inner_margin(Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if is_active {
                                ui.label(
                                    egui::RichText::new("ACTIVE")
                                        .size(9.0)
                                        .strong()
                                        .color(t.accent),
                                );
                                ui.add_space(4.0);
                            }
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
                if is_active {
                    ui.painter().line_segment(
                        [
                            header.response.rect.left_top(),
                            header.response.rect.right_top(),
                        ],
                        Stroke::new(2.0, t.accent),
                    );
                }

                ui.add(egui::Separator::default().spacing(0.0));

                // Preview mode (image or text)
                if let Some(preview) = &panel.preview {
                    use crate::panel::PreviewContent;
                    // Borrow the preview rather than cloning its (up to 1MB) text
                    // body every frame; defer the close so the immutable borrow
                    // ends before we clear it.
                    let mut close_preview = false;
                    match preview {
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
                                        close_preview = true;
                                    }
                                });
                            } else if !crate::feature_flags::enabled(
                                crate::feature_flags::RiskyFeature::ImagePreview,
                            ) {
                                ui.centered_and_justified(|ui| {
                                    ui.label(
                                        egui::RichText::new("Image preview disabled")
                                            .size(12.0)
                                            .color(t.text_muted),
                                    );
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
                                                    close_preview = true;
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
                        PreviewContent::Info(card) => {
                            Frame::NONE
                                .fill(t.bg_panel)
                                .inner_margin(Margin::same(14))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Get Info")
                                                .size(13.0)
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
                                                    close_preview = true;
                                                }
                                            },
                                        );
                                    });
                                    ui.add(egui::Separator::default().spacing(8.0));
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(&card.name)
                                            .size(15.0)
                                            .strong()
                                            .color(t.text_primary),
                                    );
                                    ui.add_space(8.0);
                                    let mut row = |k: &str, v: &str| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(k)
                                                    .size(11.0)
                                                    .color(t.text_muted),
                                            );
                                            ui.label(
                                                egui::RichText::new(v)
                                                    .size(11.0)
                                                    .color(t.text_secondary),
                                            );
                                        });
                                    };
                                    row("Kind", &card.kind);
                                    row("Size", &card.size);
                                    if let Some(n) = card.children {
                                        row("Items", &n.to_string());
                                    }
                                    row("Modified", &card.modified);
                                    row("Permissions", &card.permissions);
                                    ui.add_space(6.0);
                                    ui.label(
                                        egui::RichText::new(&card.path)
                                            .size(10.0)
                                            .color(t.text_muted),
                                    );
                                });
                        }
                    }
                    if close_preview {
                        panel.preview = None;
                    }
                    return;
                }

                // File list
                Self::render_file_list(
                    ui, panel, is_active, t, panel_side, size_bars, compare, opener, dragging,
                    metrics,
                );
            });

        tree_toggle
    }
}
