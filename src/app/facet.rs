//! Facet chips for quick filters.
//! Extracted from render.rs for SRP in UI.

use egui::{Color32, Margin, Ui};
use crate::panel::{FacetSet, KindFacet, PanelState};
use crate::theme::ThemeColors;

pub fn render_facet_chips(ui: &mut Ui, panel: &mut PanelState, t: &ThemeColors) {
    crate::app::ui_common::section_frame(t)
        .fill(Color32::TRANSPARENT)
        .inner_margin(Margin {
            left: crate::app::ui_common::SPACING_MD,
            right: crate::app::ui_common::SPACING_MD,
            top: 0,
            bottom: crate::app::ui_common::SPACING_XS,
        })
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(
                    crate::app::ui_common::SPACING_XS as f32,
                    crate::app::ui_common::SPACING_XS as f32,
                );

                let chip = |ui: &mut Ui, label: &str, active: bool| -> bool {
                    let fill = if active {
                        t.accent.linear_multiply(0.3)
                    } else {
                        t.bg_card
                    };
                    crate::app::ui_common::styled_button(ui, label, fill)
                };

                let f = panel.facets_mut();
                // Kind chips (mutually exclusive: clicking the active one clears it).
                for (label, kind) in [
                    ("Folders", KindFacet::Folders),
                    ("Images", KindFacet::Images),
                    ("Docs", KindFacet::Docs),
                    ("Archives", KindFacet::Archives),
                    ("Code", KindFacet::Code),
                ] {
                    if chip(ui, label, f.kind == Some(kind)) {
                        f.kind = if f.kind == Some(kind) {
                            None
                        } else {
                            Some(kind)
                        };
                    }
                }
                ui.add_space(6.0);
                if chip(ui, ">1MB", f.min_size == Some(1 << 20)) {
                    f.min_size = if f.min_size == Some(1 << 20) {
                        None
                    } else {
                        Some(1 << 20)
                    };
                }
                if chip(ui, ">100MB", f.min_size == Some(100 << 20)) {
                    f.min_size = if f.min_size == Some(100 << 20) {
                        None
                    } else {
                        Some(100 << 20)
                    };
                }
                ui.add_space(6.0);
                if chip(ui, "Today", f.max_age_days == Some(1)) {
                    f.max_age_days = if f.max_age_days == Some(1) {
                        None
                    } else {
                        Some(1)
                    };
                }
                if chip(ui, "Week", f.max_age_days == Some(7)) {
                    f.max_age_days = if f.max_age_days == Some(7) {
                        None
                    } else {
                        Some(7)
                    };
                }
                if !f.is_empty() && chip(ui, "\u{2715} Clear", false) {
                    *f = FacetSet::default();
                }
            });
        });
}