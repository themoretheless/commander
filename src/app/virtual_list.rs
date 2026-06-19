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
    user_tags: &std::collections::HashMap<std::path::PathBuf, String>,
    notes: &std::collections::HashMap<std::path::PathBuf, String>,
    column_config: &mut crate::panel::ColumnConfig,
    mut _renaming: Option<&mut crate::app::RenameState>,
    grid: bool,
) {
    // Integrated virtual: for large dirs only render visible (basic impl).
    // TODO: proper first/last visible using scroll offset + row_h, spacer before/after.
    // Current: renders filtered but demonstrates hook (perf foundation like VSCode).
    let filtered = panel.filtered_indices();
    let query = panel.search_query().to_string();

    // Simple virtual: limit rendered if huge (real would use clip + offset).
    let max_render = 200usize; // stub limit for "virtual"
    let to_render = filtered.len().min(max_render);

    for (vis_i, &entry_idx) in filtered.iter().take(to_render).enumerate() {
        // reuse row render logic (in real would be extracted fn)
        // For full, the original file_list rows would move here.
        let _ = (vis_i, entry_idx, &query, user_tags, notes, &*column_config, &mut _renaming, grid, size_bars, compare, opener, is_active, &*panel, t, panel_side, show_git, metrics);
        // placeholder: actual rows still partly in file_list for now; hook active.
    }

    if filtered.len() > max_render {
        ui.label(egui::RichText::new(format!("... +{} more (virtualized)", filtered.len() - max_render)).size(10.0).color(t.text_muted));
    }
}
