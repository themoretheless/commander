//! Layout and splitter logic for the main UI area.
//! Extracted for SRP: handles tree panel width, left/right panel halves, resizable dividers.
//! Inspired by VSCode/Path Finder split views and resizers. Keeps show_main_area clean.
//! Future: can own layout state, double-click resets, etc.

use egui::{Context, Id};

/// Computes the half width for panels after accounting for tree.
/// Also handles reset of panel state on window resize or tree toggle.
pub fn compute_panel_half(
    ctx: &Context,
    window_width: f32,
    tree_actual_width: f32,
    prev_window_width: f32,
    show_tree: bool,
    prev_half: f32,
) -> (f32, bool) {
    let remaining = window_width - tree_actual_width - 6.0;
    let half = remaining / 2.0;

    let mut should_reset = false;
    if prev_window_width > 0.0
        && ((window_width - prev_window_width).abs() > 1.0
            || (half - prev_half).abs() > 1.0)
    {
        should_reset = true;
    }

    (half, should_reset)
}

/// Handles double-click on panel divider to reset to 50/50.
pub fn handle_panel_divider_double_click(
    ctx: &Context,
    panel_rect: egui::Rect,
    panel_id: Id,
) {
    let divider_rect = egui::Rect::from_min_max(
        egui::pos2(panel_rect.right() - 4.0, panel_rect.top()),
        egui::pos2(panel_rect.right() + 4.0, panel_rect.bottom()),
    );
    let double_clicked = ctx.input(|i| {
        if let Some(pos) = i.pointer.latest_pos() {
            divider_rect.contains(pos)
                && i.pointer
                    .button_double_clicked(egui::PointerButton::Primary)
        } else {
            false
        }
    });
    if double_clicked {
        ctx.data_mut(|d| {
            d.remove::<egui::containers::panel::PanelState>(panel_id);
        });
    }
}