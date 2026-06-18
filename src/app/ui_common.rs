//! Common UI helpers extracted for DRY + consistent UX.
//! Best practices: central spacing, reusable styled primitives, strong affordances (grips, hovers), hierarchy via size/color.
//! Inspired by macOS Finder/Path Finder (clean, dense, consistent) + VSCode (tooltips, modern frames) + egui idioms.

use egui::{Color32, CornerRadius, Frame, Margin, RichText, Stroke, Ui};

use crate::theme::ThemeColors;

pub const SPACING_XS: i8 = 4;
pub const SPACING_SM: i8 = 8;
pub const SPACING_MD: i8 = 12;
pub const SPACING_LG: i8 = 16;

pub const LABEL_SIZE: f32 = 11.0;
pub const TITLE_SIZE: f32 = 13.0;
pub const META_SIZE: f32 = 10.0;

/// Consistent grip painter for resizers (column, preview). Good affordance with dots.
pub fn paint_grip(ui: &mut Ui, rect: egui::Rect, t: &ThemeColors, horizontal: bool) {
    ui.painter().rect_filled(rect, 0.0, t.text_muted.linear_multiply(0.5));
    let dots = if horizontal { 3 } else { 3 };
    for i in 0..dots {
        let pos = if horizontal {
            egui::pos2(rect.center().x - 4.0 + i as f32 * 4.0, rect.center().y)
        } else {
            egui::pos2(rect.center().x, rect.center().y - 4.0 + i as f32 * 4.0)
        };
        ui.painter().circle_filled(pos, 1.0, t.bg_deep);
    }
}

/// Muted secondary label (common pattern).
pub fn muted_label(ui: &mut Ui, text: &str, t: &ThemeColors) {
    ui.label(RichText::new(text).size(LABEL_SIZE).color(t.text_muted));
}

/// Primary label.
pub fn primary_label(ui: &mut Ui, text: &str, t: &ThemeColors) {
    ui.label(RichText::new(text).size(LABEL_SIZE).color(t.text_primary));
}

pub fn styled_button(ui: &mut Ui, label: &str, fill: Color32) -> bool {
    ui.add(
        egui::Button::new(label)
            .fill(fill)
            .corner_radius(CornerRadius::same(2)),
    ).clicked()
}

pub fn thin_frame(ui: &mut Ui, content: impl FnOnce(&mut Ui)) {
    Frame::NONE
        .fill(Color32::TRANSPARENT)
        .inner_margin(Margin::same(0))
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::ZERO)
        .show(ui, content);
}

/// Subtle section frame used for panels/toolbars.
pub fn section_frame(t: &ThemeColors) -> Frame {
    Frame::NONE
        .fill(t.bg_panel)
        .inner_margin(Margin::symmetric(SPACING_MD, SPACING_SM))
        .stroke(Stroke::NONE)
}

/// Hover state helper.
pub fn apply_hover_paint(ui: &mut Ui, rect: egui::Rect, t: &ThemeColors) {
    ui.painter().rect_filled(rect, CornerRadius::ZERO, t.bg_hover.linear_multiply(0.3));
}

/// Small toggle button with * when active. For discoverability in tab bar (L C G R etc).
pub fn small_toggle(ui: &mut Ui, label: &str, active: bool, tooltip: &str) -> bool {
    let l = if active { format!("{}*", label) } else { label.to_string() };
    ui.small_button(l).on_hover_text(tooltip).clicked()
}

// TODO: icon_button, consistent tooltip wrapper, divider helper.

