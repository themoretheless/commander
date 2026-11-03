//! Miller columns: context listings are separate cancellable owners; file
//! operations always target the main panel's current actionable column.
use super::*;
#[derive(Clone, Debug)]
pub struct MillerTabRequest {
    pub owner: PathBuf,
    pub generation: u64,
    pub folder: PathBuf,
}

fn column_divider(
    ui: &mut egui::Ui,
    panel: &mut PanelState,
    path: &std::path::Path,
    column: egui::Rect,
    width: f32,
    gap: f32,
    panel_side: &str,
) {
    let rect = egui::Rect::from_min_max(
        egui::pos2(column.right(), column.top()),
        egui::pos2(column.right() + gap, column.bottom()),
    );
    let response = ui
        .interact(
            rect,
            egui::Id::new(("miller-divider", panel_side, path)),
            Sense::CLICK | Sense::DRAG,
        )
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
        .on_hover_text("Drag to resize this column. Double-click to restore automatic width.");
    let stroke = if response.hovered() || response.dragged() {
        egui::Stroke::new(2.0, ui.visuals().selection.stroke.color)
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke
    };
    ui.painter().vline(rect.center().x, rect.y_range(), stroke);
    #[cfg(any(test, feature = "visual-qa"))]
    ui.ctx().data_mut(|d| {
        d.insert_temp(egui::Id::new(("miller-divider", panel_side, path)), rect);
        d.insert_temp(
            egui::Id::new(("miller-column-rect", panel_side, path)),
            column,
        );
    });
    #[cfg(feature = "visual-qa")]
    if response.clicked()
        || response.double_clicked()
        || response.triple_clicked()
        || response.drag_started()
        || response.drag_stopped()
    {
        log::info!(
            "Miller divider event path={} time={} click={} double={} triple={} start={} stop={} pos={:?}",
            path.display(),
            ui.input(|i| i.time),
            response.clicked(),
            response.double_clicked(),
            response.triple_clicked(),
            response.drag_started(),
            response.drag_stopped(),
            response.interact_pointer_pos()
        );
    }
    // A recent click elsewhere can classify the next double-click as a
    // triple-click. Width reset is idempotent for either gesture.
    if response.double_clicked() || response.triple_clicked() {
        panel.reset_miller_column(path);
        panel.miller.reveal_current = true;
    } else {
        let anchor = egui::Id::new(("miller-width-drag", panel_side, path));
        if response.drag_started() {
            ui.ctx().data_mut(|d| d.insert_temp(anchor, width));
        }
        if let Some(delta) = response.total_drag_delta() {
            let initial = ui
                .ctx()
                .data(|d| d.get_temp::<f32>(anchor))
                .unwrap_or(width);
            panel.resize_miller_column(path.to_path_buf(), initial + delta.x);
        }
        if response.drag_stopped() {
            ui.ctx().data_mut(|d| d.remove::<f32>(anchor));
            panel.miller.reveal_current = true;
        }
    }
}
impl App {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_file_list(
        ui: &mut egui::Ui,
        panel: &mut PanelState,
        image_cache: &mut crate::image_cache::ImageCache,
        is_active: bool,
        t: &ThemeColors,
        panel_side: &str,
        size_bars: bool,
        compare: Option<&crate::compare::CompareMap>,
        opener: &dyn Fn(crate::ports::OpenRequest),
        dragging: bool,
        dragged: &[PathBuf],
        metrics: crate::density::DensityMetrics,
        reduced_motion: bool,
        background_tab: &mut Option<MillerTabRequest>,
    ) -> Option<Result<ui_state::ContextMenuCandidate, String>> {
        *background_tab = None;
        if panel.browser_mode() != crate::panel::BrowserMode::Miller {
            panel.miller = Default::default();
            return Self::render_file_list_plain(
                ui,
                panel,
                image_cache,
                is_active,
                t,
                panel_side,
                size_bars,
                compare,
                opener,
                dragging,
                dragged,
                metrics,
                reduced_motion,
            );
        }
        let changed = panel.prepare_miller();
        let current = panel.current_path.clone();
        let generation = panel.entries_gen();
        let columns = std::mem::take(&mut panel.miller.columns);
        let height = (ui.available_height() - 22.0).max(0.0);
        let viewport_width = ui.available_width();
        let gap = 8.0;
        let context_widths: Vec<_> = columns
            .iter()
            .map(|col| panel.miller_column_width(&col.current_path, 220.0))
            .collect();
        let automatic = if columns.len() <= 1 {
            (viewport_width - context_widths.iter().sum::<f32>() - gap * (columns.len() + 1) as f32)
                .max(240.0)
        } else {
            viewport_width.clamp(240.0, 360.0)
        };
        let active_width = panel.miller_column_width(&current, automatic);
        let strip_width =
            context_widths.iter().sum::<f32>() + active_width + gap * (columns.len() + 1) as f32;
        let reveal = std::mem::take(&mut panel.miller.reveal_current);
        let mut navigation: Option<(PathBuf, Option<PathBuf>)> = None;
        let mut request = None;
        let mut scroll = egui::ScrollArea::horizontal()
            .id_salt(("miller-strip", panel_side))
            .auto_shrink([false, false]);
        if changed || reveal {
            scroll = scroll.horizontal_scroll_offset((strip_width - viewport_width).max(0.0));
        }
        scroll.show(ui,|ui| {
            // A horizontal scroll content area is unbounded. Give the row a
            // finite extent before allocating any child columns.
            ui.set_width(strip_width);
            ui.spacing_mut().item_spacing.x=gap;
            ui.horizontal_top(|ui| {
                for i in 0..columns.len() {
                    let child=columns.get(i+1).map_or(&current,|col|&col.current_path).clone();
                    let col=&columns[i];
                    let width=context_widths[i];
                    let ancestor_column=ui.allocate_ui_with_layout(egui::vec2(width,height),Layout::top_down(Align::Min),|ui| {
                        ui.set_min_size(egui::vec2(width,height));ui.set_max_size(egui::vec2(width,height));
                        let label=col.current_path.file_name().map_or_else(||col.current_path.display().to_string(),|n|n.to_string_lossy().into_owned());
                        let header=ui.add_sized([width,24.0],egui::Button::new(label).truncate().sense(Sense::CLICK)).on_hover_text(format!("{}\nMiddle-click to open in a background tab",col.current_path.display()));
                        if header.clicked_by(egui::PointerButton::Middle) {
                            *background_tab=Some(MillerTabRequest {owner:current.clone(),generation,folder:col.current_path.clone()});
                        } else if header.clicked() {navigation=Some((col.current_path.clone(),None));}
                        ui.separator();
                        let row_h=(metrics.name_pt+2.0*metrics.row_pad_y+6.0).max(24.0);
                        if col.entries().is_empty() {ui.label(format!("{:?}",col.dir_status()));}
                        egui::ScrollArea::vertical().id_salt(("miller-column",panel_side,&col.current_path)).auto_shrink([false,false]).show_rows(ui,row_h,col.filtered_count(),|ui,range| {
                            for row in range {
                                let Some(entry)=col.filtered_get(row) else {continue;};
                                let label=format!("{} {}",if entry.is_dir {"›"} else {"·"},entry.name);
                                let response=ui.add_sized([ui.available_width(),row_h],egui::Button::selectable(entry.path==child,label).truncate().sense(Sense::CLICK)).on_hover_text(if entry.is_dir { format!("{}\nMiddle-click to open in a background tab",entry.path.display()) } else { entry.path.display().to_string() });
                                #[cfg(any(test,feature="visual-qa"))]
                                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("miller-context",panel_side,&col.current_path,&entry.path)),response.rect));
                                if response.clicked_by(egui::PointerButton::Middle) && entry.is_dir {
                                    *background_tab=Some(MillerTabRequest {owner:current.clone(),generation,folder:entry.path.clone()});
                                } else if response.clicked() {
                                    navigation=Some(if entry.is_dir {(entry.path.clone(),None)} else {(col.current_path.clone(),Some(entry.path.clone()))});
                                }
                            }
                        });
                    });
                    column_divider(ui,panel,&col.current_path,ancestor_column.response.rect,width,gap,panel_side);
                }
                let current_column=ui.allocate_ui_with_layout(egui::vec2(active_width,height),Layout::top_down(Align::Min),|ui| {
                    ui.set_min_size(egui::vec2(active_width,height));ui.set_max_size(egui::vec2(active_width,height));
                    ui.add_sized([active_width,24.0],egui::Label::new(egui::RichText::new(current.file_name().map_or_else(||current.display().to_string(),|n|n.to_string_lossy().into_owned())).strong()).truncate()).on_hover_text(format!("Current folder: {}. File actions and keyboard commands target this column.",current.display()));
                    ui.separator();
                    request=Self::render_file_list_plain(ui,panel,image_cache,is_active,t,panel_side,size_bars,compare,opener,dragging,dragged,metrics,reduced_motion);
                });
                column_divider(ui,panel,&current,current_column.response.rect,active_width,gap,panel_side);
                #[cfg(any(test,feature="visual-qa"))]
                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("miller-current-rect",panel_side)),current_column.response.rect));
                #[cfg(not(any(test,feature="visual-qa")))]
                let _ = current_column;
            });
        });
        if panel.browser_mode() == crate::panel::BrowserMode::Miller {
            panel.miller.columns = columns;
        }
        if let Some((folder, focus)) = navigation {
            panel.miller_navigate(folder, focus);
            super::file_list::request_cursor_row_refocus(ui.ctx());
        }
        request
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn middle_click_ancestor_folder_requests_tab_without_promoting_the_action_owner() {
        let temp = crate::testutil::TempDir::new();
        let a = temp.dir("A");
        let b = temp.dir("B");
        let file = temp.file("A/a.txt", "owner");
        let plain = temp.file("plain.txt", "plain");
        let mut panel = PanelState::new(a.clone());
        panel.refresh();
        panel.focus_child("a.txt".into());
        panel.select_cursor();
        panel.set_browser_mode(crate::panel::BrowserMode::Miller);
        let workload = crate::workload::DeterministicWorkload::new(
            crate::workload::SchedulerLimits::default(),
        );
        panel.set_workload(workload.handle());
        panel.set_notify(std::sync::Arc::new(|| {}));
        let ctx = egui::Context::default();
        let mut images = crate::image_cache::ImageCache::new();
        let mut tick = 0.0;
        let mut render = |panel: &mut PanelState, events| {
            tick += 0.1;
            let mut request = None;
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(637.0, 500.0),
                    )),
                    time: Some(tick),
                    events,
                    ..Default::default()
                },
                |ui| {
                    App::render_file_list(
                        ui,
                        panel,
                        &mut images,
                        true,
                        &ThemeColors::dark(),
                        "left",
                        false,
                        None,
                        &|_| {},
                        false,
                        &[],
                        crate::density::metrics(crate::density::Density::default()),
                        true,
                        &mut request,
                    );
                },
            ));
            request
        };
        for _ in 0..8 {
            while workload.run_next() {}
            panel.poll_listing();
            assert!(render(&mut panel, vec![]).is_none());
        }
        let generation = panel.entries_gen();
        for (target, expected) in [(&b, true), (&plain, false)] {
            let rect = ctx
                .data(|d| {
                    d.get_temp::<egui::Rect>(egui::Id::new((
                        "miller-context",
                        "left",
                        temp.path(),
                        target,
                    )))
                })
                .unwrap();
            let pos = rect.center();
            let mut request = None;
            for pressed in [true, false] {
                request = render(
                    &mut panel,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Middle,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                )
                .or(request);
            }
            assert_eq!(request.is_some(), expected);
            if let Some(request) = request {
                assert_eq!(request.owner, a);
                assert_eq!(request.generation, generation);
                assert_eq!(request.folder, b);
            }
            assert_eq!(panel.current_path, a);
            assert_eq!(panel.entries_gen(), generation);
            assert_eq!(panel.selected_entries()[0].path, file);
        }
    }
    #[test]
    fn pointer_resize_and_double_click_reset_keep_the_action_owner() {
        let root = crate::testutil::TempDir::new();
        let a = root.dir("A");
        root.file("A/a.txt", "owner");
        let mut panel = PanelState::new(a.clone());
        panel.refresh();
        panel.focus_child("a.txt".into());
        panel.select_cursor();
        panel.set_browser_mode(crate::panel::BrowserMode::Miller);
        let workload = crate::workload::DeterministicWorkload::new(
            crate::workload::SchedulerLimits::default(),
        );
        panel.set_workload(workload.handle());
        panel.set_notify(std::sync::Arc::new(|| {}));
        let ctx = egui::Context::default();
        let mut images = crate::image_cache::ImageCache::new();
        let mut tick = 0.0;
        let mut render = |panel: &mut PanelState, events: Vec<egui::Event>| {
            tick += 0.05;
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(637.0, 500.0),
                    )),
                    time: Some(tick),
                    events,
                    ..Default::default()
                },
                |ui| {
                    App::render_file_list(
                        ui,
                        panel,
                        &mut images,
                        true,
                        &ThemeColors::dark(),
                        "left",
                        false,
                        None,
                        &|_| {},
                        false,
                        &[],
                        crate::density::metrics(crate::density::Density::default()),
                        true,
                        &mut None,
                    );
                },
            );
            crate::testutil::discard_egui_output(out);
        };
        for _ in 0..6 {
            while workload.run_next() {}
            render(&mut panel, vec![]);
        }
        let revision = panel.entries_gen();
        let id = egui::Id::new(("miller-divider", "left", root.path()));
        let pos = ctx.data(|d| d.get_temp::<egui::Rect>(id)).unwrap().center();
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(
            &mut panel,
            vec![egui::Event::PointerMoved(pos), button(pos, true)],
        );
        let moved = pos + egui::vec2(50.0, 0.0);
        render(&mut panel, vec![egui::Event::PointerMoved(moved)]);
        render(&mut panel, vec![button(moved, false)]);
        render(&mut panel, vec![]);
        assert_eq!(panel.miller_column_width(root.path(), 220.0), 270.0);
        let rect = ctx
            .data(|d| {
                d.get_temp::<egui::Rect>(egui::Id::new(("miller-column-rect", "left", root.path())))
            })
            .unwrap();
        assert!((rect.width() - 270.0).abs() < 0.51);
        // A recent unrelated click must not suppress the divider reset.
        let elsewhere = egui::pos2(400.0, 200.0);
        for pressed in [true, false] {
            render(
                &mut panel,
                vec![
                    egui::Event::PointerMoved(elsewhere),
                    button(elsewhere, pressed),
                ],
            );
        }
        let pos = ctx.data(|d| d.get_temp::<egui::Rect>(id)).unwrap().center();
        for pressed in [true, false, true, false] {
            render(
                &mut panel,
                vec![egui::Event::PointerMoved(pos), button(pos, pressed)],
            );
        }
        render(&mut panel, vec![]);
        assert_eq!(panel.miller_column_width(root.path(), 220.0), 220.0);
        assert_eq!(panel.entries_gen(), revision);
        assert_eq!(panel.current_path, a);
        assert_eq!(panel.cursor_entry().unwrap().path, a.join("a.txt"));
        assert!(panel.is_selected(&a.join("a.txt")));
    }
    #[test]
    fn miller_layout_stays_finite_with_ancestors_and_mode_round_trip() {
        let root = crate::testutil::TempDir::new();
        root.dir("A");
        root.dir("A/inner");
        root.file("A/a.txt", "owner");
        root.file("A/inner/leaf.txt", "leaf");
        let mut panel = PanelState::new(root.path().join("A"));
        panel.refresh();
        panel.focus_child("a.txt".into());
        panel.select_cursor();
        let workload = crate::workload::DeterministicWorkload::new(
            crate::workload::SchedulerLimits::default(),
        );
        panel.set_workload(workload.handle());
        panel.set_notify(std::sync::Arc::new(|| {}));
        let ctx = egui::Context::default();
        let mut images = crate::image_cache::ImageCache::new();
        for mode in [
            crate::panel::BrowserMode::Miller,
            crate::panel::BrowserMode::List,
            crate::panel::BrowserMode::Miller,
        ] {
            panel.set_browser_mode(mode);
            for _ in 0..6 {
                while workload.run_next() {}
                panel.poll_listing();
                let mut right_edge = 0.0;
                let output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(637.0, 500.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        right_edge = ui.max_rect().right();
                        App::render_file_list(
                            ui,
                            &mut panel,
                            &mut images,
                            true,
                            &ThemeColors::dark(),
                            "left",
                            false,
                            None,
                            &|_| {},
                            false,
                            &[],
                            crate::density::metrics(crate::density::Density::default()),
                            true,
                            &mut None,
                        );
                    },
                );
                assert!(
                    output
                        .shapes
                        .iter()
                        .all(|shape| shape.clip_rect.is_finite())
                );
                crate::testutil::discard_egui_output(output);
                if mode == crate::panel::BrowserMode::Miller {
                    let rect = ctx
                        .data(|d| {
                            d.get_temp::<egui::Rect>(egui::Id::new(("miller-current-rect", "left")))
                        })
                        .unwrap();
                    assert!(rect.is_finite());
                    if panel
                        .miller
                        .columns
                        .iter()
                        .all(|c| c.dir_status() != crate::panel::DirStatus::Loading)
                    {
                        assert!(
                            (right_edge - rect.right()).abs() < 24.0,
                            "unused trailing width: {}",
                            right_edge - rect.right()
                        );
                    }
                }
            }
            assert_eq!(
                panel.cursor_entry().unwrap().path,
                root.path().join("A/a.txt")
            );
            assert!(panel.is_selected(&root.path().join("A/a.txt")));
            if mode == crate::panel::BrowserMode::List {
                assert!(panel.miller.columns.is_empty());
            }
        }
    }
}
