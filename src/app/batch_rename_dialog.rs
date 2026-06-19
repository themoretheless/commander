//! Batch-rename studio: a modal sheet that edits a rename rule and shows a
//! live old -> new preview with collisions flagged. All planning lives in
//! `crate::rename`; this file is only the egui shell.

use super::*;
use crate::rename::{PlanStatus, changed_count, plan_batch_rename, plan_is_applicable};

impl App {
    pub(crate) fn show_batch_rename_dialog(&mut self, ctx: &egui::Context) {
        let Some(state) = &mut self.batch_rename else {
            return;
        };
        let t = self.colors;

        // Compute the live plan from the current rule.
        let names = self.ws.batch_rename_targets();
        let existing = self.ws.active_dir_names();
        let rule = state.rule();
        let plans = plan_batch_rename(&names, &existing, &rule);
        let changed = changed_count(&plans);
        let applicable = plan_is_applicable(&plans);

        let mut commit = false;
        let mut cancel = false;

        egui::Window::new("Batch rename")
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
                ui.set_width(560.0);
                ui.label(
                    egui::RichText::new(format!("Batch rename {} item(s)", names.len()))
                        .size(13.0)
                        .strong()
                        .color(t.text_primary),
                );
                ui.add_space(10.0);

                // ── Rule fields ─────────────────────────────────────────
                let field = |ui: &mut egui::Ui, label: &str, buf: &mut String, hint: &str| {
                    ui.label(egui::RichText::new(label).size(11.0).color(t.text_muted));
                    let edit = egui::TextEdit::singleline(buf)
                        .desired_width(180.0)
                        .hint_text(hint)
                        .margin(egui::vec2(6.0, 4.0));
                    ui.add(edit)
                };

                egui::Grid::new("batch_rename_fields")
                    .num_columns(4)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        let first = field(ui, "Find", &mut state.find, "text or empty");
                        if !state.focused {
                            first.request_focus();
                            state.focused = true;
                        }
                        field(ui, "Replace", &mut state.replace, "");
                        ui.end_row();

                        field(ui, "Prefix", &mut state.prefix, "before name");
                        field(ui, "Suffix", &mut state.suffix, "after name");
                        ui.end_row();
                    });

                ui.add_space(8.0);

                // ── Case + numbering ────────────────────────────────────
                ui.horizontal(|ui| {
                    use crate::rename::CaseMode;
                    ui.label(egui::RichText::new("Case").size(11.0).color(t.text_muted));
                    for (mode, text) in [
                        (CaseMode::Keep, "Keep"),
                        (CaseMode::Lower, "lower"),
                        (CaseMode::Upper, "UPPER"),
                    ] {
                        if ui
                            .selectable_label(
                                state.case == mode,
                                egui::RichText::new(text).size(12.0),
                            )
                            .clicked()
                        {
                            state.case = mode;
                        }
                    }

                    ui.add_space(16.0);
                    ui.checkbox(&mut state.numbering_on, "Number");
                    ui.add_enabled_ui(state.numbering_on, |ui| {
                        ui.label(egui::RichText::new("start").size(10.0).color(t.text_muted));
                        ui.add(egui::DragValue::new(&mut state.num_start).range(0..=100_000));
                        ui.label(egui::RichText::new("step").size(10.0).color(t.text_muted));
                        ui.add(egui::DragValue::new(&mut state.num_step).range(1..=1000));
                        ui.label(egui::RichText::new("pad").size(10.0).color(t.text_muted));
                        ui.add(egui::DragValue::new(&mut state.num_pad).range(0..=6));
                    });
                });

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(6.0);

                // ── Preview ─────────────────────────────────────────────
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for p in &plans {
                            let (color, mark) = match p.status {
                                PlanStatus::Ok => (t.accent, "→"),
                                PlanStatus::Unchanged => (t.text_muted, "="),
                                PlanStatus::Invalid => (t.accent_red, "✕"),
                                PlanStatus::Collision => (t.accent_warning, "⚠"),
                            };
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&p.from)
                                        .size(12.0)
                                        .color(t.text_secondary),
                                );
                                ui.label(egui::RichText::new(mark).size(12.0).color(color));
                                ui.label(egui::RichText::new(&p.to).size(12.0).color(color));
                            });
                        }
                    });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let summary = if applicable {
                        format!("{changed} of {} will change", plans.len())
                    } else if changed == 0 {
                        "Nothing to change".to_string()
                    } else {
                        "Fix the flagged rows to continue".to_string()
                    };
                    let summary_color = if applicable { t.accent } else { t.text_muted };
                    ui.label(egui::RichText::new(summary).size(11.0).color(summary_color));

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
                                applicable,
                                egui::Button::new(
                                    egui::RichText::new("Rename")
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

                if let Some(err) = &state.error {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(err).size(11.0).color(t.accent_red));
                }

                if ui.input(|i| i.key_pressed(egui::Key::Enter)) && applicable {
                    commit = true;
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }
            });

        if cancel {
            self.batch_rename = None;
            return;
        }
        if commit {
            let rule = self.batch_rename.as_ref().unwrap().rule();
            match self.ws.apply_batch_rename(&rule) {
                Ok(n) => {
                    self.batch_rename = None;
                    if n > 0 {
                        let now = ctx.input(|i| i.time);
                        self.toasts.push(crate::toasts::Toast::new(
                            format!("Renamed {n} item(s)"),
                            crate::toasts::ToastKind::Success,
                            true,
                            now,
                        ));
                    }
                }
                Err(msg) => {
                    // Surface the failure both inline and as an error toast.
                    let now = ctx.input(|i| i.time);
                    self.toasts.push(crate::toasts::Toast::new(
                        format!("Rename failed: {msg}"),
                        crate::toasts::ToastKind::Error,
                        false,
                        now,
                    ));
                    if let Some(s) = &mut self.batch_rename {
                        s.error = Some(msg);
                    }
                }
            }
        }
    }
}
