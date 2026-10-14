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
    folder: PathBuf,
    entries_gen: u64,
    shown: usize,
    selected: usize,
}

const MASK_COUNT_CACHE: &str = "mask_count_cache";

fn cached_mask_count(
    ctx: &egui::Context,
    key: MaskCountKey,
    count: impl FnOnce() -> usize,
) -> usize {
    let id = egui::Id::new(MASK_COUNT_CACHE);
    if let Some((cached_key, cached)) = ctx.data(|d| d.get_temp::<(MaskCountKey, usize)>(id))
        && cached_key == key
    {
        return cached;
    }
    let value = count();
    ctx.data_mut(|d| d.insert_temp(id, (key, value)));
    value
}

impl App {
    pub(crate) fn open_mask(&mut self, ctx: &egui::Context) {
        ctx.data_mut(|d| d.remove::<(MaskCountKey, usize)>(egui::Id::new(MASK_COUNT_CACHE)));
        self.ui.modals.mask_input = Some(String::new());
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

        let mut commit = false;
        let mut cancel = false;

        super::sheet_window("Select by mask", t, 14).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(
                egui::RichText::new("Select by mask  (e.g.  *.rs, !*test*)")
                    .size(12.0)
                    .color(t.text_muted),
            );
            ui.add_space(6.0);

            let edit = egui::TextEdit::singleline(buffer)
                .desired_width(f32::INFINITY)
                .hint_text("*.jpg, !*raw*")
                .margin(egui::vec2(8.0, 6.0));
            let resp = ui.add(edit);
            // Grab focus on the opening frame only; yanking it every frame
            // would trap the caret and stop the user clicking the buttons.
            if just_opened {
                resp.request_focus();
            }

            // Count after TextEdit so the label and Enter action observe
            // the exact same buffer from this frame.
            let key = MaskCountKey {
                mask: buffer.clone(),
                folder: active_panel.current_path.clone(),
                entries_gen: active_panel.entries_gen(),
                shown: active_panel.filtered_count(),
                selected: active_panel.selected_count(),
            };
            let count = cached_mask_count(ui.ctx(), key, || active_panel.mask_match_count(buffer));
            let change_label = change_count_label(count);
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(&change_label)
                    .size(11.0)
                    .color(t.accent),
            );

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add(super::dialog_button(
                        t,
                        super::ButtonRole::Primary,
                        egui::RichText::new("Select").size(13.0),
                    ))
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
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    commit = true;
                }
                if escape_requested {
                    cancel = true;
                }
            });
        });

        if cancel {
            self.ui.modals.mask_input = None;
            return;
        }
        if commit && let Some(buf) = self.ui.modals.mask_input.take() {
            self.ws.active_panel().select_by_mask(&buf);
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
    }

    #[test]
    fn preview_labels_selection_changes_not_pattern_matches() {
        assert_eq!(change_count_label(0), "0 changes");
        assert_eq!(change_count_label(1), "1 change");
        assert_eq!(change_count_label(7), "7 changes");
    }
}
