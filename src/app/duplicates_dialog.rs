//! Duplicate-finder sheet: groups byte-identical files in the active folder,
//! lets the user pick which copy to keep per group, and trashes the rest.
//! Grouping and keep-policy live in `crate::dedup`.

use super::*;
use crate::dedup::{KeepPolicy, default_keep, delete_count};
use crate::panel::format_size;

impl App {
    pub(crate) fn show_duplicates_dialog(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.ws.duplicates_request) {
            let policy = KeepPolicy::KeepShortestPath;
            let groups = self.ws.find_duplicates();
            let keep = groups.iter().map(|g| default_keep(g, policy)).collect();
            self.duplicates = Some(DupState {
                groups,
                keep,
                policy,
            });
        }
        if self.duplicates.is_none() {
            return;
        }
        let t = self.colors;

        // Snapshot display data so the window body only mutates `keep`.
        let (view, delete_total) = {
            let s = self.duplicates.as_ref().unwrap();
            let view: Vec<(u64, Vec<String>)> = s
                .groups
                .iter()
                .map(|g| {
                    let labels = g
                        .files
                        .iter()
                        .map(|f| {
                            f.path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default()
                        })
                        .collect();
                    (g.size, labels)
                })
                .collect();
            (view, delete_count(&s.groups))
        };

        let mut new_policy: Option<KeepPolicy> = None;
        let mut commit = false;
        let mut cancel = false;

        {
            let s = self.duplicates.as_mut().unwrap();
            egui::Window::new("Duplicates")
                .collapsible(false)
                .resizable(false)
                .title_bar(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(
                    Frame::NONE
                        .fill(t.bg_panel)
                        .inner_margin(Margin::same(16))
                        .stroke(Stroke::new(1.0_f32, t.border)),
                )
                .show(ctx, |ui| {
                    ui.set_width(580.0);
                    ui.label(
                        egui::RichText::new("Duplicate files")
                            .size(13.0)
                            .strong()
                            .color(t.text_primary),
                    );
                    ui.add_space(8.0);

                    if view.is_empty() {
                        ui.label(
                            egui::RichText::new("No duplicate files in this folder.")
                                .size(12.0)
                                .color(t.text_muted),
                        );
                        ui.add_space(12.0);
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
                            cancel = true;
                        }
                        return;
                    }

                    // Keep policy selector.
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Keep").size(11.0).color(t.text_muted));
                        for (p, text) in [
                            (KeepPolicy::KeepShortestPath, "shortest path"),
                            (KeepPolicy::KeepOldest, "oldest"),
                        ] {
                            if ui
                                .selectable_label(
                                    s.policy == p,
                                    egui::RichText::new(text).size(12.0),
                                )
                                .clicked()
                                && s.policy != p
                            {
                                new_policy = Some(p);
                            }
                        }
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);

                    egui::ScrollArea::vertical()
                        .max_height(320.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (gi, (size, labels)) in view.iter().enumerate() {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} copies \u{00b7} {} each",
                                        labels.len(),
                                        format_size(*size)
                                    ))
                                    .size(11.0)
                                    .color(t.accent),
                                );
                                for (fi, label) in labels.iter().enumerate() {
                                    ui.radio_value(&mut s.keep[gi], fi, label);
                                }
                                ui.add_space(6.0);
                            }
                        });

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{delete_total} file(s) will move to Trash"
                            ))
                            .size(11.0)
                            .color(t.text_muted),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new("Cancel")
                                            .size(13.0)
                                            .color(t.text_primary),
                                    )
                                    .fill(t.bg_card)
                                    .corner_radius(CornerRadius::ZERO),
                                )
                                .clicked()
                            {
                                cancel = true;
                            }
                            ui.add_space(8.0);
                            if ui
                                .add_enabled(
                                    delete_total > 0,
                                    egui::Button::new(
                                        egui::RichText::new("Move others to Trash")
                                            .size(13.0)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(t.accent_red)
                                    .corner_radius(CornerRadius::ZERO),
                                )
                                .clicked()
                            {
                                commit = true;
                            }
                        });
                    });

                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancel = true;
                    }
                });
        }

        if cancel {
            self.duplicates = None;
            return;
        }
        if let Some(p) = new_policy
            && let Some(s) = self.duplicates.as_mut()
        {
            s.policy = p;
            s.keep = s.groups.iter().map(|g| default_keep(g, p)).collect();
        }
        if commit {
            // Every non-kept file across all groups goes to the Trash.
            let to_trash: Vec<std::path::PathBuf> = {
                let s = self.duplicates.as_ref().unwrap();
                s.groups
                    .iter()
                    .enumerate()
                    .flat_map(|(gi, g)| {
                        let keep = s.keep[gi];
                        g.files
                            .iter()
                            .enumerate()
                            .filter(move |(fi, _)| *fi != keep)
                            .map(|(_, f)| f.path.clone())
                    })
                    .collect()
            };
            self.ws.trash_paths(&to_trash);
            self.duplicates = None;
        }
    }
}
