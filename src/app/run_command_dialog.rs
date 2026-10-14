//! Run-command / open-with bar: type or pick a saved template, watch the
//! placeholders expand live against the selection, and run it with Enter. The
//! command runs through `sh -c` in the active directory; only the explicit,
//! user-entered line is executed (never anything derived from file contents),
//! and all path substitutions are shell-quoted by `cmdtemplate::expand`.

use super::*;
use crate::cmdtemplate::{SegmentKind, expand, preview_segments};

const TEMPLATES_SCROLL_ID: &str = "run_command_templates";

/// Run `cmdline` through `sh -c` in `cwd`, publishing the child in `slot`
/// while it runs so the UI can kill it. Output is collected like
/// `Command::output` (stdin closed, stdout/stderr captured).
fn run_shell(cmdline: String, cwd: &std::path::Path, slot: &CommandChildSlot) -> CommandOutput {
    use std::io::Read;
    use std::process::Stdio;
    let failed = |cmdline: String, error: std::io::Error| CommandOutput {
        cmdline,
        stdout: String::new(),
        stderr: String::new(),
        exit_code: None,
        error: Some(error.to_string()),
    };
    let mut command = std::process::Command::new("sh");
    command
        .arg("-c")
        .arg(&cmdline)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Own process group, so Stop can kill everything the shell forked
    // (pipelines, `a; b`, make jobs), not just `sh` itself.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return failed(cmdline, error),
    };
    fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    *crate::lock_util::recover(slot) = Some(child);
    // Poll instead of `wait()` so the lock is free for a concurrent kill.
    let status = loop {
        let polled = crate::lock_util::recover(slot)
            .as_mut()
            .map(std::process::Child::try_wait);
        match polled {
            Some(Ok(None)) => std::thread::sleep(std::time::Duration::from_millis(25)),
            Some(Ok(Some(status))) => break Ok(status),
            Some(Err(error)) => break Err(error),
            None => break Err(std::io::Error::other("command handle lost")),
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    // Every pipe holder is gone; retire the handle so a late Stop never
    // signals a process group id that may since have been reused.
    crate::lock_util::recover(slot).take();
    match status {
        Ok(status) => CommandOutput {
            cmdline,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            exit_code: status.code(),
            error: None,
        },
        Err(error) => failed(cmdline, error),
    }
}

/// Toast text/kind for a finished command. No exit code without an error
/// means the process was killed by a signal (or Stop), which is a failure.
fn command_outcome(output: &CommandOutput) -> (String, crate::toasts::ToastKind) {
    use crate::toasts::ToastKind;
    match (&output.error, output.exit_code) {
        (Some(error), _) => (error.clone(), ToastKind::Error),
        (None, Some(0)) => ("Command finished".to_string(), ToastKind::Info),
        (None, Some(code)) => (format!("Command exited {code}"), ToastKind::Error),
        (None, None) => ("Command terminated by signal".to_string(), ToastKind::Error),
    }
}

impl App {
    pub(crate) fn open_run_command(&mut self, ctx: &egui::Context) {
        let dir = self.ws.active_panel_ref().current_path.clone();
        if !crate::trust::allows_run_command(&dir) {
            self.toast(
                ctx,
                crate::toasts::ToastKind::Error,
                format!(
                    "Run command blocked by {} trust",
                    crate::trust::label_for(&dir).label()
                ),
                false,
            );
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
        let mut stop = false;
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
                super::dialog_frame(t, 14),
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
                                    super::dialog_button(t, super::ButtonRole::Secondary, egui::RichText::new(format!("{name} {raw}")) .size(12.0)),
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
                            super::dialog_button(t, super::ButtonRole::Primary, egui::RichText::new("Run").size(13.0)),
                        )
                        .clicked()
                    {
                        run = Some(expand(&state.line, &sctx));
                    }
                    ui.add_space(8.0);
                    if ui
                        .add_enabled(
                            can_run,
                            super::dialog_button(t, super::ButtonRole::Secondary, egui::RichText::new("Save as template") .size(13.0)),
                        )
                        .on_hover_text("Save this command line as a reusable template")
                        .clicked()
                    {
                        save_template = Some(state.line.clone());
                    }
                    ui.add_space(8.0);
                    if ui
                        .add(
                            super::dialog_button(t, super::ButtonRole::Secondary, egui::RichText::new("Cancel").size(13.0)),
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
                        if ui
                            .add(super::dialog_button(
                                t,
                                super::ButtonRole::Secondary,
                                egui::RichText::new("Stop").size(12.0),
                            ))
                            .on_hover_text("Kill the running command")
                            .clicked()
                        {
                            stop = true;
                        }
                    });
                }
                if let Some(output) = &state.output {
                    ui.add_space(10.0);
                    let exit = output
                        .exit_code
                        .map(|code| format!("exit {code}"))
                        .unwrap_or_else(|| "terminated by signal".to_string());
                    let header = if let Some(error) = &output.error {
                        format!("Failed · {error}")
                    } else if output.cmdline.trim().is_empty() {
                        format!("Finished · {exit}")
                    } else {
                        format!("Finished · {exit} · {}", output.cmdline.trim())
                    };
                    let header_color = if output.error.is_some() || output.exit_code != Some(0) {
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
            // Dropping the run kills a command that is still going.
            self.ui.modals.run_command = None;
            return;
        }
        if stop
            && let Some(run) = self
                .ui
                .modals
                .run_command
                .as_ref()
                .and_then(|state| state.run.as_ref())
        {
            run.stop();
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
                self.toast(ctx, kind, text, false);
            }
            return; // keep the bar open after saving
        }
        if let Some(cmdline) = run {
            if cmdline.trim().is_empty() {
                return;
            }
            if let Some(reason) = self.ws.mutation_block_reason("running a command") {
                self.toast(ctx, crate::toasts::ToastKind::Error, reason, false);
                return;
            }
            if !crate::trust::allows_run_command(opening.dir()) {
                self.toast(
                    ctx,
                    crate::toasts::ToastKind::Error,
                    "Run command is blocked for this folder's trust label",
                    false,
                );
                return;
            }
            let cwd = opening.dir().to_path_buf();
            let (sender, receiver) = std::sync::mpsc::channel();
            let repaint = ctx.clone();
            let child = CommandChildSlot::default();
            let worker_child = child.clone();
            std::thread::spawn(move || {
                let output = run_shell(cmdline, &cwd, &worker_child);
                let _ = sender.send(output);
                repaint.request_repaint();
            });
            if let Some(state) = self.ui.modals.run_command.as_mut() {
                state.run = Some(CommandOutputRun { receiver, child });
                state.output = None;
            }
            self.toast(
                ctx,
                crate::toasts::ToastKind::Info,
                "Command started",
                false,
            );
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
            let (text, kind) = command_outcome(&output);
            state.output = Some(output);
            self.toast_at(now, kind, text, false);
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

    fn output(exit_code: Option<i32>, error: Option<&str>) -> CommandOutput {
        CommandOutput {
            cmdline: "true".to_string(),
            stdout: String::new(),
            stderr: String::new(),
            exit_code,
            error: error.map(str::to_string),
        }
    }

    #[test]
    fn signal_terminated_command_is_reported_as_failure() {
        use crate::toasts::ToastKind;
        assert!(matches!(
            command_outcome(&output(Some(0), None)),
            (_, ToastKind::Info)
        ));
        assert!(matches!(
            command_outcome(&output(Some(2), None)),
            (_, ToastKind::Error)
        ));
        let (text, kind) = command_outcome(&output(None, None));
        assert!(matches!(kind, ToastKind::Error));
        assert!(text.contains("signal"));
    }

    #[cfg(unix)]
    #[test]
    fn running_command_can_be_stopped() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let child = CommandChildSlot::default();
        let worker_child = child.clone();
        std::thread::spawn(move || {
            let cwd = std::env::temp_dir();
            // Not `exec`: the shell forks `sleep` as a grandchild that holds
            // the output pipes, so killing only `sh` would hang here.
            let _ = sender.send(run_shell("sleep 30; true".to_string(), &cwd, &worker_child));
        });
        let run = CommandOutputRun { receiver, child };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while crate::lock_util::recover(&run.child).is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "command never started"
            );
            std::thread::yield_now();
        }
        let stopped_at = std::time::Instant::now();
        run.stop();
        let output = run
            .receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("stopped command reports back");
        assert!(
            stopped_at.elapsed() < std::time::Duration::from_secs(2),
            "stop must end the whole command promptly"
        );
        assert_eq!(output.exit_code, None);
        assert_eq!(output.error, None);
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
