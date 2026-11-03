//! Miller columns: context listings are separate cancellable owners; file
//! operations always target the main panel's current actionable column.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderOrigin {
    CurrentListing,
    MillerAncestor,
    Breadcrumb(u64),
    CollapsedBreadcrumb(u64),
}

#[derive(Clone, Debug)]
pub struct FolderTabRequest {
    pub owner: PathBuf,
    pub generation: u64,
    pub folder: PathBuf,
    pub activate: bool,
    pub origin: FolderOrigin,
}

/// Preserve the tracked outer menu while an independently owned child popup is open.
pub(super) fn path_components_menu(
    ui: &mut egui::Ui,
    contents: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let response = ui.button("\u{2026}");
    egui::Popup::menu(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(contents);
    response
}

pub(super) fn folder_tab_actions(
    response: &egui::Response,
    owner: &std::path::Path,
    generation: u64,
    folder: &std::path::Path,
    origin: FolderOrigin,
    request: &mut Option<FolderTabRequest>,
) {
    #[cfg(feature = "visual-qa")]
    if matches!(
        origin,
        FolderOrigin::Breadcrumb(_) | FolderOrigin::CollapsedBreadcrumb(_)
    ) && response.ctx.input(|i| {
        i.pointer.button_pressed(egui::PointerButton::Middle)
            || i.pointer.button_released(egui::PointerButton::Middle)
    }) {
        log::info!(
            "breadcrumb pointer folder={} rect={:?} enabled={} hovered={} contains={} clicked={} position={:?} layer={:?} top={:?}",
            folder.display(),
            response.rect,
            response.enabled(),
            response.hovered(),
            response.contains_pointer(),
            response.clicked_by(egui::PointerButton::Middle),
            response.ctx.input(|i| i.pointer.interact_pos()),
            response.layer_id,
            response
                .ctx
                .input(|i| i.pointer.interact_pos())
                .and_then(|pos| response.ctx.layer_id_at(pos))
        );
    }
    if response.clicked_by(egui::PointerButton::Middle) {
        *request = Some(FolderTabRequest {
            owner: owner.into(),
            generation,
            folder: folder.into(),
            activate: false,
            origin,
        });
    }
    let binding = response.id.with("miller-tab-binding");
    if response.secondary_clicked() {
        response.ctx.data_mut(|data| {
            data.insert_temp(
                binding,
                (
                    owner.to_path_buf(),
                    generation,
                    folder.to_path_buf(),
                    origin,
                ),
            )
        });
    }
    let nested = matches!(origin, FolderOrigin::CollapsedBreadcrumb(_));
    let open_key = response.id.with("crumb-context-open");
    let pos_key = response.id.with("crumb-context-position");
    let mut nested_open = response
        .ctx
        .data(|d| d.get_temp::<bool>(open_key))
        .unwrap_or(false);
    if response.secondary_clicked() {
        nested_open = true;
        let pos = response
            .interact_pointer_pos()
            .unwrap_or(response.rect.center());
        response.ctx.data_mut(|d| d.insert_temp(pos_key, pos));
    }
    let popup = egui::Popup::context_menu(response);
    let popup = if nested {
        let pos = response
            .ctx
            .data(|d| d.get_temp::<egui::Pos2>(pos_key))
            .unwrap_or(response.rect.center());
        popup.open_bool(&mut nested_open).at_position(pos)
    } else {
        popup
    };
    let popup = popup
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            let (owner, generation, folder, origin) = ui.ctx().data_mut(|data| {
                if let Some(binding) =
                    data.get_temp::<(PathBuf, u64, PathBuf, FolderOrigin)>(binding)
                {
                    binding
                } else {
                    let captured = (
                        owner.to_path_buf(),
                        generation,
                        folder.to_path_buf(),
                        origin,
                    );
                    data.insert_temp(binding, captured.clone());
                    captured
                }
            });
            for (label, activate) in [("Open in new tab", true), ("Open in background tab", false)]
            {
                let button = ui.button(label);
                #[cfg(any(test, feature = "visual-qa"))]
                ui.ctx().data_mut(|data| {
                    data.insert_temp(
                        egui::Id::new(("miller-open-menu", &folder, label)),
                        button.rect,
                    )
                });
                if button.clicked() {
                    *request = Some(FolderTabRequest {
                        owner: owner.clone(),
                        generation,
                        folder: folder.clone(),
                        activate,
                        origin,
                    });
                    ui.close();
                }
            }
        });
    if nested {
        response
            .ctx
            .data_mut(|d| d.insert_temp(open_key, nested_open));
    }
    if popup.is_none() {
        response
            .ctx
            .data_mut(|data| data.remove::<(PathBuf, u64, PathBuf, FolderOrigin)>(binding));
    }
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
        tab_request: &mut Option<FolderTabRequest>,
    ) -> Option<Result<ui_state::ContextMenuCandidate, String>> {
        *tab_request = None;
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
                tab_request,
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
                        let header=ui.add_sized([width,24.0],egui::Button::new(label).truncate().sense(Sense::CLICK)).on_hover_text(format!("{}\nMiddle-click for a background tab; right-click for tab actions",col.current_path.display()));
                        folder_tab_actions(&header,&current,generation,&col.current_path,FolderOrigin::MillerAncestor,tab_request);
                        if header.clicked() {navigation=Some((col.current_path.clone(),None));}
                        ui.separator();
                        let row_h=(metrics.name_pt+2.0*metrics.row_pad_y+6.0).max(24.0);
                        if col.entries().is_empty() {ui.label(format!("{:?}",col.dir_status()));}
                        egui::ScrollArea::vertical().id_salt(("miller-column",panel_side,&col.current_path)).auto_shrink([false,false]).show_rows(ui,row_h,col.filtered_count(),|ui,range| {
                            for row in range {
                                let Some(entry)=col.filtered_get(row) else {continue;};
                                let label=format!("{} {}",if entry.is_dir {"›"} else {"·"},entry.name);
                                let response=ui.add_sized([ui.available_width(),row_h],egui::Button::selectable(entry.path==child,label).truncate().sense(Sense::CLICK)).on_hover_text(if entry.is_dir { format!("{}\nMiddle-click for a background tab; right-click for tab actions",entry.path.display()) } else { entry.path.display().to_string() });
                                #[cfg(any(test,feature="visual-qa"))]
                                ui.ctx().data_mut(|d|d.insert_temp(egui::Id::new(("miller-context",panel_side,&col.current_path,&entry.path)),response.rect));
                                if entry.is_dir { folder_tab_actions(&response,&current,generation,&entry.path,FolderOrigin::MillerAncestor,tab_request); }
                                if response.clicked() {
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
                    request=Self::render_file_list_plain(ui,panel,image_cache,is_active,t,panel_side,size_bars,compare,opener,dragging,dragged,metrics,reduced_motion,tab_request);
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
    fn current_folder_middle_click_preserves_owner_in_list_icons_and_columns() {
        for mode in [
            crate::panel::BrowserMode::List,
            crate::panel::BrowserMode::Icons,
            crate::panel::BrowserMode::Miller,
        ] {
            let temp = crate::testutil::TempDir::new();
            let folder = temp.dir("Target");
            let file = temp.file("keep.txt", "owner");
            let mut panel = PanelState::new(temp.path().into());
            panel.refresh();
            panel.focus_child("keep.txt".into());
            panel.select_cursor();
            panel.set_browser_mode(mode);
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
                            &|_| panic!("middle-click must not open files externally"),
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
            let cursor = panel.cursor_entry().unwrap().path.clone();
            let parent = temp.path().parent().unwrap().to_path_buf();
            for (target, expected, parent_row) in [
                (&folder, true, false),
                (&file, false, false),
                (&parent, true, true),
            ] {
                let pos = ctx
                    .data(|data| {
                        if parent_row {
                            data.get_temp::<egui::Rect>(egui::Id::new((
                                "folder-parent-row",
                                "left",
                            )))
                        } else {
                            data.get_temp::<egui::Rect>(egui::Id::new((
                                "folder-row",
                                "left",
                                target,
                            )))
                        }
                    })
                    .unwrap()
                    .center();
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
                assert_eq!(request.is_some(), expected, "{mode:?}");
                if let Some(request) = request {
                    assert_eq!(request.owner, temp.path());
                    assert_eq!(&request.folder, target);
                    assert_eq!(request.generation, generation);
                    assert!(!request.activate && request.origin == FolderOrigin::CurrentListing);
                }
                assert_eq!(panel.current_path, temp.path());
                assert_eq!(panel.entries_gen(), generation);
                assert_eq!(panel.cursor_entry().unwrap().path, cursor);
                assert_eq!(panel.selected_entries()[0].path, file);
            }
        }
    }
    #[test]
    fn folder_menu_routes_both_tab_actions_with_the_original_popup_binding() {
        for (label, activate, origin) in [
            ("Open in new tab", true, FolderOrigin::MillerAncestor),
            (
                "Open in background tab",
                false,
                FolderOrigin::MillerAncestor,
            ),
            ("Open in new tab", true, FolderOrigin::CurrentListing),
            (
                "Open in background tab",
                false,
                FolderOrigin::CurrentListing,
            ),
            ("Open in new tab", true, FolderOrigin::Breadcrumb(7)),
            ("Open in background tab", false, FolderOrigin::Breadcrumb(7)),
        ] {
            let ctx = egui::Context::default();
            let mut tick = 0.0;
            let mut render = |owner: &str, generation, folder: &str, events| {
                tick += 0.1;
                let mut request = None;
                crate::testutil::discard_egui_output(ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(640.0, 480.0),
                        )),
                        time: Some(tick),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        let response = ui.button("Folder");
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(egui::Id::new("test-folder"), response.rect)
                        });
                        folder_tab_actions(
                            &response,
                            std::path::Path::new(owner),
                            generation,
                            std::path::Path::new(folder),
                            if owner == "/changed" {
                                FolderOrigin::Breadcrumb(99)
                            } else {
                                origin
                            },
                            &mut request,
                        );
                    },
                ));
                request
            };
            for _ in 0..3 {
                assert!(render("/owner", 7, "/folder", vec![]).is_none());
            }
            let pos = ctx
                .data(|data| data.get_temp::<egui::Rect>(egui::Id::new("test-folder")))
                .unwrap()
                .center();
            let pointer = |pos, button, pressed| {
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            for pressed in [true, false] {
                assert!(
                    render(
                        "/owner",
                        7,
                        "/folder",
                        pointer(pos, egui::PointerButton::Secondary, pressed)
                    )
                    .is_none()
                );
            }
            for _ in 0..3 {
                assert!(render("/changed", 8, "/retargeted", vec![]).is_none());
            }
            let pos = ctx
                .data(|data| {
                    data.get_temp::<egui::Rect>(egui::Id::new((
                        "miller-open-menu",
                        std::path::Path::new("/folder"),
                        label,
                    )))
                })
                .unwrap()
                .center();
            let mut request = None;
            for pressed in [true, false] {
                request = render(
                    "/changed",
                    8,
                    "/retargeted",
                    pointer(pos, egui::PointerButton::Primary, pressed),
                )
                .or(request);
            }
            let request = request.unwrap();
            assert_eq!(request.owner, std::path::Path::new("/owner"));
            assert_eq!(request.generation, 7);
            assert_eq!(request.folder, std::path::Path::new("/folder"));
            assert_eq!(request.activate, activate);
            assert_eq!(request.origin, origin);
        }
    }
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

#[cfg(test)]
mod collapsed_crumb_tests {
    use super::*;
    #[test]
    fn collapsed_breadcrumb_menu_supports_middle_click_and_nested_tab_actions() {
        for action in [
            None,
            Some("Open in new tab"),
            Some("Open in background tab"),
        ] {
            let ctx = egui::Context::default();
            let mut time = 0.0;
            let mut frame = |events| {
                time += 0.1;
                let mut request = None;
                crate::testutil::discard_egui_output(ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(640.0, 480.0),
                        )),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        let menu = path_components_menu(ui, |ui| {
                            let row = ui.button("Ancestor");
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(egui::Id::new("collapsed-row"), row.rect)
                            });
                            folder_tab_actions(
                                &row,
                                std::path::Path::new("/a/b/c"),
                                5,
                                std::path::Path::new("/a"),
                                FolderOrigin::CollapsedBreadcrumb(7),
                                &mut request,
                            );
                        });
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("collapsed-open"), menu.rect)
                        });
                    },
                ));
                request
            };
            let pointer = |pos, button, pressed| {
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            for _ in 0..3 {
                frame(vec![]);
            }
            let pos = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("collapsed-open")))
                .unwrap()
                .center();
            frame(pointer(pos, egui::PointerButton::Primary, true));
            frame(pointer(pos, egui::PointerButton::Primary, false));
            frame(vec![]);
            let pos = ctx
                .data(|d| d.get_temp::<egui::Rect>(egui::Id::new("collapsed-row")))
                .unwrap()
                .center();
            let button = if action.is_some() {
                egui::PointerButton::Secondary
            } else {
                egui::PointerButton::Middle
            };
            let mut result = frame(pointer(pos, button, true));
            result = frame(pointer(pos, button, false)).or(result);
            if let Some(action) = action {
                frame(vec![]);
                let pos = ctx
                    .data(|d| {
                        d.get_temp::<egui::Rect>(egui::Id::new((
                            "miller-open-menu",
                            std::path::Path::new("/a"),
                            action,
                        )))
                    })
                    .unwrap()
                    .center();
                result = frame(pointer(pos, egui::PointerButton::Primary, true)).or(result);
                result = frame(pointer(pos, egui::PointerButton::Primary, false)).or(result);
            }
            let request = result
                .unwrap_or_else(|| panic!("collapsed breadcrumb must retain action {action:?}"));
            assert_eq!(request.owner, std::path::Path::new("/a/b/c"));
            assert_eq!(request.folder, std::path::Path::new("/a"));
            assert_eq!(request.origin, FolderOrigin::CollapsedBreadcrumb(7));
            assert_eq!(request.generation, 5);
            assert_eq!(request.activate, action == Some("Open in new tab"));
        }
    }
}
