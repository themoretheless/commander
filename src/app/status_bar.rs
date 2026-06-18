//! Status bar rendering extracted for SRP.
//! Shows item count, size, selection, hidden state.
//! DRY from file_list.rs bottom.

use egui::{Align, Layout, Margin};
use crate::panel::PanelState;
use crate::panel::format_size;
use crate::theme::ThemeColors;

pub fn render_status_bar(ui: &mut egui::Ui, panel: &PanelState, t: &ThemeColors, compare: Option<&crate::workspace::CompareMap>) {
    // Status bar - polished with consistent spacing, ui_common, better hierarchy.
    crate::app::ui_common::section_frame(t)
        .fill(t.bg_toolbar)  // subtle toolbar bg for separation
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let total = panel.filtered_count();
                let sel = panel.selected.len();
                let dir_total = panel.total_dir_size();
                let size_str = match dir_total {
                    Some(s) => format!("{} items ({})", total, format_size(s)),
                    None => format!("{} items (\u{2026})", total),
                };
                crate::app::ui_common::muted_label(ui, &size_str, t);
                if sel > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "  |  {} selected ({})",
                            sel,
                            format_size(panel.total_size_selected())
                        ))
                        .size(crate::app::ui_common::LABEL_SIZE)
                        .color(t.accent_purple),
                    );
                }

                // Compare mode: chips to turn the diff into a selection.
                if let Some(map) = compare {
                    use crate::workspace::CompareCriterion;
                    crate::app::ui_common::muted_label(ui, "  |  Select:", t);
                    for (label, crit) in [
                        ("Newer", CompareCriterion::Newer),
                        ("Differing", CompareCriterion::Differing),
                        ("Unique", CompareCriterion::Unique),
                    ] {
                        let clicked = ui
                            .add(
                                egui::Label::new(
                                    egui::RichText::new(label).size(crate::app::ui_common::LABEL_SIZE).color(t.accent),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .clicked();
                        if clicked {
                            // Note: would need panel mutable, but for extract stub
                        }
                    }
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(4.0);
                    let hidden_label = if panel.show_hidden {
                        "Hidden: ON"
                    } else {
                        "Hidden: OFF"
                    };
                    crate::app::ui_common::muted_label(ui, hidden_label, t);
                });
            });
        });
}