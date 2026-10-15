//! Run-command / open-with bar: type or pick a saved template, watch the
//! placeholders expand live against the selection, and run it with Enter. The
//! command runs through `sh -c` in the active directory; only the explicit,
//! user-entered line is executed (never anything derived from file contents),
//! and all path substitutions are shell-quoted by `cmdtemplate::expand`.

use super::*;
use crate::cmdtemplate::{SegmentKind, expand, preview_segments};

const TEMPLATES_SCROLL_ID: &str = "run_command_templates";

/// Bytes of each output stream kept for display. `find /` and the like emit
/// many megabytes; the rest is drained (so the child never blocks on a full
/// pipe) but dropped, and the shown text says how much was cut.
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

/// Read `reader` to the end, keeping at most `cap` bytes. Returns the kept
/// bytes and how many were discarded.
fn read_capped(mut reader: impl std::io::Read, cap: usize) -> (Vec<u8>, u64) {
    let mut kept = Vec::new();
    let mut dropped = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let room = cap.saturating_sub(kept.len()).min(n);
                kept.extend_from_slice(&buf[..room]);
                dropped = dropped.saturating_add((n - room) as u64);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    (kept, dropped)
}

/// Display text for one captured stream, noting any truncation.
fn captured_text(bytes: &[u8], dropped: u64) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    if dropped > 0 {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!(
            "\u{2026} output truncated ({} not shown)",
            format_size(dropped)
        ));
    }
    text
}

/// Run `cmdline` through `sh -c` in `cwd`, publishing the child in `slot`
/// while it runs so the UI can kill it. Output is collected like
/// `Command::output` (stdin closed, stdout/stderr captured).
fn run_shell(cmdline: String, cwd: &std::path::Path, slot: &CommandChildSlot) -> CommandOutput {
    use std::io::Read;
    use std::process::Stdio;
    let failed = |cmdline: String, error: String| CommandOutput {
        cmdline,
        stdout: String::new(),
        stderr: String::new(),
        exit_code: None,
        error: Some(error),
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
        Err(error) => return failed(cmdline, spawn_error_message(cwd, &error)),
    };
    fn drain(pipe: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<(Vec<u8>, u64)> {
        std::thread::spawn(move || {
            pipe.map(|pipe| read_capped(pipe, MAX_OUTPUT_BYTES))
                .unwrap_or_default()
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
            stdout: captured_text(&stdout.0, stdout.1),
            stderr: captured_text(&stderr.0, stderr.1),
            exit_code: status.code(),
            error: None,
        },
        Err(error) => failed(cmdline, error.to_string()),
    }
}

/// A missing command surfaces as exit 127 from `sh`, so a spawn failure is
/// usually the folder or the shell itself: name the folder, and suggest
/// checking it only when it is really gone.
fn spawn_error_message(cwd: &std::path::Path, error: &std::io::Error) -> String {
    let mut message = format!("Could not run in {}: {error}", cwd.display());
    // NotFound may also mean the shell itself is missing; only blame the
    // folder when it really is gone.
    if error.kind() == std::io::ErrorKind::NotFound && !cwd.is_dir() {
        message.push_str(". Check that the folder still exists");
    }
    message
}

/// Why Run is refused for a folder, naming its trust label and the remedy.
fn trust_block_message(label: crate::trust::TrustLabel) -> String {
    format!(
        "Run command is blocked: this folder is {}. Use \"Trust: mark Trusted\" to allow it",
        label.label()
    )
}

/// A change to the saved-template list from a row's context menu. Indices
/// are positions in the full store, not in the filtered list shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateEdit {
    Delete(usize),
    Move { from: usize, to: usize },
}

/// Apply `edit` to `templates`; whether anything changed.
fn apply_template_edit(templates: &mut crate::cmdtemplate::Templates, edit: TemplateEdit) -> bool {
    match edit {
        TemplateEdit::Delete(index) => templates.remove(index),
        TemplateEdit::Move { from, to } => from != to && templates.reorder(from, to),
    }
}

/// Store indices of the shown rows before and after row `pos`, the targets
/// of Move up / Move down (hidden, non-matching templates are skipped).
fn template_neighbours(shown: &[usize], pos: usize) -> (Option<usize>, Option<usize>) {
    let up = pos.checked_sub(1).and_then(|i| shown.get(i)).copied();
    let down = shown.get(pos + 1).copied();
    (up, down)
}

/// Text for a saved-template row: the raw line, prefixed by the template's
/// name only when the name says something the line does not (auto-named
/// templates are called after their first word).
fn template_row_label(name: &str, raw: &str) -> String {
    if raw.split_whitespace().next() == Some(name) {
        raw.to_string()
    } else {
        format!("{name} · {raw}")
    }
}

/// The expanded command as one label (placeholders tinted), so assistive
/// tech reads the command that will run rather than disjoint fragments.
fn preview_job(
    segs: &[crate::cmdtemplate::Segment],
    literal: egui::Color32,
    substituted: egui::Color32,
    empty: egui::Color32,
) -> egui::text::LayoutJob {
    let font = egui::FontId::monospace(11.0);
    let mut job = egui::text::LayoutJob::default();
    if segs.is_empty() {
        job.append("(empty)", 0.0, egui::TextFormat::simple(font, empty));
        return job;
    }
    for seg in segs {
        let color = match seg.kind {
            SegmentKind::Literal => literal,
            SegmentKind::Substituted => substituted,
        };
        job.append(
            &seg.text,
            0.0,
            egui::TextFormat::simple(font.clone(), color),
        );
    }
    job
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
                trust_block_message(crate::trust::label_for(&dir)),
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
        // Templates whose extension filter accepts the selection, with their
        // position in the store for the row menu.
        let matching: Vec<(usize, String, String)> = self
            .command_templates_mut()
            .items
            .iter()
            .enumerate()
            .filter(|(_, t)| t.matches(&opening.selection))
            .map(|(index, t)| (index, t.name.clone(), t.raw.clone()))
            .collect();
        let shown: Vec<usize> = matching.iter().map(|(index, _, _)| *index).collect();
        let mut template_edit: Option<TemplateEdit> = None;

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
                let title = ui.label(
                    egui::RichText::new(format!(
                        "Run command on {sel_count} {item}  ({{paths}} {{names}} {{dir}} {{dir_other}})"
                    ))
                    .size(11.0)
                    .color(t.text_muted),
                );
                if let Some(warning) = opening.stale_cursor_warning() {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(warning)
                            .size(11.0)
                            .color(t.accent_warning),
                    );
                }
                ui.add_space(6.0);

                let resp = ui.add(
                    egui::TextEdit::singleline(&mut state.line)
                        .desired_width(f32::INFINITY)
                        .hint_text("open -a Preview {paths}")
                        .margin(egui::vec2(8.0, 6.0)),
                ).labelled_by(title.id);
                // Focus on the opening frame only (never yank it back).
                if just_opened {
                    resp.request_focus();
                }

                // Live expansion preview, placeholders tinted.
                ui.add_space(6.0);
                let segs = preview_segments(&state.line, &sctx);
                ui.add(
                    egui::Label::new(preview_job(&segs, t.text_secondary, t.accent, t.text_muted))
                        .wrap(),
                );

                // Saved templates that apply to this selection.
                if !matching.is_empty() {
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Templates").size(10.0).color(t.text_muted));
                    egui::ScrollArea::vertical()
                        .id_salt((TEMPLATES_SCROLL_ID, state.scroll_nonce))
                        .max_height(160.0)
                        .show(ui, |ui| {
                            for (pos, (index, name, raw)) in matching.iter().enumerate() {
                                let row = ui
                                    .add(
                                        super::dialog_button(t, super::ButtonRole::Secondary, egui::RichText::new(template_row_label(name, raw)) .size(12.0)),
                                    )
                                    .on_hover_text("Click to use; right-click to move or delete");
                                if row.clicked() {
                                    fill = Some(raw.clone());
                                }
                                let (up, down) = template_neighbours(&shown, pos);
                                row.context_menu(|ui| {
                                    if ui.add_enabled(up.is_some(), egui::Button::new("Move up")).clicked() {
                                        template_edit = up.map(|to| TemplateEdit::Move { from: *index, to });
                                        ui.close();
                                    }
                                    if ui.add_enabled(down.is_some(), egui::Button::new("Move down")).clicked() {
                                        template_edit = down.map(|to| TemplateEdit::Move { from: *index, to });
                                        ui.close();
                                    }
                                    ui.separator();
                                    if ui.button("Delete template").clicked() {
                                        template_edit = Some(TemplateEdit::Delete(*index));
                                        ui.close();
                                    }
                                });
                            }
                        });
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let has_line = !state.line.trim().is_empty();
                    let can_run = has_line && state.run.is_none();
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
                            has_line,
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
                    let header_label = ui.label(
                        egui::RichText::new(header)
                            .size(11.0)
                            .color(header_color),
                    );
                    // Announce completion; a failure interrupts.
                    let failed = output.error.is_some();
                    ui.ctx().accesskit_node_builder(header_label.id, |node| {
                        if failed {
                            node.set_role(egui::accesskit::Role::Alert);
                            node.set_live(egui::accesskit::Live::Assertive);
                        } else {
                            node.set_role(egui::accesskit::Role::Status);
                            node.set_live(egui::accesskit::Live::Polite);
                        }
                    });
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
                                    egui::RichText::new("stderr")
                                        .size(10.0)
                                        .color(t.text_muted),
                                );
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
        if let Some(edit) = template_edit {
            if apply_template_edit(self.command_templates_mut(), edit)
                && !crate::cmdtemplate::save(self.command_templates_mut())
            {
                self.toast(
                    ctx,
                    crate::toasts::ToastKind::Error,
                    "Could not save templates to disk".to_string(),
                    false,
                );
            }
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
                        name: name.clone(),
                        raw: trimmed.to_string(),
                        exts: Vec::new(),
                    });
                let saved_ok = if added {
                    crate::cmdtemplate::save(self.command_templates_mut())
                } else {
                    true
                };
                let (text, kind) = if !added {
                    (
                        "Template already saved".to_string(),
                        crate::toasts::ToastKind::Info,
                    )
                } else if saved_ok {
                    (
                        format!("Saved template \"{name}\""),
                        crate::toasts::ToastKind::Info,
                    )
                } else {
                    (
                        "Could not save template to disk".to_string(),
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
                    trust_block_message(crate::trust::label_for(opening.dir())),
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
            permissions: None,
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
    fn captured_output_is_capped_and_marked_truncated() {
        let input = vec![b'x'; 10];
        let (kept, dropped) = read_capped(input.as_slice(), 4);
        assert_eq!(kept, b"xxxx");
        assert_eq!(dropped, 6);
        let text = captured_text(&kept, dropped);
        assert!(text.starts_with("xxxx\n"));
        assert!(text.contains("output truncated"));

        let (kept, dropped) = read_capped(&b"short"[..], 4096);
        assert_eq!((kept.as_slice(), dropped), (&b"short"[..], 0));
        assert_eq!(captured_text(&kept, dropped), "short");
        let (kept, dropped) = read_capped(&b""[..], 0);
        assert!(kept.is_empty() && dropped == 0);
    }

    #[cfg(unix)]
    #[test]
    fn huge_command_output_is_bounded() {
        let slot: CommandChildSlot = std::sync::Arc::default();
        let output = run_shell(
            format!("head -c {} /dev/zero", MAX_OUTPUT_BYTES * 3),
            std::path::Path::new("/"),
            &slot,
        );
        assert_eq!(output.exit_code, Some(0));
        assert!(output.stdout.len() < MAX_OUTPUT_BYTES + 256);
        assert!(output.stdout.contains("output truncated"));
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
    fn spawn_failure_names_the_folder() {
        let error = std::io::Error::from(std::io::ErrorKind::NotFound);
        let message = spawn_error_message(std::path::Path::new("/gone/dir"), &error);
        assert!(message.starts_with("Could not run in /gone/dir: "));
        assert!(message.contains("folder still exists"));
    }

    #[test]
    fn missing_shell_does_not_blame_an_existing_folder() {
        let error = std::io::Error::from(std::io::ErrorKind::NotFound);
        let dir = std::env::temp_dir();
        let message = spawn_error_message(&dir, &error);
        assert!(message.starts_with("Could not run in "));
        assert!(!message.contains("folder still exists"), "{message}");
    }

    #[cfg(unix)]
    #[test]
    fn missing_working_folder_is_reported_with_its_path() {
        let gone = std::env::temp_dir().join(format!(
            "commander-run-missing-{}-{}",
            std::process::id(),
            line!()
        ));
        let slot = CommandChildSlot::default();
        let output = run_shell("true".to_string(), &gone, &slot);
        let error = output.error.expect("spawn must fail without a cwd");
        assert!(error.contains(&gone.display().to_string()), "{error}");
    }

    #[test]
    fn trust_block_names_label_and_remedy() {
        let message = trust_block_message(crate::trust::TrustLabel::Untrusted);
        assert!(message.contains("Untrusted"));
        assert!(message.contains("Trust: mark Trusted"));
    }

    #[test]
    fn template_menu_moves_between_shown_rows_and_deletes() {
        let template = |raw: &str| crate::cmdtemplate::Template {
            name: raw.to_string(),
            raw: raw.to_string(),
            exts: Vec::new(),
        };
        let mut store = crate::cmdtemplate::Templates {
            items: vec![template("a"), template("hidden"), template("c")],
        };
        // Rows 0 and 2 are shown; "hidden" does not match the selection.
        let shown = [0, 2];
        assert_eq!(template_neighbours(&shown, 0), (None, Some(2)));
        assert_eq!(template_neighbours(&shown, 1), (Some(0), None));
        assert!(apply_template_edit(
            &mut store,
            TemplateEdit::Move { from: 2, to: 0 }
        ));
        let raws: Vec<&str> = store.items.iter().map(|t| t.raw.as_str()).collect();
        assert_eq!(raws, ["c", "a", "hidden"]);
        assert!(apply_template_edit(&mut store, TemplateEdit::Delete(1)));
        let raws: Vec<&str> = store.items.iter().map(|t| t.raw.as_str()).collect();
        assert_eq!(raws, ["c", "hidden"]);
        assert!(!apply_template_edit(&mut store, TemplateEdit::Delete(5)));
        assert!(!apply_template_edit(
            &mut store,
            TemplateEdit::Move { from: 0, to: 0 }
        ));
    }

    #[test]
    fn template_row_shows_name_only_when_it_adds_information() {
        assert_eq!(
            template_row_label("open", "open -a Preview {paths}"),
            "open -a Preview {paths}"
        );
        assert_eq!(
            template_row_label("Preview", "open -a Preview {paths}"),
            "Preview · open -a Preview {paths}"
        );
    }

    #[test]
    fn preview_is_one_job_with_tinted_placeholders() {
        let selection = [entry("/opening/a.txt")];
        let context = RunCommandOpeningContext {
            active_panel: ActivePanel::Left,
            selection: selection.to_vec(),
            left_dir: PathBuf::from("/opening"),
            right_dir: PathBuf::from("/other"),
            stale_cursor: false,
        }
        .selection_context();
        let segs = preview_segments("cat {paths}", &context);
        let (lit, sub, empty) = (
            egui::Color32::WHITE,
            egui::Color32::RED,
            egui::Color32::GRAY,
        );
        let job = preview_job(&segs, lit, sub, empty);
        assert_eq!(job.text, "cat '/opening/a.txt'");
        assert!(job.sections.iter().any(|s| s.format.color == sub));
        assert_eq!(preview_job(&[], lit, sub, empty).text, "(empty)");
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
    fn stale_cursor_is_warned_about_instead_of_silently_empty() {
        let mut opening = RunCommandOpeningContext {
            active_panel: ActivePanel::Left,
            selection: Vec::new(),
            left_dir: PathBuf::from("/opening"),
            right_dir: PathBuf::from("/other"),
            stale_cursor: false,
        };
        assert_eq!(opening.stale_cursor_warning(), None);
        opening.stale_cursor = true;
        assert!(opening.stale_cursor_warning().is_some());
    }

    #[test]
    fn run_command_remains_bound_to_its_opening_context() {
        let opening = RunCommandOpeningContext {
            active_panel: ActivePanel::Left,
            selection: vec![entry("/opening/selected.txt")],
            left_dir: PathBuf::from("/opening"),
            right_dir: PathBuf::from("/other"),
            stale_cursor: false,
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
