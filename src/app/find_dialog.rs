//! Recursive-find sheet: builds a `crate::query::Query` from a few fields,
//! runs it over the active folder, and lists results you can jump to.

use super::*;
use crate::panel::format_size;
use crate::selection_summary::Kind;

const FIND_CAP: usize = 1000;

impl App {
    pub(crate) fn show_find_dialog(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.ws.find_request) {
            self.find = Some(FindState {
                root: self.ws.active_panel_ref().current_path.clone(),
                ..Default::default()
            });
        }
        if self.find.is_none() {
            return;
        }
        let t = self.colors;

        let mut run = false;
        let mut cancel = false;
        let mut save = false;
        let mut reveal: Option<std::path::PathBuf> = None;

        {
            let state = self.find.as_mut().unwrap();
            let root_name = state
                .root
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());

            egui::Window::new("Find")
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
                    ui.set_width(520.0);
                    ui.label(
                        egui::RichText::new(format!("Find in {root_name}"))
                            .size(13.0)
                            .strong()
                            .color(t.text_primary),
                    );
                    ui.add_space(8.0);

                    let name_edit = egui::TextEdit::singleline(&mut state.name)
                        .desired_width(f32::INFINITY)
                        .hint_text("name contains\u{2026}")
                        .margin(egui::vec2(8.0, 6.0));
                    let resp = ui.add(name_edit);
                    if !state.focused {
                        resp.request_focus();
                        state.focused = true;
                    }
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("min MB").size(11.0).color(t.text_muted));
                        ui.add(
                            egui::TextEdit::singleline(&mut state.min_mb)
                                .desired_width(48.0)
                                .margin(egui::vec2(6.0, 4.0)),
                        );
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new("max age (days)")
                                .size(11.0)
                                .color(t.text_muted),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut state.max_age_days)
                                .desired_width(48.0)
                                .margin(egui::vec2(6.0, 4.0)),
                        );
                    });

                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new("kind").size(11.0).color(t.text_muted));
                        let mut kind_chip = |ui: &mut egui::Ui, label: &str, k: Option<Kind>| {
                            if ui
                                .selectable_label(
                                    state.kind == k,
                                    egui::RichText::new(label).size(12.0),
                                )
                                .clicked()
                            {
                                state.kind = k;
                            }
                        };
                        kind_chip(ui, "any", None);
                        kind_chip(ui, "images", Some(Kind::Image));
                        kind_chip(ui, "video", Some(Kind::Video));
                        kind_chip(ui, "audio", Some(Kind::Audio));
                        kind_chip(ui, "docs", Some(Kind::Document));
                        kind_chip(ui, "code", Some(Kind::Code));
                        kind_chip(ui, "archives", Some(Kind::Archive));
                        kind_chip(ui, "folders", Some(Kind::Folder));
                    });

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("Find").size(13.0).color(Color32::WHITE),
                                )
                                .fill(t.accent)
                                .corner_radius(CornerRadius::ZERO),
                            )
                            .clicked()
                            || enter
                        {
                            run = true;
                        }
                        ui.add_space(8.0);
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
                        {
                            cancel = true;
                        }
                        if state.ran {
                            ui.add_space(10.0);
                            let label = if state.results.len() >= FIND_CAP {
                                format!("{}+ results", FIND_CAP)
                            } else {
                                format!("{} result(s)", state.results.len())
                            };
                            ui.label(egui::RichText::new(label).size(11.0).color(t.text_muted));
                        }
                    });

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("save as")
                                .size(11.0)
                                .color(t.text_muted),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut state.save_name)
                                .desired_width(160.0)
                                .hint_text("smart folder name")
                                .margin(egui::vec2(6.0, 4.0)),
                        );
                        if ui
                            .add_enabled(
                                !state.save_name.trim().is_empty(),
                                egui::Button::new(
                                    egui::RichText::new("Save").size(12.0).color(t.text_primary),
                                )
                                .fill(t.bg_card)
                                .corner_radius(CornerRadius::ZERO),
                            )
                            .clicked()
                        {
                            save = true;
                        }
                    });

                    if state.ran && !state.results.is_empty() {
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical()
                            .max_height(300.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for e in &state.results {
                                    let rel = e
                                        .path
                                        .strip_prefix(&state.root)
                                        .unwrap_or(&e.path)
                                        .to_string_lossy()
                                        .to_string();
                                    let size = if e.is_dir {
                                        String::new()
                                    } else {
                                        format!("  \u{00b7}  {}", format_size(e.size))
                                    };
                                    let resp = ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!("{rel}{size}"))
                                                .size(12.0)
                                                .color(t.text_secondary),
                                        )
                                        .sense(Sense::click()),
                                    );
                                    if resp.hovered() {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                                    }
                                    if resp.clicked() {
                                        reveal = Some(e.path.clone());
                                    }
                                }
                            });
                    } else if state.ran {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("No matches.")
                                .size(12.0)
                                .color(t.text_muted),
                        );
                    }

                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancel = true;
                    }
                });
        }

        if let Some(path) = reveal {
            self.ws.reveal(&path);
            self.find = None;
            return;
        }
        if cancel {
            self.find = None;
            return;
        }
        if save {
            let def = {
                let s = self.find.as_ref().unwrap();
                crate::smart_folder::Definition {
                    name: s.save_name.trim().to_string(),
                    root: s.root.clone(),
                    query: s.build_query(),
                }
            };
            let name = def.name.clone();
            self.smart_folders_mut().add(def);
            crate::smart_folder::save(self.smart_folders_mut());
            let now = ctx.input(|i| i.time);
            self.toasts.push(crate::toasts::Toast::new(
                format!("Saved smart folder \u{201c}{name}\u{201d}"),
                crate::toasts::ToastKind::Success,
                false,
                now,
            ));
        }
        if run {
            let (query, root) = {
                let s = self.find.as_ref().unwrap();
                (s.build_query(), s.root.clone())
            };
            let results = self.ws.run_find(&query, &root, FIND_CAP);
            if let Some(s) = self.find.as_mut() {
                s.results = results;
                s.ran = true;
            }
        }
    }
}
