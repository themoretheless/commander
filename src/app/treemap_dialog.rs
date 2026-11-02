//! Responsive disk-usage map and cancellable compressed-tree overview.

use super::*;
use crate::panel::FileEntry;
use crate::selection_summary::{Kind, kind_of};

const ROW_HEIGHT: f32 = 28.0;

// Integer tenths keep exact u64 measurements out of floating-point arithmetic.
// Only the painted bar is clamped; inconsistent/partial ratios stay visible.
fn share_tenths(bytes: u64, denominator: u64) -> Option<u128> {
    (denominator != 0).then(|| (u128::from(bytes) * 1000) / u128::from(denominator))
}
fn share_text(share: Option<u128>) -> String {
    share.map_or_else(
        || "—".into(),
        |value| format!("{}.{:01}%", value / 10, value % 10),
    )
}
/// Height of one wrapped row of the kind legend under the map.
const LEGEND_ROW_HEIGHT: f32 = 20.0;
/// Legend swatch edge, and the gap after each swatch-and-label item.
const LEGEND_SWATCH: f32 = 10.0;
const LEGEND_ITEM_GAP: f32 = 6.0;
/// Below this the tiles are too small to read or click.
const MIN_MAP_SIZE: Vec2 = Vec2::new(120.0, 80.0);

/// The map's size in the space left by the pane, or `None` when the pane is
/// too small to draw a usable map (rather than overflowing it).
fn map_size(available: Vec2) -> Option<Vec2> {
    (available.x >= MIN_MAP_SIZE.x && available.y >= MIN_MAP_SIZE.y).then_some(available)
}

/// Placeholder when the map has nothing to draw. A finished scan has no more
/// items coming, so say why it is empty instead of "yet".
const fn map_empty_message(scan_finished: bool, any_items: bool) -> &'static str {
    match (scan_finished, any_items) {
        (false, _) => "No measured items yet",
        (true, false) => "Folder is empty",
        (true, true) => "Folder holds only empty items",
    }
}

/// Rows the wrapped legend needs for items of `widths` in a `available`-wide
/// pane. Items are packed whole, which never needs fewer rows than egui's
/// per-widget wrapping, so the reserved height is never too small.
fn legend_rows(widths: &[f32], available: f32) -> usize {
    let mut rows = 0;
    let mut used = 0.0;
    for &width in widths {
        if rows == 0 || (used > 0.0 && used + width > available) {
            rows += 1;
            used = 0.0;
        }
        used += width;
    }
    rows
}

/// Kinds present on the map, in kind order, for the colour legend.
fn legend_kinds<'a>(entries: impl IntoIterator<Item = &'a FileEntry>) -> Vec<Kind> {
    let mut kinds: Vec<Kind> = entries.into_iter().map(kind_of).collect();
    kinds.sort();
    kinds.dedup();
    kinds
}

impl App {
    fn start_disk_usage_scan(&mut self, ctx: &egui::Context) {
        let Some(state) = self.ui.tools.treemap.as_mut() else {
            return;
        };
        let root = state.initial.dir.clone();
        let repaint = ctx.clone();
        if state
            .overview
            .as_ref()
            .is_some_and(|snapshot| snapshot.metric != state.metric)
        {
            state.overview = None;
            state.tree_filter.invalidate();
        }
        let notify: crate::tree_overview::Notify =
            std::sync::Arc::new(move || repaint.request_repaint());
        state.run = Some(
            if state.metric == crate::disk_usage_metric::Metric::Logical {
                crate::tree_overview::spawn(root.clone(), notify)
            } else {
                crate::tree_overview::spawn_metric(root.clone(), state.metric, notify)
            },
        );
        state.progress = crate::tree_overview::ScanProgress {
            directories: 1,
            current: root,
            ..Default::default()
        };
        state.stopping = false;
        state.error = None;
    }

    fn poll_disk_usage_scan(&mut self) {
        let (events, disconnected) = self
            .ui
            .tools
            .treemap
            .as_ref()
            .and_then(|state| state.run.as_ref())
            .map_or_else(Default::default, |run| {
                super::drain_events(|| run.try_recv())
            });
        if let Some(state) = self.ui.tools.treemap.as_mut() {
            for event in events {
                match event {
                    crate::tree_overview::OverviewEvent::Progress(progress) => {
                        state.progress = progress;
                    }
                    crate::tree_overview::OverviewEvent::Complete(snapshot) => {
                        state.progress = snapshot.progress.clone();
                        state.overview = Some(snapshot);
                        state.tree_filter.invalidate();
                        state.run = None;
                        state.stopping = false;
                    }
                }
            }
            if disconnected && state.run.is_some() {
                state.run = None;
                state.stopping = false;
                state.error = Some("Disk scan stopped before producing a snapshot".to_string());
            }
        }
    }

    pub(crate) fn open_treemap(&mut self, ctx: &egui::Context) {
        let mut initial = self.ws.treemap_snapshot();
        initial.items.retain(|(_, bytes)| *bytes > 0);
        self.ui.tools.replace_on(self.ws.active.other());
        self.ui.tools.treemap = Some(DiskUsageState {
            metric: Default::default(),
            progress: crate::tree_overview::ScanProgress {
                directories: 1,
                current: initial.dir.clone(),
                ..Default::default()
            },
            initial,
            mode: DiskUsageMode::Map,
            run: None,
            overview: None,
            tree_filter: crate::tree_filter::View::default(),
            tree_share_parent: false,
            stopping: false,
            error: None,
        });
        self.start_disk_usage_scan(ctx);
    }

    pub(crate) fn show_treemap_tool(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let escape_requested =
            self.take_escape_request(crate::accessibility::EscapeRoute::PaneTool);
        if self.ui.tools.treemap.is_none() {
            return;
        }
        if super::ui_state::modal_close_requested(true, escape_requested) {
            self.ui.tools.treemap = None;
            return;
        }
        self.poll_disk_usage_scan();

        let t = self.colors;
        let dark = t.is_dark();
        let window_open = true;
        let mut stop = false;
        let mut rescan = false;
        let mut open_path = None;
        let mut copy_report = None;

        {
            let state = self.ui.tools.treemap.as_mut().expect("checked above");
            ui.vertical(|ui| {
                show_disk_usage_header(ui, state, t, &mut stop, &mut rescan, &mut copy_report);
                ui.horizontal_wrapped(|ui| {
                    for metric in [crate::disk_usage_metric::Metric::Logical,crate::disk_usage_metric::Metric::FileAllocation,crate::disk_usage_metric::Metric::FileCount] {
                        let response=ui.add_enabled_ui(state.run.is_none(),|ui|ui.selectable_label(state.metric==metric,metric.label())).inner.on_hover_text("Switching re-scans this captured folder. File allocation sums observed regular-file data blocks, counts hard links once, excludes directory/link storage, and is not reclaimable space. File count counts every observed non-directory entry, including zero-length files, symbolic links and hard-link aliases.");
                        #[cfg(any(test,feature="visual-qa"))]
                        ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("disk-usage-metric",metric.key())),response.rect));
                        if response.clicked() && state.metric!=metric {state.metric=metric;rescan=true;}
                    }
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                match state.mode {
                    DiskUsageMode::Map => {
                        if state.metric!=crate::disk_usage_metric::Metric::Logical && state.overview.is_none() {ui.label("Waiting for selected measurements");return;}
                        let items = state
                            .overview
                            .as_ref()
                            .map_or(state.initial.items.as_slice(), |snapshot| {
                                snapshot.top_level.as_slice()
                            });
                        let scan_finished = state.run.is_none()
                            && state.overview.as_ref().is_some_and(|s| !s.cancelled);
                        show_map(ui, items, scan_finished, t, dark, &mut open_path, state.overview.as_ref());
                    }
                    DiskUsageMode::Tree => {
                        show_tree(ui, state, t, &mut open_path, &mut copy_report);
                    }
                }
            });
        }

        if super::ui_state::modal_close_requested(window_open, false) {
            self.ui.tools.treemap = None;
            return;
        }
        if let Some(report) = copy_report {
            match report {
                Ok(text) => self.write_clipboard(&text, "disk-usage report", ctx),
                Err(error) => self.toast(ctx, crate::toasts::ToastKind::Error, error, false),
            }
        }
        if stop
            && let Some(state) = self.ui.tools.treemap.as_mut()
            && let Some(run) = state.run.as_ref()
        {
            run.cancel();
            state.stopping = true;
        }
        if rescan {
            self.start_disk_usage_scan(ctx);
        }
        if let Some((path, is_dir)) = open_path {
            if is_dir {
                self.ws.active_panel().navigate_to(path);
            } else {
                self.ws.reveal(&path);
            }
            self.ui.tools.treemap = None;
        }
    }
}

fn show_disk_usage_header(
    ui: &mut egui::Ui,
    state: &mut DiskUsageState,
    t: crate::theme::ThemeColors,
    stop: &mut bool,
    rescan: &mut bool,
    copy_report: &mut Option<Result<String, String>>,
) {
    ui.horizontal(|ui| {
        let title_width = (ui.available_width() - 230.0).max(220.0);
        ui.vertical(|ui| {
            ui.set_width(title_width);
            ui.label(
                egui::RichText::new("Disk usage")
                    .size(13.0)
                    .strong()
                    .color(t.text_primary),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(state.initial.dir.display().to_string())
                        .size(10.0)
                        .color(t.text_muted),
                )
                .truncate(),
            )
            .on_hover_text(state.initial.dir.display().to_string());
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if state.run.is_some() {
                if ui
                    .add_enabled(!state.stopping, egui::Button::new("Stop"))
                    .clicked()
                {
                    *stop = true;
                }
            } else {
                let response = ui.button("Rescan");
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|data| {
                    data.insert_temp(egui::Id::new("disk-usage-rescan"), response.rect)
                });
                *rescan |= response.clicked();
            }
            for (mode, label) in [(DiskUsageMode::Tree, "Tree"), (DiskUsageMode::Map, "Map")] {
                let response = ui.selectable_value(&mut state.mode, mode, label);
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|data| {
                    data.insert_temp(egui::Id::new(("disk-usage-mode", label)), response.rect)
                });
                #[cfg(not(any(test, feature = "visual-qa")))]
                let _ = response;
            }
        });
    });

    let status = if state.run.is_some() {
        let action = if state.stopping {
            "Stopping"
        } else {
            "Scanning"
        };
        format!(
            "{action} {} entries | {} folders | {}",
            state.progress.entries,
            state.progress.directories,
            state
                .metric
                .text(state.progress.bytes, state.progress.measurement)
        )
    } else if let Some(snapshot) = &state.overview {
        let action = if snapshot.cancelled {
            "Stopped"
        } else {
            "Complete"
        };
        format!(
            "{action} | {} folders | {} files | {} | {:.1}s",
            snapshot.progress.directories,
            snapshot.progress.files,
            snapshot
                .metric
                .text(snapshot.progress.bytes, snapshot.progress.measurement),
            snapshot.elapsed.as_secs_f32()
        )
    } else {
        "Waiting to scan".to_string()
    };
    let measurement = state
        .overview
        .as_ref()
        .filter(|_| state.run.is_none())
        .map_or(state.progress.measurement, |s| s.progress.measurement);
    let metric_label = if state.run.is_some() {
        state.metric.label()
    } else {
        state
            .overview
            .as_ref()
            .map_or(state.metric, |s| s.metric)
            .label()
    };
    let known = if state.run.is_some()
        || state
            .overview
            .as_ref()
            .is_some_and(|s| !s.measurement_complete())
    {
        "Known subtotal · "
    } else {
        ""
    };
    ui.small(format!(
        "{known}{metric_label} · {} unavailable measurements{}",
        measurement.unknown,
        if measurement.overflowed {
            " · measured total overflowed"
        } else {
            ""
        }
    ));
    let status_label = ui.label(egui::RichText::new(status).size(10.0).color(t.text_muted));
    // Announce when the scan finishes or stops; while it runs the counts
    // change every frame, so the region stays quiet.
    let scanning = state.run.is_some();
    ui.ctx().accesskit_node_builder(status_label.id, |node| {
        node.set_role(egui::accesskit::Role::Status);
        node.set_live(if scanning {
            egui::accesskit::Live::Off
        } else {
            egui::accesskit::Live::Polite
        });
    });

    let copy = ui.add_enabled(state.run.is_none() && state.error.is_none() && state.overview.is_some(), egui::Button::new("Copy scan JSON"))
        .on_hover_text("Copy the retained full scan model: root totals, map items, compressed directories and partial-scan flags. Independent of the tree filter; not a raw file inventory. Up to 1 MiB, no extra reads.");
    #[cfg(any(test, feature = "visual-qa"))]
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::new("disk-usage-copy-json"), copy.rect));
    if copy.clicked()
        && let Some(snapshot) = &state.overview
    {
        *copy_report = Some(crate::disk_usage_export::snapshot_json(snapshot));
    }

    if state.run.is_some() && !state.progress.current.as_os_str().is_empty() {
        ui.add(
            egui::Label::new(
                egui::RichText::new(state.progress.current.display().to_string())
                    .size(10.0)
                    .color(t.text_muted),
            )
            .truncate(),
        )
        .on_hover_text(state.progress.current.display().to_string());
    }
    if let Some(snapshot) = &state.overview {
        if snapshot.inaccessible_count > 0 {
            // A focusable header, so keyboard and AT users can open the list.
            egui::CollapsingHeader::new(
                egui::RichText::new(format!(
                    "{} paths could not be read",
                    snapshot.inaccessible_count
                ))
                .size(10.0)
                .color(t.accent_warning),
            )
            .id_salt(("disk-usage-inaccessible", &snapshot.root))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for path in &snapshot.inaccessible_paths {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(path.display().to_string())
                                        .size(10.0)
                                        .color(t.text_muted),
                                )
                                .truncate(),
                            );
                        }
                    });
            });
        }
        if snapshot.truncated {
            ui.label(
                egui::RichText::new(format!(
                    "Tree view capped at {} directory nodes",
                    crate::tree_overview::NODE_CAP
                ))
                .size(10.0)
                .color(t.accent_warning),
            );
        }
        if snapshot.top_level_truncated {
            ui.label(
                egui::RichText::new(format!(
                    "Map limited to the {} largest items",
                    crate::tree_overview::TOP_LEVEL_CAP
                ))
                .size(10.0)
                .color(t.accent_warning),
            );
        }
    }
    if let Some(error) = &state.error {
        ui.label(egui::RichText::new(error).size(10.0).color(t.accent_red));
    }
}

fn show_map(
    ui: &mut egui::Ui,
    items: &[(FileEntry, u64)],
    scan_finished: bool,
    t: crate::theme::ThemeColors,
    dark: bool,
    open_path: &mut Option<(PathBuf, bool)>,
    snapshot: Option<&crate::tree_overview::OverviewSnapshot>,
) {
    let metric = snapshot.map_or(crate::disk_usage_metric::Metric::Logical, |s| s.metric);
    let visible: Vec<_> = items.iter().filter(|(_, bytes)| *bytes > 0).collect();
    if visible.is_empty() {
        ui.label(
            egui::RichText::new(
                if metric == crate::disk_usage_metric::Metric::FileAllocation {
                    if snapshot.is_some_and(|s| !s.measurement_complete()) {
                        "No known allocated file blocks; measurements are incomplete"
                    } else {
                        "No allocated regular-file data blocks observed"
                    }
                } else if metric == crate::disk_usage_metric::Metric::FileCount {
                    if snapshot.is_some_and(|s| !s.measurement_complete()) {
                        "No known entries; measurements are incomplete"
                    } else {
                        "No non-directory entries observed"
                    }
                } else {
                    map_empty_message(scan_finished, !items.is_empty())
                },
            )
            .size(12.0)
            .color(t.text_muted),
        );
        return;
    }

    // Reserve every row the wrapped legend will take, so a narrow pane with
    // many kinds does not push the legend past the bottom edge.
    let kinds = legend_kinds(visible.iter().map(|(entry, _)| entry));
    let spacing = ui.spacing().item_spacing.x;
    let widths: Vec<f32> = kinds
        .iter()
        .map(|kind| {
            let label = ui
                .painter()
                .layout_no_wrap(
                    kind.label().to_string(),
                    egui::FontId::proportional(10.0),
                    t.text_muted,
                )
                .size()
                .x;
            LEGEND_SWATCH + label + LEGEND_ITEM_GAP + 3.0 * spacing
        })
        .collect();
    // A wrapped row is at least the interact height plus the row gap, and
    // egui inserts one more item gap between the map and the legend.
    let spacing = ui.spacing();
    let row_height = LEGEND_ROW_HEIGHT.max(spacing.interact_size.y) + spacing.item_spacing.y;
    let legend_height =
        legend_rows(&widths, ui.available_width()) as f32 * row_height + spacing.item_spacing.y;
    let Some(size) = map_size(ui.available_size() - Vec2::new(0.0, legend_height)) else {
        ui.label(
            egui::RichText::new("Pane too small for the map; widen it or use Tree")
                .size(12.0)
                .color(t.text_muted),
        );
        return;
    };
    let (width, height) = (size.x, size.y);
    let weights: Vec<f64> = visible.iter().map(|(_, bytes)| *bytes as f64).collect();
    let tiles = crate::treemap::squarify(
        &weights,
        crate::treemap::Rect {
            x: 0.0,
            y: 0.0,
            w: width as f64,
            h: height as f64,
        },
    );
    let (response, painter) = ui.allocate_painter(Vec2::new(width, height), Sense::hover());
    let origin = response.rect.min;

    for (index, ((entry, bytes), tile)) in visible.iter().zip(&tiles).enumerate() {
        if tile.w <= 0.0 || tile.h <= 0.0 {
            continue;
        }
        let rect = egui::Rect::from_min_size(
            origin + Vec2::new(tile.x as f32, tile.y as f32),
            Vec2::new(tile.w as f32, tile.h as f32),
        );
        let (red, green, blue) = crate::file_color::kind_color(kind_of(entry), dark);
        let fill = Color32::from_rgb(red, green, blue);
        painter.rect_filled(rect, CornerRadius::same(1), fill);
        painter.rect_stroke(
            rect,
            CornerRadius::same(1),
            Stroke::new(1.0_f32, t.bg_panel),
            egui::StrokeKind::Inside,
        );
        if rect.width() > 54.0 && rect.height() > 22.0 {
            let luminance = 0.299 * red as f32 + 0.587 * green as f32 + 0.114 * blue as f32;
            let foreground = if luminance > 140.0 {
                Color32::from_rgb(20, 20, 24)
            } else {
                Color32::from_rgb(244, 244, 248)
            };
            // Clip to the tile so long names never spill into neighbours.
            painter.with_clip_rect(rect.shrink(2.0)).text(
                rect.min + Vec2::new(5.0, 3.0),
                egui::Align2::LEFT_TOP,
                &entry.name,
                egui::FontId::proportional(11.0),
                foreground,
            );
        }
        let interaction = ui
            .interact(rect, ui.id().with(("disk-map", index)), Sense::click())
            .on_hover_text(format!(
                "{} | {} | {}: {}",
                entry.path.display(),
                kind_of(entry).label(),
                metric.label(),
                metric.text(
                    *bytes,
                    snapshot.map_or(Default::default(), |s| s.measurement_for(&entry.path))
                )
            ));
        if interaction.clicked() {
            *open_path = Some((entry.path.clone(), entry.is_dir));
        }
    }

    // Colour legend, so kinds are not told apart by fill alone.
    ui.horizontal_wrapped(|ui| {
        for kind in kinds {
            let (red, green, blue) = crate::file_color::kind_color(kind, dark);
            let (swatch, _) = ui.allocate_exact_size(Vec2::splat(LEGEND_SWATCH), Sense::hover());
            ui.painter().rect_filled(
                swatch,
                CornerRadius::same(2),
                Color32::from_rgb(red, green, blue),
            );
            ui.label(
                egui::RichText::new(kind.label())
                    .size(10.0)
                    .color(t.text_muted),
            );
            ui.add_space(LEGEND_ITEM_GAP);
        }
    });
}

fn show_tree(
    ui: &mut egui::Ui,
    state: &mut DiskUsageState,
    t: crate::theme::ThemeColors,
    open_path: &mut Option<(PathBuf, bool)>,
    copy_report: &mut Option<Result<String, String>>,
) {
    let Some(snapshot) = &state.overview else {
        ui.label(
            egui::RichText::new("Scanning directory tree")
                .size(12.0)
                .color(t.text_muted),
        );
        return;
    };
    ui.horizontal_wrapped(|ui| {
        ui.label("Filter directories");
        let query = ui.add(
            egui::TextEdit::singleline(&mut state.tree_filter.options.query)
                .id(egui::Id::new("disk-tree-filter-input"))
                .char_limit(256)
                .desired_width(180.0)
                .hint_text("Relative path"),
        );
        query.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::TextEdit,
                query.enabled(),
                "Filter disk-usage directories by relative path",
            )
        });
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new(("disk-tree-filter", "query")), query.rect)
        });
        for (label, value) in [
            ("Fuzzy", &mut state.tree_filter.options.fuzzy),
            ("Match case", &mut state.tree_filter.options.match_case),
        ] {
            let response = ui.checkbox(value, label);
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| {
                data.insert_temp(egui::Id::new(("disk-tree-filter", label)), response.rect)
            });
            #[cfg(not(any(test, feature = "visual-qa")))]
            let _ = response;
        }
        let clear = ui.add_enabled(
            !state.tree_filter.options.query.is_empty(),
            egui::Button::new("Clear"),
        );
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new(("disk-tree-filter", "Clear")), clear.rect)
        });
        if clear.clicked() {
            state.tree_filter.options.query.clear();
            query.request_focus();
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Share of");
        for (parent, label) in [(false, "Root"), (true, "Parent")] {
            let response = ui.selectable_value(&mut state.tree_share_parent, parent, label)
                .on_hover_text("Uses full snapshot totals, including filtered-out rows. Parent means the nearest represented ancestor; compressed chains may omit physical parents. Top-level rows use the root. Zero totals have no percentage; stopped/capped scans remain partial.");
            #[cfg(any(test, feature = "visual-qa"))]
            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("disk-tree-share", label)), response.rect));
            #[cfg(not(any(test, feature = "visual-qa")))]
            let _ = response;
        }
    });
    let reset_scroll = state.tree_filter.search_changed();
    state.tree_filter.refresh(&snapshot.root, &snapshot.nodes);
    ui.horizontal_wrapped(|ui| {
        let searching = !state.tree_filter.options.query.trim().is_empty();
        let collapse = ui.add_enabled(
            !searching && state.tree_filter.has_children.iter().any(|&value| value),
            egui::Button::new("Collapse all"),
        );
        let expand = ui.add_enabled(
            !state.tree_filter.options.collapsed.is_empty(),
            egui::Button::new("Expand all"),
        );
        #[cfg(any(test, feature = "visual-qa"))]
        ui.ctx().data_mut(|data| {
            data.insert_temp(egui::Id::new(("disk-tree-collapse", "all")), collapse.rect);
            data.insert_temp(egui::Id::new(("disk-tree-expand", "all")), expand.rect);
        });
        if collapse.clicked() {
            state.tree_filter.options.collapsed = snapshot
                .nodes
                .iter()
                .enumerate()
                .filter(|(index, _)| state.tree_filter.has_children[*index])
                .map(|(_, node)| node.path.clone())
                .collect();
        }
        if expand.clicked() {
            state.tree_filter.options.collapsed.clear();
        }
        if searching && !state.tree_filter.options.collapsed.is_empty() {
            ui.small("Search reveals collapsed branches");
        }
    });
    state.tree_filter.refresh(&snapshot.root, &snapshot.nodes);
    if state.tree_filter.hidden_by_collapse > 0 {
        ui.small(format!(
            "{} rows hidden in collapsed branches",
            state.tree_filter.hidden_by_collapse
        ));
    }
    ui.small(format!("{} matching directories · {} shown with ancestor context · {} loaded. Values remain full subtree totals.", state.tree_filter.matches, state.tree_filter.indices.len(), snapshot.nodes.len()));
    let copy = ui.add_enabled(state.run.is_none() && state.error.is_none() && !state.tree_filter.indices.is_empty(), egui::Button::new(format!("Copy visible CSV ({})", state.tree_filter.indices.len())))
        .on_hover_text("Copy shown compressed directory rows in view order, with full subtree totals, ancestor-context markers, active filter and partial-scan flags. Nested totals must not be summed. Up to 1 MiB; no extra reads.");
    #[cfg(any(test, feature = "visual-qa"))]
    ui.ctx()
        .data_mut(|data| data.insert_temp(egui::Id::new("disk-usage-copy-csv"), copy.rect));
    if copy.clicked() {
        *copy_report = Some(crate::disk_usage_export::visible_csv(
            snapshot,
            &state.tree_filter,
        ));
    }
    if state.tree_filter.indices.is_empty() && !snapshot.nodes.is_empty() {
        ui.label("No matching directories.");
        return;
    }
    if snapshot.nodes.is_empty() {
        ui.label(
            egui::RichText::new("No directories found")
                .size(12.0)
                .color(t.text_muted),
        );
        return;
    }

    let mut scroll = egui::ScrollArea::vertical().id_salt(("compressed-tree", &snapshot.root));
    if reset_scroll {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    let mut toggle_branch = None;
    scroll.auto_shrink([false, false]).show_rows(
        ui,
        ROW_HEIGHT,
        state.tree_filter.indices.len(),
        |ui, range| {
            for row in range {
                let index = state.tree_filter.indices[row];
                let node = &snapshot.nodes[index];
                let parent = state
                    .tree_share_parent
                    .then(|| state.tree_filter.parents[index])
                    .flatten();
                let denominator = parent.map_or(snapshot.progress.bytes, |parent| {
                    snapshot.nodes[parent].bytes
                });
                let denominator_status = parent.map_or(snapshot.progress.measurement, |parent| {
                    snapshot.nodes[parent].measurement
                });
                let share = (node.measurement.complete() && denominator_status.complete())
                    .then(|| share_tenths(node.bytes, denominator))
                    .flatten();
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|data| {
                    data.insert_temp(
                        egui::Id::new(("disk-tree-share-value", &node.path)),
                        (denominator, share),
                    )
                });
                let indentation = node.depth.min(12) as f32 * 14.0;
                let text_color = if node.unreadable {
                    t.accent_warning
                } else {
                    t.text_secondary
                };
                ui.horizontal(|ui| {
                    ui.add_space(indentation);
                    let closed = state.tree_filter.options.collapsed.contains(&node.path);
                    ui.allocate_ui_with_layout(
                        egui::vec2(32.0, ROW_HEIGHT - 2.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_size(egui::vec2(32.0, ROW_HEIGHT - 2.0));
                            if state.tree_filter.has_children[index] {
                                let action = if closed { "Expand" } else { "Collapse" };
                                let response = ui
                                    .add_enabled(
                                        state.tree_filter.options.query.trim().is_empty(),
                                        egui::Button::new(if closed { "+" } else { "−" })
                                            .min_size(egui::vec2(18.0, 18.0)),
                                    )
                                    .on_hover_text(format!(
                                        "{action} {} (search temporarily reveals branches)",
                                        node.path.display()
                                    ));
                                response.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::WidgetType::Button,
                                        response.enabled(),
                                        format!("{action} {}", node.path.display()),
                                    )
                                });
                                #[cfg(any(test, feature = "visual-qa"))]
                                ui.ctx().data_mut(|data| {
                                    data.insert_temp(
                                        egui::Id::new(("disk-tree-collapse", &node.path)),
                                        response.rect,
                                    )
                                });
                                if response.clicked() {
                                    toggle_branch = Some(node.path.clone());
                                }
                            }
                        },
                    );
                    let suffix = if node.unreadable {
                        " (unreadable)"
                    } else if !state.tree_filter.direct[index] {
                        " (context)"
                    } else {
                        ""
                    };
                    let label_width = (ui.available_width() - 245.0).max(80.0);
                    let response = ui
                        .allocate_ui_with_layout(
                            egui::vec2(label_width, ROW_HEIGHT - 2.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_min_size(egui::vec2(label_width, ROW_HEIGHT - 2.0));
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!("{}{suffix}", node.label))
                                            .size(12.0)
                                            .color(text_color),
                                    )
                                    .halign(egui::Align::Min)
                                    .truncate()
                                    .sense(Sense::click()),
                                )
                            },
                        )
                        .inner
                        .on_hover_text(format!(
                            "{}\n{} files | {}: {} | {} unavailable measurements{}",
                            node.path.display(),
                            node.files,
                            snapshot.metric.label(),
                            snapshot.metric.text(node.bytes, node.measurement),
                            node.measurement.unknown,
                            if node.measurement.overflowed {
                                " · overflowed"
                            } else {
                                ""
                            }
                        ));
                    #[cfg(any(test, feature = "visual-qa"))]
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(
                            egui::Id::new(("disk-tree-node", &node.path)),
                            response.rect,
                        )
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(snapshot.metric.text(node.bytes, node.measurement))
                                .size(11.0)
                                .color(t.text_muted),
                        );
                        let bar =
                            egui::ProgressBar::new(share.unwrap_or(0).min(1000) as f32 / 1000.0)
                                .desired_width(74.0)
                                .text(share_text(share));
                        ui.add(bar).on_hover_text(format!(
                            "{} / {} {} {} · {}{}",
                            node.bytes,
                            denominator,
                            snapshot.metric.key(),
                            snapshot.metric.unit(),
                            parent.map_or_else(
                                || "scan root".to_string(),
                                |index| snapshot.nodes[index].path.display().to_string()
                            ),
                            if snapshot.cancelled
                                || snapshot.truncated
                                || snapshot.inaccessible_count > 0
                            {
                                " · partial scan"
                            } else {
                                ""
                            }
                        ));
                        ui.label(
                            egui::RichText::new(format!("{} files", node.files))
                                .size(10.0)
                                .color(t.text_muted),
                        );
                    });
                    if response.clicked() && !node.unreadable {
                        *open_path = Some((node.path.clone(), true));
                    }
                });
            }
        },
    );
    if let Some(path) = toggle_branch
        && !state.tree_filter.options.collapsed.remove(&path)
    {
        state.tree_filter.options.collapsed.insert(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool) -> FileEntry {
        let path = PathBuf::from("/fixture").join(name);
        let extension = path
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        FileEntry {
            symlink: None,
            permissions: None,
            allocated_size: None,
            hard_links: None,
            owner_uid: None,
            owner_gid: None,
            accessed: None,
            created: None,
            accessed_str: "Unavailable".into(),
            created_str: "Unavailable".into(),
            changed: None,
            changed_str: "Unavailable".into(),
            name_lower: name.to_lowercase(),
            sort_key: crate::panel::diacritic_sort_key(&name.to_lowercase()),
            name: name.to_string(),
            path,
            identity: crate::panel::ListingIdentity::Unavailable,
            is_dir,
            size: 1,
            extension,
            modified: None,
            modified_str: String::new(),
            size_str: "1 B".to_string(),
        }
    }

    #[test]
    fn filtered_tree_click_uses_original_node_identity_after_hidden_rows() {
        let root = PathBuf::from("/fixture");
        let nodes = [
            ("branch", 0, 132),
            ("branch/other", 1, 99),
            ("branch/compressed/target", 1, 33),
        ]
        .into_iter()
        .map(|(name, depth, bytes)| crate::tree_overview::OverviewNode {
            measurement: Default::default(),
            path: root.join(name),
            label: name.into(),
            depth,
            bytes,
            files: 1,
            unreadable: false,
        })
        .collect();
        let progress = crate::tree_overview::ScanProgress {
            bytes: 200,
            ..Default::default()
        };
        let snapshot = crate::tree_overview::OverviewSnapshot {
            metric: Default::default(),
            map_measurements: Default::default(),
            root: root.clone(),
            nodes,
            progress: progress.clone(),
            top_level: Vec::new(),
            top_level_truncated: false,
            inaccessible_paths: Vec::new(),
            inaccessible_count: 0,
            truncated: false,
            cancelled: false,
            elapsed: Default::default(),
        };
        let mut state = DiskUsageState {
            metric: Default::default(),
            initial: crate::workspace::TreemapSnapshot {
                dir: root.clone(),
                items: Vec::new(),
            },
            mode: DiskUsageMode::Tree,
            run: None,
            progress,
            overview: Some(snapshot),
            tree_filter: Default::default(),
            tree_share_parent: false,
            stopping: false,
            error: None,
        };
        state.tree_filter.options.query = "target".into();
        let ctx = egui::Context::default();
        let opened = std::cell::RefCell::new(None);
        let frame = |state: &mut DiskUsageState, events| {
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 500.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    show_tree(
                        ui,
                        state,
                        crate::theme::ThemeColors::dark(),
                        &mut opened.borrow_mut(),
                        &mut None,
                    );
                },
            ));
        };
        frame(&mut state, Vec::new());
        assert_eq!(state.tree_filter.indices, [0, 2]);
        let target = root.join("branch/compressed/target");
        let probe = egui::Id::new(("disk-tree-share-value", &target));
        assert_eq!(
            ctx.data(|data| data.get_temp::<(u64, Option<u128>)>(probe)),
            Some((200, Some(165)))
        );
        state.tree_share_parent = true;
        frame(&mut state, Vec::new());
        assert_eq!(
            ctx.data(|data| data.get_temp::<(u64, Option<u128>)>(probe)),
            Some((132, Some(250)))
        );
        let pos = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::new(("disk-tree-node", &target))))
            .unwrap()
            .center();
        let target_rect = ctx
            .data(|data| data.get_temp::<egui::Rect>(egui::Id::new(("disk-tree-node", &target))))
            .unwrap();
        let parent_rect = ctx
            .data(|data| {
                data.get_temp::<egui::Rect>(egui::Id::new(("disk-tree-node", root.join("branch"))))
            })
            .unwrap();
        let indent = target_rect.left() - parent_rect.left();
        assert!(
            (13.5..=14.5).contains(&indent),
            "left-aligned hierarchy must retain indentation: {indent}"
        );
        for phase in 0..3 {
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if phase > 0 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: phase == 1,
                    modifiers: Default::default(),
                });
            }
            frame(&mut state, events);
        }
        assert_eq!(*opened.borrow(), Some((target, true)));
        let snapshot = state.overview.as_ref().unwrap();
        assert_eq!(snapshot.nodes[1].path, root.join("branch/other"));
        assert_eq!(snapshot.nodes[2].bytes, 33);
        assert_eq!(snapshot.progress.bytes, 200);
        *opened.borrow_mut() = None;
        state.tree_filter.options.query.clear();
        frame(&mut state, Vec::new());
        for expected in [vec![0], vec![0, 1, 2]] {
            let button = ctx
                .data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new((
                        "disk-tree-collapse",
                        root.join("branch"),
                    )))
                })
                .unwrap();
            for phase in 0..3 {
                let mut events = vec![egui::Event::PointerMoved(button.center())];
                if phase > 0 {
                    events.push(egui::Event::PointerButton {
                        pos: button.center(),
                        button: egui::PointerButton::Primary,
                        pressed: phase == 1,
                        modifiers: Default::default(),
                    });
                }
                frame(&mut state, events);
            }
            frame(&mut state, Vec::new());
            assert_eq!(state.tree_filter.indices, expected);
            assert!(opened.borrow().is_none(), "disclosure must not navigate");
            assert_eq!(state.overview.as_ref().unwrap().progress.bytes, 200);
        }
    }

    #[test]
    fn percentages_are_exact_bounded_for_painting_and_undefined_for_empty_totals() {
        assert_eq!(share_tenths(10, 246), Some(40));
        assert_eq!(share_tenths(10, 30), Some(333));
        assert_eq!(share_tenths(u64::MAX, u64::MAX), Some(1000));
        assert_eq!(share_tenths(u64::MAX, 1), Some(u128::from(u64::MAX) * 1000));
        assert_eq!(share_text(share_tenths(0, 0)), "—");
        assert_eq!(share_text(share_tenths(0, 1)), "0.0%");
        assert_eq!(share_text(share_tenths(1, 3)), "33.3%");
    }

    #[test]
    fn map_fits_the_pane_and_refuses_a_tiny_one() {
        assert_eq!(
            map_size(Vec2::new(200.0, 100.0)),
            Some(Vec2::new(200.0, 100.0))
        );
        assert_eq!(map_size(Vec2::new(100.0, 300.0)), None);
        assert_eq!(map_size(Vec2::new(300.0, 60.0)), None);
    }

    #[test]
    fn legend_reserves_a_row_per_wrap() {
        assert_eq!(legend_rows(&[], 300.0), 0);
        assert_eq!(legend_rows(&[80.0, 80.0, 80.0], 300.0), 1);
        assert_eq!(legend_rows(&[80.0, 80.0, 80.0, 80.0], 300.0), 2);
        assert_eq!(legend_rows(&[80.0, 80.0, 80.0], 100.0), 3);
        // An item wider than the pane still takes just its own row.
        assert_eq!(legend_rows(&[500.0], 100.0), 1);
    }

    #[test]
    fn finished_empty_scan_does_not_promise_more_items() {
        assert_eq!(map_empty_message(false, false), "No measured items yet");
        assert_eq!(map_empty_message(true, false), "Folder is empty");
        assert_eq!(
            map_empty_message(true, true),
            "Folder holds only empty items"
        );
    }

    #[test]
    fn legend_lists_each_kind_once_in_kind_order() {
        let entries = [
            entry("b.rs", false),
            entry("sub", true),
            entry("a.rs", false),
        ];
        assert_eq!(legend_kinds(&entries), vec![Kind::Folder, Kind::Code]);
    }
}
