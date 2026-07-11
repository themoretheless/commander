//! Disk-usage treemap sheet: squarifies the active folder's children by size.
//! The layout is computed by `crate::treemap`; this file only paints it.

use super::*;
use crate::panel::format_size;
use crate::selection_summary::kind_of;

const CANVAS_W: f32 = 680.0;
const CANVAS_H: f32 = 420.0;

impl App {
    pub(crate) fn show_treemap_dialog(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.ws.treemap_request) {
            let mut snapshot = self.ws.treemap_snapshot();
            snapshot.items.retain(|(_, bytes)| *bytes > 0);
            self.treemap = Some(snapshot);
        }
        let Some(state) = &self.treemap else {
            return;
        };
        let items = &state.items;
        let t = self.colors;
        let dark = (t.bg_panel.r() as u16 + t.bg_panel.g() as u16 + t.bg_panel.b() as u16) < 384;
        let folder = state
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());
        let total: u64 = items.iter().map(|(_, b)| *b).sum();
        let mut close = false;

        egui::Window::new("Disk usage")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                Frame::NONE
                    .fill(t.bg_panel)
                    .inner_margin(Margin::same(14))
                    .stroke(Stroke::new(1.0_f32, t.border)),
            )
            .show(ctx, |ui| {
                ui.set_width(CANVAS_W);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("Disk usage \u{00b7} {folder}"))
                            .size(13.0)
                            .strong()
                            .color(t.text_primary),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format_size(total))
                            .size(11.0)
                            .color(t.text_muted),
                    );
                });
                ui.add_space(8.0);

                if items.is_empty() {
                    ui.label(
                        egui::RichText::new(
                            "No size data yet. Folder sizes are still being computed.",
                        )
                        .size(12.0)
                        .color(t.text_muted),
                    );
                } else {
                    let weights: Vec<f64> = items.iter().map(|(_, b)| *b as f64).collect();
                    let tiles = crate::treemap::squarify(
                        &weights,
                        crate::treemap::Rect {
                            x: 0.0,
                            y: 0.0,
                            w: CANVAS_W as f64,
                            h: CANVAS_H as f64,
                        },
                    );

                    let (resp, painter) =
                        ui.allocate_painter(Vec2::new(CANVAS_W, CANVAS_H), Sense::hover());
                    let origin = resp.rect.min;

                    for (idx, ((entry, bytes), tile)) in items.iter().zip(&tiles).enumerate() {
                        if tile.w <= 0.0 || tile.h <= 0.0 {
                            continue;
                        }
                        let r = egui::Rect::from_min_size(
                            origin + Vec2::new(tile.x as f32, tile.y as f32),
                            Vec2::new(tile.w as f32, tile.h as f32),
                        );
                        let (cr, cg, cb) = crate::file_color::kind_color(kind_of(entry), dark);
                        let fill = Color32::from_rgb(cr, cg, cb);
                        painter.rect_filled(r, CornerRadius::same(1), fill);
                        painter.rect_stroke(
                            r,
                            CornerRadius::same(1),
                            Stroke::new(1.0_f32, t.bg_panel),
                            egui::StrokeKind::Inside,
                        );

                        // Label larger tiles; pick a text color that reads on
                        // the tile fill.
                        if r.width() > 54.0 && r.height() > 22.0 {
                            let lum = 0.299 * cr as f32 + 0.587 * cg as f32 + 0.114 * cb as f32;
                            let fg = if lum > 140.0 {
                                Color32::from_rgb(20, 20, 24)
                            } else {
                                Color32::from_rgb(244, 244, 248)
                            };
                            painter.text(
                                r.min + Vec2::new(5.0, 3.0),
                                egui::Align2::LEFT_TOP,
                                &entry.name,
                                egui::FontId::proportional(11.0),
                                fg,
                            );
                        }

                        ui.interact(r, ui.id().with(idx), Sense::hover())
                            .on_hover_text(format!(
                                "{}  \u{00b7}  {}",
                                entry.name,
                                format_size(*bytes)
                            ));
                    }
                }

                ui.add_space(10.0);
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Close")
                                .size(13.0)
                                .color(t.text_primary),
                        )
                        .fill(t.bg_card)
                        .corner_radius(CornerRadius::ZERO),
                    )
                    .clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    close = true;
                }
            });

        if close {
            self.treemap = None;
        }
    }
}
