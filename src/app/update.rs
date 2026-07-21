//! Per-frame orchestration: each concern lives in its own method/module.

use super::*;

fn clipped_label(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(3);
    let mut out: String = text.chars().take(keep).collect();
    out.push_str("...");
    out
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let _frame_latency =
            crate::measurement::LatencyGuard::new(crate::measurement::MetricName::FrameTime);
        let ctx = ui.ctx().clone();
        self.begin_frame(&ctx);
        self.capture_operations_requests(&ctx);
        let modal_was_open = self.has_modal_surface();
        self.show_transfer_dialog(&ctx);
        self.show_safe_state_dialog(&ctx);
        self.show_recovery_dialog(&ctx);
        self.show_history_dialog(&ctx);
        self.show_confirm_dialog(&ctx);
        self.show_rename_dialog(&ctx);
        self.show_batch_rename_dialog(&ctx);
        self.show_sync_dialog(&ctx);
        self.show_duplicates_dialog(&ctx);
        self.show_diff_dialog(&ctx);
        self.show_treemap_dialog(&ctx);
        self.show_find_dialog(&ctx);
        self.show_archive_dialog(&ctx);
        self.show_saved_search_dialog(&ctx);
        self.show_collections_dialog(&ctx);
        self.show_mask_dialog(&ctx);
        self.show_path_dialog(&ctx);
        self.show_recent_dialog(&ctx);
        self.show_run_command_dialog(&ctx);
        self.show_palette_dialog(&ctx);
        // Keep the background disabled on the close frame too, so the pointer
        // release that dismissed a modal cannot click through into a file row.
        let modal_open =
            crate::accessibility::modal_trap_active(modal_was_open, self.has_modal_surface());
        let trapped = crate::accessibility::focus_order(crate::accessibility::FocusLayout {
            toolbar_visible: !self.focus_mode,
            operations_open: self.show_operations_center,
            dialog_open: modal_open,
        }) == [crate::accessibility::FocusRegion::Dialog];
        let background_order =
            crate::accessibility::focus_order(crate::accessibility::FocusLayout {
                toolbar_visible: !self.focus_mode,
                operations_open: self.show_operations_center,
                dialog_open: false,
            });
        ui.add_enabled_ui(!trapped, |ui| {
            for region in background_order {
                match region {
                    crate::accessibility::FocusRegion::Toolbar => self.show_toolbar_panel(ui),
                    crate::accessibility::FocusRegion::Operations => {
                        self.show_operations_center(ui);
                    }
                    crate::accessibility::FocusRegion::LeftPanel => {
                        if !self.focus_mode {
                            self.show_shortcut_bar(ui);
                            self.show_shelf_tray(ui);
                            self.show_selection_hud(ui);
                        }
                        self.show_main_area(ui);
                    }
                    crate::accessibility::FocusRegion::RightPanel
                    | crate::accessibility::FocusRegion::Dialog => {}
                }
            }
        });
        self.show_drag_overlay(&ctx);
        self.show_type_ahead_overlay(&ctx);
        self.show_toasts(&ctx);
        self.show_developer_panel(&ctx);
        self.handle_drop(&ctx);
    }

    /// eframe calls this on exit and on its auto-save interval; persist our
    /// own session snapshot (panel paths, layout, view toggles).
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        match crate::session::save(&self.to_session()) {
            Ok(crate::persistence::AtomicWriteOutcome::Durable) => {}
            Ok(crate::persistence::AtomicWriteOutcome::CommittedButNotDurable(failure)) => {
                crate::persistence::record_durability_warning("Session", &failure);
            }
            Err(error) => crate::persistence::record_save_failure("Session", &error),
        }
    }
}

impl App {
    fn has_modal_surface(&self) -> bool {
        self.ws.pending_op.is_some()
            || self.ws.active_transfer.is_some()
            || self.ws.safe_state.is_some()
            || self.recovery.open
            || self.history_preview.is_some()
            || self.renaming.is_some()
            || self.mask_input.is_some()
            || self.path_input.is_some()
            || self.recent_input.is_some()
            || self.run_command.is_some()
            || self.palette_input.is_some()
            || self.batch_rename.is_some()
            || self.sync.is_some()
            || self.duplicates.is_some()
            || self.diff.is_some()
            || self.treemap.is_some()
            || self.find.is_some()
            || self.archive.is_some()
            || self.saved_search_open
            || self.collections_dialog.is_some()
    }

    /// Frame bookkeeping: repaint heuristics, notify wiring, fs polling,
    /// input handling and background-task polling.
    fn begin_frame(&mut self, ctx: &egui::Context) {
        ctx.data_mut(|data| {
            data.remove::<egui::Rect>(egui::Id::new("current_focus_indicator"));
            data.remove::<egui::Rect>(egui::Id::new("active_error_surface"));
        });
        let persistence_generation = crate::persistence::issue_generation();
        if persistence_generation > self.persistence_issue_seen {
            let persistence = crate::persistence::health_snapshot();
            self.persistence_issue_seen = persistence.issue_generation;
            if let Some(message) = persistence.last_issue {
                self.toasts.push(crate::toasts::Toast::new(
                    message,
                    crate::toasts::ToastKind::Error,
                    false,
                    ctx.input(|input| input.time),
                ));
            }
        }
        // Static previews do not need a frame loop. Preview workers repaint on
        // completion, while loading state schedules its own bounded cadence.
        let has_animation =
            ctx.egui_is_using_pointer() || ctx.input(|i| i.smooth_scroll_delta.length() > 0.0);
        if has_animation {
            ctx.request_repaint();
        }

        let index_idle = ctx.input(|input| {
            input.events.is_empty()
                && !input.pointer.any_down()
                && input.smooth_scroll_delta == Vec2::ZERO
        }) && self.ws.active_transfer.is_none()
            && self.ws.pending_op.is_none()
            && self.find.as_ref().is_none_or(|state| !state.searching);
        if !index_idle {
            crate::io_budget::note_foreground_activity();
        }
        self.content_index.set_idle(index_idle);
        self.content_index.poll();

        // First frame: wire the repaint callback into both panels and do
        // the initial directory read.
        let first_listing = !self.ws.left.has_notify() || !self.ws.right.has_notify();
        let listing_latency = first_listing.then(|| {
            crate::measurement::LatencyGuard::new(crate::measurement::MetricName::FirstListing)
        });
        if !self.ws.left.has_notify() {
            let c = ctx.clone();
            self.ws
                .left
                .set_notify(std::sync::Arc::new(move || c.request_repaint()));
            self.ws.left.refresh();
        }
        if !self.ws.right.has_notify() {
            let c = ctx.clone();
            self.ws
                .right
                .set_notify(std::sync::Arc::new(move || c.request_repaint()));
            self.ws.right.refresh();
        }
        drop(listing_latency);
        if first_listing && let Some(mut trace) = self.startup_trace.take() {
            trace.checkpoint(crate::measurement::StartupPhase::FirstListing);
            trace.finish();
        }

        let fs_changed = self.ws.left.poll_fs_changes() | self.ws.right.poll_fs_changes();
        if fs_changed {
            self.tree_children_cache.clear();
        }
        if !self.ws.left.watcher_active() || !self.ws.right.watcher_active() {
            ctx.request_repaint_after(crate::panel::WATCHER_RETRY_BACKOFF);
        }

        // Drop targets are only valid for the frame that set them
        // (rows re-assert them while hovered during render).
        self.ws.left.drop_target = None;
        self.ws.right.drop_target = None;

        self.handle_keys(ctx);
        self.capture_escape_request(ctx);
        self.update_focus_mode(ctx);
        self.preload_images(ctx);
        {
            let c = ctx.clone();
            // poll_transfer drains the next queued transfer (two-way sync second
            // pass, or any op queued behind the active one) via this notify.
            if self.ws.poll_transfer(move || c.request_repaint()) {
                // A clean move just finished: raise an undoable toast and
                // log a receipt (jump-back + the same live undo affordance).
                let now = ctx.input(|i| i.time);
                if let Some(a) = self.ws.stack.peek_undo() {
                    self.toasts.push(crate::toasts::Toast::new(
                        format!("{} {} item(s)", a.verb(), a.item_count()),
                        crate::toasts::ToastKind::Success,
                        true,
                        now,
                    ));
                    if let Some(jump_to) = a.jump_to() {
                        self.receipts.push(crate::receipts::Receipt {
                            verb: a.verb(),
                            item_count: a.item_count(),
                            timestamp: now,
                            jump_to,
                            undo_action: Some(a.clone()),
                        });
                    }
                }
            }
        }
        self.toasts.prune(ctx.input(|i| i.time));
        // Open a filesystem-aware undo/redo review. Execution happens only from
        // the confirmation dialog and repeats the preflight at commit time.
        if std::mem::take(&mut self.ws.undo_request) {
            self.history_preview = self.ws.preview_undo().map(|preview| HistoryPreviewState {
                mode: HistoryReplayMode::Undo,
                preview,
                error: None,
            });
            if self.history_preview.is_none() {
                self.push_history_notice(ctx, "Nothing to undo", false);
            }
        }
        if std::mem::take(&mut self.ws.redo_request) {
            self.history_preview = self.ws.preview_redo().map(|preview| HistoryPreviewState {
                mode: HistoryReplayMode::Redo,
                preview,
                error: None,
            });
            if self.history_preview.is_none() {
                let reason = self.ws.redo_unavailable_reason();
                let is_error = reason.is_some();
                let message = reason.unwrap_or_else(|| "Nothing to redo".to_string());
                self.push_history_notice(ctx, &message, is_error);
            }
        }
        // Gather the selection into a new subfolder (queues an undoable Move).
        if std::mem::take(&mut self.ws.gather_request) {
            let c = ctx.clone();
            self.ws.gather_into_folder(move || c.request_repaint());
        }
        if let Some(kind) = self.ws.keyboard_drop_request.take() {
            let c = ctx.clone();
            self.ws
                .transfer_selection_into_cursor_folder(kind, move || c.request_repaint());
        }
        // Drain the shelf (copy staged items into the active pane).
        if std::mem::take(&mut self.ws.drain_request) {
            let c = ctx.clone();
            let outcome = self.ws.drain_shelf(move || c.request_repaint());
            if outcome.unavailable > 0 {
                let now = ctx.input(|i| i.time);
                let item = |n: usize| if n == 1 { "item" } else { "items" };
                self.toasts.push(crate::toasts::Toast::new(
                    format!(
                        "{} shelf {} unavailable, kept on the shelf",
                        outcome.unavailable,
                        item(outcome.unavailable)
                    ),
                    crate::toasts::ToastKind::Error,
                    false,
                    now,
                ));
            }
        }
        // Copy the selection's path(s) to the clipboard in the requested style.
        if let Some(style) = self.ws.clipboard_request.take() {
            let paths: Vec<std::path::PathBuf> = self
                .ws
                .active_panel_ref()
                .selected_or_cursor()
                .unwrap_or_default()
                .into_iter()
                .map(|e| e.path)
                .collect();
            if !paths.is_empty() {
                let other_root = self.ws.inactive_panel().current_path.clone();
                let text = crate::clipboard::format(&paths, style, Some(&other_root));
                ctx.copy_text(text);
                let now = ctx.input(|i| i.time);
                self.toasts.push(crate::toasts::Toast::new(
                    format!(
                        "Copied {} ({})",
                        crate::clipboard::style_label(style),
                        paths.len()
                    ),
                    crate::toasts::ToastKind::Success,
                    false,
                    now,
                ));
            }
        }
        // Copy an arbitrary text payload (e.g. an exported listing).
        if let Some((text, label)) = self.ws.clipboard_text_request.take() {
            ctx.copy_text(text);
            let now = ctx.input(|i| i.time);
            self.toasts.push(crate::toasts::Toast::new(
                format!("Copied {label}"),
                crate::toasts::ToastKind::Success,
                false,
                now,
            ));
        }
    }

    fn show_toolbar_panel(&mut self, ui: &mut egui::Ui) {
        let t = self.colors;
        let ctx = ui.ctx().clone();
        egui::Panel::top(crate::accessibility::FocusRegion::Toolbar.id())
            .frame(Frame::NONE.fill(t.bg_toolbar))
            .show(ui, |ui| {
                self.toolbar(ui, &ctx);
            });
    }

    fn show_shortcut_bar(&mut self, ui: &mut egui::Ui) {
        let t = self.colors;
        let ctx = ui.ctx().clone();
        let quick_context = self.quick_action_context();
        let quick_actions = crate::quick_actions::actions(quick_context);
        let next_hint = crate::quick_actions::next_hint(quick_context);
        let shortcut_context = self.ws.action_bar_command_context();
        let shortcut_hints = crate::command::contextual_shortcuts(&shortcut_context);
        // Top/bottom panels don't reduce the panel-carving `Ui`'s width, so
        // this doubles as "the window's content width" for the inner
        // `>= 1280.0` check below, deep inside nested layout closures where
        // `ui.available_width()` would only see the narrow nested region.
        let panel_width = ui.available_width();
        let compact = crate::accessibility::toolbar_mode(panel_width)
            == crate::accessibility::ToolbarMode::Compact;
        let max_quick_actions = if panel_width < 1120.0 { 2 } else { 4 };
        let mut quick_action: Option<crate::quick_actions::QuickAction> = None;
        egui::Panel::bottom("shortcuts")
            .frame(Frame::NONE.fill(t.bg_toolbar))
            .show(ui, |ui| {
                Frame::NONE
                    .inner_margin(Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let key_limit = if compact {
                                if panel_width < 600.0 { 2 } else { 4 }
                            } else {
                                8
                            };
                            for hint in shortcut_hints.iter().take(key_limit) {
                                ui.label(
                                    egui::RichText::new(hint.key)
                                        .size(11.0)
                                        .strong()
                                        .color(t.accent),
                                );
                                ui.label(
                                    egui::RichText::new(hint.label)
                                        .size(11.0)
                                        .color(t.text_muted),
                                );
                                ui.add_space(8.0);
                            }

                            // Scale slider on the right
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.add_space(12.0);
                                let chip = |ui: &mut egui::Ui, label: String, active: bool| {
                                    let fill = if active {
                                        t.accent.linear_multiply(0.14)
                                    } else {
                                        t.bg_card
                                    };
                                    let color = if active { t.accent } else { t.text_muted };
                                    Frame::NONE
                                        .fill(fill)
                                        .corner_radius(crate::theme::ROUNDING_SM)
                                        .inner_margin(Margin::symmetric(7, 2))
                                        .show(ui, |ui| {
                                            ui.label(
                                                egui::RichText::new(label).size(10.0).color(color),
                                            );
                                        });
                                };
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{}%",
                                        (self.ui_scale * 100.0) as u32
                                    ))
                                    .size(11.0)
                                    .color(t.text_muted),
                                );
                                if compact {
                                    return;
                                }
                                let mut preview = self.ui_scale;
                                let slider = egui::Slider::new(&mut preview, 0.8..=2.0)
                                    .step_by(0.05)
                                    .show_value(false)
                                    .trailing_fill(true);
                                let resp = ui.add_sized(egui::vec2(120.0, 16.0), slider);
                                self.ui_scale = crate::accessibility::sanitize_text_scale(preview);
                                // Apply only when released
                                if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                                    if (self.ui_scale - 1.0).abs() < 0.03 {
                                        self.ui_scale = 1.0;
                                    }
                                    ctx.set_zoom_factor(self.ui_scale);
                                }
                                ui.label(
                                    egui::RichText::new("\u{1f50d}")
                                        .size(12.0)
                                        .color(t.text_muted),
                                );
                                ui.add_space(10.0);
                                chip(
                                    ui,
                                    format!(
                                        "Rows {}",
                                        crate::density::short_label(
                                            self.ws.active_panel_ref().density
                                        )
                                    ),
                                    true,
                                );
                                chip(ui, "Tree".to_string(), self.show_tree);
                                chip(ui, "Compare".to_string(), self.show_compare);
                                chip(
                                    ui,
                                    "Hidden".to_string(),
                                    self.ws.active_panel_ref().show_hidden,
                                );
                                if !self.ws.shelf.is_empty() {
                                    chip(ui, format!("Shelf {}", self.ws.shelf.len()), true);
                                }
                                if !quick_actions.is_empty() {
                                    ui.add_space(8.0);
                                    for spec in quick_actions.iter().take(max_quick_actions).rev() {
                                        let fill = match spec.action {
                                            crate::quick_actions::QuickAction::DrainShelf
                                            | crate::quick_actions::QuickAction::AddToShelf => {
                                                t.accent.linear_multiply(0.22)
                                            }
                                            crate::quick_actions::QuickAction::ClearFilters
                                            | crate::quick_actions::QuickAction::ClearSelection => {
                                                t.accent_red.linear_multiply(0.12)
                                            }
                                            crate::quick_actions::QuickAction::FocusMode => {
                                                t.accent_purple.linear_multiply(0.14)
                                            }
                                            _ => t.bg_card,
                                        };
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(&spec.label)
                                                        .size(11.0)
                                                        .color(t.text_primary),
                                                )
                                                .fill(fill)
                                                .corner_radius(crate::theme::ROUNDING_SM),
                                            )
                                            .on_hover_text(spec.hint)
                                            .clicked()
                                        {
                                            quick_action = Some(spec.action);
                                        }
                                    }
                                }
                                if panel_width >= 1280.0 {
                                    ui.add_space(8.0);
                                    ui.label(
                                        egui::RichText::new(format!("Next: {next_hint}"))
                                            .size(11.0)
                                            .color(t.text_muted),
                                    );
                                }
                            });
                        });
                    });
            });
        if let Some(action) = quick_action {
            self.run_quick_action(action, &ctx);
        }
    }

    fn quick_action_context(&self) -> crate::quick_actions::QuickActionContext {
        let active = self.ws.active_panel_ref();
        crate::quick_actions::QuickActionContext {
            selected_count: active.selected.len(),
            shelf_count: self.ws.shelf.len(),
            has_filters: crate::panel::filter_is_active(&active.search_query, &active.facets),
        }
    }

    fn update_focus_mode(&mut self, ctx: &egui::Context) {
        if !self.focus_mode {
            return;
        }
        let escape_requested =
            self.take_escape_request(crate::accessibility::EscapeRoute::FocusMode);
        let exit_focus = ctx.input(|i| {
            crate::focus_mode::should_exit(
                self.focus_started_at,
                i.time,
                i.pointer.delta().length_sq(),
                escape_requested,
            )
        });
        if exit_focus {
            self.focus_mode = false;
        }
    }

    fn run_quick_action(&mut self, action: crate::quick_actions::QuickAction, ctx: &egui::Context) {
        use crate::quick_actions::QuickAction;
        match action {
            QuickAction::AddToShelf => self.ws.execute(crate::command::Command::ShelfAdd),
            QuickAction::DrainShelf => self.ws.execute(crate::command::Command::ShelfDrain),
            QuickAction::CopyNames => {
                self.ws.clipboard_request = Some(crate::clipboard::PathStyle::NameOnly);
            }
            QuickAction::BatchRename => {
                self.ws.execute(crate::command::Command::BeginBatchRename);
            }
            QuickAction::ClearSelection => {
                self.ws.active_panel().selected.clear();
            }
            QuickAction::ClearFilters => {
                self.ws.active_panel().clear_filters();
            }
            QuickAction::SaveFilter => self.save_active_filter_as_smart_folder(ctx),
            QuickAction::OpenPalette => self.ws.palette_request = true,
            QuickAction::FindFiles => self.ws.execute(crate::command::Command::BeginFind),
            QuickAction::RecentFolders => self.ws.execute(crate::command::Command::BeginRecent),
            QuickAction::FocusMode => {
                self.focus_mode = true;
                self.focus_started_at = ctx.input(|i| i.time);
            }
        }
    }

    fn save_active_filter_as_smart_folder(&mut self, ctx: &egui::Context) {
        let (name, root, query) = {
            let active = self.ws.active_panel_ref();
            let query = crate::query::from_panel_filter(&active.search_query, &active.facets);
            if query.predicates.is_empty() {
                return;
            }
            let folder = active
                .current_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| active.current_path.display().to_string());
            let descriptor = if active.search_query.trim().is_empty() {
                format!("{} facet(s)", active.facets.active_count())
            } else {
                active.search_query.trim().to_string()
            };
            (
                format!(
                    "Filter: {} - {}",
                    clipped_label(&folder, 24),
                    clipped_label(&descriptor, 32)
                ),
                active.current_path.clone(),
                query,
            )
        };
        self.smart_folders_mut()
            .add(crate::smart_folder::Definition {
                name: name.clone(),
                root,
                query,
            });
        let ok = crate::smart_folder::save(self.smart_folders_mut());
        let now = ctx.input(|i| i.time);
        let (msg, kind) = if ok {
            (
                format!("Saved smart folder \"{name}\""),
                crate::toasts::ToastKind::Success,
            )
        } else {
            (
                format!("Could not save smart folder \"{name}\" to disk"),
                crate::toasts::ToastKind::Error,
            )
        };
        self.toasts
            .push(crate::toasts::Toast::new(msg, kind, false, now));
    }

    /// Bottom shelf (drop stack) tray, shown only when something is staged:
    /// count + total size, removable chips, Drain-here and Clear.
    fn show_shelf_tray(&mut self, ui: &mut egui::Ui) {
        if self.ws.shelf.is_empty() {
            return;
        }
        let t = self.colors;
        let count = self.ws.shelf.len();
        let total = self
            .ws
            .shelf
            .total_size(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0));
        let items: Vec<std::path::PathBuf> = self.ws.shelf.items().to_vec();

        let mut remove: Option<std::path::PathBuf> = None;
        let mut clear = false;
        let mut drain = false;

        egui::Panel::bottom("shelf_tray")
            .frame(Frame::NONE.fill(t.bg_card))
            .show(ui, |ui| {
                Frame::NONE
                    .inner_margin(Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "\u{1f4cb} Shelf {count} \u{00b7} {}",
                                    crate::panel::format_size(total)
                                ))
                                .size(11.0)
                                .strong()
                                .color(t.accent),
                            );
                            ui.add_space(8.0);
                            for p in &items {
                                let name = p
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                let chip = Frame::NONE
                                    .fill(t.bg_panel)
                                    .stroke(Stroke::new(1.0_f32, t.border))
                                    .inner_margin(Margin::symmetric(6, 2))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(format!("{name}  \u{00d7}"))
                                                .size(11.0)
                                                .color(t.text_secondary),
                                        );
                                    })
                                    .response
                                    .interact(Sense::click());
                                if chip.clicked() {
                                    remove = Some(p.clone());
                                }
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("Clear")
                                                .size(11.0)
                                                .color(t.text_primary),
                                        )
                                        .fill(t.bg_panel)
                                        .corner_radius(CornerRadius::ZERO),
                                    )
                                    .clicked()
                                {
                                    clear = true;
                                }
                                ui.add_space(6.0);
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("Drain here \u{2318}\u{21e7}V")
                                                .size(11.0)
                                                .color(Color32::WHITE),
                                        )
                                        .fill(t.accent)
                                        .corner_radius(CornerRadius::ZERO),
                                    )
                                    .clicked()
                                {
                                    drain = true;
                                }
                            });
                        });
                    });
            });

        if let Some(p) = remove {
            self.ws.shelf.remove(&p);
        }
        if clear {
            self.ws.shelf.clear();
        }
        if drain {
            self.ws.drain_request = true;
        }
    }

    /// A quiet pill near the bottom of the central area summarizing the active
    /// panel's selection (count, folders, size, kind breakdown). Shown only
    /// when something is selected; complements the per-panel status bar.
    fn show_selection_hud(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let selected = self.ws.active_panel_ref().selected_entries();
        if selected.is_empty() {
            return;
        }
        let s = crate::selection_summary::summarize(&selected);
        let t = self.colors;

        let mut head = format!("{} item(s)", s.count);
        if s.dir_count > 0 {
            head.push_str(&format!(" \u{00b7} {} folder(s)", s.dir_count));
        }
        if s.total_bytes > 0 {
            head.push_str(&format!(
                " \u{00b7} {}",
                crate::panel::format_size(s.total_bytes)
            ));
        }
        // Average file size, once more than one item is picked.
        if s.count >= 2 && s.total_bytes > 0 {
            head.push_str(&format!(
                " \u{00b7} avg {}",
                crate::panel::format_size(s.total_bytes / s.count as u64)
            ));
        }
        // Largest/oldest are only meaningful once more than one item is picked.
        if s.count >= 2 {
            let short = |name: &str| -> String {
                const MAX: usize = 18;
                if name.chars().count() > MAX {
                    let head: String = name.chars().take(MAX - 1).collect();
                    format!("{head}\u{2026}")
                } else {
                    name.to_string()
                }
            };
            if let Some((name, sz)) = &s.largest
                && *sz > 0
            {
                head.push_str(&format!(
                    " \u{00b7} largest {} ({})",
                    short(name),
                    crate::panel::format_size(*sz)
                ));
            }
            if let Some((name, _)) = &s.oldest {
                head.push_str(&format!(" \u{00b7} oldest {}", short(name)));
            }
        }
        let breakdown: String = s
            .kinds
            .iter()
            .take(4)
            .map(|(k, n)| format!("{n} {}", k.label()))
            .collect::<Vec<_>>()
            .join("  \u{00b7}  ");

        let area = ui.available_rect_before_wrap();
        let screen = ctx.input(|i| i.viewport_rect());
        egui::Area::new(egui::Id::new("selection_hud"))
            .anchor(
                egui::Align2::CENTER_BOTTOM,
                [
                    area.center().x - screen.center().x,
                    -(screen.bottom() - area.bottom()) - 14.0,
                ],
            )
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(t.bg_card)
                    .stroke(Stroke::new(1.0_f32, t.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(12, 7))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(head)
                                    .size(11.0)
                                    .strong()
                                    .color(t.accent),
                            );
                            if !breakdown.is_empty() {
                                ui.add_space(10.0);
                                ui.label(
                                    egui::RichText::new(breakdown)
                                        .size(11.0)
                                        .color(t.text_muted),
                                );
                            }
                        });
                    });
            });
    }

    fn apply_context_menu_effect(
        &mut self,
        panel: ActivePanel,
        effect: Option<crate::provider_runtime::ContextMenuUiEffect>,
        ctx: &egui::Context,
    ) {
        let Some(effect) = effect else {
            return;
        };
        match effect {
            crate::provider_runtime::ContextMenuUiEffect::RefreshPanel => match panel {
                ActivePanel::Left => self.ws.left.refresh(),
                ActivePanel::Right => self.ws.right.refresh(),
            },
            crate::provider_runtime::ContextMenuUiEffect::Notice { level, message } => {
                let kind = match level {
                    crate::provider_runtime::ContextMenuNoticeLevel::Info => {
                        crate::toasts::ToastKind::Info
                    }
                    crate::provider_runtime::ContextMenuNoticeLevel::Error => {
                        crate::toasts::ToastKind::Error
                    }
                };
                let now = ctx.input(|input| input.time);
                self.toasts
                    .push(crate::toasts::Toast::new(message, kind, false, now));
            }
        }
    }

    /// Tree sidebar plus the two file panels with the resizable divider.
    fn show_main_area(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let t = self.colors;
        let active_left = self.ws.active == ActivePanel::Left;
        let close_active_preview =
            self.take_escape_request(crate::accessibility::EscapeRoute::ActivePreview);
        let (close_left_preview, close_right_preview) = if active_left {
            (close_active_preview, false)
        } else {
            (false, close_active_preview)
        };
        let left_metrics = crate::density::metrics(self.ws.left.density);
        let right_metrics = crate::density::metrics(self.ws.right.density);
        let drag_source = if !self.ws.left.drag_entries.is_empty() {
            Some(ActivePanel::Left)
        } else if !self.ws.right.drag_entries.is_empty() {
            Some(ActivePanel::Right)
        } else {
            None
        };
        let dragging = drag_source.is_some();
        // Side surfaces have already carved their space from this Ui. Base
        // pane geometry on the remaining width so panels never overlap them.
        let window_width = ui.available_width();
        let panel_id = egui::Id::new(crate::accessibility::FocusRegion::LeftPanel.id());
        // Build cross-panel comparison maps before borrowing panels mutably:
        // each panel is tinted against the OTHER panel's entries. Reuse the
        // cached maps while neither panel's entries changed, so compare mode does
        // not rebuild two HashMaps (cloning every name) on every painted frame.
        let cmp_right_gen = self.ws.right.entries_gen();
        let cmp_left_gen = self.ws.left.entries_gen();
        let (left_compare, right_compare) = if self.show_compare {
            match self.compare_cache.take() {
                Some((rg, lg, lmap, rmap)) if rg == cmp_right_gen && lg == cmp_left_gen => {
                    (Some(lmap), Some(rmap))
                }
                _ => (
                    Some(crate::compare::build_compare_map(&self.ws.right.entries)),
                    Some(crate::compare::build_compare_map(&self.ws.left.entries)),
                ),
            }
        } else {
            self.compare_cache = None;
            (None, None)
        };

        // Global tree sidebar
        let mut tree_actual_width: f32 = 0.0;
        if self.show_tree {
            let tree_max = (window_width * 0.3).clamp(100.0, 400.0);
            let tree_resp = egui::Panel::left("global_tree")
                .resizable(true)
                .default_size(self.tree_width.min(tree_max))
                .min_size(100.0)
                .max_size(tree_max)
                .frame(Frame::NONE.fill(t.bg_deep).inner_margin(Margin::same(0)))
                .show(ui, |ui| {
                    egui::ScrollArea::both()
                        .id_salt("global_tree_scroll")
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.style_mut().interaction.selectable_labels = false;
                            let tree_nav = self.render_global_tree(ui, &t);
                            if let Some(path) = tree_nav {
                                self.tree_expand_to_path(&path);
                                self.ws.active_panel().navigate_to(path);
                            }
                        });
                });
            tree_actual_width = tree_resp.response.rect.width();
        }

        // Compute half from remaining space (after tree)
        let remaining = window_width - tree_actual_width - 6.0;
        let half = remaining / 2.0;
        // Reset panel width when window resizes or tree toggled
        {
            let prev_half: f32 =
                ctx.data_mut(|d| d.get_temp(egui::Id::new("prev_half")).unwrap_or(0.0));
            if self.prev_window_width > 0.0
                && ((window_width - self.prev_window_width).abs() > 1.0
                    || (half - prev_half).abs() > 1.0)
            {
                ctx.data_mut(|d| {
                    d.remove::<egui::containers::panel::PanelState>(panel_id);
                });
            }
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("prev_half"), half));
        }
        self.prev_window_width = window_width;

        let mut tree_toggle = false;
        let archive_open = std::cell::RefCell::new(None);
        let external_opener = self.ws.opener.as_ref();
        let opener = |path: &std::path::Path| {
            if crate::archive::is_supported(path) {
                archive_open.replace(Some(path.to_path_buf()));
            } else {
                external_opener(path);
            }
        };
        let context_menu = std::rc::Rc::clone(&self.context_menu);

        // Left panel
        let pane_min = crate::accessibility::pane_min_width(remaining);
        let left_resp = egui::Panel::left(panel_id)
            .resizable(true)
            .default_size(half)
            .min_size(pane_min)
            .frame(Frame::NONE.fill(t.bg_deep).inner_margin(Margin::same(0)))
            .show(ui, |ui| {
                if ui.rect_contains_pointer(ui.max_rect()) && ctx.input(|i| i.pointer.any_pressed())
                {
                    self.ws.active = ActivePanel::Left;
                }
                Self::render_panel(
                    &mut self.ws.left,
                    ui,
                    self.ws.active == ActivePanel::Left,
                    close_left_preview,
                    &t,
                    &mut self.image_cache,
                    "left",
                    self.show_tree,
                    self.show_size_bars,
                    left_compare.as_ref(),
                    context_menu.as_ref(),
                    &opener,
                    dragging,
                    left_metrics,
                )
            });
        let left_outcome = left_resp.inner;
        tree_toggle |= left_outcome.tree_toggle;

        let hover_pos = ctx.input(|i| i.pointer.hover_pos());
        if dragging
            && drag_source != Some(ActivePanel::Left)
            && hover_pos.is_some_and(|pos| left_resp.response.rect.contains(pos))
            && self.ws.left.drop_target.is_none()
        {
            self.ws.left.drop_target = Some(self.ws.left.current_path.clone());
        }
        if self.ws.left.drop_target.is_some() {
            ctx.layer_painter(left_resp.response.layer_id).rect_stroke(
                left_resp.response.rect.shrink(1.0),
                CornerRadius::ZERO,
                Stroke::new(1.0, t.accent),
                egui::StrokeKind::Inside,
            );
        }

        // Double-click on panel divider → reset to 50/50
        {
            let panel_rect = left_resp.response.rect;
            let divider_rect = egui::Rect::from_min_max(
                egui::pos2(panel_rect.right() - 4.0, panel_rect.top()),
                egui::pos2(panel_rect.right() + 4.0, panel_rect.bottom()),
            );
            let double_clicked = ctx.input(|i| {
                if let Some(pos) = i.pointer.latest_pos() {
                    divider_rect.contains(pos)
                        && i.pointer
                            .button_double_clicked(egui::PointerButton::Primary)
                } else {
                    false
                }
            });
            if double_clicked {
                ctx.data_mut(|d| {
                    d.remove::<egui::containers::panel::PanelState>(panel_id);
                });
            }
        }

        // Right panel (takes remaining space)
        let right_resp = egui::CentralPanel::default()
            .frame(Frame::NONE.fill(t.bg_deep).inner_margin(Margin::same(0)))
            .show(ui, |ui| {
                ui.push_id(crate::accessibility::FocusRegion::RightPanel.id(), |ui| {
                    if ui.rect_contains_pointer(ui.max_rect())
                        && ctx.input(|i| i.pointer.any_pressed())
                    {
                        self.ws.active = ActivePanel::Right;
                    }
                    Self::render_panel(
                        &mut self.ws.right,
                        ui,
                        self.ws.active == ActivePanel::Right,
                        close_right_preview,
                        &t,
                        &mut self.image_cache,
                        "right",
                        self.show_tree,
                        self.show_size_bars,
                        right_compare.as_ref(),
                        context_menu.as_ref(),
                        &opener,
                        dragging,
                        right_metrics,
                    )
                })
                .inner
            });
        let right_outcome = right_resp.inner;
        tree_toggle |= right_outcome.tree_toggle;

        let pending_archive = archive_open.into_inner();
        self.apply_context_menu_effect(ActivePanel::Left, left_outcome.context_menu, &ctx);
        self.apply_context_menu_effect(ActivePanel::Right, right_outcome.context_menu, &ctx);

        if dragging
            && drag_source != Some(ActivePanel::Right)
            && hover_pos.is_some_and(|pos| right_resp.response.rect.contains(pos))
            && self.ws.right.drop_target.is_none()
        {
            self.ws.right.drop_target = Some(self.ws.right.current_path.clone());
        }
        if self.ws.right.drop_target.is_some() {
            ctx.layer_painter(right_resp.response.layer_id).rect_stroke(
                right_resp.response.rect.shrink(1.0),
                CornerRadius::ZERO,
                Stroke::new(1.0, t.accent),
                egui::StrokeKind::Inside,
            );
        }

        if tree_toggle {
            self.show_tree = !self.show_tree;
            if self.show_tree {
                let path = self.ws.active_panel().current_path.clone();
                self.tree_expand_to_path(&path);
            }
        }

        // Stash the freshly-used compare maps (keyed by the generations they were
        // built from) so the next frame reuses them while the entries are
        // unchanged.
        if let (Some(lmap), Some(rmap)) = (left_compare, right_compare) {
            self.compare_cache = Some((cmp_right_gen, cmp_left_gen, lmap, rmap));
        }
        if let Some(path) = pending_archive {
            self.ws.archive_request = Some(path);
            ctx.request_repaint();
        }
    }

    /// Floating label with the dragged file count next to the pointer.
    fn show_drag_overlay(&mut self, ctx: &egui::Context) {
        let t = self.colors;
        let drag_entries = if !self.ws.left.drag_entries.is_empty() {
            &self.ws.left.drag_entries
        } else if !self.ws.right.drag_entries.is_empty() {
            &self.ws.right.drag_entries
        } else {
            return;
        };
        let (source, other) = if !self.ws.left.drag_entries.is_empty() {
            (&self.ws.left, &self.ws.right)
        } else {
            (&self.ws.right, &self.ws.left)
        };
        let count = drag_entries.len();
        if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
            let label = if count == 1 {
                drag_entries[0]
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "1 item".into())
            } else {
                format!("{} items", count)
            };
            let explicit_target = source.drop_target.as_ref().or(other.drop_target.as_ref());
            let announcement = if let Some(target) = explicit_target {
                if !target.is_dir() {
                    crate::operation_view::DragAnnouncement::rejected("destination is not a folder")
                } else if crate::volume_profile::profile(target).read_only {
                    crate::operation_view::DragAnnouncement::rejected(
                        "destination volume is read-only",
                    )
                } else if drag_entries
                    .iter()
                    .any(|source| crate::fs_util::is_within_or_equal(target, source))
                {
                    crate::operation_view::DragAnnouncement::rejected(
                        "a folder cannot be moved into itself",
                    )
                } else {
                    let effect = if ctx.input(|input| input.modifiers.alt) {
                        crate::operation_view::DragEffect::Copy
                    } else {
                        crate::operation_view::DragEffect::Move
                    };
                    crate::operation_view::DragAnnouncement::valid(target.clone(), effect)
                }
            } else {
                crate::operation_view::DragAnnouncement::rejected("highlight a destination folder")
            };
            let target_label = announcement.text();
            let target_color = if matches!(
                announcement,
                crate::operation_view::DragAnnouncement::Valid { .. }
            ) {
                t.accent
            } else {
                t.accent_red
            };
            egui::Area::new(egui::Id::new("drag_overlay"))
                .fixed_pos(pos + egui::vec2(12.0, 12.0))
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .inner_margin(Margin::symmetric(9, 6))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(label)
                                    .size(12.0)
                                    .strong()
                                    .color(t.text_primary),
                            );
                            ui.label(
                                egui::RichText::new(target_label)
                                    .size(10.0)
                                    .color(target_color),
                            );
                        });
                });
        }
        ctx.request_repaint();
    }

    /// Floating capsule showing the current type-ahead buffer.
    fn show_type_ahead_overlay(&mut self, ctx: &egui::Context) {
        let Some((buffer, last)) = &self.type_ahead else {
            return;
        };
        let now = ctx.input(|i| i.time);
        if now - last > 1.5 {
            self.type_ahead = None;
            return;
        }
        let t = self.colors;
        let label = format!("\u{2192} {buffer}");
        let screen = ctx.input(|i| i.viewport_rect());
        egui::Area::new(egui::Id::new("type_ahead_overlay"))
            .fixed_pos(egui::pos2(screen.center().x - 60.0, screen.bottom() - 80.0))
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(t.bg_card)
                    .inner_margin(Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(label)
                                .size(13.0)
                                .strong()
                                .color(t.accent),
                        );
                    });
            });
        // Keep repainting so the capsule fades out on idle.
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    /// Bottom-right stack of operation toasts, each with a hairline countdown
    /// and an inline Undo on undoable ops.
    fn show_toasts(&mut self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        let t = self.colors;
        let now = ctx.input(|i| i.time);
        let screen = ctx.input(|i| i.viewport_rect());
        let active = self.toasts.active();
        let mut undo = false;
        let mut avoid = Vec::with_capacity(2);
        for id in ["current_focus_indicator", "active_error_surface"] {
            if let Some(rect) = ctx.data(|data| data.get_temp::<egui::Rect>(egui::Id::new(id))) {
                avoid.push(crate::accessibility::Rect {
                    x: rect.left(),
                    y: rect.top(),
                    width: rect.width(),
                    height: rect.height(),
                });
            }
        }
        let Some(stack) = crate::accessibility::place_overlay(
            crate::accessibility::Rect {
                x: screen.left(),
                y: screen.top(),
                width: screen.width(),
                height: screen.height(),
            },
            268.0,
            active.len() as f32 * 44.0,
            &avoid,
        ) else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return;
        };

        // Newest on top, in the nearest corner that preserves focus and errors.
        for (row, toast) in active.iter().rev().enumerate() {
            let y = stack.y + row as f32 * 44.0;
            let accent = match toast.kind {
                crate::toasts::ToastKind::Success => t.accent,
                crate::toasts::ToastKind::Error => t.accent_red,
                crate::toasts::ToastKind::Info => t.text_muted,
            };
            let frac = if self.accessibility_preferences.reduced_motion {
                1.0
            } else {
                crate::toasts::remaining_fraction(toast, now)
            };
            egui::Area::new(egui::Id::new(("toast", toast.id())))
                .fixed_pos(egui::pos2(stack.x, y))
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(t.bg_card)
                        .stroke(Stroke::new(1.0_f32, t.border))
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_width(236.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&toast.message)
                                        .size(12.0)
                                        .color(t.text_primary),
                                );
                                if toast.undoable {
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new("Undo \u{2318}Z")
                                                        .size(11.0)
                                                        .color(Color32::WHITE),
                                                )
                                                .fill(accent)
                                                .corner_radius(CornerRadius::ZERO),
                                            )
                                            .clicked()
                                        {
                                            undo = true;
                                        }
                                    });
                                }
                            });
                            // Hairline countdown.
                            let (rect, _) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 2.0),
                                Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                egui::Rect::from_min_size(
                                    rect.min,
                                    Vec2::new(rect.width() * frac, 2.0),
                                ),
                                CornerRadius::ZERO,
                                accent,
                            );
                        });
                });
        }
        if undo {
            self.ws.undo_request = true;
        }
        let repaint_ms = if self.accessibility_preferences.reduced_motion {
            1_000
        } else {
            100
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(repaint_ms));
    }

    /// On mouse release, move dragged files into the hovered directory.
    fn handle_drop(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.pointer.any_released()) {
            return;
        }
        let ctx2 = ctx.clone();
        let kind = if ctx.input(|input| input.modifiers.alt) {
            TransferKind::Copy
        } else {
            TransferKind::Move
        };
        if kind == TransferKind::Move {
            self.ws.drop_dragged(move || ctx2.request_repaint());
        } else {
            self.ws
                .drop_dragged_as(kind, move || ctx2.request_repaint());
        }
    }
}
