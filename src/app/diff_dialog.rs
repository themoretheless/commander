//! Read-only unified diff sheet. The diff is computed by `crate::textdiff`;
//! this file only reads the files and renders the result.

use super::*;
use crate::textdiff::{DiffKind, change_counts, diff_lines};
use std::path::Path;

/// Largest file we will read into memory for a diff.
const DIFF_SIZE_CAP: u64 = 2 * 1024 * 1024;

fn read_text(path: &Path) -> Result<String, ()> {
    let meta = std::fs::metadata(path).map_err(|_| ())?;
    if meta.len() > DIFF_SIZE_CAP {
        return Err(());
    }
    std::fs::read_to_string(path).map_err(|_| ())
}

impl App {
    pub(crate) fn show_diff_dialog(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.ws.requests.diff_request) {
            match self.ws.diff_targets() {
                None => {
                    let now = ctx.input(|i| i.time);
                    self.toasts.push(crate::toasts::Toast::new(
                        "Select a file pair to diff",
                        crate::toasts::ToastKind::Success,
                        false,
                        now,
                    ));
                }
                Some((a, b)) => {
                    let name = |p: &Path| {
                        p.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default()
                    };
                    let (lines, message) = match (read_text(&a), read_text(&b)) {
                        (Ok(ta), Ok(tb)) => (diff_lines(&ta, &tb), None),
                        _ => (
                            Vec::new(),
                            Some(
                                "One or both files are binary, too large, or unreadable."
                                    .to_string(),
                            ),
                        ),
                    };
                    self.diff = Some(DiffState {
                        name_a: name(&a),
                        name_b: name(&b),
                        lines,
                        message,
                    });
                }
            }
        }
        let Some(state) = &self.diff else {
            return;
        };
        let t = self.colors;
        let mut close = false;

        let (ins, del) = change_counts(&state.lines);

        egui::Window::new("Diff")
            .collapsible(false)
            .resizable(true)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                Frame::NONE
                    .fill(t.bg_panel)
                    .inner_margin(Margin::same(14))
                    .stroke(Stroke::new(1.0_f32, t.border)),
            )
            .show(ctx, |ui| {
                ui.set_width(720.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{}  vs  {}", state.name_a, state.name_b))
                            .size(13.0)
                            .strong()
                            .color(t.text_primary),
                    );
                    if state.message.is_none() {
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(format!("+{ins}"))
                                .size(11.0)
                                .color(t.accent),
                        );
                        ui.label(
                            egui::RichText::new(format!("-{del}"))
                                .size(11.0)
                                .color(t.accent_red),
                        );
                    }
                });
                ui.add_space(8.0);

                if let Some(msg) = &state.message {
                    ui.label(egui::RichText::new(msg).size(12.0).color(t.text_muted));
                } else if state.lines.is_empty() {
                    ui.label(
                        egui::RichText::new("The files are identical.")
                            .size(12.0)
                            .color(t.text_muted),
                    );
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(420.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for line in &state.lines {
                                let (mark, color) = match line.kind {
                                    DiffKind::Equal => (" ", t.text_muted),
                                    DiffKind::Insert => ("+", t.accent),
                                    DiffKind::Delete => ("-", t.accent_red),
                                };
                                ui.label(
                                    egui::RichText::new(format!("{mark} {}", line.text))
                                        .monospace()
                                        .size(12.0)
                                        .color(color),
                                );
                            }
                        });
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
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
            });

        if close {
            self.diff = None;
        }
    }
}
