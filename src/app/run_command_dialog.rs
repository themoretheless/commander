//! Run-command / open-with bar: type or pick a saved template, watch the
//! placeholders expand live against the selection, and run it with Enter. The
//! command runs through `sh -c` in the active directory; only the explicit,
//! user-entered line is executed (never anything derived from file contents),
//! and all path substitutions are shell-quoted by `cmdtemplate::expand`.

use super::*;
use crate::cmdtemplate::{SegmentKind, expand, preview_segments};

const TEMPLATES_SCROLL_ID: &str = "run_command_templates";

impl App {
    pub(crate) fn open_run_command(&mut self, ctx: &egui::Context) {
        let dir = self.ws.active_panel_ref().current_path.clone();
        if !crate::trust::allows_run_command(&dir) {
            let now = ctx.input(|input| input.time);
            self.toasts.push(crate::toasts::Toast::new(
                format!(
                    "Run command blocked by {} trust",
                    crate::trust::label_for(&dir).label()
                ),
                crate::toasts::ToastKind::Error,
                false,
                now,
            ));
            return;
        }
        self.command_templates_mut();
        let scroll_nonce = self.issue_transient_nonce();
        self.ui.modals.run_command = Some(RunCommandState {
            line: String::new(),
            scroll_nonce,
            opening: RunCommandOpeningContext::capture(&self.ws),
            run: None,
            output: None,
        });
        Self::mark_modal_opened(ctx, UiModal::RunCommand);
    }

    pub(crate) fn show_run_command_dialog(&mut self, ctx: &egui::Context) {
        let just_opened = Self::take_modal_opened(ctx, UiModal::RunCommand);
        let escape_requested = self.take_escape_request(crate::accessibility::EscapeRoute::Modal(
            crate::accessibility::ModalSurface::RunCommand,
        ));
        if self.ui.modals.run_command.is_none() {
            return;
        }
        let now = ctx.input(|input| input.time);
        self.poll_run_command_output(now);
        let t = self.colors;

        let opening = self
            .ui
            .modals
            .run_command
            .as_ref()
            .expect("checked above")
            .opening
            .clone();
        let sctx = opening.selection_context();
        let sel_count = opening.selection.len();
        // Templates whose extension filter accepts the selection.
        let matching: Vec<(String, String)> = self
            .command_templates_mut()
            .matching(&opening.selection)
            .iter()
            .map(|t| (t.name.clone(), t.raw.clone()))
            .collect();

        let mut run: Option<String> = None; // expanded command line to execute
        let mut cancel = false;
        let mut fill: Option<String> = None; // template raw chosen from the list
        let mut save_template: Option<String> = None; // raw line to persist

        let state = self.ui.modals.run_command.as_mut().unwrap();
        egui::Window::new("Run command")
            .collapsible(false)
            .resizable(true)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_size([560.0, 420.0])
            .min_width(480.0)
            .min_height(280.0)
            .frame(
                Frame::NONE
                    .fill(t.bg_panel)
                    .inner_margin(Margin::same(14))
                    .stroke(Stroke::new(1.0_f32, t.border)),
            )
            .show(ctx, |ui| {
                ui.set_min_width(520.0);
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
                    egui::ScrollArea::vertical()
                        .id_salt((TEMPLATES_SCROLL_ID, state.scroll_nonce))
                        .max_height(160.0)
                        .show(ui, |ui| {
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
                    let can_run = !state.line.trim().is_empty() && state.run.is_none();
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
                    if escape_requested {
                        cancel = true;
                    }
                });

                if state.run.is_some() {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("Running…")
                                .size(11.0)
                                .color(t.text_muted),
                        );
                    });
                }
                if let Some(output) = &state.output {
                    ui.add_space(10.0);
                    let exit = output
                        .exit_code
                        .map(|code| format!("exit {code}"))
                        .unwrap_or_else(|| "no exit code".to_string());
                    let header = if let Some(error) = &output.error {
                        format!("Failed · {error}")
                    } else {
                        format!("Finished · {exit}")
                    };
                    let header_color = if output.error.is_some()
                        || output.exit_code.is_some_and(|code| code != 0)
                    {
                        t.accent_red
                    } else {
                        t.text_secondary
                    };
                    ui.label(
                        egui::RichText::new(header)
                            .size(11.0)
                            .color(header_color),
                    );
                    egui::ScrollArea::vertical()
                        .id_salt(("run_command_output", state.scroll_nonce))
                        .max_height(220.0)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            if !output.stdout.trim().is_empty() {
                                ui.label(
                                    egui::RichText::new(&output.stdout)
                                        .size(11.0)
                                        .monospace()
                                        .color(t.text_primary),
                                );
                            }
                            if !output.stderr.trim().is_empty() {
                                ui.label(
                                    egui::RichText::new(&output.stderr)
                                        .size(11.0)
                                        .monospace()
                                        .color(t.accent_warning),
                                );
                            }
                            if output.stdout.trim().is_empty()
                                && output.stderr.trim().is_empty()
                                && output.error.is_none()
                            {
                                ui.label(
                                    egui::RichText::new("(no output)")
                                        .size(11.0)
                                        .monospace()
                                        .color(t.text_muted),
                                );
                            }
                        });
                }
            });

        if cancel {
            self.ui.modals.run_command = None;
            return;
        }
        if let Some(raw) = fill
            && let Some(s) = &mut self.ui.modals.run_command
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
            if cmdline.trim().is_empty() {
                return;
            }
            let now = ctx.input(|i| i.time);
            if let Some(reason) = self.ws.mutation_block_reason("running a command") {
                self.toasts.push(crate::toasts::Toast::new(
                    reason,
                    crate::toasts::ToastKind::Error,
                    false,
                    now,
                ));
                return;
            }
            if !crate::trust::allows_run_command(opening.dir()) {
                self.toasts.push(crate::toasts::Toast::new(
                    "Run command is blocked for this folder's trust label",
                    crate::toasts::ToastKind::Error,
                    false,
                    now,
                ));
                return;
            }
            let cwd = opening.dir().to_path_buf();
            let (sender, receiver) = std::sync::mpsc::channel();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let output = match std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&cmdline)
                    .current_dir(&cwd)
                    .output()
                {
                    Ok(output) => CommandOutput {
                        cmdline,
                        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                        exit_code: output.status.code(),
                        error: None,
                    },
                    Err(error) => CommandOutput {
                        cmdline,
                        stdout: String::new(),
                        stderr: String::new(),
                        exit_code: None,
                        error: Some(error.to_string()),
                    },
                };
                let _ = sender.send(output);
                repaint.request_repaint();
            });
            if let Some(state) = self.ui.modals.run_command.as_mut() {
                state.run = Some(CommandOutputRun { receiver });
                state.output = None;
            }
            self.toasts.push(crate::toasts::Toast::new(
                "Command started",
                crate::toasts::ToastKind::Info,
                false,
                now,
            ));
        }
    }

    fn poll_run_command_output(&mut self, now: f64) {
        let ready = self.ui.modals.run_command.as_ref().and_then(|state| {
            let run = state.run.as_ref()?;
            match run.receiver.try_recv() {
                Ok(output) => Some(output),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(CommandOutput {
                    cmdline: String::new(),
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    error: Some("Command worker stopped before completion".to_string()),
                }),
            }
        });
        if let Some(output) = ready
            && let Some(state) = self.ui.modals.run_command.as_mut()
        {
            state.run = None;
            let (text, kind) = if let Some(error) = &output.error {
                (error.clone(), crate::toasts::ToastKind::Error)
            } else if output.exit_code.is_some_and(|code| code != 0) {
                (
                    format!("Command exited {}", output.exit_code.unwrap_or_default()),
                    crate::toasts::ToastKind::Error,
                )
            } else {
                (
                    "Command finished".to_string(),
                    crate::toasts::ToastKind::Info,
                )
            };
            state.output = Some(output);
            self.toasts
                .push(crate::toasts::Toast::new(text, kind, false, now));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> crate::panel::FileEntry {
        let path = PathBuf::from(path);
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        crate::panel::FileEntry {
            name_lower: name.to_lowercase(),
            name,
            path,
            identity: crate::panel::ListingIdentity::Unavailable,
            is_dir: false,
            size: 1,
            extension: "txt".to_string(),
            modified: None,
            modified_str: String::new(),
            size_str: "1 B".to_string(),
        }
    }

    #[test]
    fn run_command_remains_bound_to_its_opening_context() {
        let opening = RunCommandOpeningContext {
            active_panel: ActivePanel::Left,
            selection: vec![entry("/opening/selected.txt")],
            left_dir: PathBuf::from("/opening"),
            right_dir: PathBuf::from("/other"),
        };

        let external_active = ActivePanel::Right;
        let external_selection = [entry("/changed/new.txt")];
        let external_dir = PathBuf::from("/changed");
        let context = opening.selection_context();

        assert_eq!(opening.active_panel, ActivePanel::Left);
        assert_ne!(opening.active_panel, external_active);
        assert_ne!(opening.selection[0].path, external_selection[0].path);
        assert_ne!(opening.dir(), external_dir);
        assert_eq!(
            expand("{paths} {dir} {dir_other}", &context),
            "'/opening/selected.txt' '/opening' '/other'"
        );
    }
}
