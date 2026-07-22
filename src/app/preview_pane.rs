//! Preview pane and grip resizer rendering.
//! SRP extraction from app/render.rs render_panel.
//! Handles the non-replacing preview (image/text/info) + draggable grip for height.
//! Keeps the list below resizing naturally.

use egui::{Align, Frame, Layout, Margin, Sense};
use std::fs;

use super::*;  // for ThemeColors, etc, but adjust as needed

pub(crate) fn render_preview_and_grip(
    panel: &mut PanelState,
    ui: &mut egui::Ui,
    t: &ThemeColors,
    image_cache: &mut crate::image_cache::ImageCache,
    panel_side: &str,
) {
    if panel.preview.is_some() {
        let ph = panel.preview_height.unwrap_or(180.0).clamp(80.0, 500.0);
        let w = ui.available_width();

        // Fixed-height strip for preview content (image viewer, text, or info card).
        ui.allocate_ui_with_layout(
            egui::vec2(w, ph),
            egui::Layout::top_down(egui::Align::LEFT),
            |ui| {
                if let Some(preview) = panel.preview().cloned() {
                    use crate::panel::PreviewContent;
                    match &preview {
                        PreviewContent::Image(path) => {
                            let path = path.clone();
                            if let Some(texture) = image_cache.get_or_load_sync(ui.ctx(), &path) {
                                let tex_size = texture.size_vec2();
                                ui.centered_and_justified(|ui| {
                                    let avail = ui.available_size();
                                    let scale =
                                        (avail.x / tex_size.x).min(avail.y / tex_size.y).min(1.0);
                                    let display_size =
                                        egui::vec2(tex_size.x * scale, tex_size.y * scale);
                                    let resp = ui.add(egui::Image::from_texture(
                                        egui::load::SizedTexture::new(texture.id(), display_size),
                                    ));
                                    if resp.clicked()
                                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                                    {
                                        panel.set_preview_content(None, None);
                                    }
                                });
                            } else {
                                ui.centered_and_justified(|ui| {
                                    ui.spinner();
                                });
                            }
                        }
                        PreviewContent::Text { path, content } => {
                            // Text viewer + hex toggle (idea #98)
                            Frame::NONE
                                .fill(t.bg_panel)
                                .inner_margin(Margin::same(8))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(
                                                path.file_name()
                                                    .map(|n| n.to_string_lossy().to_string())
                                                    .unwrap_or_default(),
                                            )
                                            .size(12.0)
                                            .strong()
                                            .color(t.text_primary),
                                        );
                                        // hex/text toggle
                                        let hlabel = if panel.preview_hex { "hex*" } else { "txt" };
                                        if ui.small_button(hlabel).on_hover_text("Toggle hex dump / text view").clicked() {
                                            let new_hex = !panel.preview_hex();
                                            panel.set_preview_hex(new_hex);
                                            if new_hex {
                                                if panel.preview_hex_bytes().is_none() {
                                                    if let Ok(b) = fs::read(path) {
                                                        panel.set_preview_hex_bytes(Some(b));
                                                    }
                                                }
                                            } else {
                                                panel.set_preview_hex_bytes(None);
                                            }
                                        }
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if ui.small_button("✕").clicked()
                                                    || ui
                                                        .input(|i| i.key_pressed(egui::Key::Escape))
                                                {
                                                    panel.set_preview_content(None, None);
                                                    panel.set_preview_hex(false);
                                                    panel.set_preview_hex_bytes(None);
                                                }
                                            },
                                        );
                                    });
                                    ui.add(egui::Separator::default().spacing(4.0));

                                    let show_hex = panel.preview_hex;
                                    egui::ScrollArea::both()
                                        .id_salt(format!("text_preview_{}", panel_side))
                                        .auto_shrink([false; 2])
                                        .show(ui, |ui| {
                                            ui.style_mut().interaction.selectable_labels = true;
                                            if show_hex {
                                                // Simple hex + ascii dump (use cache to avoid fs per frame)
                                                let bytes_opt = panel.preview_hex_bytes.as_ref().cloned().or_else(|| fs::read(path).ok());
                                                if let Some(bytes) = bytes_opt {
                                                    let mut lines: Vec<String> = vec![];
                                                    for (row, chunk) in bytes.chunks(16).take(64).enumerate() {
                                                        let off = format!("{:04x}  ", row * 16);
                                                        let hexs: Vec<String> = chunk.iter().map(|b| format!("{:02x}", b)).collect();
                                                        let hex = hexs.join(" ");
                                                        let ascii: String = chunk.iter().map(|&b| if b>=32 && b<127 { b as char } else { '.' }).collect();
                                                        lines.push(format!("{}{:48}  |{}|", off, hex, ascii));
                                                    }
                                                    for l in lines {
                                                        ui.label(egui::RichText::new(l).size(9.0).font(egui::FontId::monospace(9.0)).color(t.text_secondary));
                                                    }
                                                } else {
                                                    ui.label("hex unavailable");
                                                }
                                            } else {
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(content)
                                                            .size(12.0)
                                                            .font(egui::FontId::monospace(12.0))
                                                            .color(t.text_secondary),
                                                    )
                                                    .wrap(),
                                                );
                                            }
                                        });
                                });
                        }
                        PreviewContent::Info(card) => {
                            Frame::NONE
                                .fill(t.bg_panel)
                                .inner_margin(Margin::same(14))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("Get Info")
                                                .size(13.0)
                                                .strong()
                                                .color(t.text_primary),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if ui.small_button("✕").clicked()
                                                    || ui
                                                        .input(|i| i.key_pressed(egui::Key::Escape))
                                                {
                                                    panel.set_preview_content(None, None);
                                                }
                                            },
                                        );
                                    });
                                    ui.add(egui::Separator::default().spacing(8.0));
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(&card.name)
                                            .size(15.0)
                                            .strong()
                                            .color(t.text_primary),
                                    );
                                    ui.add_space(8.0);
                                    let mut row = |k: &str, v: &str| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(k)
                                                    .size(11.0)
                                                    .color(t.text_muted),
                                            );
                                            ui.label(
                                                egui::RichText::new(v)
                                                    .size(11.0)
                                                    .color(t.text_secondary),
                                            );
                                        });
                                    };
                                    row("Kind", &card.kind);
                                    row("Size", &card.size);
                                    if let Some(n) = card.children {
                                        row("Items", &n.to_string());
                                    };
                                    row("Modified", &card.modified);
                                    row("Permissions", &card.permissions);
                                    ui.add_space(6.0);
                                    ui.label(
                                        egui::RichText::new(&card.path)
                                            .size(10.0)
                                            .color(t.text_muted),
                                    );
                                });
                        }
                    }
                }
            },
        );

        // Grip / resizer separator (only visible while preview is open).
        // Dragging updates the per-tab preview_height; the list below flows into the freed space.
        let grip_h = 6.0;
        let (grip_rect, drag_resp) = ui.allocate_exact_size(egui::vec2(w, grip_h), Sense::drag());
        crate::app::ui_common::paint_grip(ui, grip_rect, t, false);
        if drag_resp.hovered() {
            crate::app::ui_common::apply_hover_paint(ui, grip_rect, t);
        }
        if drag_resp.dragged() {
            let current = panel.preview_height.unwrap_or(180.0);
            let new_h = (current + drag_resp.drag_delta().y).max(80.0).min(500.0);
            panel.set_preview_height(Some(new_h));
        }
        // Close only via the X button inside the preview content (or Escape while focused there).
        // Grip itself is purely for resize; click on it does not close.
    }
}