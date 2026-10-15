use super::*;
use crate::panel::{FacetSet, KindFacet, SortOrder};

#[derive(Default)]
pub struct PanelRenderOutcome {
    pub tree_toggle: bool,
    pub context_menu_request: Option<ui_state::ContextMenuCandidate>,
}

fn file_name_of(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn text_preview_scroll_id(panel_side: &str, path: &std::path::Path) -> egui::Id {
    egui::Id::new(("text_preview", panel_side, path))
}

/// The close button shared by the image, text and Get Info previews.
fn close_preview_button(ui: &mut egui::Ui) -> bool {
    let response = ui.small_button("\u{00d7}");
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Close preview")
    });
    response.on_hover_text("Close preview (Esc)").clicked()
}

/// "Preview unavailable" and its cause, announced to assistive tech as a
/// status update since it replaces a spinner without any focus change.
fn preview_failure_text(ui: &mut egui::Ui, message: &str, t: &ThemeColors) {
    ui.label(
        egui::RichText::new("Preview unavailable")
            .size(13.0)
            .strong()
            .color(t.text_primary),
    );
    ui.add_space(4.0);
    let response = ui.label(
        egui::RichText::new(message)
            .size(11.0)
            .color(t.text_secondary),
    );
    let announcement = format!("Preview unavailable: {message}");
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(egui::accesskit::Role::Status);
        node.set_live(egui::accesskit::Live::Polite);
        node.set_label(announcement);
    });
}

/// Why image previews are off: the kill switch, the Developer diagnostics
/// toggle, or a staged rollout that has not reached this install.
fn image_preview_disabled_reason(state: crate::feature_flags::FeatureSnapshot) -> &'static str {
    if state.environment_killed {
        "Turned off by COMMANDER_DISABLE_IMAGE_PREVIEW"
    } else if state.runtime_killed {
        "Turned off under Developer diagnostics \u{203a} Runtime controls"
    } else {
        "Not yet rolled out to this install"
    }
}

#[allow(clippy::too_many_arguments)]
fn render_text_preview(
    ui: &mut egui::Ui,
    path: &std::path::Path,
    content: Option<&str>,
    image_cache: &mut crate::image_cache::ImageCache,
    t: &ThemeColors,
    panel_side: &str,
    close_preview: &mut bool,
) {
    let load_state = image_cache.text_preview_load_state(path);
    Frame::NONE
        .fill(t.bg_panel)
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            // The close button is placed first so a long name truncates
            // (full name on hover) instead of pushing it out of the pane.
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if close_preview_button(ui) {
                        *close_preview = true;
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(file_name_of(path))
                                    .size(12.0)
                                    .strong()
                                    .color(t.text_primary),
                            )
                            .truncate(),
                        );
                    });
                });
            });
            ui.add(egui::Separator::default().spacing(4.0));

            // `PreviewContent::Text` owns its body, so show it whenever it is
            // present. The cache's single text slot may since have been
            // cancelled or retargeted by the other pane, which would otherwise
            // leave this pane on a spinner forever.
            if let Some(content) = content {
                egui::ScrollArea::both()
                    // Keyed by file too: the preview follows the cursor, and
                    // each file should open at its top.
                    .id_salt(text_preview_scroll_id(panel_side, path))
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
                return;
            }

            match load_state {
                crate::image_cache::TextPreviewLoadState::Idle
                | crate::image_cache::TextPreviewLoadState::Debouncing
                | crate::image_cache::TextPreviewLoadState::Loading => {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.spinner();
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("Loading preview")
                                    .size(11.0)
                                    .color(t.text_muted),
                            );
                        });
                    });
                }
                crate::image_cache::TextPreviewLoadState::Waiting => {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            ui.spinner();
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("Finishing the previous preview")
                                    .size(11.0)
                                    .color(t.text_muted),
                            );
                        });
                    });
                }
                crate::image_cache::TextPreviewLoadState::Failed(error) => {
                    ui.centered_and_justified(|ui| {
                        ui.vertical_centered(|ui| {
                            preview_failure_text(ui, error.message(), t);
                            ui.add_space(10.0);
                            if ui.button("\u{21bb} Retry").clicked() {
                                image_cache.retry_text_preview(path);
                                ui.ctx().request_repaint();
                            }
                        });
                    });
                }
                // Ready without content: the body is about to be published.
                crate::image_cache::TextPreviewLoadState::Ready => {}
            }
        });
}

/// One quick-filter chip. Its on/off state is shown by fill and published
/// as a toggle, so assistive tech hears "pressed" rather than a plain button.
fn facet_chip(ui: &mut egui::Ui, label: &str, active: bool, t: &ThemeColors) -> egui::Response {
    let fill = if active {
        t.accent.linear_multiply(0.3)
    } else {
        t.bg_card
    };
    let response = ui.add(
        egui::Button::new(egui::RichText::new(label).size(10.0).color(t.text_primary))
            .fill(fill)
            .corner_radius(CornerRadius::same(2)),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), active, label)
    });
    response
}

/// A modification-age chip: "within the last N days" (max age) or "older
/// than N days" (min age).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AgeChip {
    Within(u64),
    Older(u64),
}

/// The age chips with their hover text. "24h" rather than "Today": the
/// facet counts back from now, not from midnight.
const AGE_CHIPS: [(&str, &str, AgeChip); 4] = [
    ("24h", "Modified in the last 24 hours", AgeChip::Within(1)),
    ("Week", "Modified in the last 7 days", AgeChip::Within(7)),
    ("Month", "Modified in the last 30 days", AgeChip::Within(30)),
    (
        "Older",
        "Not modified in the last 30 days",
        AgeChip::Older(30),
    ),
];

fn age_chip_active(facets: &FacetSet, chip: AgeChip) -> bool {
    match chip {
        AgeChip::Within(days) => facets.max_age_days == Some(days),
        AgeChip::Older(days) => facets.min_age_days == Some(days),
    }
}

/// The age chips are one exclusive group: picking one clears the others,
/// so "Week" + "Older" can never combine into an always-empty list.
/// Clicking the active chip clears the group.
fn toggle_age_chip(facets: &mut FacetSet, chip: AgeChip) {
    let was_active = age_chip_active(facets, chip);
    facets.max_age_days = None;
    facets.min_age_days = None;
    if !was_active {
        match chip {
            AgeChip::Within(days) => facets.max_age_days = Some(days),
            AgeChip::Older(days) => facets.min_age_days = Some(days),
        }
    }
}

/// Frame margin plus stroke around a breadcrumb label.
const CRUMB_CHROME_WIDTH: f32 = 2.0 * 8.0 + 2.0;
/// Width of the chevron drawn between crumbs.
const CRUMB_SEPARATOR_WIDTH: f32 = 12.0;
const HIDDEN_CRUMBS_LABEL: &str = "Show hidden parent folders";

/// What a breadcrumb chip announces: a link to its folder, or the current
/// folder itself.
fn crumb_accessible_label(path: &std::path::Path, is_current: bool) -> String {
    if is_current {
        format!("Current folder {}", path.display())
    } else {
        format!("Go to {}", path.display())
    }
}

/// A step through a pane's navigation history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryStep {
    Back,
    Forward,
}

/// The mouse's side buttons: Extra1 is "back" and Extra2 "forward" on
/// every platform egui supports. Both at once cancel out.
const fn mouse_history_step(back: bool, forward: bool) -> Option<HistoryStep> {
    match (back, forward) {
        (true, false) => Some(HistoryStep::Back),
        (false, true) => Some(HistoryStep::Forward),
        _ => None,
    }
}

/// A painted 28px path-bar button with an accessible name, a tooltip, hover
/// feedback and a focus ring. A disabled button only senses hover: it keeps
/// its tooltip but neither takes focus nor reacts to clicks.
fn path_bar_button(
    ui: &mut egui::Ui,
    enabled: bool,
    label: &str,
    tooltip: &str,
    t: &ThemeColors,
) -> (egui::Rect, egui::Response) {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(28.0, 28.0), sense);
    if enabled && response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::ZERO, t.bg_hover.linear_multiply(0.3));
    }
    ui.painter().rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.0_f32, t.border),
        egui::StrokeKind::Outside,
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            CornerRadius::ZERO,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let response = response.on_hover_text(tooltip);
    (rect, response)
}

/// Name a pane's region for assistive tech, so moving focus into a pane
/// announces which one it is and whether it is active.
fn mark_pane_region(ui: &egui::Ui, panel_side: &str, is_active: bool) {
    let label = pane_accessible_label(panel_side, is_active);
    ui.ctx().accesskit_node_builder(ui.unique_id(), |node| {
        node.set_role(egui::accesskit::Role::Group);
        node.set_label(label);
    });
}

fn pane_accessible_label(panel_side: &str, is_active: bool) -> String {
    let mut chars = panel_side.chars();
    let side = chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect::<String>()
    });
    if is_active {
        format!("{side} pane, active")
    } else {
        format!("{side} pane")
    }
}

/// The pane's filter box. Both panes share the hint text, so the field is
/// named after its pane for screen readers.
fn pane_filter_field(
    ui: &mut egui::Ui,
    query: &mut String,
    width: f32,
    panel_side: &str,
) -> egui::Response {
    let response = ui.add_sized(
        Vec2::new(width, 26.0),
        egui::TextEdit::singleline(query)
            .hint_text("\u{1f50d} Filter\u{2026}")
            .desired_width(f32::INFINITY)
            .margin(egui::vec2(8.0, 4.0)),
    );
    let label = format!("Filter {panel_side} pane");
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label(label);
    });
    response
}

/// The "×" button that clears the filter text; named "Clear filter" for
/// assistive tech, which would otherwise read the glyph.
fn clear_filter_button(ui: &mut egui::Ui, t: &ThemeColors) -> egui::Response {
    let response = ui.add_sized(
        Vec2::new(26.0, 24.0),
        egui::Button::new(
            egui::RichText::new("\u{00d7}")
                .size(12.0)
                .color(t.text_secondary),
        )
        .fill(t.bg_card)
        .corner_radius(crate::theme::ROUNDING_SM),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Clear filter")
    });
    response.on_hover_text("Clear filter")
}

/// Width a row spends ahead of its name column (selection mark, icon and
/// padding) that the header row does not, so both make the same
/// [`file_row_layout`](crate::accessibility::file_row_layout) call.
const ROW_LEAD_WIDTH: f32 = 32.0;

/// Whether the column header shows Size/Modified: only when the rows below
/// show those columns too.
fn header_shows_metadata(header_width: f32) -> bool {
    crate::accessibility::file_row_layout(header_width - ROW_LEAD_WIDTH).show_metadata
}

/// The sortable column headers; returns the clicked column. Size and
/// Modified sit right-aligned but are laid out left to right, so Tab
/// visits Name, Size, Modified in the order they appear.
fn column_headers(
    ui: &mut egui::Ui,
    order_for: impl Fn(SortColumn) -> Option<SortOrder>,
    color: Color32,
    show_metadata: bool,
) -> Option<SortColumn> {
    let mut clicked = None;
    if crate::app::glyphs::sort_header(ui, "Name", order_for(SortColumn::Name), color).clicked() {
        clicked = Some(SortColumn::Name);
    }
    if !show_metadata {
        return clicked;
    }
    const GAP: f32 = 24.0;
    // `sort_header` draws an 11pt label, plus a 3pt gap and a 9pt triangle
    // when that column is sorted.
    let header_width = |ui: &egui::Ui, label: &str, order: Option<SortOrder>| {
        let text = ui
            .painter()
            .layout_no_wrap(label.to_owned(), egui::FontId::proportional(11.0), color)
            .size()
            .x;
        text + if order.is_some() { 12.0 } else { 0.0 }
    };
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.add_space(8.0);
        let size_order = order_for(SortColumn::Size);
        let modified_order = order_for(SortColumn::Modified);
        let width = header_width(ui, "Size", size_order)
            + GAP
            + header_width(ui, "Modified", modified_order);
        ui.allocate_ui_with_layout(
            Vec2::new(width, ui.available_height()),
            Layout::left_to_right(Align::Center),
            |ui| {
                if crate::app::glyphs::sort_header(ui, "Size", size_order, color).clicked() {
                    clicked = Some(SortColumn::Size);
                }
                ui.add_space(GAP);
                if crate::app::glyphs::sort_header(ui, "Modified", modified_order, color).clicked()
                {
                    clicked = Some(SortColumn::Modified);
                }
            },
        );
    });
    clicked
}

impl App {
    /// A row of toggleable quick-filter chips under the filter box.
    fn facet_chips(ui: &mut egui::Ui, panel: &mut PanelState, t: &ThemeColors) -> bool {
        let before = panel.facets();
        let mut facets = before;
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
                        facet_chip(ui, label, active, t).clicked()
                    };

                    let f = &mut facets;
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
                    for (label, hover, age) in AGE_CHIPS {
                        if facet_chip(ui, label, age_chip_active(f, age), t)
                            .on_hover_text(hover)
                            .clicked()
                        {
                            toggle_age_chip(f, age);
                        }
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
                    if !f.is_empty() && chip(ui, "\u{00d7} Clear", false) {
                        *f = FacetSet::default();
                    }
                });
            });
        if facets != before {
            panel.set_facets(facets);
            true
        } else {
            false
        }
    }

    /// Render one file panel and return deferred UI effects after its borrows
    /// end (the tree and context-menu port are owned by [`App`]).
    // The arguments are mutable borrows of disjoint `self` fields (panel,
    // image_cache) plus the shared theme/opener; a bundling struct can't
    // hold them together without fighting the borrow checker.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_panel(
        panel: &mut PanelState,
        ui: &mut egui::Ui,
        is_active: bool,
        close_preview_on_escape: bool,
        t: &ThemeColors,
        image_cache: &mut crate::image_cache::ImageCache,
        panel_side: &str,
        tree_open: bool,
        size_bars: bool,
        compare: Option<&crate::compare::CompareMap>,
        opener: &dyn Fn(crate::ports::OpenRequest),
        dragging: bool,
        metrics: crate::density::DensityMetrics,
        reduced_motion: bool,
    ) -> PanelRenderOutcome {
        let panel_bg = t.bg_panel;
        let mut tree_toggle = false;
        let mut context_menu_request = None;

        // The mouse's side buttons step through this pane's history while
        // the pointer is over it, like the Back/Forward buttons.
        let pane_rect = ui.available_rect_before_wrap();
        if ui.rect_contains_pointer(pane_rect) {
            let step = ui.input(|input| {
                mouse_history_step(
                    input.pointer.button_pressed(egui::PointerButton::Extra1),
                    input.pointer.button_pressed(egui::PointerButton::Extra2),
                )
            });
            match step {
                Some(HistoryStep::Back) if panel.can_go_back() => panel.go_back(),
                Some(HistoryStep::Forward) if panel.can_go_forward() => panel.go_forward(),
                _ => {}
            }
        }

        Frame::NONE
            .fill(panel_bg)
            .inner_margin(Margin::same(0))
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::ZERO)
            .show(ui, |ui| {
                mark_pane_region(ui, panel_side, is_active);
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
                            let can_back = panel.can_go_back();
                            let (back_rect, back_resp) =
                                path_bar_button(ui, can_back, "Back", "Back (Cmd+[)", t);
                            let back_color = if can_back {
                                t.text_primary
                            } else {
                                t.text_muted
                            };
                            crate::app::glyphs::navigation_triangle(
                                ui, back_rect, false, back_color,
                            );
                            if back_resp.clicked() && can_back {
                                panel.go_back();
                            }

                            let can_fwd = panel.can_go_forward();
                            let (fwd_rect, fwd_resp) =
                                path_bar_button(ui, can_fwd, "Forward", "Forward (Cmd+])", t);
                            let fwd_color = if can_fwd {
                                t.text_primary
                            } else {
                                t.text_muted
                            };
                            crate::app::glyphs::navigation_triangle(ui, fwd_rect, true, fwd_color);
                            if fwd_resp.clicked() && can_fwd {
                                panel.go_forward();
                            }

                            // Tree toggle button
                            let tree_color = if tree_open { t.accent } else { t.text_muted };
                            let tree_tip = if tree_open {
                                "Hide folder tree"
                            } else {
                                "Show folder tree"
                            };
                            let (tree_rect, tree_resp) =
                                path_bar_button(ui, true, "Folder tree", tree_tip, t);
                            tree_resp.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::Button,
                                    ui.is_enabled(),
                                    tree_open,
                                    "Folder tree",
                                )
                            });
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
                            // Keep the deepest crumbs that fit the measured
                            // width of their labels; always keep two.
                            let crumb_width = |label: &str| {
                                ui.painter()
                                    .layout_no_wrap(
                                        label.to_owned(),
                                        egui::FontId::proportional(12.0),
                                        t.text_primary,
                                    )
                                    .size()
                                    .x
                                    + CRUMB_CHROME_WIDTH
                                    + CRUMB_SEPARATOR_WIDTH
                            };
                            let widths: Vec<f32> =
                                all.iter().map(|c| crumb_width(&c.label)).collect();
                            let ellipsis_width = ui
                                .painter()
                                .layout_no_wrap(
                                    "\u{2026}".to_owned(),
                                    egui::TextStyle::Button.resolve(ui.style()),
                                    t.text_primary,
                                )
                                .size()
                                .x
                                + 2.0 * ui.spacing().button_padding.x
                                + CRUMB_SEPARATOR_WIDTH;
                            let max_visible = crate::crumbs::visible_crumb_count(
                                &widths,
                                ellipsis_width,
                                ui.available_width(),
                            );
                            let layout = crate::crumbs::elide_crumbs(&all, max_visible);

                            // Draw one crumb chip; returns its response. The
                            // current folder's chip is not a link, so it only
                            // senses hover and never takes keyboard focus.
                            let chip = |ui: &mut egui::Ui,
                                        crumb: &crate::crumbs::Crumb,
                                        fg: Color32,
                                        is_current: bool| {
                                let response = Frame::NONE
                                    .fill(Color32::TRANSPARENT)
                                    .corner_radius(CornerRadius::ZERO)
                                    .stroke(Stroke::new(1.0_f32, t.border))
                                    .inner_margin(Margin::symmetric(8, 3))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(&crumb.label).size(12.0).color(fg),
                                        );
                                    })
                                    .response
                                    .interact(if is_current {
                                        Sense::hover()
                                    } else {
                                        Sense::click()
                                    });
                                let accessible =
                                    crumb_accessible_label(&crumb.full_path, is_current);
                                let kind = if is_current {
                                    egui::WidgetType::Label
                                } else {
                                    egui::WidgetType::Button
                                };
                                response.widget_info(|| {
                                    egui::WidgetInfo::labeled(kind, ui.is_enabled(), &accessible)
                                });
                                if response.has_focus() {
                                    ui.painter().rect_stroke(
                                        response.rect.shrink(1.0),
                                        CornerRadius::ZERO,
                                        ui.visuals().selection.stroke,
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                response
                            };
                            let sep = |ui: &mut egui::Ui| {
                                crate::app::glyphs::breadcrumb_chevron(ui, t.text_muted);
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
                                let resp = chip(ui, &layout.head, fg, head_is_current);
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
                                    let menu = ui.menu_button("\u{2026}", |ui| {
                                        for c in &layout.collapsed {
                                            if ui.button(&c.label).clicked() {
                                                nav_to_crumb = Some(c.full_path.clone());
                                                ui.close();
                                            }
                                        }
                                    });
                                    menu.response.widget_info(|| {
                                        egui::WidgetInfo::labeled(
                                            egui::WidgetType::Button,
                                            ui.is_enabled(),
                                            HIDDEN_CRUMBS_LABEL,
                                        )
                                    });
                                    menu.response.on_hover_text(HIDDEN_CRUMBS_LABEL);
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
                                    let resp = chip(ui, c, fg, is_last);
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
                            let mut query = panel.search_query().to_string();
                            let text_filter_active = !query.trim().is_empty();
                            let clear_width = if text_filter_active { 30.0 } else { 0.0 };
                            let input_width = (ui.available_width() - clear_width).max(80.0);
                            filter_changed |=
                                pane_filter_field(ui, &mut query, input_width, panel_side)
                                    .changed();
                            if text_filter_active && clear_filter_button(ui, t).clicked() {
                                query.clear();
                                filter_changed = true;
                            }
                            if query != panel.search_query() {
                                panel.set_search_query(query);
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
                            // Measured before the "ACTIVE" tag, like the rows.
                            let show_metadata = header_shows_metadata(ui.available_width());
                            if is_active {
                                ui.label(
                                    egui::RichText::new("ACTIVE")
                                        .size(9.0)
                                        .strong()
                                        .color(t.accent),
                                );
                                ui.add_space(4.0);
                            }
                            if let Some(column) = column_headers(
                                ui,
                                |column| panel.sort_order_for(column),
                                header_text,
                                show_metadata,
                            ) {
                                panel.set_sort(column);
                            }
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
                    let mut close_preview = close_preview_on_escape;
                    match preview {
                        PreviewContent::Image(path) => {
                            let path = path.clone();
                            let preview_extent = ui.available_size();
                            image_cache.set_preview_extent(
                                preview_extent.x,
                                preview_extent.y,
                                ui.ctx().pixels_per_point(),
                            );
                            let close_button = close_preview_button;
                            if let Some(texture) = image_cache.get(&path) {
                                let tex_size = texture.size_vec2();
                                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                                    close_preview |= close_button(ui);
                                });
                                let image_extent = ui.available_size();
                                ui.centered_and_justified(|ui| {
                                    let scale = (image_extent.x / tex_size.x)
                                        .min(image_extent.y / tex_size.y)
                                        .min(1.0);
                                    let display_size =
                                        egui::vec2(tex_size.x * scale, tex_size.y * scale);
                                    // Images only sense hover by default; the
                                    // click-to-close needs an explicit sense.
                                    let resp = ui.add(
                                        egui::Image::from_texture(egui::load::SizedTexture::new(
                                            texture.id(),
                                            display_size,
                                        ))
                                        .alt_text(file_name_of(&path))
                                        .sense(Sense::click()),
                                    );
                                    if resp.clicked() {
                                        close_preview = true;
                                    }
                                });
                            } else {
                                match image_cache.load_state(&path) {
                                    crate::image_cache::PreviewLoadState::Disabled => {
                                        ui.centered_and_justified(|ui| {
                                            ui.vertical_centered(|ui| {
                                                ui.label(
                                                    egui::RichText::new("Image preview disabled")
                                                        .size(12.0)
                                                        .color(t.text_muted),
                                                );
                                                ui.add_space(4.0);
                                                ui.label(
                                                    egui::RichText::new(
                                                        image_preview_disabled_reason(
                                                            crate::feature_flags::snapshot(
                                                                crate::feature_flags::RiskyFeature::ImagePreview,
                                                            ),
                                                        ),
                                                    )
                                                    .size(11.0)
                                                    .color(t.text_muted),
                                                );
                                                ui.add_space(10.0);
                                                close_preview |= close_button(ui);
                                            });
                                        });
                                    }
                                    crate::image_cache::PreviewLoadState::Failed(failure) => {
                                        ui.centered_and_justified(|ui| {
                                            ui.vertical_centered(|ui| {
                                                preview_failure_text(ui, failure.message(), t);
                                                ui.add_space(10.0);
                                                ui.horizontal_centered(|ui| {
                                                    if ui.button("\u{21bb} Retry").clicked() {
                                                        image_cache.retry(&path);
                                                        ui.ctx().request_repaint();
                                                    }
                                                    close_preview |= close_button(ui);
                                                });
                                            });
                                        });
                                    }
                                    crate::image_cache::PreviewLoadState::Loading
                                    | crate::image_cache::PreviewLoadState::Idle => {
                                        ui.centered_and_justified(|ui| {
                                            ui.vertical_centered(|ui| {
                                                ui.spinner();
                                                ui.add_space(10.0);
                                                close_preview |= close_button(ui);
                                            });
                                        });
                                    }
                                }
                            }
                        }
                        PreviewContent::Pending(identity) => render_text_preview(
                            ui,
                            &identity.path,
                            None,
                            image_cache,
                            t,
                            panel_side,
                            &mut close_preview,
                        ),
                        PreviewContent::Text { identity, content } => render_text_preview(
                            ui,
                            &identity.path,
                            Some(content.as_ref()),
                            image_cache,
                            t,
                            panel_side,
                            &mut close_preview,
                        ),
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
                                                if close_preview_button(ui) {
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
                        if let Some(
                            PreviewContent::Pending(identity)
                            | PreviewContent::Text { identity, .. },
                        ) = panel.preview.as_ref()
                        {
                            image_cache.cancel_text_preview_if_active(identity);
                        }
                        panel.preview = None;
                    }
                    return;
                }

                // File list
                // Keep the scroll viewport inside the parent panel after the
                // final separator stroke and egui's cursor rounding.
                let list_size = (ui.available_size() - Vec2::new(0.0, 1.0)).max(Vec2::ZERO);
                ui.allocate_ui_with_layout(list_size, Layout::top_down(Align::Min), |list_ui| {
                    list_ui.set_min_size(list_size);
                    list_ui.set_max_size(list_size);
                    context_menu_request = Self::render_file_list(
                        list_ui,
                        panel,
                        is_active,
                        t,
                        panel_side,
                        size_bars,
                        compare,
                        opener,
                        dragging,
                        metrics,
                        reduced_motion,
                    );
                });
            });

        PanelRenderOutcome {
            tree_toggle,
            context_menu_request,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accesskit_nodes(
        add: impl FnMut(&mut egui::Ui),
    ) -> Vec<(egui::accesskit::NodeId, egui::accesskit::Node)> {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let output = crate::testutil::take_egui_output(ctx.run_ui(egui::RawInput::default(), add));
        output
            .platform_output
            .accesskit_update
            .expect("headless AccessKit output")
            .nodes
    }

    fn node_labelled<'a>(
        nodes: &'a [(egui::accesskit::NodeId, egui::accesskit::Node)],
        label: &str,
    ) -> &'a egui::accesskit::Node {
        nodes
            .iter()
            .map(|(_, node)| node)
            .find(|node| node.label() == Some(label))
            .unwrap_or_else(|| panic!("no node labelled {label:?}"))
    }

    #[test]
    fn facet_chips_publish_their_toggle_state() {
        let t = ThemeColors::dark();
        let nodes = accesskit_nodes(|ui| {
            facet_chip(ui, "Images", true, &t);
            facet_chip(ui, "Docs", false, &t);
        });
        assert_eq!(
            node_labelled(&nodes, "Images").toggled(),
            Some(egui::accesskit::Toggled::True)
        );
        assert_eq!(
            node_labelled(&nodes, "Docs").toggled(),
            Some(egui::accesskit::Toggled::False)
        );
    }

    #[test]
    fn age_chips_are_one_exclusive_group() {
        let mut facets = FacetSet::default();
        toggle_age_chip(&mut facets, AgeChip::Within(7));
        toggle_age_chip(&mut facets, AgeChip::Older(30));
        assert_eq!(facets.max_age_days, None);
        assert_eq!(facets.min_age_days, Some(30));
        toggle_age_chip(&mut facets, AgeChip::Within(1));
        assert_eq!(facets.max_age_days, Some(1));
        assert_eq!(facets.min_age_days, None);
        // Clicking the active chip clears the group.
        toggle_age_chip(&mut facets, AgeChip::Within(1));
        assert_eq!(facets, FacetSet::default());
    }

    #[test]
    fn the_one_day_age_chip_does_not_claim_calendar_today() {
        let (label, hover, chip) = AGE_CHIPS[0];
        assert_eq!(chip, AgeChip::Within(1));
        assert_eq!(label, "24h");
        assert!(hover.contains("24 hours"));
        assert!(AGE_CHIPS.iter().all(|(label, ..)| *label != "Today"));
    }

    #[test]
    fn path_bar_buttons_are_named_and_disabled_ones_are_marked() {
        let t = ThemeColors::dark();
        let nodes = accesskit_nodes(|ui| {
            path_bar_button(ui, false, "Back", "Back", &t);
            path_bar_button(ui, true, "Forward", "Forward", &t);
        });
        assert!(node_labelled(&nodes, "Back").is_disabled());
        let forward = node_labelled(&nodes, "Forward");
        assert!(!forward.is_disabled());
        assert_eq!(forward.role(), egui::accesskit::Role::Button);
    }

    #[test]
    fn crumbs_announce_their_target_folder() {
        let path = std::path::Path::new("/a/b");
        assert_eq!(crumb_accessible_label(path, false), "Go to /a/b");
        assert_eq!(crumb_accessible_label(path, true), "Current folder /a/b");
    }

    #[test]
    fn mouse_side_buttons_step_through_history() {
        assert_eq!(mouse_history_step(true, false), Some(HistoryStep::Back));
        assert_eq!(mouse_history_step(false, true), Some(HistoryStep::Forward));
        assert_eq!(mouse_history_step(false, false), None);
        assert_eq!(mouse_history_step(true, true), None);
    }

    /// Labels of the nodes in AccessKit tree order (depth first), which is
    /// the order widgets were created in and so the Tab order.
    fn labels_in_tree_order(ctx_run: impl FnMut(&mut egui::Ui)) -> Vec<String> {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let output =
            crate::testutil::take_egui_output(ctx.run_ui(egui::RawInput::default(), ctx_run));
        let update = output
            .platform_output
            .accesskit_update
            .expect("headless AccessKit output");
        let nodes: std::collections::HashMap<_, _> = update.nodes.into_iter().collect();
        let mut stack = vec![update.tree.expect("full tree").root];
        let mut labels = Vec::new();
        while let Some(id) = stack.pop() {
            let Some(node) = nodes.get(&id) else { continue };
            if let Some(label) = node.label() {
                labels.push(label.to_string());
            }
            stack.extend(node.children().iter().rev());
        }
        labels
    }

    #[test]
    fn column_headers_tab_in_visual_order() {
        let labels = labels_in_tree_order(|ui| {
            ui.horizontal(|ui| {
                column_headers(ui, |_| None, Color32::WHITE, true);
            });
        });
        let position = |name: &str| {
            labels
                .iter()
                .position(|label| label.starts_with(name))
                .unwrap_or_else(|| panic!("no {name} header in {labels:?}"))
        };
        assert!(position("Name") < position("Size"));
        assert!(position("Size") < position("Modified"));
    }

    #[test]
    fn narrow_headers_drop_the_columns_the_rows_drop() {
        assert!(!header_shows_metadata(150.0));
        assert!(header_shows_metadata(400.0));
        // The rows lose their metadata below 160 px of name space.
        assert!(!header_shows_metadata(160.0 + ROW_LEAD_WIDTH - 1.0));
        let labels = labels_in_tree_order(|ui| {
            column_headers(ui, |_| None, Color32::WHITE, false);
        });
        assert!(labels.iter().all(|label| !label.starts_with("Size")));
    }

    #[test]
    fn filter_controls_are_named_for_their_pane() {
        let t = ThemeColors::dark();
        let nodes = accesskit_nodes(|ui| {
            let mut query = String::from("abc");
            pane_filter_field(ui, &mut query, 120.0, "left");
            clear_filter_button(ui, &t);
        });
        assert_eq!(
            node_labelled(&nodes, "Filter left pane").role(),
            egui::accesskit::Role::TextInput
        );
        assert_eq!(
            node_labelled(&nodes, "Clear filter").role(),
            egui::accesskit::Role::Button
        );
    }

    #[test]
    fn panes_are_named_regions() {
        assert_eq!(pane_accessible_label("left", true), "Left pane, active");
        assert_eq!(pane_accessible_label("right", false), "Right pane");
        let nodes = accesskit_nodes(|ui| {
            ui.scope(|ui| mark_pane_region(ui, "right", false));
        });
        assert_eq!(
            node_labelled(&nodes, "Right pane").role(),
            egui::accesskit::Role::Group
        );
    }

    #[test]
    fn preview_close_buttons_share_name_and_tooltip() {
        let nodes = accesskit_nodes(|ui| {
            close_preview_button(ui);
        });
        assert_eq!(
            node_labelled(&nodes, "Close preview").role(),
            egui::accesskit::Role::Button
        );
    }

    #[test]
    fn preview_failures_are_announced() {
        let t = ThemeColors::dark();
        let nodes = accesskit_nodes(|ui| preview_failure_text(ui, "Binary file", &t));
        let status = node_labelled(&nodes, "Preview unavailable: Binary file");
        assert_eq!(status.role(), egui::accesskit::Role::Status);
        assert_eq!(status.live(), Some(egui::accesskit::Live::Polite));
    }

    #[test]
    fn disabled_image_preview_names_the_switch() {
        let state = |environment_killed, runtime_killed| crate::feature_flags::FeatureSnapshot {
            feature: crate::feature_flags::RiskyFeature::ImagePreview,
            runtime_killed,
            environment_killed,
            rollout_percent: 100,
            bucket: 0,
            enabled: false,
        };
        assert!(
            image_preview_disabled_reason(state(true, true))
                .contains("COMMANDER_DISABLE_IMAGE_PREVIEW")
        );
        assert!(image_preview_disabled_reason(state(false, true)).contains("Runtime controls"));
        assert!(image_preview_disabled_reason(state(false, false)).contains("rolled out"));
    }

    #[test]
    fn image_alt_text_is_the_file_name() {
        assert_eq!(
            file_name_of(std::path::Path::new("/photos/cat.png")),
            "cat.png"
        );
    }

    #[test]
    fn each_previewed_file_gets_its_own_scroll_state() {
        let a = std::path::Path::new("/a.txt");
        let b = std::path::Path::new("/b.txt");
        assert_ne!(
            text_preview_scroll_id("left", a),
            text_preview_scroll_id("left", b)
        );
        assert_ne!(
            text_preview_scroll_id("left", a),
            text_preview_scroll_id("right", a)
        );
    }
}
