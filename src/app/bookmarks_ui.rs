//! Bookmarks UI (full picker hotlist).
//! Designer: like FAR hotlist / TC favorites + VSCode bookmarks. Click to jump, assign current, delete.
//! Persist via session already.

use egui::{Align2, Frame, Margin, Stroke};

use super::*;

impl App {
    pub(crate) fn show_bookmarks_dialog(&mut self, ctx: &egui::Context) {
        if self.ui.bookmarks_open.is_none() {
            return;
        }
        let Some(filter) = &mut self.ui.bookmarks_open else { return; };
        let t = self.colors;
        let ws = &mut self.ws;

        let mut go_to: Option<std::path::PathBuf> = None;
        let mut do_assign = false;
        let mut remove_idx: Option<usize> = None;
        let mut close = false;

        egui::Window::new("Bookmarks")
            .collapsible(false)
            .resizable(true)
            .default_width(420.0)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(Frame::NONE.fill(t.bg_panel).inner_margin(Margin::same(10)).stroke(Stroke::new(1.0_f32, t.border)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(filter).hint_text("Filter bookmarks\u{2026}").desired_width(200.0));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close = true;
                    }
                });
                ui.add_space(4.0);

                let f = filter.to_lowercase();
                let filtered: Vec<(usize, &crate::session::Bookmark)> = ws
                    .bookmarks
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| {
                        b.name.to_lowercase().contains(&f)
                            || b.path.to_string_lossy().to_lowercase().contains(&f)
                    })
                    .collect();

                if filtered.is_empty() {
                    ui.label(
                        egui::RichText::new("No bookmarks. Use Assign or add.").size(11.0).color(t.text_muted),
                    );
                }

                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for (orig_idx, b) in filtered {
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new(&b.name).frame(false)).clicked() {
                                go_to = Some(b.path.clone());
                                close = true;
                            }
                            ui.label(egui::RichText::new(b.path.display().to_string()).size(10.0).color(t.text_muted));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("x").clicked() {
                                    remove_idx = Some(orig_idx);
                                }
                            });
                        });
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Assign current dir").clicked() {
                        do_assign = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                    ui.label(egui::RichText::new(format!("{} total", ws.bookmarks.len())).size(10.0).color(t.text_muted));
                });
            });

        if let Some(idx) = remove_idx {
            ws.bookmarks.remove(idx);
        }
        if do_assign {
            let p = ws.active_panel_ref().current_path().clone();
            if !ws.bookmarks.iter().any(|b| b.path == p) {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string());
                ws.bookmarks.push(crate::session::Bookmark { name, path: p });
            }
        }
        if let Some(path) = go_to {
            ws.active_panel().navigate_to(path);
        }
        if close {
            self.ui.bookmarks_open = None;
        }
    }
}
