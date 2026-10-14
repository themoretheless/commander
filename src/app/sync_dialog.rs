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
pub struct PlanScan {
    receiver: std::sync::mpsc::Receiver<PlanResult>,
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
        Self { receiver }
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

/// Install a finished plan (or its error) into the sheet.
fn apply_plan_result(state: &mut SyncState, result: PlanResult) {
    match result {
        Ok((actions, stamp)) => {
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

/// Take the worker's plan once it is ready.
fn poll_plan(state: &mut SyncState) {
    if let Some(result) = state.scan.as_ref().and_then(PlanScan::poll) {
        state.scan = None;
        apply_plan_result(state, result);
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
                ui.add_space(8.0);

                // ── Policy selector ─────────────────────────────────
                ui.horizontal(|ui| {
                    for (p, text) in [
                        (SyncPolicy::TwoWay, "Two-way"),
                        (SyncPolicy::MirrorLeftToRight, "Mirror \u{00bb}"),
                        (SyncPolicy::MirrorRightToLeft, "Mirror \u{00ab}"),
                    ] {
                        if ui
                            .selectable_label(
                                state.policy == p,
                                egui::RichText::new(text).size(12.0),
                            )
                            .clicked()
                            && state.policy != p
                        {
                            new_policy = Some(p);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Durability")
                            .size(10.0)
                            .color(t.text_muted),
                    );
                    for profile in crate::operation::DurabilityProfile::ALL {
                        ui.selectable_value(&mut state.durability, profile, profile.label());
                    }
                    if state.durability == crate::operation::DurabilityProfile::Versioned {
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
                            });
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
                                        .on_hover_text(hint);
                                    if response.clicked() {
                                        a.direction = dir;
                                    }
                                }
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(status_label(a.status))
                                        .size(10.0)
                                        .color(status_color(a.status, t)),
                                );
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(&a.name)
                                        .size(12.0)
                                        .color(t.text_secondary),
                                );
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
                        egui::RichText::new(format!(
                            "{} actions   {} additions   {} replacements   {:.0}% changed",
                            assessment.planned_actions,
                            assessment.additions,
                            assessment.changed_existing,
                            assessment.change_fraction * 100.0
                        ))
                        .size(10.0)
                        .color(t.text_muted),
                    );
                });
                if let Some(reason) = &assessment.circuit_breaker {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(reason)
                                .size(11.0)
                                .color(t.accent_warning),
                        );
                        ui.checkbox(&mut state.allow_large_plan, "Reviewed");
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
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{to_right} \u{00bb}    {to_left} \u{00ab}    {} skip",
                            state.actions.len() - pending
                        ))
                        .size(11.0)
                        .color(t.text_muted),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
                        ui.add_space(8.0);
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
                    });
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
        if let Some(p) = new_policy
            && let Some(s) = self.ui.tools.sync.as_mut()
        {
            s.policy = p;
            refresh_plan = true;
        }
        if refresh_plan && let Some(state) = self.ui.tools.sync.as_mut() {
            // The previous rows stay visible (Apply disabled) until the new
            // plan arrives; a newer request supersedes one still running.
            state.scan = Some(PlanScan::start(state));
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
            {
                let names: Vec<String> = state
                    .actions
                    .iter()
                    .filter(|action| action.direction != SyncDirection::Skip)
                    .map(|action| action.name.clone())
                    .collect();
                let file_kind = names
                    .first()
                    .map(|name| crate::conflict_rules::ConflictRuleBook::kind_for_name(name))
                    .unwrap_or_else(|| "sync".into());
                if crate::conflict_rules::load()
                    .find(&state.left_dir, &state.right_dir, &file_kind)
                    .is_none()
                {
                    let policy = crate::conflict_rules::StoredRelationPolicy::KeepNewer;
                    let preview =
                        crate::conflict_rules::ConflictRuleBook::sample_preview(policy, &names, 5);
                    self.toast(ctx, crate::toasts::ToastKind::Info, preview, false);
                    crate::conflict_rules::upsert(crate::conflict_rules::ConflictRule {
                        left_root: state.left_dir.clone(),
                        right_root: state.right_dir.clone(),
                        file_kind,
                        policy,
                    });
                }
            }
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
    use super::{PlanScan, plan_shows_panels_match, poll_plan};
    use crate::sync::SyncPolicy;

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
    fn failed_plan_is_not_reported_as_already_matching() {
        assert!(plan_shows_panels_match(true, false, true));
        assert!(!plan_shows_panels_match(true, true, false));
        assert!(!plan_shows_panels_match(true, false, false));
        assert!(!plan_shows_panels_match(false, false, true));
    }
}
