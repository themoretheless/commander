//! Responsive disk-usage map and cancellable compressed-tree overview.

use super::*;
use crate::panel::{FileEntry, format_size};
use crate::selection_summary::kind_of;

const ROW_HEIGHT: f32 = 28.0;

impl App {
    fn start_disk_usage_scan(&mut self, ctx: &egui::Context) {
        let Some(state) = self.treemap.as_mut() else {
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
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(run) = self.treemap.as_ref().and_then(|state| state.run.as_ref()) {
            loop {
                match run.try_recv() {
                    Ok(event) => events.push(event),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if let Some(state) = self.treemap.as_mut() {
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
        self.treemap = Some(DiskUsageState {
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

    pub(crate) fn show_treemap_dialog(&mut self, ctx: &egui::Context) {
        if self.treemap.is_none() {
            return;
        }
        self.poll_disk_usage_scan();

        let t = self.colors;
        let dark = (t.bg_panel.r() as u16 + t.bg_panel.g() as u16 + t.bg_panel.b() as u16) < 384;
        let mut window_open = true;
        let mut stop = false;
        let mut rescan = false;
        let mut open_path = None;

        {
            let state = self.treemap.as_mut().expect("checked above");
            egui::Window::new("Disk usage")
                .open(&mut window_open)
                .collapsible(false)
                .resizable(true)
                .default_size([780.0, 580.0])
                .min_width(620.0)
                .min_height(420.0)
                .frame(
                    Frame::NONE
                        .fill(t.bg_panel)
                        .inner_margin(Margin::same(14))
                        .stroke(Stroke::new(1.0_f32, t.border)),
                )
                .show(ctx, |ui| {
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
                            show_map(ui, items, t, dark, &mut open_path);
                        }
                        DiskUsageMode::Tree => {
                            show_tree(ui, state, t, &mut open_path);
                        }
                    }
                });
        }

        if !window_open {
            self.treemap = None;
            return;
        }
        if stop
            && let Some(state) = self.treemap.as_mut()
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
            self.treemap = None;
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
    ui.label(egui::RichText::new(status).size(10.0).color(t.text_muted));

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
            let details = snapshot
                .inaccessible_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            ui.label(
                egui::RichText::new(format!(
                    "{} paths could not be read",
                    snapshot.inaccessible_count
                ))
                .size(10.0)
                .color(t.accent_warning),
            )
            .on_hover_text(details);
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
    t: crate::theme::ThemeColors,
    dark: bool,
    open_path: &mut Option<(PathBuf, bool)>,
) {
    let visible: Vec<_> = items.iter().filter(|(_, bytes)| *bytes > 0).collect();
    if visible.is_empty() {
        ui.label(
            egui::RichText::new("No measured items yet")
                .size(12.0)
                .color(t.text_muted),
        );
        return;
    }

    let width = ui.available_width().max(320.0);
    let height = ui.available_height().max(240.0);
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
            painter.text(
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
                "{} | {}",
                entry.path.display(),
                format_size(*bytes)
            ));
        if interaction.clicked() {
            *open_path = Some((entry.path.clone(), entry.is_dir));
        }
    }
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
