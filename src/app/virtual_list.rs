//! Virtual list / virtualization for file rows.
//! SRP: handle only visible rows rendering for large dirs (perf like VSCode virtual lists, modern FMs).
//! Stub for now; logic from file_list can move here (e.g. first/last visible, allocate space).

use egui::Ui;
use crate::panel::PanelState;
use crate::theme::ThemeColors;
use crate::density::DensityMetrics;

pub fn render_virtual_file_rows(
    ui: &mut Ui,
    panel: &mut PanelState,
    is_active: bool,
    t: &ThemeColors,
    panel_side: &str,
    size_bars: bool,
    compare: Option<&crate::workspace::CompareMap>,
    opener: &dyn Fn(&std::path::Path),
    metrics: DensityMetrics,
    show_git: bool,
) {
    // TODO: move the visible rows loop, first_visible calc, etc from file_list here.
    // For now, stub (demonstrates virtual idea for large dirs perf, like VSCode).
    // In future: compute first/last visible, allocate space only for them.
    let _ = (ui, panel, is_active, t, panel_side, size_bars, compare, opener, metrics, show_git);
}
