//! Fallback context menu: queuing, rendering and applying its effect.

use super::*;

impl App {
    pub(super) fn process_pending_context_menu(&mut self, ctx: &egui::Context) {
        if self.has_modal_surface() || !ctx.input(|input| input.focused) {
            self.ui.cancel_context_menu();
            return;
        }
        let request = match self.ui.poll_context_menu() {
            ui_state::ContextMenuPoll::Idle => return,
            ui_state::ContextMenuPoll::AwaitingPaint => {
                // The rest of frame B paints and publishes the new row cursor
                // and AccessKit tree; request frame C for native tracking.
                ctx.request_repaint();
                return;
            }
            ui_state::ContextMenuPoll::Ready(request) => request,
        };
        if self.ws.active != request.panel {
            return;
        }
        if !self.context_menu.presents_native_menu() {
            let pos = fallback_menu_pos(
                &request.invocation,
                ctx.input(|input| input.viewport_rect().min),
                ctx.pointer_latest_pos(),
                ctx.content_rect().center(),
            );
            self.fallback_menu = Some((request.panel, request.invocation.target.path.clone(), pos));
            ctx.data_mut(|data| {
                data.insert_temp(
                    egui::Id::new(FALLBACK_MENU_FOCUS),
                    FallbackMenuFocus {
                        restore: request.focus_id,
                        armed: false,
                        keyboard: matches!(
                            request.invocation.trigger,
                            crate::ports::ContextMenuTrigger::Keyboard
                        ),
                    },
                );
            });
            return;
        }
        let context_menu = std::rc::Rc::clone(&self.context_menu);
        let effect = crate::provider_runtime::request_context_menu(
            context_menu.as_ref(),
            &request.invocation,
        );
        self.apply_context_menu_effect(request.panel, effect, ctx);
        if !self.has_modal_surface() {
            ctx.memory_mut(|memory| memory.request_focus(request.focus_id));
        }
    }

    /// Context menu drawn by egui where no native menu exists (Linux, Windows).
    pub(super) fn show_fallback_context_menu(&mut self, ctx: &egui::Context) {
        use crate::provider_runtime::ContextMenuUiEffect as Effect;
        let focus_key = egui::Id::new(FALLBACK_MENU_FOCUS);
        // A modal opened from the keyboard (F8, F2...) dismisses the menu,
        // which would otherwise float above it with live items.
        if self.has_modal_surface() {
            self.fallback_menu = None;
        }
        let Some((panel, path, pos)) = self.fallback_menu.clone() else {
            ctx.data_mut(|data| data.remove::<FallbackMenuFocus>(focus_key));
            return;
        };
        let t = self.colors;
        let mut effect: Option<Effect> = None;
        let mut items: Vec<egui::Id> = Vec::new();
        let mut focus = ctx.data(|data| data.get_temp::<FallbackMenuFocus>(focus_key));
        // A right-click focuses the first item too, but shows the keyboard
        // focus ring only once the user starts navigating by key.
        let key_nav = ctx.input(|input| {
            [
                egui::Key::ArrowUp,
                egui::Key::ArrowDown,
                egui::Key::ArrowLeft,
                egui::Key::ArrowRight,
                egui::Key::Tab,
            ]
            .into_iter()
            .any(|key| input.key_pressed(key))
        });
        if key_nav && let Some(focus) = focus.as_mut() {
            focus.keyboard = true;
            ctx.data_mut(|data| data.insert_temp(focus_key, *focus));
        }
        let show_ring = focus.is_some_and(|focus| focus.keyboard);
        let area = egui::Area::new(egui::Id::new("fallback_context_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(t.bg_panel)
                    .stroke(Stroke::new(1.0_f32, t.border))
                    .corner_radius(crate::theme::ROUNDING_MD)
                    .inner_margin(Margin::same(4))
                    .show(ui, |ui| {
                        ui.set_min_width(180.0);
                        let mut item = |ui: &mut egui::Ui, label: &str, value: Effect| {
                            let response = ui.add(
                                egui::Button::new(label)
                                    .fill(Color32::TRANSPARENT)
                                    .stroke(Stroke::NONE)
                                    .min_size(Vec2::new(ui.available_width(), 24.0)),
                            );
                            if show_ring {
                                paint_focus_ring(ui, &response, &t);
                            }
                            items.push(response.id);
                            // The first item is focused on open, so Space
                            // (select, Quick Look) must not run it; Enter does.
                            if super::confirm_dialog::confirm_clicked(ui, &response) {
                                effect = Some(value);
                            }
                        };
                        item(
                            ui,
                            "Open",
                            Effect::Open(crate::ports::OpenRequest::OpenPath(path.clone())),
                        );
                        item(
                            ui,
                            "Show in file manager",
                            Effect::Open(crate::ports::OpenRequest::Reveal(path.clone())),
                        );
                        item(ui, "Copy path", Effect::CopyPath(path.clone()));
                        item(ui, "Rename\u{2026}", Effect::Rename(path.clone()));
                        ui.separator();
                        item(ui, "Move to Trash", Effect::MoveToTrash(path.clone()));
                        ui.separator();
                        item(ui, "Refresh", Effect::RefreshPanels);
                    });
            });
        let focus_in_menu = ctx
            .memory(|memory| memory.focused())
            .is_some_and(|id| items.contains(&id));
        let escaped = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        let mut focus_lost = false;
        match focus.map(|focus| fallback_menu_focus_step(focus.armed, focus_in_menu)) {
            Some(MenuFocusStep::FocusFirst) => {
                if let Some(first) = items.first() {
                    ctx.memory_mut(|memory| memory.request_focus(*first));
                }
                if let Some(focus) = focus.as_mut() {
                    focus.armed = true;
                    ctx.data_mut(|data| data.insert_temp(focus_key, *focus));
                }
            }
            Some(MenuFocusStep::Close) => focus_lost = true,
            Some(MenuFocusStep::Keep) | None => {}
        }
        let dismissed =
            escaped || focus_lost || (area.response.clicked_elsewhere() && effect.is_none());
        if effect.is_some() || dismissed {
            self.fallback_menu = None;
            ctx.data_mut(|data| data.remove::<FallbackMenuFocus>(focus_key));
            // Like the native menu, hand focus back to the row, unless the
            // user clicked elsewhere. Arrow keys and Tab move egui focus
            // spatially out of the menu; that closes it but returns to the row.
            let pointer_active = ctx.input(|input| {
                input.pointer.any_pressed()
                    || input.pointer.any_down()
                    || input.pointer.any_released()
            });
            if fallback_menu_restores_focus(escaped, effect.is_some(), focus_lost, pointer_active)
                && let Some(focus) = focus
            {
                ctx.memory_mut(|memory| memory.request_focus(focus.restore));
            }
        }
        if effect.is_some() {
            self.apply_context_menu_effect(panel, effect, ctx);
        }
    }

    pub(super) fn queue_context_menu_candidate(
        &mut self,
        panel: ActivePanel,
        candidate: ui_state::ContextMenuCandidate,
        ctx: &egui::Context,
    ) {
        self.ws.active = ui_state::context_menu_owner(self.ws.active, panel);
        let invocation = self.context_menu.prepare_context_menu(
            candidate.target,
            candidate.trigger,
            candidate.anchor,
        );
        self.ui
            .queue_context_menu(panel, invocation, candidate.focus_id);
        ctx.request_repaint();
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
        let effect = match effect {
            crate::provider_runtime::ContextMenuUiEffect::Perform(
                crate::ports::ContextMenuAction::Duplicate(target),
            ) => {
                self.ws.request_context_duplicate(panel, &target);
                None
            }
            crate::provider_runtime::ContextMenuUiEffect::Perform(action) => {
                let path = action.path().to_path_buf();
                let result = self.context_menu.perform_deferred_action(&action);
                crate::provider_runtime::reduce_deferred_context_menu_result(result, &path)
            }
            terminal => Some(terminal),
        };
        let Some(effect) = effect else {
            return;
        };
        match effect {
            crate::provider_runtime::ContextMenuUiEffect::RefreshPanels => {
                self.ws.left.refresh();
                self.ws.right.refresh();
            }
            crate::provider_runtime::ContextMenuUiEffect::Open(request) => {
                self.open_external(request, ctx);
            }
            crate::provider_runtime::ContextMenuUiEffect::CopyPath(path) => {
                let text = crate::clipboard::format(
                    std::slice::from_ref(&path),
                    crate::clipboard::PathStyle::FullPath,
                    None,
                );
                self.write_clipboard(&text, "path", ctx);
            }
            crate::provider_runtime::ContextMenuUiEffect::MoveToTrash(path) => {
                self.ws.request_context_delete(panel, &path);
            }
            crate::provider_runtime::ContextMenuUiEffect::Rename(path) => {
                self.open_rename(path, ctx);
            }
            crate::provider_runtime::ContextMenuUiEffect::Perform(action) => {
                self.toast(
                    ctx,
                    crate::toasts::ToastKind::Error,
                    format!(
                        "Could not {:?}: nested context-menu actions are not allowed",
                        action.command()
                    ),
                    false,
                );
            }
            crate::provider_runtime::ContextMenuUiEffect::Notice { level, message } => {
                let kind = match level {
                    crate::provider_runtime::ContextMenuNoticeLevel::Info => {
                        crate::toasts::ToastKind::Info
                    }
                    crate::provider_runtime::ContextMenuNoticeLevel::Error => {
                        crate::toasts::ToastKind::Error
                    }
                };
                self.toast(ctx, kind, message, false);
            }
        }
    }
}
