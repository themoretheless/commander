//! Responsive disk-usage map and cancellable compressed-tree overview.

use super::*;
use crate::panel::{FileEntry, format_size};
use crate::selection_summary::{Kind, kind_of};

const ROW_HEIGHT: f32 = 28.0;
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
        state.run = Some(crate::tree_overview::spawn(
            root.clone(),
            std::sync::Arc::new(move || repaint.request_repaint()),
        ));
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
            progress: crate::tree_overview::ScanProgress {
                directories: 1,
                current: initial.dir.clone(),
                ..Default::default()
            },
            initial,
            mode: DiskUsageMode::Map,
            run: None,
            overview: None,
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

        {
            let state = self.ui.tools.treemap.as_mut().expect("checked above");
            ui.vertical(|ui| {
                show_disk_usage_header(ui, state, t, &mut stop, &mut rescan);
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                match state.mode {
                    DiskUsageMode::Map => {
                        let items = state
                            .overview
                            .as_ref()
                            .map_or(state.initial.items.as_slice(), |snapshot| {
                                snapshot.top_level.as_slice()
                            });
                        let scan_finished = state.run.is_none()
                            && state.overview.as_ref().is_some_and(|s| !s.cancelled);
                        show_map(ui, items, scan_finished, t, dark, &mut open_path);
                    }
                    DiskUsageMode::Tree => {
                        show_tree(ui, state, t, &mut open_path);
                    }
                }
            });
        }

        if super::ui_state::modal_close_requested(window_open, false) {
            self.ui.tools.treemap = None;
            return;
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
            } else if ui.button("Rescan").clicked() {
                *rescan = true;
            }
            ui.selectable_value(&mut state.mode, DiskUsageMode::Tree, "Tree");
            ui.selectable_value(&mut state.mode, DiskUsageMode::Map, "Map");
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
            format_size(state.progress.bytes)
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
            format_size(snapshot.progress.bytes),
            snapshot.elapsed.as_secs_f32()
        )
    } else {
        "Waiting to scan".to_string()
    };
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
) {
    let visible: Vec<_> = items.iter().filter(|(_, bytes)| *bytes > 0).collect();
    if visible.is_empty() {
        ui.label(
            egui::RichText::new(map_empty_message(scan_finished, !items.is_empty()))
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
                "{} | {} | {}",
                entry.path.display(),
                kind_of(entry).label(),
                format_size(*bytes)
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
    state: &DiskUsageState,
    t: crate::theme::ThemeColors,
    open_path: &mut Option<(PathBuf, bool)>,
) {
    let Some(snapshot) = &state.overview else {
        ui.label(
            egui::RichText::new("Scanning directory tree")
                .size(12.0)
                .color(t.text_muted),
        );
        return;
    };
    if snapshot.nodes.is_empty() {
        ui.label(
            egui::RichText::new("No directories found")
                .size(12.0)
                .color(t.text_muted),
        );
        return;
    }

    egui::ScrollArea::vertical()
        .id_salt(("compressed-tree", &snapshot.root))
        .auto_shrink([false, false])
        .show_rows(ui, ROW_HEIGHT, snapshot.nodes.len(), |ui, range| {
            for index in range {
                let node = &snapshot.nodes[index];
                let indentation = node.depth.min(12) as f32 * 14.0;
                let text_color = if node.unreadable {
                    t.accent_warning
                } else {
                    t.text_secondary
                };
                ui.horizontal(|ui| {
                    ui.add_space(indentation);
                    let suffix = if node.unreadable { " (unreadable)" } else { "" };
                    let label_width = (ui.available_width() - 170.0).max(120.0);
                    let response = ui
                        .add_sized(
                            [label_width, ROW_HEIGHT - 2.0],
                            egui::Label::new(
                                egui::RichText::new(format!("{}{suffix}", node.label))
                                    .size(12.0)
                                    .color(text_color),
                            )
                            .truncate()
                            .sense(Sense::click()),
                        )
                        .on_hover_text(format!(
                            "{}\n{} files | {}",
                            node.path.display(),
                            node.files,
                            format_size(node.bytes)
                        ));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format_size(node.bytes))
                                .size(11.0)
                                .color(t.text_muted),
                        );
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
        });
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
            name_lower: name.to_lowercase(),
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
