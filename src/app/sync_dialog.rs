//! Synchronise sheet: pick a policy, review the per-row plan (each row's
//! direction is editable), then apply. The diff and direction defaults live in
//! `crate::sync`; copies run through the workspace transfer engine.

use super::*;
use crate::sync::{SyncDirection, SyncPolicy, SyncStatus};

impl App {
    pub(crate) fn open_sync(&mut self) {
        let policy = SyncPolicy::TwoWay;
        let left_dir = self.ws.left.current_path.clone();
        let right_dir = self.ws.right.current_path.clone();
        let left_show_hidden = self.ws.left.show_hidden;
        let right_show_hidden = self.ws.right.show_hidden;
        let guard = self.ws.sync_guard_policy.clone();
        let marker_input = guard
            .health_marker
            .as_deref()
            .map_or_else(String::new, |path| path.to_string_lossy().into_owned());
        let marker_enabled = guard.health_marker.is_some();
        let (actions, stamp, error) = match self.ws.build_guarded_sync_plan(policy) {
            Ok((actions, stamp)) => (actions, Some(stamp), None),
            Err(error) => (Vec::new(), None, Some(error)),
        };
        let settings_fingerprint = stamp.as_ref().map_or(0, |stamp| {
            crate::sync_guard::settings_fingerprint(policy, &guard, stamp.filter_key())
        });
        self.ui.sync = Some(SyncState {
            policy,
            durability: self.ws.durability_profile,
            version_retention: self.ws.version_retention,
            actions,
            left_dir,
            right_dir,
            left_show_hidden,
            right_show_hidden,
            guard,
            stamp,
            settings_fingerprint,
            allow_large_plan: false,
            marker_enabled,
            marker_input,
            error,
        });
    }

    pub(crate) fn show_sync_dialog(&mut self, ctx: &egui::Context) {
        let escape_requested = self.take_modal_escape(crate::accessibility::ModalSurface::Sync);
        if self.ui.sync.is_none() {
            return;
        }
        let t = self.colors;

        let mut new_policy: Option<SyncPolicy> = None;
        let mut refresh_plan = false;
        let mut commit = false;
        let mut cancel = false;

        // Borrow the sync state only for the window body (no `self.ws` use here).
        {
            let state = self.ui.sync.as_mut().unwrap();
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

            egui::Window::new("Synchronize")
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
                    ui.set_width(640.0);
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
                            (SyncPolicy::MirrorLeftToRight, "Mirror \u{2192}"),
                            (SyncPolicy::MirrorRightToLeft, "Mirror \u{2190}"),
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
                    if state.actions.is_empty() {
                        ui.label(
                            egui::RichText::new("Both panels already match.")
                                .size(12.0)
                                .color(t.text_muted),
                        );
                    }
                    egui::ScrollArea::vertical()
                        .max_height(280.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for a in &mut state.actions {
                                ui.horizontal(|ui| {
                                    // Direction control: ← skip → as selectable arrows.
                                    for (dir, glyph) in [
                                        (SyncDirection::ToLeft, "\u{2190}"),
                                        (SyncDirection::Skip, "\u{00b7}"),
                                        (SyncDirection::ToRight, "\u{2192}"),
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
                                                    egui::RichText::new(glyph)
                                                        .size(13.0)
                                                        .color(color),
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

                    let guard_ready = marker_error.is_none()
                        && state.error.is_none()
                        && state.stamp.is_some()
                        && (assessment.circuit_breaker.is_none() || state.allow_large_plan);

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "{to_right} \u{2192}    {to_left} \u{2190}    {} skip",
                                state.actions.len() - pending
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
                                    pending > 0 && guard_ready,
                                    egui::Button::new(
                                        egui::RichText::new(format!("Sync {pending}"))
                                            .size(13.0)
                                            .color(Color32::WHITE),
                                    )
                                    .fill(t.accent)
                                    .corner_radius(CornerRadius::ZERO),
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
            self.ui.sync = None;
            return;
        }
        if let Some(p) = new_policy
            && let Some(s) = self.ui.sync.as_mut()
        {
            s.policy = p;
            refresh_plan = true;
        }
        if refresh_plan && let Some(state) = self.ui.sync.as_mut() {
            match crate::sync_guard::build_plan(
                &state.left_dir,
                &state.right_dir,
                state.left_show_hidden,
                state.right_show_hidden,
                state.policy,
            ) {
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
        if commit {
            let Some(mut state) = self.ui.sync.take() else {
                return;
            };
            if let Some(error) = configure_marker(&mut state) {
                state.error = Some(error);
                self.ui.sync = Some(state);
                return;
            }
            let Some(stamp) = state.stamp.clone() else {
                state.error = Some("Refresh the synchronization plan before applying".to_string());
                self.ui.sync = Some(state);
                return;
            };
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
                    self.ui.sync = Some(state);
                }
            }
        }
    }
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

fn status_label(status: SyncStatus) -> &'static str {
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

fn status_color(status: SyncStatus, t: ThemeColors) -> Color32 {
    match status {
        SyncStatus::Identical | SyncStatus::DirectoryPair => t.text_muted,
        SyncStatus::TypeConflict => t.accent_red,
        SyncStatus::Differing | SyncStatus::CaseConflict => t.accent_warning,
        _ => t.accent,
    }
}
