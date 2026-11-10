//! Drag, type-ahead and toast overlays drawn above the panes.

use super::*;

impl App {
    /// Floating label with the dragged file count next to the pointer.
    pub(super) fn show_drag_overlay(&mut self, ctx: &egui::Context) {
        let t = self.colors;
        let Some(session) = self.ws.drag_session() else {
            self.drag_announcement.clear();
            return;
        };
        let drag_entries = session.entries;
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
            let alt = ctx.input(|input| input.modifiers.alt);
            let target = session.target;
            // A drop while other work runs is cancelled (`drop_dragged_as`), so
            // say why up front instead of announcing a move that never happens.
            // Checked every frame: the busy state can change mid-drag.
            let busy = drop_busy_reason(
                self.ws.mutation_block_reason("dropping"),
                self.ws.has_unfinished_transfer_work(),
                self.ws.pending_op.is_some(),
            );
            let announcement = if let Some(reason) = busy {
                crate::operation_view::DragAnnouncement::rejected(reason)
            } else {
                self.drag_announcement.get_or_compute(target, alt, || {
                    let Some(target) = target else {
                        return crate::operation_view::DragAnnouncement::rejected(
                            "point at a folder or the other pane",
                        );
                    };
                    if !target.is_dir() {
                        crate::operation_view::DragAnnouncement::rejected(
                            "destination is not a folder",
                        )
                    } else if crate::volume_profile::profile(target).read_only {
                        crate::operation_view::DragAnnouncement::rejected(
                            "destination volume is read-only",
                        )
                    } else if let Some(reason) = drag_path_rejection(target, drag_entries, alt) {
                        crate::operation_view::DragAnnouncement::rejected(reason)
                    } else {
                        let effect = if alt {
                            crate::operation_view::DragEffect::Copy
                        } else {
                            crate::operation_view::DragEffect::Move
                        };
                        crate::operation_view::DragAnnouncement::valid(target.clone(), effect)
                    }
                })
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
    pub(super) fn show_type_ahead_overlay(
        &mut self,
        ctx: &egui::Context,
        allow_state_updates: bool,
    ) {
        let Some((buffer, last)) = &self.ui.type_ahead else {
            return;
        };
        let now = ctx.input(|i| i.time);
        if now - last > super::keys::TYPE_AHEAD_IDLE {
            if allow_state_updates {
                self.ui.type_ahead = None;
            }
            return;
        }
        let t = self.colors;
        let miss = self.ui.type_ahead_miss;
        let label = type_ahead_label(buffer, miss);
        egui::Area::new(egui::Id::new("type_ahead_overlay"))
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -50.0])
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
                                .color(if miss { t.accent_red } else { t.accent }),
                        );
                    });
            });
        // Keep repainting so the capsule fades out on idle.
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }

    /// Bottom-right stack of operation toasts, each with a hairline countdown
    /// and an inline Undo on undoable ops.
    pub(super) fn show_toasts(&mut self, ctx: &egui::Context, input_enabled: bool) {
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
        // Newest on top; each row is as tall as it measured last frame.
        let heights: Vec<Option<f32>> = active
            .iter()
            .rev()
            .map(|toast| {
                ctx.memory(|memory| memory.area_rect(egui::Id::new(("toast", toast.id()))))
                    .map(|rect| rect.height())
            })
            .collect();
        let (offsets, stack_height) = toast_stack_offsets(&heights);
        let Some(stack) = crate::accessibility::place_overlay(
            crate::accessibility::Rect {
                x: screen.left(),
                y: screen.top(),
                width: screen.width(),
                height: screen.height(),
            },
            268.0,
            stack_height,
            &avoid,
        ) else {
            // Hidden toasts must not expire unseen (an error would be lost);
            // they keep their full lifetime until a corner is clear again.
            self.toasts.hold(now);
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
            return;
        };

        // Undo reverses the newest action, so only the newest undoable toast
        // offers it; an older toast's button would undo something else.
        let undo_toast = newest_undoable_toast(active);
        // Newest on top, in the nearest corner that preserves focus and errors.
        let undo_label = format!("Undo {}", command_shortcut("Z"));
        let mut shown = Vec::with_capacity(active.len());
        for (toast, offset) in active.iter().rev().zip(offsets) {
            let y = stack.y + offset;
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
            let area = egui::Area::new(egui::Id::new(("toast", toast.id())))
                .fixed_pos(egui::pos2(stack.x, y))
                .order(egui::Order::Tooltip)
                .enabled(input_enabled)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(t.bg_card)
                        .stroke(Stroke::new(1.0_f32, t.border))
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_width(236.0);
                            ui.horizontal(|ui| {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if undo_toast == Some(toast.id())
                                        && ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(&undo_label)
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
                                    // The message wraps in the space the Undo
                                    // button leaves instead of widening the toast.
                                    ui.allocate_ui_with_layout(
                                        Vec2::new(ui.available_width(), 0.0),
                                        Layout::top_down(Align::Min),
                                        |ui| {
                                            let message = ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(toast_text(
                                                        toast.kind,
                                                        &toast.message,
                                                    ))
                                                    .size(12.0)
                                                    .color(t.text_primary),
                                                )
                                                .wrap(),
                                            );
                                            let (role, live) = toast_live_region(toast.kind);
                                            ui.ctx().accesskit_node_builder(message.id, |node| {
                                                node.set_role(role);
                                                node.set_live(live);
                                            });
                                        },
                                    );
                                });
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
            shown.push(area.response.rect);
        }
        if pointer_over_toast(&shown, ctx.pointer_hover_pos()) {
            self.toasts.hold(now);
        }
        if input_enabled && undo {
            self.ws.execute(crate::command::Command::Undo);
        }
        let repaint_ms = if self.accessibility_preferences.reduced_motion {
            1_000
        } else {
            100
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(repaint_ms));
    }
}
