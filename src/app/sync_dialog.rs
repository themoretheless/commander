//! Synchronise sheet: pick a policy, review the per-row plan (each row's
//! direction is editable), then apply. The diff and direction defaults live in
//! `crate::sync`; copies run through the workspace transfer engine.

use super::*;
use crate::sync::{SyncDirection, SyncPolicy, SyncStatus};

type PlanResult = Result<(Vec<crate::sync::SyncAction>, crate::sync_guard::PlanStamp), String>;

/// A synchronization plan being built on a worker thread. Reading both
/// folders (and every entry's metadata) can stall on slow or network
/// volumes, so the sheet shows "Scanning" instead of freezing the UI.
/// Dropping it abandons the result; the worker ends after its one scan.
///
/// At most one worker runs per sheet: a refresh requested while a scan is
/// still running only marks `rerun`, and the sheet starts one fresh scan (with
/// the then-current settings) when the running one finishes. Repeated Refresh
/// clicks or policy changes therefore never pile up concurrent full scans.
pub struct PlanScan {
    receiver: std::sync::mpsc::Receiver<PlanResult>,
    rerun: bool,
    /// Policy the rows on screen were built with, so the user's per-row
    /// direction edits can be told from defaults and carried into the new
    /// plan. `None` when no plan was shown yet.
    shown_policy: Option<SyncPolicy>,
}

impl PlanScan {
    fn start(state: &SyncState) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        let left = state.left_dir.clone();
        let right = state.right_dir.clone();
        let (left_hidden, right_hidden) = (state.left_show_hidden, state.right_show_hidden);
        let policy = state.policy;
        let build =
            move || crate::sync_guard::build_plan(&left, &right, left_hidden, right_hidden, policy);
        let worker_sender = sender.clone();
        let spawned = std::thread::Builder::new()
            .name("sync-plan".to_string())
            .spawn(move || {
                let _ = worker_sender.send(build());
            });
        if let Err(error) = spawned {
            let _ = sender.send(Err(format!("Could not start the folder scan: {error}")));
        }
        Self {
            receiver,
            rerun: false,
            shown_policy: None,
        }
    }

    /// The finished plan, if the worker has delivered it.
    fn poll(&self) -> Option<PlanResult> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err("The folder scan stopped unexpectedly".to_string()))
            }
        }
    }
}

/// Rows whose direction the user changed from `policy`'s default.
fn edited_directions(
    actions: &[crate::sync::SyncAction],
    policy: SyncPolicy,
) -> std::collections::HashMap<String, SyncDirection> {
    actions
        .iter()
        .filter(|a| a.direction != crate::sync::default_direction(a.status, policy))
        .map(|a| (a.name.clone(), a.direction))
        .collect()
}

/// Re-apply edited directions by name where the new row still allows them.
/// Returns how many rows now differ from the new plan's default because of
/// a kept edit.
fn carry_edits(
    actions: &mut [crate::sync::SyncAction],
    edits: &std::collections::HashMap<String, SyncDirection>,
) -> usize {
    let mut kept = 0;
    for action in actions {
        if let Some(&dir) = edits.get(&action.name)
            && action.allows(dir)
        {
            if action.direction != dir {
                kept += 1;
            }
            action.direction = dir;
        }
    }
    kept
}

/// Note shown after a policy switch kept earlier per-row (or bulk) edits,
/// so a switch that seems to do nothing explains itself.
fn kept_edits_note(kept: usize) -> Option<String> {
    match kept {
        0 => None,
        1 => Some("1 edited row kept its direction from before the policy change".to_string()),
        n => Some(format!(
            "{n} edited rows kept their directions from before the policy change"
        )),
    }
}

/// Bulk direction: every row that can go `dir` does, except identical files
/// (copying those would only add work).
fn set_all_directions(actions: &mut [crate::sync::SyncAction], dir: SyncDirection) {
    for action in actions {
        if action.status != SyncStatus::Identical && action.allows(dir) {
            action.direction = dir;
        }
    }
}

/// Skip every row whose two sides disagree without a clear winner.
fn skip_conflicts(actions: &mut [crate::sync::SyncAction]) {
    for action in actions {
        if matches!(
            action.status,
            SyncStatus::Differing | SyncStatus::TypeConflict | SyncStatus::CaseConflict
        ) {
            action.direction = SyncDirection::Skip;
        }
    }
}

/// Install a finished plan (or its error) into the sheet. `shown_policy` is
/// the policy of the rows being replaced; their edits carry over.
fn apply_plan_result(state: &mut SyncState, result: PlanResult, shown_policy: Option<SyncPolicy>) {
    match result {
        Ok((mut actions, stamp)) => {
            let kept = shown_policy.map_or(0, |policy| {
                carry_edits(&mut actions, &edited_directions(&state.actions, policy))
            });
            // Only a policy switch can make kept edits look like the switch
            // was ignored; a plain refresh stays quiet.
            state.kept_edits = if shown_policy.is_some_and(|policy| policy != state.policy) {
                kept
            } else {
                0
            };
            state.actions = actions;
            state.settings_fingerprint = crate::sync_guard::settings_fingerprint(
                state.policy,
                &state.guard,
                stamp.filter_key(),
            );
            state.stamp = Some(stamp);
            state.allow_large_plan = false;
            state.error = None;
        }
        Err(error) => {
            state.actions.clear();
            state.stamp = None;
            state.error = Some(error);
        }
    }
}

/// Take the worker's plan once it is ready. A result whose settings were
/// superseded while it ran is dropped and one fresh scan starts instead.
fn poll_plan(state: &mut SyncState) {
    let Some(scan) = state.scan.as_ref() else {
        return;
    };
    let Some(result) = scan.poll() else {
        return;
    };
    let shown_policy = scan.shown_policy;
    if scan.rerun {
        // The rows on screen are still the ones the first scan replaced.
        let mut rerun = PlanScan::start(state);
        rerun.shown_policy = shown_policy;
        state.scan = Some(rerun);
        return;
    }
    state.scan = None;
    apply_plan_result(state, result, shown_policy);
}

/// Ask for a new plan: start a scan now, or, when one is already running,
/// queue a single rerun for when it finishes. `shown_policy` is the policy
/// the rows currently on screen were built with.
fn request_plan(state: &mut SyncState, shown_policy: SyncPolicy) {
    match state.scan.as_mut() {
        Some(scan) => scan.rerun = true,
        None => {
            let mut scan = PlanScan::start(state);
            scan.shown_policy = Some(shown_policy);
            state.scan = Some(scan);
        }
    }
}

/// "1 action", "2 additions": the sync assessment line, pluralized.
fn assessment_summary(
    actions: usize,
    additions: usize,
    replacements: usize,
    fraction: f32,
) -> String {
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    format!(
        "{}   {}   {}   {:.0}% changed",
        plural(actions, "action", "actions"),
        plural(additions, "addition", "additions"),
        plural(replacements, "replacement", "replacements"),
        fraction * 100.0
    )
}

/// Accessible name for a row's direction button, naming the file.
fn direction_label(dir: SyncDirection, name: &str) -> String {
    match dir {
        SyncDirection::ToLeft => format!("Copy {name} to left"),
        SyncDirection::Skip => format!("Skip {name}"),
        SyncDirection::ToRight => format!("Copy {name} to right"),
    }
}

/// Why `dir` is unavailable for a row (shown on the disabled button).
const fn unavailable_reason(status: SyncStatus, dir: SyncDirection) -> &'static str {
    match (status, dir) {
        (SyncStatus::DirectoryPair, _) => "Folders are synchronized by their contents",
        (SyncStatus::TypeConflict, _) => "A file and a folder share this name; resolve it by hand",
        (_, SyncDirection::ToLeft) => "Only exists on the left",
        (_, SyncDirection::ToRight) => "Only exists on the right",
        (_, SyncDirection::Skip) => "Cannot skip this row",
    }
}

impl App {
    pub(crate) fn open_sync(&mut self) {
        let policy = SyncPolicy::TwoWay;
        let left_dir = self.ws.left.current_path.clone();
        let right_dir = self.ws.right.current_path.clone();
        let left_show_hidden = self.ws.left.show_hidden();
        let right_show_hidden = self.ws.right.show_hidden();
        let guard = self.ws.sync_guard_policy.clone();
        let marker_input = guard
            .health_marker
            .as_deref()
            .map_or_else(String::new, |path| path.to_string_lossy().into_owned());
        let marker_enabled = guard.health_marker.is_some();
        let mut state = SyncState {
            policy,
            durability: self.ws.durability_profile,
            version_retention: self.ws.version_retention,
            actions: Vec::new(),
            left_dir,
            right_dir,
            left_show_hidden,
            right_show_hidden,
            guard,
            stamp: None,
            settings_fingerprint: 0,
            allow_large_plan: false,
            marker_enabled,
            marker_input,
            error: None,
            dry_run: false,
            scan: None,
            kept_edits: 0,
        };
        state.scan = Some(PlanScan::start(&state));
        self.ui.tools.replace_on(self.ws.active.other());
        self.ui.tools.sync = Some(state);
    }

    pub(crate) fn show_sync_tool(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let escape_requested =
            self.take_escape_request(crate::accessibility::EscapeRoute::PaneTool);
        let escape_requested = super::ui_state::escape_closes_surface(ctx, escape_requested);
        if self.ui.tools.sync.is_none() {
            return;
        }
        let t = self.colors;

        let mut new_policy: Option<SyncPolicy> = None;
        let mut refresh_plan = false;
        let mut commit = false;
        let mut cancel = false;

        // Borrow the sync state only for the window body (no `self.ws` use here).
        {
            let state = self.ui.tools.sync.as_mut().unwrap();
            poll_plan(state);
            let scanning = state.scan.is_some();
            let mut to_right = 0usize;
            let mut to_left = 0usize;
            for a in &state.actions {
                match a.direction {
                    SyncDirection::ToRight => to_right += 1,
                    SyncDirection::ToLeft => to_left += 1,
                    SyncDirection::Skip => {}
                }
            }
            let pending = to_right + to_left;

            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new("Synchronize panels")
                        .size(13.0)
                        .strong()
                        .color(t.text_primary),
                );
                // The plan is bound to these folders, not to wherever the
                // panels have since navigated.
                for (side, dir) in [("Left", &state.left_dir), ("Right", &state.right_dir)] {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(format!("{side}: {}", dir.display()))
                                .size(10.0)
                                .color(t.text_muted),
                        )
                        .truncate(),
                    );
                }
                ui.add_space(8.0);

                // ── Policy selector ─────────────────────────────────
                ui.horizontal(|ui| {
                    for (p, text, name) in [
                        (SyncPolicy::TwoWay, "Two-way", "Two-way: copy newer and unique files both ways"),
                        (
                            SyncPolicy::MirrorLeftToRight,
                            "Mirror \u{00bb}",
                            "Mirror left to right: make the right folder match the left",
                        ),
                        (
                            SyncPolicy::MirrorRightToLeft,
                            "Mirror \u{00ab}",
                            "Mirror right to left: make the left folder match the right",
                        ),
                    ] {
                        let selected = state.policy == p;
                        let response = ui
                            .selectable_label(selected, egui::RichText::new(text).size(12.0))
                            .on_hover_text(name);
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::SelectableLabel,
                                true,
                                selected,
                                name,
                            )
                        });
                        if response.clicked() && !selected {
                            new_policy = Some(p);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    // Applying writes these back as the app-wide defaults
                    // (as the copy/move sheet does), so say so.
                    ui.label(
                        egui::RichText::new("Default durability")
                            .size(10.0)
                            .color(t.text_muted),
                    );
                    for profile in crate::operation::DurabilityProfile::ALL {
                        ui.selectable_value(&mut state.durability, profile, profile.label())
                            .on_hover_text(
                                "Used for this sync and kept as the default for later copies and moves",
                            );
                    }
                    if state.durability == crate::operation::DurabilityProfile::Versioned {
                        ui.add_space(8.0);
                        let retention_label = ui.label(
                            egui::RichText::new("Retention")
                                .size(10.0)
                                .color(t.text_muted),
                        );
                        egui::ComboBox::from_id_salt("sync_version_retention")
                            .selected_text(state.version_retention.label())
                            .show_ui(ui, |ui| {
                                for policy in crate::operation::VersionRetentionPolicy::ALL {
                                    ui.selectable_value(
                                        &mut state.version_retention,
                                        policy,
                                        policy.label(),
                                    )
                                    .on_hover_text(policy.consequence());
                                }
                            })
                            .response
                            .labelled_by(retention_label.id);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Refresh").clicked() {
                            refresh_plan = true;
                        }
                    });
                });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(6.0);

                // ── Per-row plan ────────────────────────────────────
                if scanning {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("Scanning both folders\u{2026}")
                                .size(12.0)
                                .color(t.text_muted),
                        );
                    });
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                } else if plan_shows_panels_match(
                    state.actions.is_empty(),
                    state.error.is_some(),
                    state.stamp.is_some(),
                ) {
                    ui.label(
                        egui::RichText::new("Both panels already match.")
                            .size(12.0)
                            .color(t.text_muted),
                    );
                }
                if !state.actions.is_empty() {
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("All \u{00ab}")
                            .on_hover_text("Copy every row that can go left to the left")
                            .clicked()
                        {
                            set_all_directions(&mut state.actions, SyncDirection::ToLeft);
                        }
                        if ui
                            .small_button("All \u{00bb}")
                            .on_hover_text("Copy every row that can go right to the right")
                            .clicked()
                        {
                            set_all_directions(&mut state.actions, SyncDirection::ToRight);
                        }
                        if ui
                            .small_button("Skip conflicts")
                            .on_hover_text("Skip rows that differ without a newer side, and type or case conflicts")
                            .clicked()
                        {
                            skip_conflicts(&mut state.actions);
                        }
                    });
                }
                // Fixed-height rows, so only the visible ones are laid out.
                const ROW_H: f32 = 22.0;
                let rows = state.actions.len();
                egui::ScrollArea::vertical()
                    .max_height(280.0)
                    .auto_shrink([false, true])
                    .show_rows(ui, ROW_H, rows, |ui, range| {
                        for a in &mut state.actions[range] {
                            ui.horizontal(|ui| {
                                ui.set_height(ROW_H);
                                // Direction control: ← skip → as selectable arrows.
                                for (dir, glyph) in [
                                    (SyncDirection::ToLeft, "\u{00ab}"),
                                    (SyncDirection::Skip, "\u{00b7}"),
                                    (SyncDirection::ToRight, "\u{00bb}"),
                                ] {
                                    let active = a.direction == dir;
                                    let available = a.allows(dir);
                                    let hint = match dir {
                                        SyncDirection::ToLeft => "Copy to left",
                                        SyncDirection::Skip => "Skip",
                                        SyncDirection::ToRight => "Copy to right",
                                    };
                                    let color = if active { t.accent } else { t.text_muted };
                                    let response = ui
                                        .add_enabled(
                                            available,
                                            egui::Button::selectable(
                                                active,
                                                egui::RichText::new(glyph).size(13.0).color(color),
                                            ),
                                        )
                                        .on_hover_text(hint)
                                        .on_disabled_hover_text(unavailable_reason(a.status, dir));
                                    response.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::WidgetType::Button,
                                            available,
                                            active,
                                            direction_label(dir, &a.name),
                                        )
                                    });
                                    if response.clicked() {
                                        a.direction = dir;
                                    }
                                }
                                ui.add_space(6.0);
                                let status = ui.label(
                                    egui::RichText::new(status_label(a.status))
                                        .size(10.0)
                                        .color(status_color(a.status, t)),
                                );
                                ui.add_space(6.0);
                                let name = ui.label(
                                    egui::RichText::new(&a.name)
                                        .size(12.0)
                                        .color(t.text_secondary),
                                );
                                // Paired files show both sides' size and date,
                                // so a "differs" row can be decided.
                                if let Some(sides) = a.comparison_text() {
                                    status.on_hover_text(&sides);
                                    name.on_hover_text(sides);
                                }
                            });
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);

                let mut marker_error = configure_marker(state);
                ui.horizontal(|ui| {
                    let changed = ui
                        .checkbox(&mut state.marker_enabled, "Require health marker")
                        .changed();
                    if state.marker_enabled {
                        let response = ui.add_sized(
                            [240.0, 22.0],
                            egui::TextEdit::singleline(&mut state.marker_input)
                                .hint_text(".commander-health"),
                        );
                        if changed || response.changed() {
                            marker_error = configure_marker(state);
                        }
                    } else if changed {
                        marker_error = configure_marker(state);
                    }
                });

                let assessment = crate::sync_guard::assess(&state.actions, &state.guard);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(assessment_summary(
                            assessment.planned_actions,
                            assessment.additions,
                            assessment.changed_existing,
                            assessment.change_fraction,
                        ))
                        .size(10.0)
                        .color(t.text_muted),
                    );
                });
                if let Some(note) = kept_edits_note(state.kept_edits) {
                    ui.label(egui::RichText::new(note).size(10.0).color(t.text_muted));
                }
                if let Some(reason) = &assessment.circuit_breaker {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(reason)
                                .size(11.0)
                                .color(t.accent_warning),
                        );
                        ui.checkbox(
                            &mut state.allow_large_plan,
                            "I reviewed this large change; allow sync",
                        );
                    });
                } else {
                    state.allow_large_plan = false;
                }
                if let Some(error) = marker_error.as_ref().or(state.error.as_ref()) {
                    ui.label(
                        egui::RichText::new(error)
                            .size(11.0)
                            .color(t.accent_warning),
                    );
                }
                if marker_error.is_none()
                    && let Some(stamp) = &state.stamp
                {
                    state.settings_fingerprint = crate::sync_guard::settings_fingerprint(
                        state.policy,
                        &state.guard,
                        stamp.filter_key(),
                    );
                }

                let guard_ready = !scanning
                    && marker_error.is_none()
                    && state.error.is_none()
                    && state.stamp.is_some()
                    && (assessment.circuit_breaker.is_none() || state.allow_large_plan);

                ui.add_space(6.0);
                ui.checkbox(
                    &mut state.dry_run,
                    "Dry run (preview only — do not copy or delete)",
                );

                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!(
                        "{to_right} \u{00bb}    {to_left} \u{00ab}    {} skip",
                        state.actions.len() - pending
                    ))
                    .size(11.0)
                    .color(t.text_muted),
                );
                // Primary first, left to right, so Tab order matches the
                // visual order and the other sheets.
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let apply_label = if state.dry_run {
                        format!("Preview {pending}")
                    } else {
                        format!("Sync {pending}")
                    };
                    if ui
                        .add_enabled(
                            pending > 0 && guard_ready,
                            super::dialog_button(
                                t,
                                super::ButtonRole::Primary,
                                egui::RichText::new(apply_label).size(13.0),
                            ),
                        )
                        .clicked()
                    {
                        commit = true;
                    }
                    ui.add_space(8.0);
                    if ui
                        .add(super::dialog_button(
                            t,
                            super::ButtonRole::Secondary,
                            egui::RichText::new("Cancel").size(13.0),
                        ))
                        .clicked()
                    {
                        cancel = true;
                    }
                });

                if escape_requested {
                    cancel = true;
                }
            });
        }

        if cancel {
            self.ui.tools.sync = None;
            return;
        }
        let mut shown_policy = None;
        if let Some(p) = new_policy
            && let Some(s) = self.ui.tools.sync.as_mut()
        {
            shown_policy = Some(s.policy);
            s.policy = p;
            refresh_plan = true;
        }
        if refresh_plan && let Some(state) = self.ui.tools.sync.as_mut() {
            // The previous rows stay visible (Apply disabled) until the new
            // plan arrives; a request made while a scan runs is coalesced
            // into one rerun after it. Per-row edits carry into the new plan.
            let shown = shown_policy.unwrap_or(state.policy);
            request_plan(state, shown);
        }
        if commit {
            let Some(mut state) = self.ui.tools.sync.take() else {
                return;
            };
            if let Some(error) = configure_marker(&mut state) {
                state.error = Some(error);
                self.ui.tools.sync = Some(state);
                return;
            }
            let Some(stamp) = state.stamp.clone() else {
                state.error = Some("Refresh the synchronization plan before applying".to_string());
                self.ui.tools.sync = Some(state);
                return;
            };
            if state.dry_run {
                let pending = state
                    .actions
                    .iter()
                    .filter(|action| action.direction != SyncDirection::Skip)
                    .count();
                self.toast(
                    ctx,
                    crate::toasts::ToastKind::Info,
                    format!("Dry run: {pending} planned action(s), nothing changed"),
                    false,
                );
                // A preview keeps the reviewed plan open so Sync can follow.
                self.ui.tools.sync = Some(state);
                return;
            }
            // No conflict rule is stored here: sync applies the reviewed
            // per-row directions, and silently saving a rule would change
            // later copy/move previews without the user asking.
            let c = ctx.clone();
            self.ws.durability_profile = state.durability;
            self.ws.version_retention = state.version_retention;
            let plan = crate::sync_guard::GuardedPlan {
                actions: &state.actions,
                stamp: &stamp,
                policy: state.policy,
                guard: &state.guard,
                expected_settings: state.settings_fingerprint,
                allow_large_plan: state.allow_large_plan,
            };
            match self
                .ws
                .apply_sync_guarded(plan, move || c.request_repaint())
            {
                Ok(_) => {}
                Err(error) => {
                    state.error = Some(error);
                    self.ui.tools.sync = Some(state);
                }
            }
        }
    }
}

/// An empty plan means "already in sync" only when a plan was actually
/// built; a failed build also leaves no actions but must show just its error.
const fn plan_shows_panels_match(no_actions: bool, has_error: bool, has_stamp: bool) -> bool {
    no_actions && !has_error && has_stamp
}

fn configure_marker(state: &mut SyncState) -> Option<String> {
    if !state.marker_enabled {
        state.guard.health_marker = None;
        return None;
    }
    if state.marker_input.trim().is_empty() {
        return Some("Enter a relative health-marker path".to_string());
    }
    state.guard.set_marker(&state.marker_input).err()
}

const fn status_label(status: SyncStatus) -> &'static str {
    match status {
        SyncStatus::LeftOnly => "left only",
        SyncStatus::RightOnly => "right only",
        SyncStatus::LeftNewer => "left newer",
        SyncStatus::RightNewer => "right newer",
        SyncStatus::Differing => "differs",
        SyncStatus::Identical => "identical",
        SyncStatus::DirectoryPair => "folder pair",
        SyncStatus::TypeConflict => "type conflict",
        SyncStatus::CaseConflict => "case conflict",
    }
}

const fn status_color(status: SyncStatus, t: ThemeColors) -> Color32 {
    match status {
        SyncStatus::Identical | SyncStatus::DirectoryPair => t.text_muted,
        SyncStatus::TypeConflict => t.accent_red,
        SyncStatus::Differing | SyncStatus::CaseConflict => t.accent_warning,
        _ => t.accent,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PlanScan, assessment_summary, direction_label, kept_edits_note, plan_shows_panels_match,
        poll_plan, request_plan, set_all_directions, skip_conflicts, unavailable_reason,
    };
    use crate::sync::{SyncDirection, SyncPolicy, SyncStatus, test_action};

    fn sync_state(left: &std::path::Path, right: &std::path::Path) -> super::SyncState {
        super::SyncState {
            policy: SyncPolicy::MirrorLeftToRight,
            durability: crate::operation::DurabilityProfile::Fast,
            version_retention: crate::operation::VersionRetentionPolicy::ALL[0],
            actions: Vec::new(),
            left_dir: left.to_path_buf(),
            right_dir: right.to_path_buf(),
            left_show_hidden: false,
            right_show_hidden: false,
            guard: crate::sync_guard::GuardPolicy::default(),
            stamp: None,
            settings_fingerprint: 0,
            allow_large_plan: false,
            marker_enabled: false,
            marker_input: String::new(),
            error: None,
            dry_run: false,
            scan: None,
            kept_edits: 0,
        }
    }

    fn wait_for_plan(state: &mut super::SyncState) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while state.scan.is_some() {
            assert!(std::time::Instant::now() < deadline, "plan never arrived");
            std::thread::yield_now();
            poll_plan(state);
        }
    }

    #[test]
    fn plan_is_built_off_the_ui_thread_and_installed_when_ready() {
        let temp = crate::testutil::TempDir::new();
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        std::fs::create_dir_all(&left).expect("left");
        std::fs::create_dir_all(&right).expect("right");
        std::fs::write(left.join("only-left.txt"), "x").expect("file");
        let mut state = sync_state(&left, &right);
        state.scan = Some(PlanScan::start(&state));
        wait_for_plan(&mut state);
        assert!(state.error.is_none(), "{:?}", state.error);
        assert!(state.stamp.is_some());
        assert_eq!(state.actions.len(), 1);
        assert_eq!(state.actions[0].name, "only-left.txt");
    }

    #[test]
    fn failed_background_plan_reports_its_error() {
        let temp = crate::testutil::TempDir::new();
        let missing = temp.path().join("missing");
        let mut state = sync_state(&missing, temp.path());
        state.scan = Some(PlanScan::start(&state));
        wait_for_plan(&mut state);
        assert!(state.error.is_some());
        assert!(state.stamp.is_none());
        assert!(state.actions.is_empty());
    }

    #[test]
    fn refresh_during_a_scan_coalesces_into_one_rerun_with_new_settings() {
        let temp = crate::testutil::TempDir::new();
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        std::fs::create_dir_all(&left).expect("left");
        std::fs::create_dir_all(&right).expect("right");
        std::fs::write(left.join("only-left.txt"), "x").expect("file");
        let mut state = sync_state(&left, &right);
        request_plan(&mut state, SyncPolicy::MirrorLeftToRight);
        assert!(state.scan.as_ref().is_some_and(|scan| !scan.rerun));
        // Further requests while the scan runs only mark one rerun; no
        // second worker is started.
        state.policy = SyncPolicy::MirrorRightToLeft;
        request_plan(&mut state, SyncPolicy::MirrorLeftToRight);
        request_plan(&mut state, SyncPolicy::MirrorLeftToRight);
        assert!(state.scan.as_ref().is_some_and(|scan| scan.rerun));
        wait_for_plan(&mut state);
        assert!(state.error.is_none(), "{:?}", state.error);
        // The installed plan comes from the rerun, built with the newer
        // policy: a right-to-left mirror skips the left-only file instead of
        // copying it right.
        assert_eq!(state.actions.len(), 1);
        assert_eq!(state.actions[0].name, "only-left.txt");
        assert_eq!(state.actions[0].direction, crate::sync::SyncDirection::Skip);
        let stamp = state.stamp.as_ref().expect("stamp");
        assert_eq!(
            state.settings_fingerprint,
            crate::sync_guard::settings_fingerprint(
                SyncPolicy::MirrorRightToLeft,
                &state.guard,
                stamp.filter_key(),
            )
        );
    }

    #[test]
    fn policy_switch_keeps_per_row_edits_the_new_plan_allows() {
        let temp = crate::testutil::TempDir::new();
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        std::fs::create_dir_all(&left).expect("left");
        std::fs::create_dir_all(&right).expect("right");
        std::fs::write(left.join("keep-skipped.txt"), "x").expect("file");
        std::fs::write(left.join("default.txt"), "x").expect("file");
        let mut state = sync_state(&left, &right);
        state.scan = Some(PlanScan::start(&state));
        wait_for_plan(&mut state);
        // Mirror left->right copies both; the user skips one row.
        let edited = state
            .actions
            .iter_mut()
            .find(|a| a.name == "keep-skipped.txt")
            .expect("row");
        assert_eq!(edited.direction, SyncDirection::ToRight);
        edited.direction = SyncDirection::Skip;

        let shown = state.policy;
        state.policy = SyncPolicy::TwoWay;
        request_plan(&mut state, shown);
        wait_for_plan(&mut state);
        let direction = |name: &str| {
            state
                .actions
                .iter()
                .find(|a| a.name == name)
                .map(|a| a.direction)
        };
        assert_eq!(direction("keep-skipped.txt"), Some(SyncDirection::Skip));
        assert_eq!(direction("default.txt"), Some(SyncDirection::ToRight));
        // The switch says why the skipped row did not follow the new policy.
        assert_eq!(state.kept_edits, 1);
        assert!(kept_edits_note(state.kept_edits).is_some());

        // A plain refresh with the same policy does not repeat the note.
        let shown = state.policy;
        request_plan(&mut state, shown);
        wait_for_plan(&mut state);
        assert_eq!(state.kept_edits, 0);
        assert_eq!(kept_edits_note(0), None);
    }

    #[test]
    fn bulk_controls_respect_row_availability() {
        let mut actions = vec![
            test_action("same", SyncStatus::Identical, SyncDirection::Skip),
            test_action("dir", SyncStatus::DirectoryPair, SyncDirection::Skip),
            test_action("newer", SyncStatus::LeftNewer, SyncDirection::ToRight),
            test_action("diff", SyncStatus::Differing, SyncDirection::ToRight),
        ];
        set_all_directions(&mut actions, SyncDirection::ToLeft);
        let dirs: Vec<_> = actions.iter().map(|a| a.direction).collect();
        assert_eq!(
            dirs,
            [
                SyncDirection::Skip,
                SyncDirection::Skip,
                SyncDirection::ToLeft,
                SyncDirection::ToLeft
            ]
        );
        skip_conflicts(&mut actions);
        assert_eq!(actions[2].direction, SyncDirection::ToLeft);
        assert_eq!(actions[3].direction, SyncDirection::Skip);
    }

    #[test]
    fn assessment_line_is_pluralized() {
        assert_eq!(
            assessment_summary(1, 1, 0, 0.5),
            "1 action   1 addition   0 replacements   50% changed"
        );
        assert_eq!(
            assessment_summary(2, 0, 2, 1.0),
            "2 actions   0 additions   2 replacements   100% changed"
        );
    }

    #[test]
    fn direction_buttons_name_the_file_and_explain_unavailability() {
        assert_eq!(
            direction_label(SyncDirection::ToLeft, "a.txt"),
            "Copy a.txt to left"
        );
        assert_eq!(direction_label(SyncDirection::Skip, "a.txt"), "Skip a.txt");
        assert_eq!(
            unavailable_reason(SyncStatus::LeftOnly, SyncDirection::ToLeft),
            "Only exists on the left"
        );
        assert_eq!(
            unavailable_reason(SyncStatus::RightOnly, SyncDirection::ToRight),
            "Only exists on the right"
        );
    }

    #[test]
    fn failed_plan_is_not_reported_as_already_matching() {
        assert!(plan_shows_panels_match(true, false, true));
        assert!(!plan_shows_panels_match(true, true, false));
        assert!(!plan_shows_panels_match(true, false, false));
        assert!(!plan_shows_panels_match(false, false, true));
    }
}
