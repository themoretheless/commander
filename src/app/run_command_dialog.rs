//! Run-command / open-with bar: type or pick a saved template, watch the
//! placeholders expand live against the selection, and run it with Enter. The
//! command runs through `sh -c` in the active directory; only the explicit,
//! user-entered line is executed (never anything derived from file contents),
//! and all path substitutions are shell-quoted by `cmdtemplate::expand`.

use super::*;
use crate::cmdtemplate::{SegmentKind, preview_segments};

const TEMPLATES_SCROLL_ID: &str = "run_command_templates";

fn output_find_id(nonce: u64) -> egui::Id {
    egui::Id::new(("command-output-find", nonce))
}

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
#[cfg(test)]
fn run_shell(cmdline: String, cwd: &std::path::Path, slot: &CommandChildSlot) -> CommandOutput {
    run_shell_cancellable(cmdline, cwd, slot, &crate::command_batch::Progress::new(1))
}
fn run_shell_cancellable(
    cmdline: String,
    cwd: &std::path::Path,
    slot: &CommandChildSlot,
    progress: &crate::command_batch::Progress,
) -> CommandOutput {
    use std::io::Read;
    use std::process::Stdio;
    let failed = |cmdline: String, error: String| CommandOutput {
        cmdline,
        stdout: String::new(),
        stderr: String::new(),
        exit_code: None,
        error: Some(error),
        batch: None,
        context: None,
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
    let mut publication = crate::lock_util::recover(slot);
    if progress
        .cancelled
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return failed(cmdline, "Command cancelled before spawn".into());
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return failed(cmdline, spawn_error_message(cwd, &error)),
    };
    fn drain(
        pipe: Option<impl Read + Send + 'static>,
        done: std::sync::mpsc::Sender<()>,
    ) -> std::thread::JoinHandle<(Vec<u8>, u64)> {
        std::thread::spawn(move || {
            let result = pipe
                .map(|pipe| read_capped(pipe, MAX_OUTPUT_BYTES))
                .unwrap_or_default();
            let _ = done.send(());
            result
        })
    }
    let (done, _drain_events) = std::sync::mpsc::channel();
    let stdout = drain(child.stdout.take(), done.clone());
    let stderr = drain(child.stderr.take(), done.clone());
    drop(done);
    *publication = Some(child);
    drop(publication);
    // Keep the Unix leader waitable until both output pipes close. Reaping it
    // earlier releases PID ownership while Stop may still need its group.
    #[cfg(unix)]
    let status = {
        let mut drained = 0;
        let observed = loop {
            let guard = crate::lock_util::recover(slot);
            let Some(child) = guard.as_ref() else {
                break Err(std::io::Error::other("command handle lost"));
            };
            if progress
                .cancelled
                .load(std::sync::atomic::Ordering::Acquire)
            {
                super::kill_command_group(child);
            }
            match exited_without_reaping(child) {
                Ok(true) if drained == 2 => break Ok(()),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => break Err(error),
            }
            drop(guard);
            if drained == 2 {
                std::thread::sleep(std::time::Duration::from_millis(25));
            } else {
                match _drain_events.recv_timeout(std::time::Duration::from_millis(25)) {
                    Ok(()) => drained += 1,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => drained = 2,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        };
        // Reap and retire under the same lock as Stop, so no Stop can signal
        // a recycled group number between the final wait and slot removal.
        let mut guard = crate::lock_util::recover(slot);
        let status = observed.and_then(|()| {
            guard
                .as_mut()
                .ok_or_else(|| std::io::Error::other("command handle lost"))?
                .wait()
        });
        guard.take();
        status
    };
    #[cfg(not(unix))]
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
    #[cfg(not(unix))]
    crate::lock_util::recover(slot).take();
    match status {
        Ok(status) => CommandOutput {
            cmdline,
            stdout: captured_text(&stdout.0, stdout.1),
            stderr: captured_text(&stderr.0, stderr.1),
            exit_code: status.code(),
            error: None,
            batch: None,
            context: None,
        },
        Err(error) => failed(cmdline, error.to_string()),
    }
}

/// WNOWAIT observes termination without releasing the leader's PID. A zombie
/// leader pins that identifier while its descendants still hold output pipes.
#[cfg(unix)]
fn exited_without_reaping(child: &std::process::Child) -> std::io::Result<bool> {
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: a valid writable siginfo_t, this owned child id, and standard
    // waitid flags. No process is reaped by this call.
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            child.id() as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: storage was zero-initialized; successful waitid fills siginfo.
    // si_pid remains zero with WNOHANG when the child has not exited.
    Ok(unsafe { info.assume_init().si_pid() != 0 })
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
    if let Some(report) = &output.batch
        && report.cancelled
    {
        return (
            format!(
                "Per-item command cancelled after {} / {} invocations",
                report.completed, report.total
            ),
            ToastKind::Info,
        );
    }
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
            output_view: Default::default(),
            template_browser: Default::default(),
            exchange: Default::default(),
            template_name: String::new(),
            template_extensions: String::new(),
            template_require_all: false,
            template_working_dir: Default::default(),
            template_execution: Default::default(),
            template_kind: Default::default(),
            template_limits: Default::default(),
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
        let mut template_edit: Option<TemplateEdit> = None;

        let mut run: Option<(
            String,
            crate::cmdtemplate::ExecutionMode,
            crate::cmdtemplate::WorkingDirectory,
        )> = None; // expanded command line to execute
        let mut cancel = false;
        let mut stop = false;
        let mut fill: Option<String> = None; // template raw chosen from the list
        let mut save_template: Option<crate::cmdtemplate::Template> = None; // raw line to persist

        let exchange_snapshot = self.command_templates_mut().clone();
        let mut report_copy = None;
        let mut exchange_copy = None;
        let mut exchange_import = false;
        let state = self.ui.modals.run_command.as_mut().unwrap();
        let dialog_response = egui::Window::new("Run command")
            .collapsible(false)
            .resizable(true)
            .title_bar(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_size([560.0, 480.0])
            .min_width(480.0)
            .min_height(280.0)
            .max_height((ctx.input(|i| i.viewport_rect().height()) - 60.0).max(280.0))
            .vscroll(true)
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

                ui.menu_button("Command recipes", |ui| {
                    for recipe in crate::command_recipes::RECIPES {
                        if ui.button(recipe.name).clicked() {
                            fill=Some(recipe.command.to_string());
                            state.template_name=recipe.name.to_string();
                            state.template_extensions=recipe.extensions.join(", ");
                            ui.close();
                        }
                        ui.small(format!("Requires: {}",recipe.requires));
                    }
                });
                let template_metadata = ui.collapsing("Template name and file types", |ui| {
                    ui.label("Name (optional)");
                    ui.text_edit_singleline(&mut state.template_name);
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Apply to");
                        for kind in crate::cmdtemplate::InputKind::ALL {
                            ui.selectable_value(&mut state.template_kind, kind, kind.label());
                        }
                    });
                    ui.label("Working folder");
                    for directory in crate::cmdtemplate::WorkingDirectory::ALL { ui.selectable_value(&mut state.template_working_dir,directory,directory.label()); }
                    ui.label("Execution");
                    for mode in crate::cmdtemplate::ExecutionMode::ALL { ui.selectable_value(&mut state.template_execution,mode,mode.label()); }
                    ui.label("Extensions (comma separated; empty or * means all)");
                    ui.text_edit_singleline(&mut state.template_extensions);
                    ui.checkbox(&mut state.template_require_all, "Require every selected file to match").on_hover_text("With extension filters, every opening item must be a matching file. A mismatch also disables Run, including after choosing All templates.");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Minimum items");
                        ui.add(egui::DragValue::new(&mut state.template_limits.min).range(0..=u32::MAX));
                        let mut bounded = state.template_limits.max.is_some();
                        if ui.checkbox(&mut bounded, "Maximum items").changed() { state.template_limits.max = bounded.then_some(state.template_limits.min.max(1)); }
                        if let Some(max) = &mut state.template_limits.max { ui.add(egui::DragValue::new(max).range(0..=u32::MAX)); }
                    });
                    if !state.template_limits.valid() { ui.colored_label(t.accent_warning, "Minimum selection cannot exceed maximum"); }
                    ui.small("Use the Run button while these template settings are open.");

                });
                let template_draft=crate::command_recipes::template(&state.line,&state.template_name,&state.template_extensions).and_then(|mut template| { template.require_all = state.template_require_all; template.working_dir = state.template_working_dir; template.execution = state.template_execution; template.kind = state.template_kind; template.limits = state.template_limits; if !template.limits.valid() { return Err("Minimum selection cannot exceed maximum"); } Ok(template) });
                let count_allowed = state.template_limits.accepts(opening.selection.len());
                let kind_allowed = state.template_kind.accepts(&opening.selection);
                let mode_allowed = !state.template_execution.per_item() || !opening.selection.is_empty();
                let working_dir = state.template_working_dir.resolve(&sctx,&opening.selection);
                let selection_allowed = working_dir.is_ok() && count_allowed && kind_allowed && mode_allowed && (!state.template_require_all || template_draft.as_ref().is_ok_and(|template| template.allows_selection(&opening.selection)));
                if let Err(message)=working_dir { ui.colored_label(t.accent_warning,format!("Run blocked: {message}")); }
                else if !mode_allowed { ui.colored_label(t.accent_warning, "Per-item mode needs at least one opening item"); }
                else if !count_allowed { ui.colored_label(t.accent_warning, format!("Run blocked: {} selected; expected {}. Review template selection limits.", opening.selection.len(), state.template_limits.description())); }
                else if !kind_allowed { ui.colored_label(t.accent_warning, format!("Run blocked: this template accepts {} only; every opening item must match.", state.template_kind.label())); }
                else if !selection_allowed { ui.colored_label(t.accent_warning, "Run blocked: every opening item must be a matching file. Review the selection or template file types."); }
                if let Err(message)=&template_draft
                    && !state.template_extensions.trim().is_empty() { ui.small(*message); }
                if let Ok(path)=working_dir { ui.small(format!("Working folder: {}",path.display())); }
                // Live expansion preview, placeholders tinted.
                ui.add_space(6.0);
                let single_preview = state.template_execution.per_item().then(|| crate::cmdtemplate::SelectionCtx { paths:sctx.paths.first().cloned().into_iter().collect(),dir:sctx.dir.clone(),dir_other:sctx.dir_other.clone() });
                if state.template_execution.per_item() { ui.small(format!("{} · preview of the first of {} invocations",state.template_execution.label(),sctx.paths.len())); }
                let segs = preview_segments(&state.line, single_preview.as_ref().unwrap_or(&sctx));
                ui.add(
                    egui::Label::new(preview_job(&segs, t.text_secondary, t.accent, t.text_muted))
                        .wrap(),
                );
                let unencodable = crate::cmdtemplate::non_utf8_path(&state.line, &sctx);
                if let Some(path) = unencodable {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "Cannot run: {} is not valid UTF-8 and would expand to a different name",
                            path.display()
                        ))
                        .size(11.0)
                        .color(t.accent_warning),
                    );
                }
                let unencodable = unencodable.is_some();

                let (exchange_open, requested_import) = state.exchange.show(ui, &exchange_snapshot, &mut exchange_copy);
                exchange_import = requested_import;

                let mut template_browser_active = false;
                // Saved templates: queries keep original store indices for edits.
                if !exchange_snapshot.items.is_empty() {
                    template_browser_active = state.template_browser.show(ui, &exchange_snapshot, &opening.selection);
                    let shown = &state.template_browser.indices;
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Templates").size(10.0).color(t.text_muted));
                    egui::ScrollArea::vertical()
                        .id_salt((TEMPLATES_SCROLL_ID, state.scroll_nonce))
                        .max_height(160.0)
                        .show(ui, |ui| {
                            for (pos, index) in shown.iter().enumerate() {
                                let template = &exchange_snapshot.items[*index];
                                let (name, raw, extensions) = (&template.name, &template.raw, &template.exts);
                                let row = ui
                                    .add(
                                        super::dialog_button(t, super::ButtonRole::Secondary, egui::RichText::new(template_row_label(name, raw)) .size(12.0)),
                                    )
                                    .on_hover_text("Click to use; right-click to move or delete");
                                #[cfg(any(test, feature = "visual-qa"))]
                                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("run-template-row", *index)), row.rect));
                                if row.clicked() {
                                    fill = Some(raw.clone());
                                    state.template_name = name.clone();
                                    state.template_extensions = extensions.join(", ");
                                    state.template_require_all = template.require_all;
                                    state.template_working_dir = template.working_dir;
                                    state.template_execution = template.execution;
                                    state.template_kind = template.kind;
                                    state.template_limits = template.limits;
                                }
                                let (up, down) = template_neighbours(shown, pos);
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
                    let can_run = has_line && state.run.is_none() && !unencodable && selection_allowed;
                    let run_button = ui
                        .add_enabled(
                            can_run,
                            super::dialog_button(t, super::ButtonRole::Primary, egui::RichText::new("Run").size(13.0)),
                        )
                        ;
                    #[cfg(any(test, feature = "visual-qa"))]
                    ui.ctx().data_mut(|data| { data.insert_temp(egui::Id::new("run-command-enabled"), run_button.enabled()); data.insert_temp(egui::Id::new("run-command-button"), run_button.rect); });
                    if run_button.clicked() { run = Some((state.line.clone(),state.template_execution,state.template_working_dir)); }
                    ui.add_space(8.0);
                    if ui
                        .add_enabled(
                            template_draft.is_ok(),
                            super::dialog_button(t, super::ButtonRole::Secondary, egui::RichText::new("Save/update template") .size(13.0)),
                        )
                        .on_hover_text("Save this command line as a reusable template")
                        .clicked()
                    {
                        save_template = template_draft.ok();
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
                    if can_run && !state.output_view.owns_input(ui.ctx(),output_find_id(state.scroll_nonce)) && !exchange_open && template_metadata.body_response.is_none() && !template_browser_active && resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        run = Some((state.line.clone(),state.template_execution,state.template_working_dir));
                    }
                    if escape_requested {
                        cancel = true;
                    }
                });

                if let Some(running) = &state.run {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new(format!("Running… {} / {} completed · {} succeeded · {} failed",running.progress.completed.load(std::sync::atomic::Ordering::Acquire),running.progress.total,running.progress.succeeded.load(std::sync::atomic::Ordering::Acquire),running.progress.failed.load(std::sync::atomic::Ordering::Acquire)))
                                .size(11.0)
                                .color(t.text_muted),
                        );
                        let stop_button = ui
                            .add(super::dialog_button(
                                t,
                                super::ButtonRole::Secondary,
                                egui::RichText::new("Stop").size(12.0),
                            ))
                            .on_hover_text("Kill the running command");
                        #[cfg(any(test, feature = "visual-qa"))]
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(egui::Id::new("run-command-stop"), stop_button.rect);
                        });
                        if stop_button.clicked() {
                            stop = true;
                        }
                    });
                }
                if let Some(output) = &state.output {
                    ui.add_space(10.0);
                    ui.horizontal_wrapped(|ui| {
                        for format in crate::command_report::Format::ALL {
                            let enabled = match format { crate::command_report::Format::Stdout=>!output.stdout.is_empty(),crate::command_report::Format::Stderr=>!output.stderr.is_empty(),_=>true };
                            let response=ui.add_enabled(enabled,egui::Button::new(format.label()));
                            if response.clicked() {report_copy=Some(format);}
                            #[cfg(any(test,feature="visual-qa"))]
                            ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new(("run-copy",format.key())),response.rect));
                        }
                    });
                    let exit = output
                        .exit_code
                        .map(|code| format!("exit {code}"))
                        .unwrap_or_else(|| "terminated by signal".to_string());
                    let cancelled = output.batch.as_ref().is_some_and(|report|report.cancelled);
                    if let Some(report)=&output.batch { ui.small(format!("Per item: {} / {} completed · {} succeeded · {} failed · {} remaining",report.completed,report.total,report.succeeded,report.failed,report.total.saturating_sub(report.completed))); }
                    let header = if cancelled { "Per-item command cancelled".to_string() } else if let Some(error) = &output.error {
                        format!("Failed · {error}")
                    } else if output.cmdline.trim().is_empty() {
                        format!("Finished · {exit}")
                    } else {
                        format!("Finished · {exit} · {}", output.cmdline.trim())
                    };
                    let header_color = if !cancelled && (output.error.is_some() || output.exit_code != Some(0)) {
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
                    let failed = output.error.is_some() && !cancelled;
                    ui.ctx().accesskit_node_builder(header_label.id, |node| {
                        if failed {
                            node.set_role(egui::accesskit::Role::Alert);
                            node.set_live(egui::accesskit::Live::Assertive);
                        } else {
                            node.set_role(egui::accesskit::Role::Status);
                            node.set_live(egui::accesskit::Live::Polite);
                        }
                    });
                    state.output_view.show(ui, output_find_id(state.scroll_nonce), opening.dir(), output, t);

                }
            });

        #[cfg(not(feature = "visual-qa"))]
        let _ = dialog_response;
        #[cfg(feature = "visual-qa")]
        if let Some(response) = dialog_response {
            crate::visual_qa::record_response(
                ctx,
                crate::visual_qa::ProbeId::Confirmation,
                &response.response,
            );
        }

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
        if let Some(format) = report_copy {
            if let Some(output) = self
                .ui
                .modals
                .run_command
                .as_ref()
                .and_then(|state| state.output.as_ref())
            {
                match crate::command_report::export(output, format) {
                    Ok(text) => self.write_clipboard(&text, "command result", ctx),
                    Err(error) => self.toast(ctx, crate::toasts::ToastKind::Error, error, false),
                }
            }
            return;
        }
        if let Some(text) = exchange_copy {
            self.write_clipboard(&text, "command templates JSON", ctx);
            return;
        }
        if exchange_import {
            let mut editor =
                std::mem::take(&mut self.ui.modals.run_command.as_mut().unwrap().exchange);
            let result = if let Some(preview) = &editor.preview {
                crate::template_exchange::commit(
                    &editor.source,
                    preview,
                    self.command_templates_mut(),
                    crate::cmdtemplate::save,
                )
            } else {
                Err("Review the JSON first".into())
            };
            if result.is_ok() {
                editor.rebase(self.command_templates_mut());
            }
            editor.message = Some(match result {
                Ok(added) => format!("Added {added} template(s); existing commands kept"),
                Err(error) => error,
            });
            self.ui.modals.run_command.as_mut().unwrap().exchange = editor;
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
        if let Some(template) = save_template {
            {
                let name = template.name.clone();
                let added = self.command_templates_mut().save_template(template);
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
        if let Some((raw, mode, working_directory)) = run {
            if raw.trim().is_empty() {
                return;
            }
            if let Some(reason) = self.ws.mutation_block_reason("running a command") {
                self.toast(ctx, crate::toasts::ToastKind::Error, reason, false);
                return;
            }
            let cwd = match working_directory.resolve(&sctx, &opening.selection) {
                Ok(path) => path.to_path_buf(),
                Err(message) => {
                    self.toast(ctx, crate::toasts::ToastKind::Error, message, false);
                    return;
                }
            };
            if let Some(blocked) = crate::cmdtemplate::blocked_command_root(
                opening.dir(),
                &cwd,
                crate::trust::allows_run_command,
            ) {
                self.toast(
                    ctx,
                    crate::toasts::ToastKind::Error,
                    format!(
                        "{}: {}",
                        blocked.display(),
                        trust_block_message(crate::trust::label_for(blocked))
                    ),
                    false,
                );
                return;
            }
            let (sender, receiver) = std::sync::mpsc::channel();
            let repaint = ctx.clone();
            let child = CommandChildSlot::default();
            let worker_child = child.clone();
            let progress =
                std::sync::Arc::new(crate::command_batch::Progress::new(if mode.per_item() {
                    sctx.paths.len()
                } else {
                    1
                }));
            let worker_progress = progress.clone();
            let report_template = raw.clone();
            let report_name = self
                .ui
                .modals
                .run_command
                .as_ref()
                .map(|state| state.template_name.clone())
                .unwrap_or_default();
            std::thread::spawn(move || {
                let mut output =
                    crate::command_batch::run(raw, mode, &sctx, &worker_progress, |cmdline| {
                        let output =
                            run_shell_cancellable(cmdline, &cwd, &worker_child, &worker_progress);
                        repaint.request_repaint();
                        output
                    });
                output.context = Some(crate::command_report::Context {
                    name: report_name,
                    template: report_template,
                    execution: mode,
                    attempted_invocations: worker_progress
                        .completed
                        .load(std::sync::atomic::Ordering::Acquire),
                    working_directory_policy: working_directory,
                    initial_working_directory: cwd,
                    opening_directory: sctx.dir,
                    other_directory: sctx.dir_other,
                    inputs: sctx.paths,
                });
                let _ = sender.send(output);
                repaint.request_repaint();
            });
            if let Some(state) = self.ui.modals.run_command.as_mut() {
                state.run = Some(CommandOutputRun {
                    receiver,
                    child,
                    progress,
                });
                state.output = None;
                state.output_view = Default::default();
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
                    batch: None,
                    context: None,
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
    use crate::cmdtemplate::expand;

    fn entry(path: &str) -> crate::panel::FileEntry {
        let path = PathBuf::from(path);
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        crate::panel::FileEntry {
            symlink: None,
            permissions: None,
            allocated_size: None,
            hard_links: None,
            owner_uid: None,
            owner_gid: None,
            accessed: None,
            created: None,
            accessed_str: "Unavailable".into(),
            created_str: "Unavailable".into(),
            changed: None,
            changed_str: "Unavailable".into(),
            name_lower: name.to_lowercase(),
            sort_key: crate::panel::diacritic_sort_key(&name.to_lowercase()),
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
            batch: None,
            context: None,
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
            require_all: false,
            working_dir: Default::default(),
            execution: Default::default(),
            kind: Default::default(),
            limits: Default::default(),
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
        let progress = std::sync::Arc::new(crate::command_batch::Progress::new(1));
        let worker_progress = progress.clone();
        std::thread::spawn(move || {
            let cwd = std::env::temp_dir();
            // Not `exec`: the shell forks `sleep` as a grandchild that holds
            // the output pipes, so killing only `sh` would hang here.
            let _ = sender.send(run_shell_cancellable(
                "sleep 30; true".to_string(),
                &cwd,
                &worker_child,
                &worker_progress,
            ));
        });
        let run = CommandOutputRun {
            receiver,
            child,
            progress,
        };
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

#[cfg(all(test, unix))]
mod per_item_process_tests {
    use super::*;
    use crate::{
        cmdtemplate::{ExecutionMode, SelectionCtx},
        command_batch::{self, Progress},
    };
    use std::sync::{Arc, atomic::Ordering};
    #[test]
    fn resolved_other_and_selected_directories_are_actual_process_cwd_and_missing_never_falls_back()
    {
        let left = crate::testutil::TempDir::new();
        let right = crate::testutil::TempDir::new();
        let selected = left.path().join("selected");
        std::fs::create_dir(&selected).unwrap();
        let entry = crate::panel::FileEntry::from_meta(
            selected.clone(),
            &std::fs::metadata(&selected).unwrap(),
        )
        .unwrap();
        let entries = [entry];
        let context = SelectionCtx {
            paths: vec![selected.clone()],
            dir: left.path().into(),
            dir_other: right.path().into(),
        };
        for mode in [
            crate::cmdtemplate::WorkingDirectory::OtherPane,
            crate::cmdtemplate::WorkingDirectory::SelectedFolder,
        ] {
            let path = mode.resolve(&context, &entries).unwrap();
            let slot = CommandChildSlot::default();
            let progress = Progress::new(1);
            let output = run_shell_cancellable("pwd".into(), path, &slot, &progress);
            assert_eq!(output.exit_code, Some(0));
            assert_eq!(
                output.stdout.trim_end(),
                std::fs::canonicalize(path).unwrap().to_string_lossy()
            );
        }
        let missing = left.path().join("not-created");
        let context = SelectionCtx {
            paths: vec![],
            dir: left.path().into(),
            dir_other: missing.clone(),
        };
        let path = crate::cmdtemplate::WorkingDirectory::OtherPane
            .resolve(&context, &[])
            .unwrap();
        let output = run_shell_cancellable(
            "touch no-fallback".into(),
            path,
            &CommandChildSlot::default(),
            &Progress::new(1),
        );
        assert!(output.error.is_some());
        assert!(!left.path().join("no-fallback").exists());
        assert!(!missing.exists());
    }

    #[test]
    fn real_shell_per_item_preserves_quoted_paths_order_and_error_policies() {
        let tmp = crate::testutil::TempDir::new();
        let paths = vec![
            tmp.file("one quote's.txt", ""),
            tmp.file("-two {dir}.txt", ""),
        ];
        let context = SelectionCtx {
            paths: paths.clone(),
            dir: tmp.path().into(),
            dir_other: "/other".into(),
        };
        let slot = CommandChildSlot::default();
        let progress = Progress::new(2);
        let result = command_batch::run(
            "printf '<%s>\\n' {paths}".into(),
            ExecutionMode::PerItemContinue,
            &context,
            &progress,
            |line| run_shell_cancellable(line, tmp.path(), &slot, &progress),
        );
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.batch.as_ref().unwrap().succeeded, 2);
        for path in &paths {
            assert_eq!(
                result
                    .stdout
                    .matches(&format!("<{}>", path.display()))
                    .count(),
                1
            );
        }
        assert!(crate::lock_util::recover(&slot).is_none());
        for mode in [ExecutionMode::PerItemStop, ExecutionMode::PerItemContinue] {
            let tmp = crate::testutil::TempDir::new();
            let paths = vec![
                tmp.file("one", "initial"),
                tmp.file("fail", "initial"),
                tmp.file("after", "initial"),
            ];
            let context = SelectionCtx {
                paths: paths.clone(),
                dir: tmp.path().into(),
                dir_other: tmp.path().into(),
            };
            let slot = CommandChildSlot::default();
            let progress = Progress::new(3);
            let result = command_batch::run(
                "case {names} in fail) exit 7;; *) printf done > {paths};; esac".into(),
                mode,
                &context,
                &progress,
                |line| run_shell_cancellable(line, tmp.path(), &slot, &progress),
            );
            assert_eq!(result.exit_code, Some(7));
            assert!(result.stderr.contains("Invocation failed: exit 7"));
            assert!(result.stderr.contains("/fail"));
            let report = result.batch.unwrap();
            assert_eq!(report.failed, 1);
            assert_eq!(
                report.completed,
                if mode == ExecutionMode::PerItemStop {
                    2
                } else {
                    3
                }
            );
            assert_eq!(
                std::fs::read_to_string(&paths[2]).unwrap(),
                if mode == ExecutionMode::PerItemStop {
                    "initial"
                } else {
                    "done"
                }
            );
        }
    }
    #[test]
    fn stopping_live_per_item_run_kills_current_group_and_leaves_later_paths_untouched() {
        let tmp = crate::testutil::TempDir::new();
        let first = tmp.file("first", "initial");
        let later = tmp.file("later", "initial");
        let context = SelectionCtx {
            paths: vec![first.clone(), later.clone()],
            dir: tmp.path().into(),
            dir_other: tmp.path().into(),
        };
        let child = CommandChildSlot::default();
        let progress = Arc::new(Progress::new(2));
        let worker_child = child.clone();
        let worker_progress = progress.clone();
        let cwd = tmp.path().to_path_buf();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = command_batch::run(
                "sleep 30; printf done > {paths}".into(),
                ExecutionMode::PerItemContinue,
                &context,
                &worker_progress,
                |line| run_shell_cancellable(line, &cwd, &worker_child, &worker_progress),
            );
            sender.send(result).unwrap();
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while crate::lock_util::recover(&child).is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let run = CommandOutputRun {
            receiver,
            child: child.clone(),
            progress: progress.clone(),
        };
        let stopped_pid = crate::lock_util::recover(&child).as_ref().unwrap().id();
        run.stop();
        let result = run
            .receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|error| {
                let processes = std::process::Command::new("ps")
                    .args(["-Ao", "pid,ppid,pgid,stat,command"])
                    .output()
                    .unwrap();
                panic!(
                    "stopped shell {stopped_pid} did not report: {error}; processes:\n{}",
                    String::from_utf8_lossy(&processes.stdout)
                        .lines()
                        .filter(|line| {
                            let columns: Vec<_> = line.split_whitespace().take(3).collect();
                            columns.get(2).and_then(|group| group.parse::<u32>().ok())
                                == Some(stopped_pid)
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                );
            });
        worker.join().unwrap();
        assert!(result.batch.as_ref().unwrap().cancelled);
        assert_eq!(result.batch.as_ref().unwrap().completed, 1);
        assert!(progress.cancelled.load(Ordering::Acquire));
        assert_eq!(std::fs::read_to_string(first).unwrap(), "initial");
        assert_eq!(std::fs::read_to_string(later).unwrap(), "initial");
        assert!(crate::lock_util::recover(&child).is_none());
        assert!(matches!(
            command_outcome(&result).1,
            crate::toasts::ToastKind::Info
        ));
    }
    #[test]
    fn exited_leader_stays_waitable_until_pipe_holders_drain_and_stop_retires_it() {
        let dir = crate::testutil::TempDir::new();
        let cwd = dir.path().to_path_buf();
        let slot = CommandChildSlot::default();
        let progress = Arc::new(Progress::new(1));
        let worker_slot = slot.clone();
        let worker_progress = progress.clone();
        let (send, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            send.send(run_shell_cancellable(
                "sleep 30 & printf ready".into(),
                &cwd,
                &worker_slot,
                &worker_progress,
            ))
            .unwrap();
        });
        let run = CommandOutputRun {
            receiver,
            child: slot.clone(),
            progress,
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let guard = crate::lock_util::recover(&slot);
            if let Some(child) = guard.as_ref()
                && exited_without_reaping(child).expect("owned leader remains waitable")
            {
                break;
            }
            drop(guard);
            assert!(std::time::Instant::now() < deadline, "leader never exited");
            std::thread::yield_now();
        }
        run.stop();
        let output = run
            .receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("Stop drains background pipe holders");
        worker.join().unwrap();
        assert_eq!(output.stdout, "ready");
        assert!(crate::lock_util::recover(&slot).is_none());
        run.stop();
        assert!(crate::lock_util::recover(&slot).is_none());
    }
    #[test]
    fn cancellation_before_child_publication_never_spawns_the_command() {
        let tmp = crate::testutil::TempDir::new();
        let marker = tmp.path().join("not-created");
        let slot = CommandChildSlot::default();
        let guard = crate::lock_util::recover(&slot);
        let progress = Arc::new(Progress::new(1));
        let worker_slot = slot.clone();
        let worker_progress = progress.clone();
        let cwd = tmp.path().to_path_buf();
        let worker = std::thread::spawn(move || {
            run_shell_cancellable(
                "touch not-created".into(),
                &cwd,
                &worker_slot,
                &worker_progress,
            )
        });
        progress.cancelled.store(true, Ordering::Release);
        drop(guard);
        let output = worker.join().unwrap();
        assert!(output.error.is_some());
        assert!(!marker.exists());
        assert!(crate::lock_util::recover(&slot).is_none());
    }
}
