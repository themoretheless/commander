//! Batch-rename studio: a modal sheet that edits a rename rule and shows a
//! live old -> new preview with collisions flagged. All planning lives in
//! `crate::rename`; this file is only the egui shell.

use super::*;
use crate::rename::{
    PlanStatus, RenamePlan, RenameRule, plan_batch_rename_with, plan_is_applicable,
};
use crate::rename_order::{RenameOrder, safe_rename_order_with};
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

fn refresh_existing_names(
    context: &mut BatchRenameContext,
    cached_generation: &mut u64,
    current_dir: &std::path::Path,
    current_generation: u64,
    existing: std::collections::HashSet<String>,
) -> bool {
    if context.dir != current_dir || *cached_generation == current_generation {
        return false;
    }
    context.existing = existing;
    *cached_generation = current_generation;
    true
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
    #[cfg(feature = "visual-qa")]
    pub fn extension_case_preview(
        &self,
        mode: crate::rename::ExtensionCase,
        expected: &str,
    ) -> bool {
        self.rule.extension_case == mode
            && self.plans.len() == 1
            && self.plans[0].to == expected
            && self.regex_error.is_none()
    }
    #[cfg(feature = "visual-qa")]
    pub fn options_preview(&self, ignore_case: bool, first_only: bool, expected: &str) -> bool {
        self.rule.ignore_case == ignore_case
            && self.rule.first_only == first_only
            && self.plans.len() == 1
            && self.plans[0].to == expected
            && self.regex_error.is_none()
    }
    #[cfg(feature = "visual-qa")]
    pub fn target_preview(&self, target: crate::rename::ReplaceTarget, expected: &str) -> bool {
        self.rule.replace_target == target
            && self.plans.len() == 1
            && self.plans[0].to == expected
            && self.regex_error.is_none()
    }

    #[cfg(feature = "visual-qa")]
    pub fn reset_preview(&self, expected: &str) -> bool {
        self.rule == RenameRule::default()
            && self.plans.len() == 1
            && self.plans[0].from == expected
            && self.plans[0].to == expected
            && !self.applicable
            && self.regex_error.is_none()
    }

    fn build(context: &BatchRenameContext, rule: RenameRule) -> Self {
        let regex_err = crate::rename::regex_error(&rule);
        let plans = plan_batch_rename_with(
            &context.targets,
            &context.existing,
            &rule,
            &context.dirs,
            context.folding,
        );
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
            match safe_rename_order_with(&changes, &context.existing, context.folding) {
                RenameOrder::Steps(steps) => (!steps.is_empty(), None),
                RenameOrder::Conflict(reason) => (false, Some(reason)),
            }
        };

        let error_kind = if rule.regex { "regex" } else { "find pattern" };
        Self {
            rule,
            plans,
            will_change,
            applicable,
            block_reason,
            regex_error: regex_err.map(|error| format!("Invalid {error_kind}: {error}")),
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
        let listing_generation = self.ws.active_panel_ref().entries_gen();
        let scroll_nonce = self.issue_transient_nonce();
        self.ui.modals.batch_rename = Some(BatchRenameState {
            context,
            listing_generation,
            remove_prefix: String::new(),
            remove_suffix: String::new(),
            find: String::new(),
            replace: String::new(),
            regex_mode: false,
            ignore_case: false,
            first_only: false,
            trim_spaces: false,
            collapse_repeats: false,
            normalization: crate::rename::NameNormalization::Keep,
            stem_limit_on: false,
            stem_limit: 32,
            replace_target: crate::rename::ReplaceTarget::WholeName,
            prefix: String::new(),
            suffix: String::new(),
            case: crate::rename::CaseMode::Keep,
            extension_case: crate::rename::ExtensionCase::Keep,
            numbering_on: false,
            number_position: crate::rename::NumberPosition::default(),
            num_start: 1,
            num_step: 1,
            num_pad: 2,
            preset_name: String::new(),
            preset_selected: None,
            preset_status: None,
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
        let listing_update = self.ui.modals.batch_rename.as_ref().map(|state| {
            let panel = match state.context.panel {
                crate::workspace::ActivePanel::Left => &self.ws.left,
                crate::workspace::ActivePanel::Right => &self.ws.right,
            };
            (
                panel.current_path.clone(),
                panel.entries_gen(),
                panel
                    .entries()
                    .iter()
                    .map(|entry| entry.name.clone())
                    .collect(),
            )
        });
        let Some(state) = &mut self.ui.modals.batch_rename else {
            return;
        };
        if let Some((current_dir, current_generation, existing)) = listing_update
            && refresh_existing_names(
                &mut state.context,
                &mut state.listing_generation,
                &current_dir,
                current_generation,
                existing,
            )
        {
            state.validation = None;
        }
        let t = self.colors;

        let target_count = state.context.targets.len();
        let rule_before_controls = state.rule();

        let mut commit: Option<(RenameRule, BatchRenameContext)> = None;
        let mut cancel = false;
        let mut presets_changed = false;
        let presets = &mut self.rename_presets;
        let preset_name_id = egui::Id::new("batch-rename-preset-name");
        let editing_preset_name = ctx.memory(|memory| memory.has_focus(preset_name_id));
        let mut preset_interaction = false;

        let dialog_response = super::sheet_window("Batch rename", t, 16)
            .default_height((ctx.input(|input| input.viewport_rect().height()) - 96.0).max(160.0))
            .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("batch-rename-sheet")
                .max_height((ctx.input(|input| input.viewport_rect().height()) - 64.0).max(100.0))
                .show(ui, |ui| {
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

                    ui.horizontal_wrapped(|ui| {
                        let mut chosen = None;
                        let combo = egui::ComboBox::from_id_salt("batch-rename-presets")
                            .width(180.0)
                            .truncate()
                            .selected_text(
                                state.preset_selected.as_deref().unwrap_or("Load preset"),
                            )
                            .show_ui(ui, |ui| {
                                for preset in presets.items() {
                                    let response = ui.selectable_label(
                                        state.preset_selected.as_deref()
                                            == Some(preset.name.as_str()),
                                        &preset.name,
                                    );
                                    #[cfg(feature = "visual-qa")]
                                    ui.ctx().data_mut(|data| {
                                        data.insert_temp(
                                            egui::Id::new((
                                                "rename-preset-row",
                                                preset.name.as_str(),
                                            )),
                                            response.rect,
                                        )
                                    });
                                    if response.clicked() {
                                        chosen = Some(preset.clone());
                                    }
                                }
                            });
                        preset_interaction |= combo.response.has_focus()
                            || combo.response.lost_focus()
                            || combo.inner.is_some();
                        #[cfg(not(feature = "visual-qa"))]
                        let _ = combo;
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(
                                egui::Id::new("rename-preset-combo"),
                                combo.response.rect,
                            )
                        });
                        if let Some(preset) = chosen {
                            preset_interaction = true;
                            state.load_rule(&preset.rule);
                            state.preset_name = preset.name.clone();
                            state.preset_selected = Some(preset.name);
                            state.preset_status = Some(Ok(
                                "Preset loaded; review the preview before applying".into(),
                            ));
                        }
                        if ui
                            .add_enabled(
                                state.preset_selected.is_some(),
                                egui::Button::new("Update selected"),
                            )
                            .on_hover_text(
                                "Replace the selected preset's rule; its name stays the same",
                            )
                            .clicked()
                        {
                            preset_interaction = true;
                            let result = presets
                                .update(state.preset_selected.as_deref().unwrap(), state.rule());
                            presets_changed |= result.is_ok();
                            state.preset_status = Some(result.map(|()| "Preset updated".into()));
                        }
                        if ui
                            .add_enabled(
                                state.preset_selected.is_some(),
                                egui::Button::new("Delete preset"),
                            )
                            .clicked()
                        {
                            preset_interaction = true;
                            presets_changed |=
                                presets.remove(state.preset_selected.as_deref().unwrap());
                            state.preset_selected = None;
                            state.preset_status = Some(Ok("Preset removed".into()));
                        }
                        let reset = ui.button("Reset rule")
                            .on_hover_text("Start a new rename rule; keep saved presets");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("rename-reset-rule"), reset.rect));
                        preset_interaction |= reset.has_focus() || reset.lost_focus();
                        if reset.clicked() { state.reset_rule(); preset_interaction = true; }
                    });
                    ui.horizontal(|ui| {
                        let field = ui.add(
                            egui::TextEdit::singleline(&mut state.preset_name)
                                .id(preset_name_id)
                                .char_limit(64)
                                .hint_text("New preset name")
                                .desired_width(210.0),
                        );
                        preset_interaction |= (field.has_focus() || field.lost_focus())
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(egui::Id::new("rename-preset-name"), field.rect)
                        });
                        field.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::TextEdit,
                                field.enabled(),
                                "New rename preset name",
                            )
                        });
                        if ui.button("Save new preset").clicked() {
                            preset_interaction = true;
                            let result = presets.add(&state.preset_name, state.rule());
                            if let Ok(name) = &result {
                                state.preset_selected = Some(name.clone());
                            }
                            presets_changed |= result.is_ok();
                            state.preset_status = Some(result.map(|_| "Preset saved".into()));
                        }
                    });
                    if let Some(status) = &state.preset_status {
                        match status {
                            Ok(message) => {
                                ui.small(message);
                            }
                            Err(error) => {
                                ui.colored_label(t.accent_red, error);
                            }
                        }
                    }
                    ui.separator();

                    // ── Rule fields ─────────────────────────────────────────
                    let field = |ui: &mut egui::Ui, label: &str, buf: &mut String, hint: &str| {
                        let label =
                            ui.label(egui::RichText::new(label).size(11.0).color(t.text_muted));
                        let edit = egui::TextEdit::singleline(buf)
                            .desired_width(180.0)
                            .min_size(egui::vec2(180.0, 0.0))
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
                            let remove_prefix = field(ui, "Remove prefix", &mut state.remove_prefix, "exact original start");
                            let remove_suffix = field(ui, "Remove suffix", &mut state.remove_suffix, "exact original end");
                            ui.end_row();
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("rename-remove-prefix"), remove_prefix.rect));
                            [first.id, replace.id, prefix.id, suffix.id, remove_prefix.id, remove_suffix.id]
                        })
                        .inner;

                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        let regex = ui.checkbox(&mut state.regex_mode, "Find is a regex");
                        let case = ui.checkbox(&mut state.ignore_case, "Ignore case");
                        let first = ui.checkbox(&mut state.first_only, "First match only");
                        preset_interaction |= regex.changed() || case.changed() || first.changed();
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(egui::Id::new("rename-ignore-case"), case.rect);
                            data.insert_temp(egui::Id::new("rename-first-only"), first.rect);
                        });
                    });
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

                    ui.horizontal_wrapped(|ui| {
                        use crate::rename::ReplaceTarget;
                        ui.label("Apply find/replace to");
                        let combo = egui::ComboBox::from_id_salt("rename-replace-target")
                            .selected_text(state.replace_target.label())
                            .show_ui(ui, |ui| {
                                for target in [ReplaceTarget::WholeName, ReplaceTarget::Stem, ReplaceTarget::Extension] {
                                    let response = ui.selectable_value(&mut state.replace_target, target, target.label());
                                    #[cfg(feature = "visual-qa")]
                                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("rename-replace-target-option", target.label())), response.rect));
                                    if response.clicked() { preset_interaction = true; }
                                }
                            });
                        preset_interaction |= combo.response.has_focus() || combo.response.lost_focus() || combo.inner.is_some();
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("rename-replace-target-combo"), combo.response.rect));
                    });
                    ui.small(match state.replace_target {
                        crate::rename::ReplaceTarget::WholeName => "Find/replace can change the extension. Stem edits and extension case apply afterward.",
                        crate::rename::ReplaceTarget::Stem => "Find/replace preserves the original extension before optional extension case. Folder names are whole stems.",
                        crate::rename::ReplaceTarget::Extension => "The extension excludes its dot; folders skip find/replace. Case and stem edits apply afterward.",
                    });
                    ui.add_space(8.0);

                    ui.horizontal_wrapped(|ui| {
                    let trim = ui.checkbox(&mut state.trim_spaces, "Trim spaces")
                        .on_hover_text("Remove whitespace at the beginning and end of the final name");
                    preset_interaction |= trim.has_focus() || trim.lost_focus() || trim.clicked();
                    #[cfg(feature = "visual-qa")]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("rename-trim-spaces"), trim.rect));

                    let collapse = ui.checkbox(&mut state.collapse_repeats, "Collapse repeats")
                        .on_hover_text("Reduce identical consecutive punctuation, whitespace or symbols to one; keep letters and digits");
                    preset_interaction |= collapse.has_focus() || collapse.lost_focus() || collapse.clicked();
                    #[cfg(feature = "visual-qa")]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("rename-collapse-repeats"), collapse.rect));

                    });
                    ui.horizontal_wrapped(|ui| {
                        use crate::rename::NameNormalization;
                        ui.label("Unicode").on_hover_text("Normalize the complete final name after all other transforms. Some filesystems enforce their own form.");
                        for mode in [NameNormalization::Keep, NameNormalization::Nfc, NameNormalization::Nfd, NameNormalization::Nfkc, NameNormalization::Nfkd] {
                            let response = ui.selectable_value(&mut state.normalization, mode, mode.label())
                                .on_hover_text("NFC/NFD preserve canonical meaning; NFKC/NFKD also replace compatibility characters (for example ① becomes 1)");
                            preset_interaction |= response.has_focus() || response.lost_focus() || response.clicked();
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("rename-normalization", mode.label())), response.rect));
                        }
                    });

                    ui.horizontal_wrapped(|ui| {
                        let limit = ui.checkbox(&mut state.stem_limit_on, "Limit stem")
                            .on_hover_text("Keep at most this many complete Unicode characters in the final stem; retain the last extension. Folders have no extension.");
                        preset_interaction |= limit.has_focus() || limit.lost_focus() || limit.clicked();
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("rename-stem-limit"), limit.rect));
                        ui.add_enabled_ui(state.stem_limit_on, |ui| {
                            ui.add(egui::DragValue::new(&mut state.stem_limit).range(1..=255));
                            ui.label("characters (extension kept)");
                        });
                    });

                    // ── Case + numbering ────────────────────────────────────
                    ui.horizontal_wrapped(|ui| {
                        use crate::rename::CaseMode;
                        ui.label(egui::RichText::new("Case").size(11.0).color(t.text_muted));
                        for (mode, text) in [
                            (CaseMode::Keep, "Keep"),
                            (CaseMode::Lower, "lower"),
                            (CaseMode::Upper, "UPPER"),
                            (CaseMode::Capitalize, "First capital"),
                            (CaseMode::TitleWords, "Title Words"),
                        ] {
                            if ui
                                .selectable_label(
                                    state.case == mode,
                                    egui::RichText::new(text).size(12.0),
                                )
                                .clicked()
                            {
                                state.case = mode;
                                preset_interaction = true;
                            }
                        }

                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new("Extension case").size(11.0).color(t.text_muted));
                        for mode in [crate::rename::ExtensionCase::Keep, crate::rename::ExtensionCase::Lower, crate::rename::ExtensionCase::Upper] {
                            let response = ui.selectable_label(state.extension_case == mode, mode.label());
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("rename-extension-case", mode.label())), response.rect));
                            preset_interaction |= response.has_focus() || response.lost_focus();
                            if response.clicked() { state.extension_case = mode; preset_interaction = true; }
                        }
                    });
                    ui.horizontal(|ui| {
                        let numbering = ui.checkbox(&mut state.numbering_on, "Number");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("rename-numbering"), numbering.rect));
                        preset_interaction |= numbering.has_focus() || numbering.lost_focus();
                        ui.add_enabled_ui(state.numbering_on, |ui| {
                            let label = ui
                                .label(egui::RichText::new("start").size(10.0).color(t.text_muted));
                            ui.add(egui::DragValue::new(&mut state.num_start).range(-100_000..=100_000))
                                .labelled_by(label.id);
                            let label = ui
                                .label(egui::RichText::new("step").size(10.0).color(t.text_muted));
                            ui.add(egui::DragValue::new(&mut state.num_step).range(-1000..=1000))
                                .labelled_by(label.id);
                            let label =
                                ui.label(egui::RichText::new("pad").size(10.0).color(t.text_muted));
                            ui.add(egui::DragValue::new(&mut state.num_pad).range(0..=6))
                                .labelled_by(label.id);
                        });
                    });

                    ui.add_enabled_ui(state.numbering_on, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Counter position");
                            for position in [crate::rename::NumberPosition::BeforeName, crate::rename::NumberPosition::AfterName, crate::rename::NumberPosition::CounterOnly] {
                                let response = ui.selectable_value(&mut state.number_position, position, position.label());
                                let response = if position == crate::rename::NumberPosition::CounterOnly {
                                    response.on_hover_text("Replace the name, prefix and suffix with the counter; keep the extension")
                                } else { response };
                                #[cfg(feature = "visual-qa")]
                                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("rename-number-position", position.label())), response.rect));
                                preset_interaction |= response.has_focus() || response.lost_focus() || response.clicked();
                            }
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
                                    PlanStatus::Ok => (t.accent, "->"),
                                    PlanStatus::Unchanged => (t.text_muted, "="),
                                    PlanStatus::Invalid => (t.accent_red, "x"),
                                    PlanStatus::Collision => (t.accent_warning, "!"),
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
                                                        egui::RichText::new(text)
                                                            .size(12.0)
                                                            .color(color),
                                                    )
                                                    .truncate(),
                                                )
                                            },
                                        );
                                    };
                                    name_cell(ui, &p.from, t.text_secondary);
                                    let mark =
                                        ui.label(egui::RichText::new(mark).size(12.0).color(color));
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

                    if !preset_interaction
                        && !editing_preset_name
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
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

        if presets_changed {
            let snapshot = self.to_session();
            let outcome = crate::persistence::report_save(
                "Session",
                crate::session::save_with(
                    self.persistence.as_ref(),
                    &snapshot,
                    &mut self.session_gate,
                ),
            );
            if let Some(state) = &mut self.ui.modals.batch_rename {
                state.preset_status = Some(match outcome {
                    Some(crate::persistence::Committed::Durable) => {
                        Ok("Preset changes saved".into())
                    }
                    Some(crate::persistence::Committed::NotDurable) => Err(
                        "Preset changes saved, but storage reported a durability warning".into(),
                    ),
                    None => Err(
                        "Settings could not be saved; preset changes remain in this session".into(),
                    ),
                });
            }
        }

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
                        // undo stack.
                        self.push_receipt_for_top_undo(now);
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
    fn reset_rule_clears_edits_and_stale_validation_but_keeps_context_identity() {
        let mut state = BatchRenameState {
            context: context(&["Photo.JPG"], &[]),
            listing_generation: 42,
            remove_prefix: "backup_".into(),
            remove_suffix: ".old".into(),
            find: "(".into(),
            replace: "replacement".into(),
            regex_mode: true,
            ignore_case: true,
            first_only: true,
            trim_spaces: true,
            collapse_repeats: true,
            normalization: crate::rename::NameNormalization::Nfkd,
            stem_limit_on: true,
            stem_limit: 2,
            replace_target: crate::rename::ReplaceTarget::Extension,
            prefix: "prefix".into(),
            suffix: "suffix".into(),
            case: crate::rename::CaseMode::Upper,
            extension_case: crate::rename::ExtensionCase::Lower,
            numbering_on: true,
            number_position: crate::rename::NumberPosition::BeforeName,
            num_start: -9,
            num_step: -2,
            num_pad: 6,
            preset_name: "draft".into(),
            preset_selected: Some("Saved preset".into()),
            preset_status: Some(Err("old status".into())),
            error: Some("old apply error".into()),
            scroll_nonce: 17,
            validation: None,
        };
        state.validation = Some(ValidatedBatchRename::build(&state.context, state.rule()));
        assert!(state.validation.as_ref().unwrap().regex_error.is_some());
        state.reset_rule();
        assert_eq!(state.rule(), RenameRule::default());
        assert_eq!((state.num_start, state.num_step, state.num_pad), (1, 1, 2));
        assert_eq!(state.listing_generation, 42);
        assert_eq!(state.scroll_nonce, 17);
        assert_eq!(state.context.targets, vec!["Photo.JPG"]);
        assert!(state.preset_name.is_empty() && state.preset_selected.is_none());
        assert!(state.validation.is_none() && state.error.is_none());
        assert_eq!(state.preset_status, Some(Ok("Rule reset".into())));
        let rule = state.rule();
        let validation = ValidatedBatchRename::cached(&mut state.validation, &state.context, rule);
        assert_eq!(validation.plans[0].to, "Photo.JPG");
        assert!(validation.rule_for_submit(true).is_none());
    }

    #[test]
    fn stem_limit_rebuilds_cached_submission_preview() {
        let context = context(&["abcdef.txt"], &[]);
        let mut cached = None;
        let rule = RenameRule {
            stem_limit: Some(2),
            ..Default::default()
        };
        let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
        assert_eq!(validation.plans[0].to, "ab.txt");
        assert_eq!(validation.rule_for_submit(true), Some(rule));
        let validation = ValidatedBatchRename::cached(&mut cached, &context, RenameRule::default());
        assert_eq!(validation.plans[0].to, "abcdef.txt");
        assert!(validation.rule_for_submit(true).is_none());
    }

    #[test]
    fn removing_affixes_rebuilds_cached_submission_preview() {
        let context = context(&["backup_photo.jpg.old"], &[]);
        let mut cached = None;
        let rule = RenameRule {
            remove_prefix: "backup_".into(),
            remove_suffix: ".old".into(),
            ..Default::default()
        };
        let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
        assert_eq!(validation.plans[0].to, "photo.jpg");
        assert_eq!(validation.rule_for_submit(true), Some(rule));
        let validation = ValidatedBatchRename::cached(&mut cached, &context, RenameRule::default());
        assert_eq!(validation.plans[0].to, "backup_photo.jpg.old");
        assert!(validation.rule_for_submit(true).is_none());
    }

    #[test]
    fn normalization_rebuilds_cached_submission_preview() {
        let context = context(&["cafe\u{301}.txt"], &[]);
        let mut cached = None;
        let rule = RenameRule {
            normalization: crate::rename::NameNormalization::Nfc,
            ..Default::default()
        };
        let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
        assert_eq!(validation.plans[0].to, "café.txt");
        assert_eq!(validation.rule_for_submit(true), Some(rule));
        let validation = ValidatedBatchRename::cached(&mut cached, &context, RenameRule::default());
        assert_eq!(validation.plans[0].to, "cafe\u{301}.txt");
        assert!(validation.rule_for_submit(true).is_none());
    }

    #[test]
    fn collapse_repeats_rebuilds_cached_submission_preview() {
        let context = context(&["report___final--v22.txt"], &[]);
        let mut cached = None;
        let rule = RenameRule {
            collapse_repeats: true,
            ..Default::default()
        };
        let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
        assert_eq!(validation.plans[0].to, "report_final-v22.txt");
        assert_eq!(validation.rule_for_submit(true), Some(rule));
        let validation = ValidatedBatchRename::cached(&mut cached, &context, RenameRule::default());
        assert_eq!(validation.plans[0].to, "report___final--v22.txt");
        assert!(validation.rule_for_submit(true).is_none());
    }

    #[test]
    fn trim_spaces_rebuilds_preview_and_round_trips_state() {
        let context = context(&[" Photo.JPG "], &[]);
        let mut cached = None;
        let rule = RenameRule {
            trim_spaces: true,
            ..Default::default()
        };
        let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
        assert_eq!(validation.plans[0].to, "Photo.JPG");
        assert_eq!(validation.rule_for_submit(true), Some(rule));
        let validation = ValidatedBatchRename::cached(&mut cached, &context, RenameRule::default());
        assert_eq!(validation.plans[0].to, " Photo.JPG ");
        assert!(validation.rule_for_submit(true).is_none());
    }

    #[test]
    fn number_position_rebuilds_cached_plan_before_submission() {
        let context = context(&["Photo.JPG"], &[]);
        let mut cached = None;
        for (number_position, expected) in [
            (crate::rename::NumberPosition::BeforeName, "03Photo.JPG"),
            (crate::rename::NumberPosition::AfterName, "Photo03.JPG"),
            (crate::rename::NumberPosition::CounterOnly, "03.JPG"),
        ] {
            let rule = RenameRule {
                number_position,
                numbering: Some(crate::rename::Numbering {
                    start: 3,
                    step: 1,
                    pad: 2,
                }),
                ..Default::default()
            };
            let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
            assert_eq!(validation.plans[0].to, expected);
            assert_eq!(validation.rule_for_submit(true), Some(rule));
        }
    }

    #[test]
    fn signed_numbering_rebuilds_cached_preview_and_preserves_step_in_state() {
        let context = context(&["item.txt"], &[]);
        let mut cached = None;
        for (start, step, expected) in [
            (3, -2, "item03.txt"),
            (-1, -2, "item-1.txt"),
            (-1, 0, "item-1.txt"),
        ] {
            let rule = RenameRule {
                numbering: Some(crate::rename::Numbering {
                    start,
                    step,
                    pad: 2,
                }),
                ..Default::default()
            };
            let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
            assert_eq!(validation.plans[0].to, expected);
            assert_eq!(validation.rule_for_submit(true), Some(rule));
        }
    }

    #[test]
    fn extension_case_rebuilds_cached_plan_before_submit() {
        let context = context(&["Photo.JPG"], &[]);
        let mut cached = None;
        for (extension_case, expected) in [
            (crate::rename::ExtensionCase::Keep, "Photo.JPG"),
            (crate::rename::ExtensionCase::Lower, "Photo.jpg"),
            (crate::rename::ExtensionCase::Upper, "Photo.JPG"),
        ] {
            let rule = RenameRule {
                extension_case,
                ..Default::default()
            };
            let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
            assert_eq!(validation.plans[0].to, expected);
            assert_eq!(
                validation.rule_for_submit(true),
                if extension_case == crate::rename::ExtensionCase::Lower {
                    Some(rule)
                } else {
                    None
                }
            );
        }
    }

    #[test]
    fn capitalization_changes_rebuild_cached_preview_before_submission() {
        let context = context(&["hELLO wORLD.TXT"], &[]);
        let mut cache = None;
        for (case, expected) in [
            (crate::rename::CaseMode::Capitalize, "Hello world.TXT"),
            (crate::rename::CaseMode::TitleWords, "Hello World.TXT"),
            (crate::rename::CaseMode::Keep, "hELLO wORLD.TXT"),
        ] {
            let rule = RenameRule {
                case,
                ..Default::default()
            };
            let plan = ValidatedBatchRename::cached(&mut cache, &context, rule.clone());
            assert_eq!(plan.plans[0].to, expected);
            assert_eq!(
                plan.rule_for_submit(true),
                if case == crate::rename::CaseMode::Keep {
                    None
                } else {
                    Some(rule)
                }
            );
        }
    }

    #[test]
    fn replacement_options_rebuild_the_cached_plan_before_submit() {
        let context = context(&["TXT-TXT.txt"], &[]);
        let mut rule = RenameRule {
            find: "txt".into(),
            replace: "doc".into(),
            ..Default::default()
        };
        let mut slot = None;
        assert_eq!(
            ValidatedBatchRename::cached(&mut slot, &context, rule.clone()).plans[0].to,
            "TXT-TXT.doc"
        );
        rule.ignore_case = true;
        assert_eq!(
            ValidatedBatchRename::cached(&mut slot, &context, rule.clone()).plans[0].to,
            "doc-doc.doc"
        );
        rule.first_only = true;
        let plan = ValidatedBatchRename::cached(&mut slot, &context, rule.clone());
        assert_eq!(plan.plans[0].to, "doc-TXT.txt");
        assert_eq!(plan.rule_for_submit(true), Some(rule));
    }

    #[test]
    fn replacement_target_rebuilds_preview_and_prevents_submitting_the_previous_scope() {
        let context = context(&["item.txt"], &[]);
        let mut rule = RenameRule {
            find: "txt".into(),
            replace: "log".into(),
            ..Default::default()
        };
        let mut cache = None;
        let whole = ValidatedBatchRename::cached(&mut cache, &context, rule.clone());
        assert_eq!(whole.plans[0].to, "item.log");
        assert!(whole.rule_for_submit(true).is_some());
        rule.replace_target = crate::rename::ReplaceTarget::Stem;
        let stem = ValidatedBatchRename::cached(&mut cache, &context, rule.clone());
        assert_eq!(stem.plans[0].to, "item.txt");
        assert!(stem.rule_for_submit(true).is_none());
        rule.replace_target = crate::rename::ReplaceTarget::Extension;
        let extension = ValidatedBatchRename::cached(&mut cache, &context, rule.clone());
        assert_eq!(extension.plans[0].to, "item.log");
        assert_eq!(extension.rule_for_submit(true), Some(rule));
    }

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
            folding: crate::rename_order::NameFolding::CaseFolded,
            targets: vec!["a.txt".to_string()],
            dirs: std::collections::HashSet::new(),
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

    fn context(targets: &[&str], siblings: &[&str]) -> BatchRenameContext {
        BatchRenameContext {
            panel: ActivePanel::Left,
            dir: PathBuf::from("/fixture"),
            folding: crate::rename_order::NameFolding::CaseFolded,
            targets: targets.iter().map(ToString::to_string).collect(),
            dirs: std::collections::HashSet::new(),
            existing: targets
                .iter()
                .chain(siblings)
                .map(ToString::to_string)
                .collect(),
        }
    }

    #[test]
    fn chained_rename_onto_a_name_being_vacated_is_applicable() {
        // "f" -> "f1" lands on a name that "f1" -> "f12" vacates.
        let rule = RenameRule {
            numbering: Some(crate::rename::Numbering {
                start: 1,
                step: 1,
                pad: 0,
            }),
            ..RenameRule::default()
        };
        let validation = ValidatedBatchRename::build(&context(&["f", "f1"], &[]), rule);
        assert_eq!(validation.will_change, 2);
        assert!(validation.applicable);
        assert_eq!(validation.block_reason, None);
    }

    #[test]
    fn no_op_rule_is_not_applicable_and_gives_no_reason() {
        let validation =
            ValidatedBatchRename::build(&context(&["a.txt", "b.txt"], &[]), RenameRule::default());
        assert_eq!(validation.will_change, 0);
        assert!(!validation.applicable);
        assert_eq!(validation.block_reason, None);
        assert_eq!(validation.rule_for_submit(true), None);
    }

    #[test]
    fn collisions_block_with_the_conflicting_name() {
        let to_b = RenameRule {
            find: "a".to_string(),
            replace: "b".to_string(),
            ..RenameRule::default()
        };
        // Onto a batch member that keeps its name.
        let batch = ValidatedBatchRename::build(&context(&["a.md", "b.md"], &[]), to_b.clone());
        assert!(!batch.applicable);
        assert!(
            batch
                .block_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("b.md")),
            "{:?}",
            batch.block_reason
        );
        // Onto an untouched sibling, compared case-insensitively.
        let sibling = ValidatedBatchRename::build(&context(&["a.md"], &["B.MD"]), to_b);
        assert!(!sibling.applicable);
        assert!(sibling.block_reason.is_some());
    }

    #[test]
    fn invalid_target_names_block_before_ordering() {
        let rule = RenameRule {
            find: "a".to_string(),
            replace: "/".to_string(),
            ..RenameRule::default()
        };
        let validation = ValidatedBatchRename::build(&context(&["a.txt"], &[]), rule);
        assert!(!validation.applicable);
        assert_eq!(
            validation.block_reason.as_deref(),
            Some("Fix the invalid names to continue")
        );
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
            folding: crate::rename_order::NameFolding::CaseFolded,
            targets: vec!["a.txt".to_string(), "b.txt".to_string()],
            dirs: std::collections::HashSet::new(),
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
    fn listing_refresh_adds_new_sibling_collision_to_preview() {
        let mut context = context(&["a.md"], &[]);
        let rule = RenameRule {
            find: "a".to_string(),
            replace: "b".to_string(),
            ..RenameRule::default()
        };
        assert!(ValidatedBatchRename::build(&context, rule.clone()).applicable);

        let mut generation = 4;
        assert!(refresh_existing_names(
            &mut context,
            &mut generation,
            std::path::Path::new("/fixture"),
            5,
            std::collections::HashSet::from(["a.md".to_string(), "b.md".to_string()]),
        ));
        let refreshed = ValidatedBatchRename::build(&context, rule);
        assert!(!refreshed.applicable);
        assert_eq!(generation, 5);
        assert_eq!(refreshed.plans[0].status, PlanStatus::Collision);
    }

    #[test]
    fn listing_refresh_never_retargets_a_different_directory() {
        let mut context = context(&["a.md"], &[]);
        let original = context.existing.clone();
        let mut generation = 4;

        assert!(!refresh_existing_names(
            &mut context,
            &mut generation,
            std::path::Path::new("/elsewhere"),
            5,
            std::collections::HashSet::from(["b.md".to_string()]),
        ));
        assert_eq!(context.existing, original);
        assert_eq!(generation, 4);
    }

    #[test]
    fn same_frame_invalid_edit_cannot_submit_under_previous_validation() {
        let context = BatchRenameContext {
            panel: ActivePanel::Left,
            dir: PathBuf::from("/fixture"),
            folding: crate::rename_order::NameFolding::CaseFolded,
            targets: vec!["report.txt".to_string()],
            dirs: std::collections::HashSet::new(),
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
