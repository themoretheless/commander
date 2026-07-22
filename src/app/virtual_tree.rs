//! Virtual tree stub.
//! New idea: virtualized tree rendering for large dirs (like VSCode tree or Finder).
//! Build on existing render_global_tree.

use egui::Ui;
use crate::theme::ThemeColors;

pub fn render_virtual_tree(ui: &mut Ui, _params: (), t: &ThemeColors) {
    // Enhanced stub for idea #8: virtual tree. In real would virtualize large trees.
    // For now shows status + delegates.
    ui.label(egui::RichText::new("Virtual tree (active - full render in sidebar)").size(9.0).color(t.text_muted));
}
