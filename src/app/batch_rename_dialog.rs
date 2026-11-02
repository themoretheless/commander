//! Batch-rename studio: a modal sheet that edits a rename rule and shows a
//! live old -> new preview with collisions flagged. All planning lives in
//! `crate::rename`; this file is only the egui shell.

use super::*;
use crate::rename::{
    PlanStatus, RenamePlan, RenameRule, plan_batch_rename_in_folder, plan_is_applicable,
};
use crate::rename_order::{RenameOrder, safe_rename_order_with};
use crate::workspace::BatchRenameContext;

const PREVIEW_SCROLL_ID: &str = "batch_rename_preview";

/// Tooltip for a flagged preview row, so the glyph says why it is flagged.
/// Return persistence and interaction changes separately so Enter on a focused
/// ordering button cannot submit the rename operation underneath it.
fn preset_order_controls(
    ui: &mut egui::Ui,
    presets: &mut crate::rename_presets::Presets,
    selected: Option<&str>,
) -> (bool, bool) {
    let mut changed = false;
    let mut interaction = false;
    ui.horizontal_wrapped(|ui| {
        let position = selected.and_then(|name| presets.position(name));
        for (id, label, enabled) in [
            (
                "rename-preset-up",
                "Preset up",
                position.is_some_and(|index| index > 0),
            ),
            (
                "rename-preset-down",
                "Preset down",
                position.is_some_and(|index| index + 1 < presets.items().len()),
            ),
            (
                "rename-preset-sort-asc",
                "Sort A–Z",
                presets.items().len() > 1,
            ),
            (
                "rename-preset-sort-desc",
                "Sort Z–A",
                presets.items().len() > 1,
            ),
        ] {
            let response = ui.add_enabled(enabled, egui::Button::new(label));
            #[cfg(feature = "visual-qa")]
            ui.ctx()
                .data_mut(|data| data.insert_temp(egui::Id::new(id), response.rect));
            interaction |= response.has_focus() || response.lost_focus() || response.clicked();
            if response.clicked() {
                changed |= match id {
                    "rename-preset-up" => {
                        selected.is_some_and(|name| presets.move_selected(name, false))
                    }
                    "rename-preset-down" => {
                        selected.is_some_and(|name| presets.move_selected(name, true))
                    }
                    "rename-preset-sort-asc" => presets.sort_names(false),
                    _ => presets.sort_names(true),
                };
            }
        }
    });
    (changed, interaction)
}

fn literal_replacement_control(
    ui: &mut egui::Ui,
    regex: bool,
    literal: &mut bool,
) -> egui::Response {
    let response = ui.add_enabled(regex, egui::Checkbox::new(literal, "Literal replacement"))
        .on_hover_text("With regex search, insert replacement text exactly: $1, ${name} and $$ stay literal. Applies to every replacement pair. Disable to expand capture groups.");
    #[cfg(feature = "visual-qa")]
    ui.ctx().data_mut(|data| {
        data.insert_temp(egui::Id::new("rename-literal-replacement"), response.rect)
    });
    response
}

fn modified_saved_preset(
    presets: &crate::rename_presets::Presets,
    selected: Option<&str>,
    rule: &RenameRule,
) -> bool {
    selected
        .and_then(|name| presets.get(name))
        .is_some_and(|preset| preset.rule != *rule)
}

fn status_hint(status: PlanStatus) -> Option<&'static str> {
    match status {
        PlanStatus::Ok | PlanStatus::Unchanged => None,
        PlanStatus::ExpansionLimit => Some("Replacement output exceeds 64 KiB"),
        PlanStatus::Invalid => Some("Name is not valid"),
        PlanStatus::FolderUnavailable => {
            Some("Folder level is unavailable or separator is invalid")
        }
        PlanStatus::DateUnavailable => {
            Some("Captured modification time is missing or the UTC date format is invalid")
        }
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
    pub fn metadata_snapshot(&self) -> serde_json::Value {
        serde_json::json!({"rule":self.rule,"applicable":self.applicable,"plans":self.plans.iter().map(|plan|serde_json::json!({"from":plan.from,"to":plan.to,"status":format!("{:?}",plan.status)})).collect::<Vec<_>>()})
    }

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
    pub fn targets_preview(&self, expected: &[(&str, &str)]) -> bool {
        self.applicable
            && self.regex_error.is_none()
            && self.plans.len() == expected.len()
            && self
                .plans
                .iter()
                .zip(expected)
                .all(|(plan, (from, to))| plan.from == *from && plan.to == *to)
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
        let plans = plan_batch_rename_in_folder(
            &context.targets,
            &context.existing,
            &rule,
            &context.dirs,
            context.folding,
            &context.modified,
            Some(&context.dir),
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
        } else if plans
            .iter()
            .any(|plan| plan.status == PlanStatus::ExpansionLimit)
        {
            (
                false,
                Some("Reduce the replacement expansion (64 KiB per stage)".into()),
            )
        } else if plans
            .iter()
            .any(|plan| plan.status == PlanStatus::FolderUnavailable)
        {
            (
                false,
                Some("Fix the folder level or separator to continue".into()),
            )
        } else if plans
            .iter()
            .any(|plan| plan.status == PlanStatus::DateUnavailable)
        {
            (
                false,
                Some("Fix the date format or reopen with available modification times".into()),
            )
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

fn reorder_targets(
    context: &mut BatchRenameContext,
    validation: &mut Option<ValidatedBatchRename>,
    error: &mut Option<String>,
    from: usize,
    to: usize,
) -> bool {
    if from == to || from >= context.targets.len() || to >= context.targets.len() {
        return false;
    }
    context.targets.swap(from, to);
    *validation = None;
    *error = None;
    true
}

fn exclude_target(
    context: &mut BatchRenameContext,
    validation: &mut Option<ValidatedBatchRename>,
    error: &mut Option<String>,
    index: usize,
) -> bool {
    if index >= context.targets.len() {
        return false;
    }
    let name = context.targets.remove(index);
    context.dirs.remove(&name);
    context.modified.remove(&name);
    // This sibling stays on disk: retain it in existing to detect collisions
    // with a target that previously could have relied on its vacated name.
    *validation = None;
    *error = None;
    true
}

impl App {
    pub(crate) fn open_batch_rename(&mut self, ctx: &egui::Context) {
        let Some(context) = self.ws.batch_rename_context() else {
            return;
        };
        let listing_generation = self.ws.active_panel_ref().entries_gen();
        let scroll_nonce = self.issue_transient_nonce();
        self.ui.modals.batch_rename = Some(BatchRenameState {
            confirm_preset_discard: false,
            parent_on: false,
            parent_depth: 1,
            parent_separator: "_".into(),
            parent_suffix: false,
            date_on: false,
            date_format: crate::rename::DateStamp::default().format,
            date_position: crate::rename::DatePosition::Prefix,
            context,
            listing_generation,
            remove_prefix: String::new(),
            remove_suffix: String::new(),
            find: String::new(),
            replace: String::new(),
            extra_replacements: Vec::new(),
            regex_mode: false,
            ignore_case: false,
            first_only: false,
            literal_replacement: false,
            trim_spaces: false,
            collapse_repeats: false,
            normalization: crate::rename::NameNormalization::Keep,
            character_target: crate::rename::CharacterTarget::Stem,
            insert_text_on: false,
            insert_text: String::new(),
            insert_offset: 0,
            insert_from_end: false,
            insert_overwrite: false,
            range_action: crate::rename::RangeAction::Remove,
            remove_range_on: false,
            remove_offset: 0,
            remove_count: 1,
            remove_from_end: false,
            stem_limit_on: false,
            stem_limit: 32,
            replace_target: crate::rename::ReplaceTarget::WholeName,
            prefix: String::new(),
            suffix: String::new(),
            case: crate::rename::CaseMode::Keep,
            extension_case: crate::rename::ExtensionCase::Keep,
            numbering_on: false,
            number_format: crate::rename::NumberFormat::Decimal,
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
        if let (Some(state), Some(preset)) = (
            self.ui.modals.batch_rename.as_mut(),
            self.rename_presets.launch_preset(),
        ) {
            state.load_launch_preset(preset);
        }
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
        let mut discard_confirmed = false;
        let mut move_target = None;
        let mut remove_target = None;
        let mut copy_format = None;
        let mut clipboard_payload = None;
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
                    ui.horizontal_wrapped(|ui| {
                        for (duplicate, label, hint) in [
                            (false, "Rename selected", "Use the name above; preserve the saved rule and current editor changes"),
                            (true, "Duplicate saved", "Copy the selected saved rule under the name above; unsaved editor changes are not copied"),
                        ] {
                            let response = ui.add_enabled(state.preset_selected.is_some(), egui::Button::new(label)).on_hover_text(hint);
                            preset_interaction |= response.has_focus() || response.lost_focus() || response.clicked();
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(if duplicate { "rename-preset-duplicate" } else { "rename-preset-rename" }), response.rect));
                            if response.clicked() {
                                let source = state.preset_selected.as_deref().unwrap();
                                let result = if duplicate { presets.duplicate(source, &state.preset_name) } else { presets.rename(source, &state.preset_name) };
                                if let Ok(name) = &result {
                                    // Keep selection on the source for duplication: the editor may
                                    // contain unsaved changes belonging to that source.
                                    if !duplicate { state.preset_selected = Some(name.clone()); }
                                    state.preset_name = name.clone();
                                    presets_changed = true;
                                }
                                state.preset_status = Some(result.map(|_| if duplicate { "Saved rule duplicated; current editor retained".into() } else { "Preset renamed; current editor retained".into() }));
                            }
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!("At launch: {}", presets.launch_preset().map(|preset| preset.name.as_str()).unwrap_or("Empty rule")));
                        let set = ui.add_enabled(state.preset_selected.is_some(), egui::Button::new("Use selected at launch"));
                        let clear = ui.add_enabled(presets.launch_preset().is_some(), egui::Button::new("Start empty"));
                        preset_interaction |= set.has_focus() || set.lost_focus() || set.clicked() || clear.has_focus() || clear.lost_focus() || clear.clicked();
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| {
                            data.insert_temp(egui::Id::new("rename-preset-launch"), set.rect);
                            data.insert_temp(egui::Id::new("rename-preset-launch-clear"), clear.rect);
                        });
                        if set.clicked() || clear.clicked() {
                            let result = presets.set_launch(if clear.clicked() { None } else { state.preset_selected.as_deref() });
                            presets_changed |= result.is_ok();
                            state.preset_status = Some(result.map(|()| "Launch preference saved; current editor retained".into()));
                        }
                    });
                    if state.confirm_preset_discard {
                        if modified_saved_preset(presets, state.preset_selected.as_deref(), &state.rule()) {
                            ui.colored_label(t.accent_warning, "The selected saved rule has unsaved changes.");
                            ui.horizontal_wrapped(|ui| {
                                let keep = ui.button("Keep editing");
                                let discard = ui.button("Discard edits and close");
                                #[cfg(feature="visual-qa")]
                                ui.ctx().data_mut(|data| { data.insert_temp(egui::Id::new("rename-preset-keep"),keep.rect); data.insert_temp(egui::Id::new("rename-preset-discard"),discard.rect); });
                                if keep.clicked() { state.confirm_preset_discard = false; }
                                if discard.clicked() { discard_confirmed = true; cancel = true; }
                            });
                            ui.small("Use Update selected to save the edited rule before closing.");
                            preset_interaction = true;
                        } else { state.confirm_preset_discard = false; }
                    }
                    let (order_changed, order_interaction) = preset_order_controls(ui, presets, state.preset_selected.as_deref());
                    preset_interaction |= order_interaction;
                    if order_changed {
                        presets_changed = true;
                        state.preset_status = Some(Ok("Preset order saved; current editor retained".into()));
                    }
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

                    let mut text_fields = egui::Grid::new("batch_rename_fields")
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
                            #[cfg(feature="visual-qa")]
                            ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new("rename-prefix"),prefix.rect));
                            let suffix = field(ui, "Suffix", &mut state.suffix, "after name");
                            ui.end_row();
                            let remove_prefix = field(ui, "Remove prefix", &mut state.remove_prefix, "exact original start");
                            let remove_suffix = field(ui, "Remove suffix", &mut state.remove_suffix, "exact original end");
                            ui.end_row();
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("rename-remove-prefix"), remove_prefix.rect));
                            [first.id, replace.id, prefix.id, suffix.id, remove_prefix.id, remove_suffix.id]
                        })
                        .inner.to_vec();

                    let add = ui.add_enabled(state.extra_replacements.len() < crate::rename::MAX_EXTRA_REPLACEMENTS, egui::Button::new("Add replacement").small())
                        .on_hover_text("Run another Find/Replace after the previous pair, before case/numbering. All pairs share the matching options and target below; at most 16 additional pairs.");
                    #[cfg(feature = "visual-qa")]
                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("rename-add-replacement"), add.rect));
                    let added = add.clicked();
                    if added { state.extra_replacements.push(crate::rename::ReplacementPair::default()); preset_interaction = true; }
                    let pair_count = state.extra_replacements.len();
                    let mut remove = None;
                    let mut move_pair = None;
                    for (index, pair) in state.extra_replacements.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            let label = ui.small(format!("Then {}", index + 1));
                            let find = ui.add(egui::TextEdit::singleline(&mut pair.find).id(egui::Id::new(("rename-extra-find", index))).desired_width(140.0).hint_text("Find (required)")).labelled_by(label.id);
                            let arrow = ui.label("to");
                            let replace = ui.add(egui::TextEdit::singleline(&mut pair.replace).id(egui::Id::new(("rename-extra-replace", index))).desired_width(140.0).hint_text("Replace")).labelled_by(arrow.id);
                            text_fields.extend([find.id, replace.id]);
                            if added && index + 1 == pair_count { find.request_focus(); }
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| { data.insert_temp(egui::Id::new(("rename-extra-find", index)), find.rect); data.insert_temp(egui::Id::new(("rename-extra-replace", index)), replace.rect); });
                            let up = ui.add_enabled(index > 0, egui::Button::new("Up").small()).on_hover_text("Run this replacement before the previous pair");
                            let down = ui.add_enabled(index + 1 < pair_count, egui::Button::new("Down").small()).on_hover_text("Run this replacement after the next pair");
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| { data.insert_temp(egui::Id::new(("rename-extra-up", index)), up.rect); data.insert_temp(egui::Id::new(("rename-extra-down", index)), down.rect); });
                            if up.clicked() { move_pair = Some((index, index - 1)); preset_interaction = true; }
                            if down.clicked() { move_pair = Some((index, index + 1)); preset_interaction = true; }
                            let delete = ui.small_button("Remove");
                            if delete.clicked() { remove = Some(index); preset_interaction = true; }
                        });
                    }
                    if let Some(index) = remove { state.extra_replacements.remove(index); }
                    else if let Some((from, to)) = move_pair { state.extra_replacements.swap(from, to); }
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        let regex = ui.checkbox(&mut state.regex_mode, "Find is a regex");
                        let case = ui.checkbox(&mut state.ignore_case, "Ignore case");
                        let first = ui.checkbox(&mut state.first_only, "First match only");
                        let literal = literal_replacement_control(ui, state.regex_mode, &mut state.literal_replacement);
                        preset_interaction |= regex.changed() || case.changed() || first.changed() || literal.changed();
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
                        use crate::rename::CharacterTarget;
                        ui.label("Character edits").on_hover_text("Scope of range edits and Insert text; independent of find/replace. Extension excludes its dot and skips folders.");
                        for target in [CharacterTarget::Stem, CharacterTarget::Extension, CharacterTarget::WholeName] {
                            let response = ui.selectable_value(&mut state.character_target, target, target.label());
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("rename-character-target", target.label())), response.rect));
                            preset_interaction |= response.has_focus() || response.clicked();
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        let enabled = ui.checkbox(&mut state.remove_range_on, "Range")
                            .on_hover_text("Remove a range of whole Unicode graphemes, or keep only that range; applies to the chosen character-edit scope before insertion, case and decoration");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("rename-remove-range"), enabled.rect));
                        preset_interaction |= enabled.has_focus() || enabled.clicked();
                        ui.add_enabled_ui(state.remove_range_on, |ui| {
                            for action in [crate::rename::RangeAction::Remove, crate::rename::RangeAction::Keep] {
                                let response = ui.selectable_value(&mut state.range_action, action, action.label());
                                #[cfg(feature = "visual-qa")]
                                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("rename-range-action", action.label())), response.rect));
                                preset_interaction |= response.has_focus() || response.clicked();
                            }
                            ui.label("skip");
                            ui.add(egui::DragValue::new(&mut state.remove_offset).range(0..=255));
                            ui.label("count");
                            ui.add(egui::DragValue::new(&mut state.remove_count).range(1..=255));
                            let edge = ui.checkbox(&mut state.remove_from_end, "from end").on_hover_text("Skip from the end of the chosen component, then count backwards; shorter names clamp and an out-of-range selection is empty");
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("rename-remove-range-end"), edge.rect));
                            preset_interaction |= edge.has_focus() || edge.clicked();
                        });
                    });
                    ui.horizontal_wrapped(|ui| {
                        let enabled = ui.checkbox(&mut state.insert_text_on, "Insert text")
                            .on_hover_text("Insert into the chosen character-edit scope after range removal, before case and decoration");
                        #[cfg(feature = "visual-qa")]
                        ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("rename-insert-enabled"), enabled.rect));
                        preset_interaction |= enabled.has_focus() || enabled.clicked();
                        ui.add_enabled_ui(state.insert_text_on, |ui| {
                            let edit = ui.add(egui::TextEdit::singleline(&mut state.insert_text).desired_width(110.0).hint_text("literal text"));
                            text_fields.push(edit.id);
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new("rename-insert-text"), edit.rect));
                            ui.label("at");
                            ui.add(egui::DragValue::new(&mut state.insert_offset).range(0..=255));
                            let edge = ui.checkbox(&mut state.insert_from_end, "from end");
                            let overwrite = ui.checkbox(&mut state.insert_overwrite, "Overwrite").on_hover_text("Replace following whole graphemes equal to the inserted text length; reaching the end appends the rest");
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| { data.insert_temp(egui::Id::new("rename-insert-end"), edge.rect); data.insert_temp(egui::Id::new("rename-insert-overwrite"), overwrite.rect); });
                            preset_interaction |= edge.has_focus() || edge.clicked() || overwrite.has_focus() || overwrite.clicked();
                        });
                    });
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

                    ui.horizontal_wrapped(|ui| {
                        let enabled=ui.checkbox(&mut state.date_on,"Modified date (UTC)");
                        #[cfg(feature="visual-qa")]
                        ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new("rename-date-on"),enabled.rect));
                        preset_interaction|=enabled.changed();
                        ui.add_enabled_ui(state.date_on,|ui| {
                            let input=ui.add(egui::TextEdit::singleline(&mut state.date_format).char_limit(64).desired_width(180.0)).on_hover_text("strftime: %Y-%m-%d_, %H-%M-%S. Uses each item's modification time from the opening listing, always UTC; does not reread metadata.");
                            text_fields.push(input.id);
                            #[cfg(feature="visual-qa")]
                            ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new("rename-date-format"),input.rect));
                            preset_interaction|=input.changed();
                            for (position,label) in [(crate::rename::DatePosition::Prefix,"Date prefix"),(crate::rename::DatePosition::Suffix,"Date suffix")] {
                                preset_interaction|=ui.selectable_value(&mut state.date_position,position,label).changed();
                            }
                        });
                    });
                    ui.horizontal_wrapped(|ui| {
                        let enabled = ui.checkbox(&mut state.parent_on, "Folder name").on_hover_text("Level 1 is the containing folder; level 2 is its parent. Uses the captured folder path without resolving links.");
                        #[cfg(feature="visual-qa")]
                        ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new("rename-parent-on"),enabled.rect));
                        preset_interaction |= enabled.changed();
                        ui.add_enabled_ui(state.parent_on, |ui| {
                            ui.label("level");
                            preset_interaction |= ui.add(egui::DragValue::new(&mut state.parent_depth).range(1..=32)).changed();
                            ui.label("separator");
                            let input = ui.add(egui::TextEdit::singleline(&mut state.parent_separator).char_limit(32).desired_width(70.0));
                            text_fields.push(input.id);
                            #[cfg(feature="visual-qa")]
                            ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new("rename-parent-separator"),input.rect));
                            preset_interaction |= input.changed();
                            preset_interaction |= ui.selectable_value(&mut state.parent_suffix, false, "Folder prefix").changed();
                            let suffix=ui.selectable_value(&mut state.parent_suffix, true, "Folder suffix");
                            #[cfg(feature="visual-qa")]
                            ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new("rename-parent-suffix"),suffix.rect));
                            preset_interaction |= suffix.changed();
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
                            ui.add_enabled(state.number_format == crate::rename::NumberFormat::Decimal, egui::DragValue::new(&mut state.num_pad).range(0..=6))
                                .labelled_by(label.id);
                        });
                    });

                    ui.add_enabled_ui(state.numbering_on, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            use crate::rename::NumberFormat;
                            ui.label("Counter format").on_hover_text("Letters use 1=a, 26=z, 27=aa; zero stays 0 and negatives get a minus sign. Padding applies only to Decimal.");
                            for format in [NumberFormat::Decimal, NumberFormat::LowerLetters, NumberFormat::UpperLetters] {
                                let response = ui.selectable_value(&mut state.number_format, format, format.label());
                                #[cfg(feature = "visual-qa")]
                                ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("rename-number-format", format.label())), response.rect));
                                preset_interaction |= response.has_focus() || response.clicked();
                            }
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
                    const ROW_H: f32 = 24.0;
                    egui::ScrollArea::vertical()
                        .id_salt((PREVIEW_SCROLL_ID, state.scroll_nonce))
                        .max_height(240.0)
                        .auto_shrink([false, true])
                        .show_rows(ui, ROW_H, validation.plans.len(), |ui, range| {
                            for index in range {
                                let p = &validation.plans[index];
                                let (color, mark) = match p.status {
                                    PlanStatus::Ok => (t.accent, "->"),
                                    PlanStatus::Unchanged => (t.text_muted, "="),
                                    PlanStatus::ExpansionLimit => (t.accent_red, "x"),
                                    PlanStatus::Invalid | PlanStatus::DateUnavailable | PlanStatus::FolderUnavailable => (t.accent_red, "x"),
                                    PlanStatus::Collision => (t.accent_warning, "!"),
                                };
                                ui.horizontal(|ui| {
                                    ui.set_height(ROW_H);
                                    if validation.plans.len() > 1 {
                                        let up = ui.add_enabled(index > 0, egui::Button::new("Up").small()).on_hover_text("Move this file earlier in the batch; recalculate numbering");
                                        let down = ui.add_enabled(index + 1 < validation.plans.len(), egui::Button::new("Down").small()).on_hover_text("Move this file later in the batch; recalculate numbering");
                                        #[cfg(feature = "visual-qa")]
                                        ui.ctx().data_mut(|data| { data.insert_temp(egui::Id::new(("rename-target-up", index)), up.rect); data.insert_temp(egui::Id::new(("rename-target-down", index)), down.rect); });
                                        if up.clicked() { move_target = Some((index, index - 1)); preset_interaction = true; }
                                        if down.clicked() { move_target = Some((index, index + 1)); preset_interaction = true; }
                                    }
                                    let exclude = ui.small_button("Exclude").on_hover_text("Remove this file from the rename batch; leave it untouched on disk");
                                    #[cfg(feature = "visual-qa")]
                                    ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("rename-target-exclude", index)), exclude.rect));
                                    if exclude.clicked() { remove_target = Some(index); preset_interaction = true; }
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

                    ui.horizontal_wrapped(|ui| {
                        use crate::listing_export::RenamePlanFormat;
                        for (format, label) in [(RenamePlanFormat::OriginalNames, "Copy original names"), (RenamePlanFormat::ProposedNames, "Copy proposed names"), (RenamePlanFormat::MappingCsv, "Copy mapping (CSV)")] {
                            let enabled = crate::listing_export::can_copy_rename_plan(&validation.plans, format, validation.regex_error.as_deref());
                            let response = ui.add_enabled(enabled, egui::Button::new(label).small()).on_hover_text(if format == RenamePlanFormat::MappingCsv { "Copy every current row with raw CSV-quoted names, status and any rule error; does not apply the plan" } else { "Copy every current row in displayed order, with line controls escaped; proposed names require a valid conflict-free plan" });
                            #[cfg(feature = "visual-qa")]
                            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(("rename-copy-plan", label)), response.rect));
                            if response.clicked() { copy_format = Some(format); preset_interaction = true; }
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
                        let rename = ui
                            .add_enabled(
                                validation.applicable && !state.confirm_preset_discard,
                                super::dialog_button(
                                    t,
                                    super::ButtonRole::Primary,
                                    egui::RichText::new("Rename").size(13.0),
                                ),
                            );
                        #[cfg(feature="visual-qa")]
                        ui.ctx().data_mut(|data|data.insert_temp(egui::Id::new("rename-apply"),rename.rect));
                        if rename.clicked() { submit_requested = true; }
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
                    if let Some(rule) = validation.rule_for_submit(submit_requested && !state.confirm_preset_discard && move_target.is_none() && remove_target.is_none() && copy_format.is_none()) {
                        commit = Some((rule, state.context.clone()));
                    }
                    if move_target.is_none() && remove_target.is_none()
                        && let Some(format) = copy_format
                        && let Some(text) = crate::listing_export::rename_plan(&validation.plans, format, validation.regex_error.as_deref()) {
                        clipboard_payload = Some((text, format!("{} {}", validation.plans.len(), format.label())));
                    }
                    if escape_requested {
                        cancel = true;
                    }
                    if let Some(index) = remove_target {
                        if exclude_target(&mut state.context, &mut state.validation, &mut state.error, index) {
                            cancel |= state.context.targets.is_empty();
                            ctx.request_repaint();
                        }
                    } else if let Some((from, to)) = move_target
                        && reorder_targets(&mut state.context, &mut state.validation, &mut state.error, from, to) {
                        ctx.request_repaint();
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

        if let Some((text, label)) = clipboard_payload {
            self.write_clipboard(&text, &label, ctx);
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

        if cancel
            && !discard_confirmed
            && let Some(state) = &mut self.ui.modals.batch_rename
            && modified_saved_preset(
                &self.rename_presets,
                state.preset_selected.as_deref(),
                &state.rule(),
            )
        {
            state.confirm_preset_discard = true;
            ctx.request_repaint();
            return;
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
    fn literal_replacement_pointer_input_updates_same_frame_plan_and_retains_disabled_choice() {
        let ctx = egui::Context::default();
        let context = context(&["old.txt"], &[]);
        let mut cache = None;
        let mut rule = RenameRule {
            find: "(old)".into(),
            replace: "$1".into(),
            regex: true,
            ..Default::default()
        };
        let mut frame = |regex: bool, events: Vec<egui::Event>| {
            rule.regex = regex;
            let mut rect = egui::Rect::NOTHING;
            let mut changed = false;
            let mut target = String::new();
            let mut submitted = None;
            crate::testutil::discard_egui_output(ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 300.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let response =
                        literal_replacement_control(ui, rule.regex, &mut rule.literal_replacement);
                    rect = response.rect;
                    changed = response.changed();
                    let validation =
                        ValidatedBatchRename::cached(&mut cache, &context, rule.clone());
                    target = validation.plans[0].to.clone();
                    submitted = validation.rule_for_submit(true);
                },
            ));
            (rect, changed, target, submitted, rule.literal_replacement)
        };
        let pos = frame(true, vec![]).0.center();
        for (regex, expected_changed, expected_literal, expected_target) in [
            (true, true, true, "$1.txt"),
            (false, false, true, "old.txt"),
            (true, true, false, "old.txt"),
        ] {
            frame(regex, vec![egui::Event::PointerMoved(pos)]);
            frame(
                regex,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                }],
            );
            let result = frame(
                regex,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
            );
            assert_eq!(
                (result.1, result.4, result.2.as_str()),
                (expected_changed, expected_literal, expected_target)
            );
            assert_eq!(result.3.is_some(), expected_changed && expected_literal);
            if let Some(rule) = result.3 {
                assert!(rule.literal_replacement && rule.regex);
            }
        }
    }

    #[test]
    fn literal_replacement_rebuilds_preview_and_submission() {
        let context = context(&["old.txt"], &[]);
        let mut cache = None;
        let mut rule = RenameRule {
            find: "(old)".into(),
            replace: "$1".into(),
            regex: true,
            ..Default::default()
        };
        assert_eq!(
            ValidatedBatchRename::cached(&mut cache, &context, rule.clone()).plans[0].to,
            "old.txt"
        );
        rule.literal_replacement = true;
        let plan = ValidatedBatchRename::cached(&mut cache, &context, rule.clone());
        assert_eq!(plan.plans[0].to, "$1.txt");
        assert_eq!(plan.rule_for_submit(true), Some(rule));
    }

    #[cfg(feature = "visual-qa")]
    #[test]
    fn preset_order_buttons_use_pointer_input_and_retain_launch_identity() {
        let ctx = egui::Context::default();
        let mut presets = crate::rename_presets::Presets::default();
        for name in ["Zulu", "Alpha", "Middle"] {
            presets.add(name, RenameRule::default()).unwrap();
        }
        presets.set_launch(Some("Middle")).unwrap();
        {
            let mut frame = |events: Vec<egui::Event>| {
                let mut changed = false;
                crate::testutil::discard_egui_output(ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(900.0, 300.0),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        changed = preset_order_controls(ui, &mut presets, Some("Middle")).0;
                    },
                ));
                changed
            };
            frame(vec![]);
            for id in [
                "rename-preset-up",
                "rename-preset-sort-asc",
                "rename-preset-sort-desc",
                "rename-preset-down",
            ] {
                let pos = ctx
                    .data(|data| data.get_temp::<egui::Rect>(egui::Id::new(id)))
                    .unwrap()
                    .center();
                frame(vec![egui::Event::PointerMoved(pos)]);
                frame(vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                }]);
                assert!(frame(vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default()
                }]));
            }
        }
        assert_eq!(
            presets
                .items()
                .iter()
                .map(|preset| preset.name.as_str())
                .collect::<Vec<_>>(),
            ["Zulu", "Alpha", "Middle"]
        );
        assert_eq!(presets.launch_preset().unwrap().name, "Middle");
    }

    #[test]
    fn parent_folder_preview_uses_captured_folder_and_blocks_unavailable_levels() {
        let mut context = context(&["photo.jpg"], &[]);
        context.dir = PathBuf::from("/Trips/Album");
        let mut rule = RenameRule {
            parent_affix: Some(crate::rename::ParentAffix::default()),
            ..Default::default()
        };
        let mut cached = None;
        let preview = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
        assert_eq!(preview.plans[0].to, "Album_photo.jpg");
        assert!(preview.rule_for_submit(true).is_some());
        rule.parent_affix.as_mut().unwrap().depth = 32;
        let preview = ValidatedBatchRename::cached(&mut cached, &context, rule);
        assert_eq!(preview.plans[0].status, PlanStatus::FolderUnavailable);
        assert!(preview.rule_for_submit(true).is_none());
    }

    #[test]
    fn date_preview_is_source_bound_and_blocks_missing_dates_before_submission() {
        let mut context = context(&["a.txt", "b.txt"], &[]);
        context
            .modified
            .insert("a.txt".into(), std::time::UNIX_EPOCH);
        context.modified.insert(
            "b.txt".into(),
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(86400),
        );
        let rule = RenameRule {
            date_stamp: Some(crate::rename::DateStamp::default()),
            ..Default::default()
        };
        let mut cached = None;
        let mut error = None;
        assert_eq!(
            ValidatedBatchRename::cached(&mut cached, &context, rule.clone()).plans[0].to,
            "1970-01-01_a.txt"
        );
        assert!(reorder_targets(&mut context, &mut cached, &mut error, 0, 1));
        assert_eq!(
            ValidatedBatchRename::cached(&mut cached, &context, rule.clone()).plans[0].to,
            "1970-01-02_b.txt"
        );
        context.modified.remove("a.txt");
        cached = None;
        let validation = ValidatedBatchRename::cached(&mut cached, &context, rule);
        assert!(validation.rule_for_submit(true).is_none());
        assert!(
            validation
                .block_reason
                .as_ref()
                .unwrap()
                .contains("date format")
        );
    }

    #[test]
    fn reset_rule_clears_edits_and_stale_validation_but_keeps_context_identity() {
        let mut state = BatchRenameState {
            confirm_preset_discard: false,
            parent_on: false,
            parent_depth: 1,
            parent_separator: "_".into(),
            parent_suffix: false,
            date_on: false,
            date_format: crate::rename::DateStamp::default().format,
            date_position: crate::rename::DatePosition::Prefix,
            context: context(&["Photo.JPG"], &[]),
            listing_generation: 42,
            remove_prefix: "backup_".into(),
            remove_suffix: ".old".into(),
            find: "(".into(),
            replace: "replacement".into(),
            extra_replacements: vec![crate::rename::ReplacementPair {
                find: "old".into(),
                replace: "new".into(),
            }],
            regex_mode: true,
            ignore_case: true,
            first_only: true,
            literal_replacement: true,
            trim_spaces: true,
            collapse_repeats: true,
            normalization: crate::rename::NameNormalization::Nfkd,
            character_target: crate::rename::CharacterTarget::WholeName,
            insert_text_on: true,
            insert_text: "draft".into(),
            insert_offset: 2,
            insert_from_end: true,
            insert_overwrite: true,
            range_action: crate::rename::RangeAction::Keep,
            remove_range_on: true,
            remove_offset: 2,
            remove_count: 3,
            remove_from_end: true,
            stem_limit_on: true,
            stem_limit: 2,
            replace_target: crate::rename::ReplaceTarget::Extension,
            prefix: "prefix".into(),
            suffix: "suffix".into(),
            case: crate::rename::CaseMode::Upper,
            extension_case: crate::rename::ExtensionCase::Lower,
            numbering_on: true,
            number_format: crate::rename::NumberFormat::UpperLetters,
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
        let range_rule = RenameRule {
            range_action: crate::rename::RangeAction::Keep,
            number_format: crate::rename::NumberFormat::LowerLetters,
            character_target: crate::rename::CharacterTarget::Extension,
            insert_text: Some(crate::rename::InsertText {
                text: "文".into(),
                offset: 3,
                from_end: true,
                overwrite: true,
            }),
            remove_range: Some(crate::rename::RemoveRange {
                offset: 4,
                count: 5,
                from_end: false,
            }),
            ..Default::default()
        };
        state.load_rule(&range_rule);
        assert_eq!(state.rule(), range_rule);
        assert_eq!(
            (
                state.remove_offset,
                state.remove_count,
                state.remove_from_end
            ),
            (4, 5, false)
        );
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
        let preset = crate::rename_presets::Preset {
            name: "Opening".into(),
            rule: RenameRule {
                parent_affix: Some(crate::rename::ParentAffix::default()),
                prefix: "auto_".into(),
                ..Default::default()
            },
            at_launch: true,
        };
        state.error = Some("stale".into());
        state.load_launch_preset(&preset);
        assert_eq!(state.rule(), preset.rule);
        assert_eq!(state.context.targets, vec!["Photo.JPG"]);
        assert_eq!(state.preset_selected.as_deref(), Some("Opening"));
        assert!(state.validation.is_none() && state.error.is_none());
        let rule = state.rule();
        let validation = ValidatedBatchRename::cached(&mut state.validation, &state.context, rule);
        assert_eq!(validation.plans[0].to, "fixture_auto_Photo.JPG");
        state.reset_rule();
        assert_eq!(state.rule(), RenameRule::default());
    }

    #[test]
    fn range_action_changes_refresh_cached_proposals_and_submit_rules() {
        let context = context(&["abcdef.txt"], &[]);
        let mut cached = None;
        for (action, expected) in [
            (crate::rename::RangeAction::Remove, "adef.txt"),
            (crate::rename::RangeAction::Keep, "bc.txt"),
        ] {
            let rule = RenameRule {
                range_action: action,
                remove_range: Some(crate::rename::RemoveRange {
                    offset: 1,
                    count: 2,
                    from_end: false,
                }),
                ..Default::default()
            };
            let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
            assert_eq!(validation.plans[0].to, expected);
            assert_eq!(validation.rule_for_submit(true), Some(rule));
        }
    }

    #[test]
    fn counter_format_changes_refresh_cached_proposals_and_submit_rules() {
        let context = context(&["photo.txt"], &[]);
        let mut cached = None;
        for (format, expected) in [
            (crate::rename::NumberFormat::Decimal, "27.txt"),
            (crate::rename::NumberFormat::LowerLetters, "aa.txt"),
            (crate::rename::NumberFormat::UpperLetters, "AA.txt"),
        ] {
            let rule = RenameRule {
                number_format: format,
                numbering: Some(crate::rename::Numbering {
                    start: 27,
                    step: 1,
                    pad: 2,
                }),
                number_position: crate::rename::NumberPosition::CounterOnly,
                ..Default::default()
            };
            let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
            assert_eq!(validation.plans[0].to, expected);
            assert_eq!(validation.rule_for_submit(true), Some(rule));
        }
    }

    #[test]
    fn character_target_changes_refresh_cached_proposals_and_submit_rules() {
        let context = context(&["report.txt"], &[]);
        let mut cached = None;
        for (target, expected) in [
            (crate::rename::CharacterTarget::Stem, "report_.txt"),
            (crate::rename::CharacterTarget::Extension, "report.txt_"),
            (crate::rename::CharacterTarget::WholeName, "report_.txt"),
        ] {
            let rule = RenameRule {
                character_target: target,
                insert_text: Some(crate::rename::InsertText {
                    text: "_".into(),
                    offset: 6,
                    from_end: false,
                    overwrite: false,
                }),
                ..Default::default()
            };
            let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
            assert_eq!(validation.plans[0].to, expected);
            assert_eq!(validation.rule_for_submit(true), Some(rule));
        }
    }

    #[test]
    fn insertion_rebuilds_cached_proposal_and_submission_for_each_mode() {
        let context = context(&["abcdef.txt"], &[]);
        let mut cached = None;
        for (from_end, overwrite, text, expected) in [
            (false, false, "X", "aXbcdef.txt"),
            (false, true, "Y", "aYcdef.txt"),
            (true, true, "X", "abcdeX.txt"),
        ] {
            let rule = RenameRule {
                insert_text: Some(crate::rename::InsertText {
                    text: text.into(),
                    offset: 1,
                    from_end,
                    overwrite,
                }),
                ..Default::default()
            };
            let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
            assert_eq!(validation.plans[0].to, expected);
            assert_eq!(validation.rule_for_submit(true), Some(rule));
        }
        let validation = ValidatedBatchRename::cached(&mut cached, &context, RenameRule::default());
        assert_eq!(validation.plans[0].to, "abcdef.txt");
        assert!(validation.rule_for_submit(true).is_none());
    }

    #[test]
    fn removal_range_rebuilds_preview_and_submission_after_edge_changes() {
        let context = context(&["abcdef.txt"], &[]);
        let mut cached = None;
        for (from_end, expected) in [(false, "adef.txt"), (true, "abcf.txt")] {
            let rule = RenameRule {
                remove_range: Some(crate::rename::RemoveRange {
                    offset: 1,
                    count: 2,
                    from_end,
                }),
                ..Default::default()
            };
            let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
            assert_eq!(validation.plans[0].to, expected);
            assert_eq!(validation.rule_for_submit(true), Some(rule));
        }
        let validation = ValidatedBatchRename::cached(&mut cached, &context, RenameRule::default());
        assert_eq!(validation.plans[0].to, "abcdef.txt");
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
    fn excluded_target_remains_a_sibling_and_last_exclusion_empties_the_batch() {
        let mut context = context(&["a.txt", "b.txt", "folder"], &["folder"]);
        let rule = RenameRule {
            extra_replacements: vec![
                crate::rename::ReplacementPair {
                    find: "a".into(),
                    replace: "tmp".into(),
                },
                crate::rename::ReplacementPair {
                    find: "b".into(),
                    replace: "a".into(),
                },
                crate::rename::ReplacementPair {
                    find: "tmp".into(),
                    replace: "b".into(),
                },
            ],
            ..Default::default()
        };
        let mut cache = Some(ValidatedBatchRename::build(&context, rule.clone()));
        let mut error = Some("stale".into());
        assert!(cache.as_ref().unwrap().applicable);
        assert!(!exclude_target(&mut context, &mut cache, &mut error, 3));
        assert!(cache.is_some());
        assert!(error.is_some());
        assert!(exclude_target(&mut context, &mut cache, &mut error, 0));
        assert!(cache.is_none());
        assert!(error.is_none());
        assert!(context.existing.contains("a.txt"));
        let validation = ValidatedBatchRename::cached(&mut cache, &context, rule.clone());
        assert_eq!(validation.plans[0].from, "b.txt");
        assert_eq!(validation.plans[0].to, "a.txt");
        assert_eq!(validation.plans[0].status, PlanStatus::Collision);
        assert!(validation.rule_for_submit(true).is_none());
        assert!(exclude_target(&mut context, &mut cache, &mut error, 1));
        assert!(!context.dirs.contains("folder"));
        assert!(context.existing.contains("folder"));
        assert!(exclude_target(&mut context, &mut cache, &mut error, 0));
        assert!(context.targets.is_empty());
        assert!(!ValidatedBatchRename::cached(&mut cache, &context, rule).applicable);
    }

    #[test]
    fn excluding_a_target_compacts_numbering_and_preserves_remaining_order() {
        let mut context = context(&["a.txt", "b.md", "c.txt"], &[]);
        let rule = RenameRule {
            numbering: Some(crate::rename::Numbering {
                start: 10,
                step: 2,
                pad: 2,
            }),
            number_position: crate::rename::NumberPosition::CounterOnly,
            ..Default::default()
        };
        let mut cache = Some(ValidatedBatchRename::build(&context, rule.clone()));
        let mut error = None;
        assert_eq!(cache.as_ref().unwrap().plans[2].to, "14.txt");
        assert!(exclude_target(&mut context, &mut cache, &mut error, 1));
        let rebuilt = ValidatedBatchRename::cached(&mut cache, &context, rule);
        assert_eq!(
            rebuilt
                .plans
                .iter()
                .map(|plan| (plan.from.as_str(), plan.to.as_str()))
                .collect::<Vec<_>>(),
            vec![("a.txt", "10.txt"), ("c.txt", "12.txt")]
        );
        assert!(context.existing.contains("b.md"));
        assert!(rebuilt.applicable);
    }

    #[test]
    fn reordered_targets_invalidate_cached_numbering_without_changing_membership() {
        let mut context = context(&["photo.txt", "notes.md", "folder"], &["folder"]);
        let original_existing = context.existing.clone();
        let original_dirs = context.dirs.clone();
        let rule = RenameRule {
            numbering: Some(crate::rename::Numbering {
                start: 1,
                step: 1,
                pad: 2,
            }),
            number_position: crate::rename::NumberPosition::CounterOnly,
            ..Default::default()
        };
        let mut cache = Some(ValidatedBatchRename::build(&context, rule.clone()));
        let mut error = Some("stale".into());
        assert_eq!(cache.as_ref().unwrap().plans[0].to, "01.txt");
        assert!(!reorder_targets(&mut context, &mut cache, &mut error, 0, 0));
        assert!(!reorder_targets(&mut context, &mut cache, &mut error, 3, 0));
        assert!(cache.is_some());
        assert!(error.is_some());
        assert!(reorder_targets(&mut context, &mut cache, &mut error, 0, 1));
        assert!(cache.is_none());
        assert!(error.is_none());
        let reordered = ValidatedBatchRename::cached(&mut cache, &context, rule.clone());
        assert_eq!(
            reordered
                .plans
                .iter()
                .map(|plan| (plan.from.as_str(), plan.to.as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("notes.md", "01.md"),
                ("photo.txt", "02.txt"),
                ("folder", "03")
            ]
        );
        assert_eq!(reordered.rule_for_submit(true), Some(rule.clone()));
        assert_eq!(context.existing, original_existing);
        assert_eq!(context.dirs, original_dirs);
        assert!(reorder_targets(&mut context, &mut cache, &mut error, 1, 0));
        assert_eq!(
            ValidatedBatchRename::cached(&mut cache, &context, rule).plans[0].to,
            "01.txt"
        );
    }

    #[test]
    fn reordered_replacement_chain_revalidates_preview_and_submitted_order() {
        let context = context(&["TXT.txt"], &[]);
        let mut cache = None;
        let mut rule = RenameRule {
            extra_replacements: vec![
                crate::rename::ReplacementPair {
                    find: "TXT".into(),
                    replace: "Doc".into(),
                },
                crate::rename::ReplacementPair {
                    find: "Doc".into(),
                    replace: "Final".into(),
                },
            ],
            ..Default::default()
        };
        assert_eq!(
            ValidatedBatchRename::cached(&mut cache, &context, rule.clone()).plans[0].to,
            "Final.txt"
        );
        rule.extra_replacements.swap(0, 1);
        let reversed = ValidatedBatchRename::cached(&mut cache, &context, rule.clone());
        assert_eq!(reversed.plans[0].to, "Doc.txt");
        assert_eq!(reversed.rule_for_submit(true), Some(rule.clone()));
        assert_eq!(
            serde_json::from_str::<RenameRule>(&serde_json::to_string(&rule).unwrap()).unwrap(),
            rule
        );
        rule.extra_replacements.swap(1, 0);
        let restored = ValidatedBatchRename::cached(&mut cache, &context, rule.clone());
        assert_eq!(restored.plans[0].to, "Final.txt");
        assert_eq!(restored.rule_for_submit(true), Some(rule));
    }

    #[test]
    fn replacement_chain_edits_rebuild_preview_and_block_invalid_submission() {
        let context = context(&["old-old.TXT"], &[]);
        let mut cached = None;
        let mut rule = RenameRule {
            find: "old".into(),
            replace: "tmp".into(),
            extra_replacements: vec![crate::rename::ReplacementPair {
                find: "tmp".into(),
                replace: "new".into(),
            }],
            ..Default::default()
        };
        assert_eq!(
            ValidatedBatchRename::cached(&mut cached, &context, rule.clone()).plans[0].to,
            "new-new.TXT"
        );
        rule.extra_replacements[0].replace = "final".into();
        let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
        assert_eq!(validation.plans[0].to, "final-final.TXT");
        assert_eq!(validation.rule_for_submit(true), Some(rule.clone()));
        rule.extra_replacements[0].find.clear();
        let validation = ValidatedBatchRename::cached(&mut cached, &context, rule.clone());
        assert!(validation.regex_error.is_some());
        assert!(validation.rule_for_submit(true).is_none());
        rule.extra_replacements.clear();
        assert_eq!(
            ValidatedBatchRename::cached(&mut cached, &context, rule).plans[0].to,
            "tmp-tmp.TXT"
        );
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
            modified: Default::default(),
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
            modified: Default::default(),
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
            modified: Default::default(),
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
            modified: Default::default(),
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
