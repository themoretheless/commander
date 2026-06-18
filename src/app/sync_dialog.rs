//! Synchronise sheet: pick a policy, review the per-row plan (each row's
//! direction is editable), then apply. The diff and direction defaults live in
//! `crate::sync`; copies run through the workspace transfer engine.

use super::*;
use crate::sync::{SyncDirection, SyncPolicy, SyncStatus};

impl App {
    pub(crate) fn show_sync_dialog(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.ws.requests.sync_request) {
            let policy = SyncPolicy::TwoWay;
            let actions = self.ws.build_sync_actions(policy);
            self.sync = Some(SyncState { policy, actions });
        }
        if self.sync.is_none() {
            return;
        }
        let t = self.colors;

        let mut new_policy: Option<SyncPolicy> = None;
        let mut commit = false;
        let mut cancel = false;

        // Borrow the sync state only for the window body (no `self.ws` use here).
        {
            let state = self.sync.as_mut().unwrap();
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
                    ui.set_width(600.0);
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
                                        let color = if active { t.accent } else { t.text_muted };
                                        if ui
                                            .selectable_label(
                                                active,
                                                egui::RichText::new(glyph).size(13.0).color(color),
                                            )
                                            .clicked()
                                        {
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

                    ui.add_space(10.0);
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
                                    pending > 0,
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

                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        cancel = true;
                    }
                });
        }

        if cancel {
            self.sync = None;
            return;
        }
        if let Some(p) = new_policy {
            let actions = self.ws.build_sync_actions(p);
            if let Some(s) = self.sync.as_mut() {
                s.policy = p;
                s.actions = actions;
            }
        }
        if commit {
            let actions = self.sync.take().map(|s| s.actions).unwrap_or_default();
            let c = ctx.clone();
            self.ws.apply_sync(&actions, move || c.request_repaint());
        }
    }
}

fn status_label(status: SyncStatus) -> &'static str {
    match status {
        SyncStatus::LeftOnly => "left only",
        SyncStatus::RightOnly => "right only",
        SyncStatus::LeftNewer => "left newer",
        SyncStatus::RightNewer => "right newer",
        SyncStatus::Differing => "differs",
        SyncStatus::Identical => "identical",
    }
}

fn status_color(status: SyncStatus, t: ThemeColors) -> Color32 {
    match status {
        SyncStatus::Identical => t.text_muted,
        SyncStatus::Differing => t.accent_warning,
        _ => t.accent,
    }
}
