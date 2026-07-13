//! Run-command / open-with bar: type or pick a saved template, watch the
//! placeholders expand live against the selection, and run it with Enter. The
//! command runs through `sh -c` in the active directory; only the explicit,
//! user-entered line is executed (never anything derived from file contents),
//! and all path substitutions are shell-quoted by `cmdtemplate::expand`.

use super::*;
use crate::cmdtemplate::{SegmentKind, SelectionCtx, expand, preview_segments};

impl App {
    pub(crate) fn show_run_command_dialog(&mut self, ctx: &egui::Context) {
        let just_opened = std::mem::take(&mut self.ws.run_command_request);
        if just_opened {
            self.command_templates_mut(); // force a load from disk
            self.run_command = Some(RunCommandState {
                line: String::new(),
            });
        }
        if self.run_command.is_none() {
            return;
        }
        let t = self.colors;

        // Snapshot the selection context before borrowing the bar state.
        let sel = self
            .ws
            .active_panel_ref()
            .selected_or_cursor()
            .unwrap_or_default();
        let dir = self.ws.active_panel_ref().current_path.clone();
        let sctx = SelectionCtx {
            paths: sel.iter().map(|e| e.path.clone()).collect(),
            dir: dir.clone(),
            dir_other: self.ws.inactive_panel().current_path.clone(),
        };
        let sel_count = sel.len();
        // Templates whose extension filter accepts the selection.
        let matching: Vec<(String, String)> = self
            .command_templates_mut()
            .matching(&sel)
            .iter()
            .map(|t| (t.name.clone(), t.raw.clone()))
            .collect();

        let mut run: Option<String> = None; // expanded command line to execute
        let mut cancel = false;
        let mut fill: Option<String> = None; // template raw chosen from the list
        let mut save_template: Option<String> = None; // raw line to persist

        let state = self.run_command.as_mut().unwrap();
        egui::Window::new("Run command")
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
                ui.set_width(560.0);
                let item = if sel_count == 1 { "item" } else { "items" };
                ui.label(
                    egui::RichText::new(format!(
                        "Run command on {sel_count} {item}  ({{paths}} {{names}} {{dir}} {{dir_other}})"
                    ))
                    .size(11.0)
                    .color(t.text_muted),
                );
                ui.add_space(6.0);

                let resp = ui.add(
                    egui::TextEdit::singleline(&mut state.line)
                        .desired_width(f32::INFINITY)
                        .hint_text("open -a Preview {paths}")
                        .margin(egui::vec2(8.0, 6.0)),
                );
                // Focus on the opening frame only (never yank it back).
                if just_opened {
                    resp.request_focus();
                }

                // Live expansion preview, placeholders tinted.
                ui.add_space(6.0);
                let segs = preview_segments(&state.line, &sctx);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if segs.is_empty() {
                        ui.label(
                            egui::RichText::new("(empty)")
                                .size(11.0)
                                .monospace()
                                .color(t.text_muted),
                        );
                    }
                    for seg in &segs {
                        let color = match seg.kind {
                            SegmentKind::Literal => t.text_secondary,
                            SegmentKind::Substituted => t.accent,
                        };
                        ui.label(egui::RichText::new(&seg.text).size(11.0).monospace().color(color));
                    }
                });

                // Saved templates that apply to this selection.
                if !matching.is_empty() {
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Templates").size(10.0).color(t.text_muted));
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        for (name, raw) in &matching {
                            let row = ui.add(
                                egui::Button::new(
                                    egui::RichText::new(format!("{name}    {raw}"))
                                        .size(12.0)
                                        .color(t.text_primary),
                                )
                                .fill(t.bg_card)
                                .corner_radius(CornerRadius::ZERO),
                            );
                            if row.clicked() {
                                fill = Some(raw.clone());
                            }
                        }
                    });
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let can_run = !state.line.trim().is_empty();
                    if ui
                        .add_enabled(
                            can_run,
                            egui::Button::new(
                                egui::RichText::new("Run").size(13.0).color(Color32::WHITE),
                            )
                            .fill(t.accent)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                    {
                        run = Some(expand(&state.line, &sctx));
                    }
                    ui.add_space(8.0);
                    if ui
                        .add_enabled(
                            can_run,
                            egui::Button::new(
                                egui::RichText::new("Save as template")
                                    .size(13.0)
                                    .color(t.text_primary),
                            )
                            .fill(t.bg_card)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .on_hover_text("Save this command line as a reusable template")
                        .clicked()
                    {
                        save_template = Some(state.line.clone());
                    }
                    ui.add_space(8.0);
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("Cancel").size(13.0).color(t.text_primary),
                            )
                            .fill(t.bg_card)
                            .corner_radius(CornerRadius::ZERO),
                        )
                        .clicked()
                    {
                        cancel = true;
                    }
                    if can_run && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        run = Some(expand(&state.line, &sctx));
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancel = true;
                    }
                });
            });

        if cancel {
            self.run_command = None;
            return;
        }
        if let Some(raw) = fill
            && let Some(s) = &mut self.run_command
        {
            s.line = raw;
            return;
        }
        if let Some(line) = save_template {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                // Name the template after its leading word; an empty extension
                // filter means it applies to any selection. The user can refine
                // the name/filter in the JSON store.
                let name = trimmed
                    .split_whitespace()
                    .next()
                    .unwrap_or("Command")
                    .to_string();
                let now = ctx.input(|i| i.time);
                let added = self
                    .command_templates_mut()
                    .add(crate::cmdtemplate::Template {
                        name,
                        raw: trimmed.to_string(),
                        exts: Vec::new(),
                    });
                let saved_ok = if added {
                    crate::cmdtemplate::save(self.command_templates_mut())
                } else {
                    true
                };
                let (text, kind) = if !added {
                    ("Template already saved", crate::toasts::ToastKind::Info)
                } else if saved_ok {
                    ("Template saved", crate::toasts::ToastKind::Info)
                } else {
                    (
                        "Could not save template to disk",
                        crate::toasts::ToastKind::Error,
                    )
                };
                self.toasts
                    .push(crate::toasts::Toast::new(text, kind, false, now));
            }
            return; // keep the bar open after saving
        }
        if let Some(cmdline) = run {
            self.run_command = None;
            if cmdline.trim().is_empty() {
                return;
            }
            let now = ctx.input(|i| i.time);
            if self.ws.mutations_blocked() {
                self.toasts.push(crate::toasts::Toast::new(
                    "Safe-state review required",
                    crate::toasts::ToastKind::Error,
                    false,
                    now,
                ));
                return;
            }
            match std::process::Command::new("sh")
                .arg("-c")
                .arg(&cmdline)
                .current_dir(&dir)
                .spawn()
            {
                Ok(_) => {
                    self.toasts.push(crate::toasts::Toast::new(
                        "Command started",
                        crate::toasts::ToastKind::Info,
                        false,
                        now,
                    ));
                }
                Err(e) => {
                    self.toasts.push(crate::toasts::Toast::new(
                        format!("Command failed to start: {e}"),
                        crate::toasts::ToastKind::Error,
                        false,
                        now,
                    ));
                }
            }
        }
    }
}
