//! Batch-rename studio: a modal sheet that edits a rename rule and shows a
//! live old -> new preview with collisions flagged. All planning lives in
//! `crate::rename`; this file is only the egui shell.

use super::*;
use crate::rename::{PlanStatus, RenamePlan, RenameRule, plan_batch_rename, plan_is_applicable};
use crate::rename_order::{RenameOrder, safe_rename_order};
use crate::workspace::BatchRenameContext;

const PREVIEW_SCROLL_ID: &str = "batch_rename_preview";

fn clear_error_after_rule_change(
    error: &mut Option<String>,
    previous: &RenameRule,
    current: &RenameRule,
) {
    if previous != current {
        *error = None;
    }
}

struct ValidatedBatchRename {
    rule: RenameRule,
    plans: Vec<RenamePlan>,
    will_change: usize,
    applicable: bool,
    block_reason: Option<String>,
}

impl ValidatedBatchRename {
    fn build(context: &BatchRenameContext, rule: RenameRule) -> Self {
        let regex_err = crate::rename::regex_error(&rule);
        let plans = plan_batch_rename(&context.targets, &context.existing, &rule);
        let changes: Vec<(String, String)> = plans
            .iter()
            .filter(|plan| plan.to != plan.from)
            .map(|plan| (plan.from.clone(), plan.to.clone()))
            .collect();
        let will_change = changes.len();
        let (applicable, block_reason) = if let Some(error) = regex_err {
            (false, Some(format!("Invalid regex: {error}")))
        } else if plan_is_applicable(&plans) {
            (true, None)
        } else if plans.iter().any(|plan| plan.status == PlanStatus::Invalid) {
            (false, Some("Fix the invalid names to continue".to_string()))
        } else if changes.is_empty() {
            (false, None)
        } else {
            match safe_rename_order(&changes, &context.existing) {
                RenameOrder::Steps(steps) => (!steps.is_empty(), None),
                RenameOrder::Conflict(reason) => (false, Some(reason)),
            }
        };

        Self {
            rule,
            plans,
            will_change,
            applicable,
            block_reason,
        }
    }

    fn rule_for_submit(&self, submit_requested: bool) -> Option<RenameRule> {
        (submit_requested && self.applicable).then(|| self.rule.clone())
    }
}

impl App {
    pub(crate) fn open_batch_rename(&mut self) {
        let Some(context) = self.ws.batch_rename_context() else {
            return;
        };
        let scroll_nonce = self.issue_transient_nonce();
        self.ui.modals.batch_rename = Some(BatchRenameState {
            context,
            find: String::new(),
            replace: String::new(),
            regex_mode: false,
            prefix: String::new(),
            suffix: String::new(),
            case: crate::rename::CaseMode::Keep,
            numbering_on: false,
            num_start: 1,
            num_step: 1,
            num_pad: 2,
            focused: false,
            error: None,
            scroll_nonce,
        });
    }

    pub(crate) fn show_batch_rename_dialog(&mut self, ctx: &egui::Context) {
        let escape_requested = self.take_escape_request(crate::accessibility::EscapeRoute::Modal(
            crate::accessibility::ModalSurface::BatchRename,
        ));
        let Some(state) = &mut self.ui.modals.batch_rename else {
            return;
        };
        let t = self.colors;

        let target_count = state.context.targets.len();
        let rule_before_controls = state.rule();

        let mut commit: Option<(RenameRule, BatchRenameContext)> = None;
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
                    egui::RichText::new(format!("Batch rename {target_count} item(s)"))
                        .size(13.0)
                        .strong()
                        .color(t.text_primary),
                );
                ui.label(
                    egui::RichText::new(state.context.dir.display().to_string())
                        .size(10.0)
                        .color(t.text_muted),
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
                        let find_hint = if state.regex_mode {
                            "regex pattern"
                        } else {
                            "text or empty"
                        };
                        let first = field(ui, "Find", &mut state.find, find_hint);
                        if !state.focused {
                            first.request_focus();
                            state.focused = true;
                        }
                        let replace_hint = if state.regex_mode { "$1 groups ok" } else { "" };
                        field(ui, "Replace", &mut state.replace, replace_hint);
                        ui.end_row();

                        field(ui, "Prefix", &mut state.prefix, "before name");
                        field(ui, "Suffix", &mut state.suffix, "after name");
                        ui.end_row();
                    });

                ui.add_space(4.0);
                ui.checkbox(&mut state.regex_mode, "Find is a regex");

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

                let validation = ValidatedBatchRename::build(&state.context, state.rule());
                clear_error_after_rule_change(
                    &mut state.error,
                    &rule_before_controls,
                    &validation.rule,
                );

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(6.0);

                // ── Preview ─────────────────────────────────────────────
                egui::ScrollArea::vertical()
                    .id_salt((PREVIEW_SCROLL_ID, state.scroll_nonce))
                    .max_height(240.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for p in &validation.plans {
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
                let mut submit_requested = false;
                ui.horizontal(|ui| {
                    let summary = if validation.applicable {
                        format!(
                            "{} of {} will change",
                            validation.will_change,
                            validation.plans.len()
                        )
                    } else if let Some(reason) = &validation.block_reason {
                        reason.clone()
                    } else if validation.will_change == 0 {
                        "Nothing to change".to_string()
                    } else {
                        "Fix the flagged rows to continue".to_string()
                    };
                    let summary_color = if validation.applicable {
                        t.accent
                    } else {
                        t.text_muted
                    };
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
                                validation.applicable,
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
                            submit_requested = true;
                        }
                    });
                });

                if let Some(err) = &state.error {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(err).size(11.0).color(t.accent_red));
                }

                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit_requested = true;
                }
                if let Some(rule) = validation.rule_for_submit(submit_requested) {
                    commit = Some((rule, state.context.clone()));
                }
                if escape_requested {
                    cancel = true;
                }
            });

        if cancel {
            self.ui.modals.batch_rename = None;
            return;
        }
        if let Some((rule, context)) = commit {
            match self.ws.apply_batch_rename_in(&context, &rule) {
                Ok(n) => {
                    self.ui.modals.batch_rename = None;
                    if n > 0 {
                        let now = ctx.input(|i| i.time);
                        self.toasts.push(crate::toasts::Toast::new(
                            format!("Renamed {n} item(s)"),
                            crate::toasts::ToastKind::Success,
                            true,
                            now,
                        ));
                        // apply_batch_rename just pushed this run onto the
                        // undo stack; reuse it for the receipt so the undo
                        // affordance stays exactly in sync with Cmd+Z.
                        if let Some(a) = self.ws.stack.peek_undo()
                            && let Some(jump_to) = a.jump_to()
                        {
                            self.receipts.push(crate::receipts::Receipt {
                                verb: a.verb(),
                                item_count: a.item_count(),
                                timestamp: now,
                                jump_to,
                                undo_action: Some(a.clone()),
                            });
                        }
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
                    if let Some(s) = &mut self.ui.modals.batch_rename {
                        s.error = Some(msg);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_rule_clears_previous_apply_error() {
        let previous = RenameRule::default();
        let mut current = previous.clone();
        current.prefix = "archive-".to_string();
        let mut error = Some("target already exists".to_string());

        clear_error_after_rule_change(&mut error, &previous, &current);

        assert_eq!(error, None);
    }

    #[test]
    fn unchanged_rule_keeps_apply_error() {
        let rule = RenameRule::default();
        let mut error = Some("permission denied".to_string());

        clear_error_after_rule_change(&mut error, &rule, &rule);

        assert_eq!(error.as_deref(), Some("permission denied"));
    }

    #[test]
    fn same_frame_invalid_edit_cannot_submit_under_previous_validation() {
        let context = BatchRenameContext {
            panel: ActivePanel::Left,
            dir: PathBuf::from("/fixture"),
            targets: vec!["report.txt".to_string()],
            existing: std::collections::HashSet::from(["report.txt".to_string()]),
        };
        let previously_valid = RenameRule {
            prefix: "archived-".to_string(),
            ..RenameRule::default()
        };
        let previous = ValidatedBatchRename::build(&context, previously_valid.clone());
        assert_eq!(previous.rule_for_submit(true), Some(previously_valid));

        let edited_rule = RenameRule {
            find: "(".to_string(),
            regex: true,
            ..RenameRule::default()
        };
        let current = ValidatedBatchRename::build(&context, edited_rule);

        assert!(!current.applicable);
        assert!(
            current
                .block_reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("Invalid regex:"))
        );
        assert_eq!(current.rule_for_submit(true), None);
    }
}
