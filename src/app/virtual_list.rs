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
    user_tags: &std::sync::Arc<std::collections::HashMap<std::path::PathBuf, String>>,
    notes: &std::sync::Arc<std::collections::HashMap<std::path::PathBuf, String>>,
    column_config: &mut crate::panel::ColumnConfig,
    mut _renaming: Option<&mut crate::app::RenameState>,
    grid: bool,
) {
    // Full virtual integration: render only visible rows + spacers for perf.
    // Uses approximate row height from metrics. For large lists, this avoids rendering thousands of rows.
    let filtered = panel.filtered_indices();
    let query = panel.search_query().to_string();
    let row_h = (metrics.name_pt + metrics.row_pad_y * 2.0) + 2.0; // approx row height from density metrics
    let total = filtered.len() as f32;

    // Get current scroll to compute visible window (inside ScrollArea show).
    // Simple estimation: use clip to decide range.
    let clip = ui.clip_rect();
    let visible_top = 0.0f32; // relative
    let visible_bottom = clip.height();

    let first = ((visible_top / row_h).floor() as usize).min(filtered.len());
    let last = ((visible_bottom / row_h).ceil() as usize + 5).min(filtered.len()); // + buffer

    // Spacer for rows above
    if first > 0 {
        ui.add_space(first as f32 * row_h);
    }

    for (i, &entry_idx) in filtered.iter().enumerate().skip(first).take(last - first) {
        // For full, move the full row render logic here from file_list.
        // Currently placeholder to keep working; real would render the entry row.
        let _ = (i, entry_idx, &query, user_tags, notes, &*column_config, &mut _renaming, grid, size_bars, compare, opener, is_active, &*panel, t, panel_side, show_git, metrics);
        // To make it actually render something visible for now, we delegate a label as demo.
        // In practice, the main loop in file_list still does real render; this is the virtual skeleton.
        ui.label(egui::RichText::new(format!("[virtual row {}]", i)).size(10.0));
        ui.add_space(row_h - 20.0); // rough
    }

    // Spacer below
    let rendered = (last - first) as f32;
    let remaining = total - first as f32 - rendered;
    if remaining > 0.0 {
        ui.add_space(remaining * row_h);
    }

    if filtered.len() > 50 {
        ui.label(egui::RichText::new(format!("virtualized: showing ~{} of {}", last-first, filtered.len())).size(9.0).color(t.text_muted));
    }
}
