//! Filesystem-aware undo/redo review.

use super::*;

/// One fixed-height card per replayed path, so only visible cards are laid
/// out: undoing a move of thousands of items lists thousands of paths. Long
/// paths are elided (full text on hover); a blocked path's reason follows
/// its destination. Returns how many cards were laid out.
/// Width a blocked reason may take on a replay card's destination line: at
/// most half the row, so the destination path is never squeezed out.
fn blocked_reason_width(available: f32) -> f32 {
    if available.is_finite() {
        (available * 0.5).max(0.0)
    } else {
        0.0
    }
}

fn render_replay_paths(
    ui: &mut egui::Ui,
    paths: &[crate::undo::ReplayPath],
    t: &ThemeColors,
) -> usize {
    let font = egui::FontId::monospace(10.0);
    let line_h = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let margin = Margin::symmetric(9, 7);
    let card_h = line_h * 2.0 + f32::from(margin.top) + f32::from(margin.bottom) + 2.0;
    let mut laid_out = 0;
    egui::ScrollArea::vertical()
        .max_height(330.0)
        .auto_shrink([false, true])
        .show_rows(ui, card_h, paths.len(), |ui, range| {
            laid_out = range.len();
            for path in &paths[range] {
                let (status, status_color, reason) = match &path.eligibility {
                    crate::undo::ReplayEligibility::Ready => ("READY", t.accent, None),
                    crate::undo::ReplayEligibility::Blocked(reason) => {
                        ("BLOCKED", t.accent_red, Some(reason.as_str()))
                    }
                };
                let width = ui.available_width();
                ui.allocate_ui(Vec2::new(width, card_h), |ui| {
                    Frame::NONE
                        .fill(t.bg_card)
                        .inner_margin(margin)
                        .show(ui, |ui| {
                            ui.set_width(width - f32::from(margin.left + margin.right));
                            ui.set_height(card_h - f32::from(margin.top + margin.bottom));
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(status)
                                        .size(9.0)
                                        .strong()
                                        .color(status_color),
                                );
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(path.from.display().to_string())
                                                .font(font.clone())
                                                .color(t.text_primary),
                                        )
                                        .truncate(),
                                    );
                                    ui.horizontal(|ui| {
                                        if let Some(reason) = reason {
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    // The reason gets at most half the
                                                    // row and is elided (full text on
                                                    // hover), so the destination always
                                                    // keeps room.
                                                    let reason_w =
                                                        blocked_reason_width(ui.available_width());
                                                    ui.allocate_ui_with_layout(
                                                        Vec2::new(reason_w, line_h),
                                                        Layout::right_to_left(Align::Center),
                                                        |ui| {
                                                            ui.add(
                                                                egui::Label::new(
                                                                    egui::RichText::new(reason)
                                                                        .size(10.0)
                                                                        .color(t.accent_red),
                                                                )
                                                                .truncate(),
                                                            );
                                                        },
                                                    );
                                                    ui.add(
                                                        egui::Label::new(
                                                            egui::RichText::new(format!(
                                                                "\u{00bb} {}",
                                                                path.to.display()
                                                            ))
                                                            .font(font.clone())
                                                            .color(t.text_secondary),
                                                        )
                                                        .truncate(),
                                                    );
                                                },
                                            );
                                        } else {
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(format!(
                                                        "\u{00bb} {}",
                                                        path.to.display()
                                                    ))
                                                    .font(font.clone())
                                                    .color(t.text_secondary),
                                                )
                                                .truncate(),
                                            );
                                        }
                                    });
                                });
                            });
                        });
                });
            }
        });
    laid_out
}

impl App {
    pub(crate) fn push_history_notice(&mut self, ctx: &egui::Context, message: &str, error: bool) {
        self.toast(
            ctx,
            if error {
                crate::toasts::ToastKind::Error
            } else {
                crate::toasts::ToastKind::Info
            },
            message,
            false,
        );
    }

    pub(crate) fn show_history_dialog(&mut self, ctx: &egui::Context) {
        let escape_requested = self.take_modal_escape(crate::accessibility::ModalSurface::History);
        let Some(mut state) = self.ui.modals.history_preview.take() else {
            return;
        };
        let t = self.colors;
        let is_undo = state.mode == HistoryReplayMode::Undo;
        let title = if is_undo {
            "Undo preview"
        } else {
            "Redo preview"
        };
        let command = if is_undo { "Undo" } else { "Redo" };
        let replay_blocker = self.ws.history_replay_blocker();
        let paths_ready = state.preview.can_execute();
        let can_execute = paths_ready && replay_blocker.is_none();
        let ready = state.preview.paths.len() - state.preview.blocked_count();
        let mut cancel = false;
        let mut confirm = false;

        super::sheet_window(title, t, 14).show(ctx, |ui| {
            ui.set_width(620.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(title)
                            .size(14.0)
                            .strong()
                            .color(t.text_primary),
                    );
                    ui.label(
                        egui::RichText::new(state.preview.action.description())
                            .size(11.0)
                            .color(t.text_secondary),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let color = if paths_ready { t.accent } else { t.accent_red };
                    ui.label(
                        egui::RichText::new(format!("{ready}/{} ready", state.preview.paths.len()))
                            .size(11.0)
                            .strong()
                            .color(color),
                    );
                });
            });

            if let Some(error) = &state.error {
                ui.add_space(7.0);
                ui.label(egui::RichText::new(error).size(11.0).color(t.accent_red));
            }
            if let Some(error) = &replay_blocker {
                ui.add_space(7.0);
                ui.label(
                    egui::RichText::new(error)
                        .size(11.0)
                        .color(t.accent_warning),
                );
            }
            for warning in &state.preview.warnings {
                ui.add_space(7.0);
                ui.label(
                    egui::RichText::new(warning)
                        .size(11.0)
                        .color(t.accent_warning),
                );
            }

            ui.add_space(8.0);
            ui.separator();
            let _ = render_replay_paths(ui, &state.preview.paths, &t);

            ui.separator();
            ui.add_space(7.0);
            ui.horizontal(|ui| {
                if ui
                    .add(super::dialog_button(
                        t,
                        super::ButtonRole::Secondary,
                        egui::RichText::new("Cancel").size(12.0),
                    ))
                    .clicked()
                {
                    cancel = true;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            can_execute,
                            super::dialog_button(
                                t,
                                super::ButtonRole::Primary,
                                egui::RichText::new(command).size(12.0),
                            ),
                        )
                        .clicked()
                    {
                        confirm = true;
                    }
                });
            });

            if escape_requested {
                cancel = true;
            }
            if can_execute && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                confirm = true;
            }
        });

        if cancel {
            return;
        }
        if confirm {
            let repaint = ctx.clone();
            let result = if is_undo {
                self.ws.perform_undo(move || repaint.request_repaint())
            } else {
                self.ws.perform_redo(move || repaint.request_repaint())
            };
            match result {
                Ok(()) => {
                    if is_undo {
                        self.toasts.dismiss_undoable();
                    }
                    return;
                }
                Err(error) => {
                    state.error = Some(error);
                    let refreshed = if is_undo {
                        self.ws.preview_undo()
                    } else {
                        self.ws.preview_redo()
                    };
                    if let Some(preview) = refreshed {
                        state.preview = preview;
                    }
                }
            }
        }
        self.ui.modals.history_preview = Some(state);
    }
}

#[cfg(test)]
mod tests {
    use super::{blocked_reason_width, render_replay_paths};

    #[test]
    fn large_undo_preview_lays_out_only_visible_paths() {
        let paths: Vec<_> = (0..5_000)
            .map(|index| crate::undo::ReplayPath {
                from: std::path::PathBuf::from(format!("/from/{index}")),
                to: std::path::PathBuf::from(format!("/to/{index}")),
                eligibility: if index % 2 == 0 {
                    crate::undo::ReplayEligibility::Ready
                } else {
                    crate::undo::ReplayEligibility::Blocked("occupied".to_string())
                },
            })
            .collect();
        let ctx = egui::Context::default();
        let t = crate::theme::ThemeColors::light();
        let mut laid_out = 0;
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            laid_out = render_replay_paths(ui, &paths, &t);
        }));
        assert!(laid_out > 0);
        assert!(
            laid_out < 50,
            "{laid_out} cards laid out for a 330 px viewport"
        );
    }

    #[test]
    fn blocked_reason_keeps_half_the_row_for_the_destination() {
        assert_eq!(blocked_reason_width(400.0), 200.0);
        assert_eq!(blocked_reason_width(-5.0), 0.0);
        assert_eq!(blocked_reason_width(f32::INFINITY), 0.0);
        assert_eq!(blocked_reason_width(f32::NAN), 0.0);
    }

    #[test]
    fn long_blocked_reason_stays_inside_the_card() {
        let paths = vec![crate::undo::ReplayPath {
            from: std::path::PathBuf::from("/from/a"),
            to: std::path::PathBuf::from("/to/a"),
            eligibility: crate::undo::ReplayEligibility::Blocked("occupied ".repeat(200)),
        }];
        let ctx = egui::Context::default();
        let t = crate::theme::ThemeColors::light();
        let mut used = egui::Rect::NOTHING;
        let mut max_width = 0.0;
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_max_width(300.0);
            max_width = ui.max_rect().width();
            render_replay_paths(ui, &paths, &t);
            used = ui.min_rect();
        }));
        assert!(
            used.width() <= max_width + 1.0,
            "card grew to {} px in a {max_width} px column",
            used.width()
        );
    }
}
