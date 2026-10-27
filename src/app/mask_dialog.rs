//! Select-by-mask input: a small modal that toggles the selection by a
//! glob/extension mask. The matching logic lives in `panel`.

use super::*;

fn change_count_label(count: usize) -> String {
    if count == 1 {
        "1 change".to_string()
    } else {
        format!("{count} changes")
    }
}

/// What the live "N changes" preview depends on. The count scans every
/// filtered entry against the parsed mask, so it is recomputed only when the
/// mask text or the listing/selection behind it changes, not per repaint.
#[derive(Clone, PartialEq)]
struct MaskCountKey {
    mask: String,
    names_mode: bool,
    case_sensitive: bool,
    ignore_accents: bool,
    extensionless_star: bool,
    target: crate::panel::SelectionTarget,
    folder: PathBuf,
    entries_gen: u64,
    shown: usize,
    selected: usize,
}

const MASK_COUNT_CACHE: &str = "mask_count_cache";

fn cached_mask_count<T: Clone + Send + Sync + 'static>(
    ctx: &egui::Context,
    key: MaskCountKey,
    count: impl FnOnce() -> T,
) -> T {
    let id = egui::Id::new(MASK_COUNT_CACHE);
    if let Some((cached_key, cached)) = ctx.data(|d| d.get_temp::<(MaskCountKey, T)>(id))
        && cached_key == key
    {
        return cached;
    }
    let value = count();
    ctx.data_mut(|d| d.insert_temp(id, (key, value.clone())));
    value
}

/// Select all of the field's text (the prefilled last mask), so typing
/// replaces it while Enter reuses it.
fn select_all(ctx: &egui::Context, id: egui::Id, text: &str) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(text.chars().count()),
        )));
    state.store(ctx, id);
}

impl App {
    pub(crate) fn open_mask(&mut self, ctx: &egui::Context) {
        ctx.data_mut(|d| d.remove::<(MaskCountKey, usize)>(egui::Id::new(MASK_COUNT_CACHE)));
        ctx.data_mut(|data| {
            data.remove::<(MaskCountKey, Result<usize, String>)>(egui::Id::new(MASK_COUNT_CACHE));
            data.insert_temp(egui::Id::new("mask-names-mode"), false);
            data.insert_temp(egui::Id::new("mask-case-sensitive"), false);
            data.insert_temp(egui::Id::new("mask-ignore-accents"), false);
            data.insert_temp(egui::Id::new("mask-extensionless-star"), false);
            data.insert_temp(
                egui::Id::new("mask-selection-target"),
                crate::panel::SelectionTarget::Both,
            );
        });
        self.ui.modals.mask_input = Some(self.ui.last_mask.clone());
        Self::mark_modal_opened(ctx, UiModal::Mask);
    }

    pub(crate) fn show_mask_dialog(&mut self, ctx: &egui::Context) {
        let just_opened = Self::take_modal_opened(ctx, UiModal::Mask);
        let escape_requested = self.take_escape_request(crate::accessibility::EscapeRoute::Modal(
            crate::accessibility::ModalSurface::Mask,
        ));
        let active_panel = self.ws.active_panel_ref();
        let Some(buffer) = &mut self.ui.modals.mask_input else {
            return;
        };
        let t = self.colors;

        let mut names_mode = ctx.data(|data| {
            data.get_temp::<bool>(egui::Id::new("mask-names-mode"))
                .unwrap_or(false)
        });
        let mut case_sensitive = ctx.data(|data| {
            data.get_temp::<bool>(egui::Id::new("mask-case-sensitive"))
                .unwrap_or(false)
        });
        let mut ignore_accents = ctx.data(|data| {
            data.get_temp::<bool>(egui::Id::new("mask-ignore-accents"))
                .unwrap_or(false)
        });
        let mut extensionless_star = ctx.data(|data| {
            data.get_temp::<bool>(egui::Id::new("mask-extensionless-star"))
                .unwrap_or(false)
        });
        let mut target = ctx.data(|data| {
            data.get_temp::<crate::panel::SelectionTarget>(egui::Id::new("mask-selection-target"))
                .unwrap_or_default()
        });
        let mut controls_interacting = false;
        let mut commit = false;
        let mut cancel = false;

        let dialog_response = super::sheet_window("Select by mask", t, 14).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("Apply to");
                for choice in [
                    crate::panel::SelectionTarget::Both,
                    crate::panel::SelectionTarget::Files,
                    crate::panel::SelectionTarget::Folders,
                ] {
                    let response = ui.selectable_value(&mut target, choice, choice.label());
                    controls_interacting |=
                        response.clicked() || response.has_focus() || response.lost_focus();
                    #[cfg(feature = "visual-qa")]
                    ui.ctx().data_mut(|data| {
                        data.insert_temp(
                            egui::Id::new(("mask-selection-choice", choice.label())),
                            response.rect,
                        )
                    });
                }
            });
            let names_toggle = ui.checkbox(&mut names_mode, "Exact names (one per line)");
            #[cfg(feature = "visual-qa")]
            ui.ctx().data_mut(|data| {
                data.insert_temp(egui::Id::new("mask-names-toggle"), names_toggle.rect)
            });
            controls_interacting |=
                names_toggle.clicked() || names_toggle.has_focus() || names_toggle.lost_focus();
            let case = ui
                .add_enabled(
                    !names_mode,
                    egui::Checkbox::new(&mut case_sensitive, "Match case"),
                )
                .on_hover_text(
                    "Distinguish uppercase and lowercase in masks. Exact names always match case.",
                );
            controls_interacting |= case.clicked() || case.has_focus() || case.lost_focus();
            let accents = ui.add_enabled(!names_mode, egui::Checkbox::new(&mut ignore_accents, "Ignore accents"))
                .on_hover_text("Compare without combining marks and expand common Latin ligatures, for example café → cafe and cœur → coeur. Names stay unchanged.");
            controls_interacting |= accents.clicked() || accents.has_focus() || accents.lost_focus();
            let extensionless = ui.add_enabled(!names_mode, egui::Checkbox::new(&mut extensionless_star, "Treat *.* as *"))
                .on_hover_text("Let the exact *.* mask match names without a dot too. Applies to exclusions; other masks keep their normal meaning.");
            controls_interacting |= extensionless.clicked() || extensionless.has_focus() || extensionless.lost_focus();
            let label = ui.label(
                egui::RichText::new(if names_mode {
                    "Paste filenames: exact, case-sensitive; no paths or escaping"
                } else {
                    "Masks separated by , or ;  (e.g. *.rs;!*test*)"
                })
                .size(12.0)
                .color(t.text_muted),
            );
            ui.add_space(6.0);

            let edit = if names_mode {
                egui::TextEdit::multiline(buffer).desired_rows(7)
            } else {
                egui::TextEdit::singleline(buffer)
            }
            .id(egui::Id::new("selection-mask-or-names-input"))
            .char_limit(crate::name_list::MAX_BYTES)
            .desired_width(f32::INFINITY)
            .hint_text(if names_mode {
                "photo.jpg\nreport.pdf"
            } else {
                "*.jpg;*.png;!*raw*"
            })
            .margin(egui::vec2(8.0, 6.0));
            let resp = ui.add(edit).labelled_by(label.id);
            #[cfg(feature = "visual-qa")]
            ui.ctx()
                .data_mut(|data| data.insert_temp(egui::Id::new("mask-names-input"), resp.rect));
            // Grab focus on the opening frame only; yanking it every frame
            // would trap the caret and stop the user clicking the buttons.
            if just_opened {
                resp.request_focus();
                select_all(ui.ctx(), resp.id, buffer);
            }

            // Count after TextEdit so the label and Enter action observe
            // the exact same buffer from this frame.
            let key = MaskCountKey {
                mask: buffer.clone(),
                names_mode,
                case_sensitive,
                ignore_accents,
                extensionless_star,
                target,
                folder: active_panel.current_path.clone(),
                entries_gen: active_panel.entries_gen(),
                shown: active_panel.filtered_count(),
                selected: active_panel.selected_count(),
            };
            let result = cached_mask_count(ui.ctx(), key, || {
                if names_mode {
                    if target == crate::panel::SelectionTarget::Both {
                        active_panel.names_match_count(buffer)
                    } else {
                        active_panel.names_match_count_target(buffer, target)
                    }
                } else {
                    Ok(if extensionless_star {
                        active_panel.mask_match_count_compatibility(buffer, target, case_sensitive, ignore_accents, true)
                    } else if ignore_accents {
                        active_panel.mask_match_count_folding(buffer, target, case_sensitive, true)
                    } else if case_sensitive {
                        active_panel.mask_match_count_options(buffer, target, true)
                    } else if target == crate::panel::SelectionTarget::Both {
                        active_panel.mask_match_count(buffer)
                    } else {
                        active_panel.mask_match_count_target(buffer, target)
                    })
                }
            });
            let valid = result.is_ok();
            #[cfg(feature = "visual-qa")]
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    egui::Id::new("mask-names-preview"),
                    if names_mode {
                        result.as_ref().ok().copied()
                    } else {
                        None::<usize>
                    },
                )
            });
            let change_label = match result {
                Ok(count) => change_count_label(count),
                Err(error) => error,
            };
            if names_mode {
                ui.small("64 KiB / 4096 names maximum. Enter adds a line; Cmd+Enter applies.");
            }
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(&change_label)
                    .size(11.0)
                    .color(if valid {
                        t.accent
                    } else {
                        ui.visuals().error_fg_color
                    }),
            );

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        valid,
                        super::dialog_button(
                            t,
                            super::ButtonRole::Primary,
                            egui::RichText::new("Select").size(13.0),
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
                if valid
                    && !controls_interacting
                    && ui.input(|i| {
                        i.key_pressed(egui::Key::Enter) && (!names_mode || i.modifiers.command)
                    })
                {
                    commit = true;
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
        ctx.data_mut(|data| {
            data.insert_temp(egui::Id::new("mask-names-mode"), names_mode);
            data.insert_temp(egui::Id::new("mask-selection-target"), target);
            data.insert_temp(egui::Id::new("mask-case-sensitive"), case_sensitive);
            data.insert_temp(egui::Id::new("mask-ignore-accents"), ignore_accents);
            data.insert_temp(egui::Id::new("mask-extensionless-star"), extensionless_star);
        });
        if cancel {
            self.ui.modals.mask_input = None;
            return;
        }
        if commit && let Some(buf) = self.ui.modals.mask_input.take() {
            if names_mode {
                let result = if target == crate::panel::SelectionTarget::Both {
                    self.ws.active_panel().select_by_names(&buf)
                } else {
                    self.ws.active_panel().select_by_names_target(&buf, target)
                };
                if let Err(error) = result {
                    self.toast(ctx, crate::toasts::ToastKind::Error, error, false);
                }
            } else {
                if extensionless_star {
                    self.ws.active_panel().select_by_mask_compatibility(
                        &buf,
                        target,
                        case_sensitive,
                        ignore_accents,
                        true,
                    );
                } else if ignore_accents {
                    self.ws.active_panel().select_by_mask_folding(
                        &buf,
                        target,
                        case_sensitive,
                        true,
                    );
                } else if case_sensitive {
                    self.ws
                        .active_panel()
                        .select_by_mask_options(&buf, target, true);
                } else if target == crate::panel::SelectionTarget::Both {
                    self.ws.active_panel().select_by_mask(&buf);
                } else {
                    self.ws.active_panel().select_by_mask_target(&buf, target);
                }
                self.ui.last_mask = buf;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_count_is_recomputed_only_when_its_inputs_change() {
        let ctx = egui::Context::default();
        let key = |mask: &str, selected| MaskCountKey {
            mask: mask.to_string(),
            names_mode: false,
            case_sensitive: false,
            ignore_accents: false,
            extensionless_star: false,
            target: crate::panel::SelectionTarget::Both,
            folder: PathBuf::from("/fixture"),
            entries_gen: 3,
            shown: 10,
            selected,
        };
        let mut scans = 0;
        let mut count = |key| {
            cached_mask_count(&ctx, key, || {
                scans += 1;
                scans
            })
        };
        assert_eq!(count(key("*.rs", 0)), 1);
        assert_eq!(count(key("*.rs", 0)), 1, "repaint reuses the count");
        assert_eq!(count(key("*.md", 0)), 2);
        assert_eq!(count(key("*.md", 1)), 3, "selection changed");
        let mut changed_target = key("*.md", 1);
        changed_target.target = crate::panel::SelectionTarget::Files;
        assert_eq!(count(changed_target.clone()), 4, "target changed");
        assert_eq!(count(changed_target.clone()), 4, "same target reuses count");
        changed_target.case_sensitive = true;
        assert_eq!(count(changed_target.clone()), 5, "case option changed");
        assert_eq!(
            count(changed_target.clone()),
            5,
            "same case option reuses count"
        );
        changed_target.ignore_accents = true;
        assert_eq!(count(changed_target.clone()), 6, "accent option changed");
        assert_eq!(
            count(changed_target.clone()),
            6,
            "same accent option reuses count"
        );
        changed_target.extensionless_star = true;
        assert_eq!(
            count(changed_target.clone()),
            7,
            "extensionless option changed"
        );
        assert_eq!(
            count(changed_target),
            7,
            "same compatibility option reuses count"
        );
    }

    #[test]
    fn preview_labels_selection_changes_not_pattern_matches() {
        assert_eq!(change_count_label(0), "0 changes");
        assert_eq!(change_count_label(1), "1 change");
        assert_eq!(change_count_label(7), "7 changes");
    }

    #[test]
    fn prefilled_mask_is_fully_selected() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("mask_field");
        select_all(&ctx, id, "*.r\u{e9}s");
        let range = egui::TextEdit::load_state(&ctx, id)
            .and_then(|state| state.cursor.char_range())
            .expect("selection stored");
        assert_eq!(range.primary, egui::text::CCursor::new(5), "{range:?}");
        assert_eq!(range.secondary, egui::text::CCursor::new(0), "{range:?}");
    }
}
