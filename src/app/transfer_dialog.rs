//! Progress window for an active transfer: bars, speed graph, errors.

use super::*;

impl App {
    pub(crate) fn show_transfer_dialog(&mut self, ctx: &egui::Context) {
        let Some(state) = self.ws.active_transfer.clone() else {
            return;
        };
        let t = self.colors;

        let s = state.lock().unwrap();
        let progress_frac = if s.total_bytes > 0 {
            s.copied_bytes as f32 / s.total_bytes as f32
        } else {
            0.0
        };
        let file_frac = if s.current_file_size > 0 {
            s.current_file_copied as f32 / s.current_file_size as f32
        } else {
            0.0
        };
        let speed = s.speed_bps();
        let eta = s.eta_secs();
        let current_file = s.current_file.clone();
        let current_file_copied = s.current_file_copied;
        let current_file_size = s.current_file_size;
        let files_done = s.files_done;
        let files_total = s.files_total;
        let copied = s.copied_bytes;
        let total = s.total_bytes;
        let samples: Vec<(f64, f64)> = s.speed_samples.clone();
        let finished = s.finished;
        let errors = s.errors.clone();
        drop(s);

        let title = if finished {
            if errors.is_empty() {
                "Transfer Complete"
            } else {
                "Completed with Errors"
            }
        } else {
            "Transferring..."
        };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .default_width(450.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                // Current file name
                ui.label(
                    egui::RichText::new(format!("File: {}", current_file))
                        .size(12.0)
                        .color(t.text_primary),
                );

                // Current file progress bar (no rounding)
                ui.add_space(4.0);
                Self::draw_progress_bar(
                    ui,
                    file_frac,
                    &format!(
                        "{} / {}",
                        format_size(current_file_copied),
                        format_size(current_file_size),
                    ),
                    t.accent,
                    &t,
                );

                // Total progress bar (no rounding)
                ui.add_space(6.0);
                ui.label(egui::RichText::new("Total:").size(11.0).color(t.text_muted));
                ui.add_space(2.0);
                Self::draw_progress_bar(
                    ui,
                    progress_frac,
                    &format!(
                        "{} / {} ({:.0}%)",
                        format_size(copied),
                        format_size(total),
                        progress_frac * 100.0,
                    ),
                    t.accent.linear_multiply(0.7),
                    &t,
                );

                // Speed + ETA + files
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{}/s", format_size(speed as u64)))
                            .size(11.0)
                            .color(t.text_muted),
                    );

                    ui.add_space(16.0);
                    if eta > 0.0 && !finished {
                        let mins = (eta / 60.0) as u64;
                        let secs = (eta % 60.0) as u64;
                        ui.label(
                            egui::RichText::new(format!("ETA: {}:{:02}", mins, secs))
                                .size(11.0)
                                .color(t.text_muted),
                        );
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("{}/{} files", files_done, files_total))
                                .size(11.0)
                                .color(t.text_muted),
                        );
                    });
                });

                Self::draw_speed_graph(ui, &samples, &t);

                // Errors collected during the transfer
                if !errors.is_empty() {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!("{} error(s):", errors.len()))
                            .size(12.0)
                            .strong()
                            .color(t.accent_red),
                    );
                    egui::ScrollArea::vertical()
                        .id_salt("transfer_errors")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for err in &errors {
                                ui.label(egui::RichText::new(err).size(11.0).color(t.accent_red));
                            }
                        });
                }

                ui.add_space(8.0);
                if finished {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("OK").size(13.0).color(Color32::WHITE),
                            )
                            .fill(t.accent)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                    {
                        self.ws.active_transfer = None;
                        self.ws.left.refresh();
                        self.ws.right.refresh();
                    }
                } else {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Cancel")
                                    .size(13.0)
                                    .color(Color32::WHITE),
                            )
                            .fill(t.accent_red)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                    {
                        self.ws.cancel_transfer();
                    }
                }
            });

        // Keep repainting during transfer
        if !finished {
            ctx.request_repaint();
        }
    }

    fn draw_speed_graph(ui: &mut egui::Ui, samples: &[(f64, f64)], t: &ThemeColors) {
        if samples.len() <= 2 {
            return;
        }
        ui.add_space(8.0);
        let graph_h = 60.0;
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), graph_h), Sense::hover());

        // Compute per-sample speed
        let mut speeds: Vec<f64> = Vec::new();
        for i in 1..samples.len() {
            let dt = samples[i].0 - samples[i - 1].0;
            let db = samples[i].1 - samples[i - 1].1;
            if dt > 0.01 {
                speeds.push(db / dt);
            } else {
                speeds.push(0.0);
            }
        }

        let max_speed = speeds.iter().cloned().fold(1.0f64, f64::max);
        let p = ui.painter();

        // Background
        p.rect_filled(rect, CornerRadius::same(3), t.bg_card.linear_multiply(0.3));

        // Draw speed line
        if speeds.len() >= 2 {
            let n = speeds.len();
            let points: Vec<egui::Pos2> = speeds
                .iter()
                .enumerate()
                .map(|(i, &s)| {
                    let x = rect.left() + (i as f32 / (n - 1) as f32) * rect.width();
                    let y = rect.bottom() - (s as f32 / max_speed as f32) * rect.height() * 0.9;
                    egui::pos2(x, y)
                })
                .collect();

            for w in points.windows(2) {
                p.line_segment([w[0], w[1]], Stroke::new(1.5, t.accent));
            }
        }

        // Max speed label
        p.text(
            egui::pos2(rect.left() + 4.0, rect.top() + 2.0),
            egui::Align2::LEFT_TOP,
            format!("{}/s", format_size(max_speed as u64)),
            egui::FontId::proportional(9.0),
            t.text_muted,
        );
    }

    /// Draw a flat progress bar without rounding.
    fn draw_progress_bar(
        ui: &mut egui::Ui,
        frac: f32,
        text: &str,
        fill_color: Color32,
        t: &ThemeColors,
    ) {
        let bar_h = 18.0;
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, bar_h), Sense::hover());
        let p = ui.painter();

        // Background
        p.rect_filled(rect, CornerRadius::ZERO, t.bg_card.linear_multiply(0.5));

        // Filled portion
        let filled_w = rect.width() * frac.clamp(0.0, 1.0);
        if filled_w > 0.0 {
            let filled_rect = egui::Rect::from_min_size(rect.min, Vec2::new(filled_w, bar_h));
            p.rect_filled(filled_rect, CornerRadius::ZERO, fill_color);
        }

        // Text centered
        p.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            egui::FontId::proportional(10.0),
            t.text_primary,
        );
    }
}
