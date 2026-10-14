//! Batch-rename studio: a modal sheet that edits a rename rule and shows a
//! live old -> new preview with collisions flagged. All planning lives in
//! `crate::rename`; this file is only the egui shell.

use super::*;
use crate::rename::{PlanStatus, RenamePlan, RenameRule, plan_batch_rename, plan_is_applicable};
use crate::rename_order::{RenameOrder, safe_rename_order};
use crate::workspace::BatchRenameContext;

const PREVIEW_SCROLL_ID: &str = "batch_rename_preview";

/// Tooltip for a flagged preview row, so the glyph says why it is flagged.
fn status_hint(status: PlanStatus) -> Option<&'static str> {
    match status {
        PlanStatus::Ok | PlanStatus::Unchanged => None,
        PlanStatus::Invalid => Some("Name is not valid"),
        PlanStatus::Collision => Some("Name already exists or repeats another row"),
    }
}

fn renamed_toast(n: usize) -> String {
    format!("Renamed {n} item{}", if n == 1 { "" } else { "s" })
}

/// Whether a global Enter should submit: only when the caret is in one of
/// the rule's text fields (or nothing is focused). A focused checkbox or
/// case toggle consumes Enter itself, and applying in the same frame would
/// skip the preview of the rule it just changed.
fn enter_submits(focused: Option<egui::Id>, text_fields: &[egui::Id]) -> bool {
    focused.is_none_or(|id| text_fields.contains(&id))
}

fn clear_error_after_rule_change(
    error: &mut Option<String>,
    previous: &RenameRule,
    current: &RenameRule,
) {
    if previous != current {
        *error = None;
    }
}

pub struct ValidatedBatchRename {
    rule: RenameRule,
    plans: Vec<RenamePlan>,
    will_change: usize,
    applicable: bool,
    block_reason: Option<String>,
    regex_error: Option<String>,
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
        // The regex error itself is shown under the Find field; the summary
        // only points there instead of repeating it.
        let (applicable, block_reason) = if regex_err.is_some() {
            (false, Some("Fix the Find pattern to continue".to_string()))
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
            regex_error: regex_err.map(|error| format!("Invalid regex: {error}")),
        }
    }

    fn rule_for_submit(&self, submit_requested: bool) -> Option<RenameRule> {
        (submit_requested && self.applicable).then(|| self.rule.clone())
    }

    /// The validation for `rule`, reusing `slot` while the rule is unchanged:
    /// planning compiles the regex and checks every target, too much to redo
    /// on every repaint of a large selection.
    fn cached<'a>(
        slot: &'a mut Option<Self>,
        context: &BatchRenameContext,
        rule: RenameRule,
    ) -> &'a Self {
        match slot {
            Some(validation) if validation.rule == rule => validation,
            slot => slot.insert(Self::build(context, rule)),
        }
    }
}

impl App {
    pub(crate) fn open_batch_rename(&mut self, ctx: &egui::Context) {
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
            error: None,
            scroll_nonce,
            validation: None,
        });
        Self::mark_modal_opened(ctx, UiModal::BatchRename);
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

        super::sheet_window("Batch rename", t, 16).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.label(
                egui::RichText::new(format!(
                    "Batch rename {target_count} item{}",
                    if target_count == 1 { "" } else { "s" }
                ))
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
                let label = ui.label(egui::RichText::new(label).size(11.0).color(t.text_muted));
                let edit = egui::TextEdit::singleline(buf)
                    .desired_width(180.0)
                    .hint_text(hint)
                    .margin(egui::vec2(6.0, 4.0));
                ui.add(edit).labelled_by(label.id)
            };

            let text_fields = egui::Grid::new("batch_rename_fields")
                .num_columns(4)
                .spacing([10.0, 8.0])
                .show(ui, |ui| {
                    let find_hint = if state.regex_mode {
                        "regex pattern"
                    } else {
                        "text or empty"
                    };
                    let first = field(ui, "Find", &mut state.find, find_hint);
                    if Self::take_modal_opened(ui.ctx(), UiModal::BatchRename) {
                        first.request_focus();
                    }
                    let replace_hint = if state.regex_mode { "$1 groups ok" } else { "" };
                    let replace = field(ui, "Replace", &mut state.replace, replace_hint);
                    ui.end_row();

                    let prefix = field(ui, "Prefix", &mut state.prefix, "before name");
                    let suffix = field(ui, "Suffix", &mut state.suffix, "after name");
                    ui.end_row();
                    [first.id, replace.id, prefix.id, suffix.id]
                })
                .inner;

            ui.add_space(4.0);
            ui.checkbox(&mut state.regex_mode, "Find is a regex");
            // Report a bad pattern next to the field it came from. The regex
            // does not depend on the case/numbering controls below, so this
            // validation already holds this frame's error.
            let rule = state.rule();
            let regex_error =
                ValidatedBatchRename::cached(&mut state.validation, &state.context, rule)
                    .regex_error
                    .clone();
            if let Some(error) = regex_error {
                ui.label(egui::RichText::new(error).size(11.0).color(t.accent_red));
            }

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
                        .selectable_label(state.case == mode, egui::RichText::new(text).size(12.0))
                        .clicked()
                    {
                        state.case = mode;
                    }
                }

                ui.add_space(16.0);
                ui.checkbox(&mut state.numbering_on, "Number");
                ui.add_enabled_ui(state.numbering_on, |ui| {
                    let label =
                        ui.label(egui::RichText::new("start").size(10.0).color(t.text_muted));
                    ui.add(egui::DragValue::new(&mut state.num_start).range(0..=100_000))
                        .labelled_by(label.id);
                    let label =
                        ui.label(egui::RichText::new("step").size(10.0).color(t.text_muted));
                    ui.add(egui::DragValue::new(&mut state.num_step).range(1..=1000))
                        .labelled_by(label.id);
                    let label = ui.label(egui::RichText::new("pad").size(10.0).color(t.text_muted));
                    ui.add(egui::DragValue::new(&mut state.num_pad).range(0..=6))
                        .labelled_by(label.id);
                });
            });

            let rule = state.rule();
            let validation =
                ValidatedBatchRename::cached(&mut state.validation, &state.context, rule);
            clear_error_after_rule_change(
                &mut state.error,
                &rule_before_controls,
                &validation.rule,
            );

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);

            // ── Preview ─────────────────────────────────────────────
            // Fixed-height rows, so only the visible ones are laid out.
            const ROW_H: f32 = 18.0;
            egui::ScrollArea::vertical()
                .id_salt((PREVIEW_SCROLL_ID, state.scroll_nonce))
                .max_height(240.0)
                .auto_shrink([false, true])
                .show_rows(ui, ROW_H, validation.plans.len(), |ui, range| {
                    for p in &validation.plans[range] {
                        let (color, mark) = match p.status {
                            PlanStatus::Ok => (t.accent, "→"),
                            PlanStatus::Unchanged => (t.text_muted, "="),
                            PlanStatus::Invalid => (t.accent_red, "✕"),
                            PlanStatus::Collision => (t.accent_warning, "⚠"),
                        };
                        ui.horizontal(|ui| {
                            ui.set_height(ROW_H);
                            // Split the row so a long name truncates (full
                            // name on hover) instead of widening the sheet.
                            let name_w = ((ui.available_width() - 24.0) / 2.0).max(60.0);
                            let name_cell = |ui: &mut egui::Ui, text: &str, color| {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(name_w, ROW_H),
                                    Layout::left_to_right(Align::Center),
                                    |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(text).size(12.0).color(color),
                                            )
                                            .truncate(),
                                        )
                                    },
                                );
                            };
                            name_cell(ui, &p.from, t.text_secondary);
                            let mark = ui.label(egui::RichText::new(mark).size(12.0).color(color));
                            if let Some(hint) = status_hint(p.status) {
                                mark.on_hover_text(hint);
                            }
                            name_cell(ui, &p.to, color);
                        });
                    }
                });

            ui.add_space(10.0);
            let mut submit_requested = false;
            {
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
            }

            // Primary first, left to right, so Tab order matches the visual
            // order and the mask/rename sheets.
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        validation.applicable,
                        super::dialog_button(
                            t,
                            super::ButtonRole::Primary,
                            egui::RichText::new("Rename").size(13.0),
                        ),
                    )
                    .clicked()
                {
                    submit_requested = true;
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

            if let Some(err) = &state.error {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(err).size(11.0).color(t.accent_red));
            }

            if ui.input(|i| i.key_pressed(egui::Key::Enter))
                && enter_submits(ui.memory(|m| m.focused()), &text_fields)
            {
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
                        self.toast(
                            ctx,
                            crate::toasts::ToastKind::Success,
                            renamed_toast(n),
                            true,
                        );
                        // apply_batch_rename just pushed this run onto the
                        // undo stack; reuse it for the receipt so the undo
                        // affordance stays exactly in sync with Cmd+Z.
                        if let Some(a) = self.ws.top_undo_action()
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
                    self.toast(
                        ctx,
                        crate::toasts::ToastKind::Error,
                        format!("Rename failed: {msg}"),
                        false,
                    );
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
    fn flagged_rows_explain_their_glyph() {
        assert_eq!(status_hint(PlanStatus::Ok), None);
        assert_eq!(status_hint(PlanStatus::Unchanged), None);
        assert_eq!(status_hint(PlanStatus::Invalid), Some("Name is not valid"));
        assert!(status_hint(PlanStatus::Collision).is_some_and(|hint| hint.contains("exists")));
    }

    #[test]
    fn renamed_toast_pluralizes() {
        assert_eq!(renamed_toast(1), "Renamed 1 item");
        assert_eq!(renamed_toast(3), "Renamed 3 items");
    }

    #[test]
    fn enter_submits_only_from_text_fields_or_no_focus() {
        let find = egui::Id::new("find");
        let checkbox = egui::Id::new("regex checkbox");
        assert!(enter_submits(None, &[find]));
        assert!(enter_submits(Some(find), &[find]));
        assert!(!enter_submits(Some(checkbox), &[find]));
    }

    #[test]
    fn regex_error_is_reported_separately_from_the_summary() {
        let context = BatchRenameContext {
            panel: ActivePanel::Left,
            dir: PathBuf::from("/fixture"),
            targets: vec!["a.txt".to_string()],
            existing: std::collections::HashSet::from(["a.txt".to_string()]),
        };
        let bad = RenameRule {
            find: "(".to_string(),
            regex: true,
            ..RenameRule::default()
        };
        let validation = ValidatedBatchRename::build(&context, bad);
        assert!(
            validation
                .regex_error
                .as_deref()
                .is_some_and(|error| error.starts_with("Invalid regex:"))
        );
        let good = ValidatedBatchRename::build(&context, RenameRule::default());
        assert_eq!(good.regex_error, None);
    }

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
    fn validation_is_rebuilt_only_when_the_rule_changes() {
        let context = BatchRenameContext {
            panel: ActivePanel::Left,
            dir: PathBuf::from("/fixture"),
            targets: vec!["a.txt".to_string(), "b.txt".to_string()],
            existing: std::collections::HashSet::from(["a.txt".to_string(), "b.txt".to_string()]),
        };
        let rule = RenameRule {
            prefix: "x-".to_string(),
            ..RenameRule::default()
        };
        let mut slot = None;
        let first = ValidatedBatchRename::cached(&mut slot, &context, rule.clone());
        assert_eq!(first.will_change, 2);
        // Mark the cached plan: an unchanged rule must hand it back as is.
        if let Some(validation) = slot.as_mut() {
            validation.will_change = usize::MAX;
        }
        let again = ValidatedBatchRename::cached(&mut slot, &context, rule);
        assert_eq!(again.will_change, usize::MAX);

        let edited = RenameRule {
            find: "(".to_string(),
            regex: true,
            ..RenameRule::default()
        };
        let current = ValidatedBatchRename::cached(&mut slot, &context, edited.clone());
        assert_eq!(current.rule, edited);
        assert!(!current.applicable);
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
        assert_eq!(
            current.block_reason.as_deref(),
            Some("Fix the Find pattern to continue")
        );
        assert!(
            current
                .regex_error
                .as_deref()
                .is_some_and(|error| error.starts_with("Invalid regex:"))
        );
        assert_eq!(current.rule_for_submit(true), None);
    }
}
