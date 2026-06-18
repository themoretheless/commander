//! Column abstraction for file list (name, size, modified, git status, future custom).
//! SOLID/ISP idea: instead of hard-coded columns + boolean toggle for git,
//! have pluggable columns (inspired by Total Commander "content" plugins,
//! Double Commander columns, Finder tags/info columns, VSCode custom views).
//! This starts the foundation so "column customization (toggle + more)" can be real.

use super::{FileEntry, PanelState};

/// A column that can contribute to the file list row (header + per-entry cell).
/// For now focused on simple text/glyph; later can have render fn for colors etc.
pub trait FileColumn {
    fn header(&self) -> &'static str;
    /// Short display for the cell (e.g. git letter 'M', or size string).
    fn cell(&self, entry: &FileEntry, panel: &PanelState) -> String;
}

/// Built-in name column (always present).
pub struct NameColumn;

impl FileColumn for NameColumn {
    fn header(&self) -> &'static str { "Name" }
    fn cell(&self, entry: &FileEntry, _panel: &PanelState) -> String {
        entry.name.clone()
    }
}

/// Git status column (the 'G' one we had as toggle).
pub struct GitColumn;

impl FileColumn for GitColumn {
    fn header(&self) -> &'static str { "Git" }
    fn cell(&self, entry: &FileEntry, panel: &PanelState) -> String {
        panel.git_status().get(&entry.path)
            .copied()
            .map(|c| c.to_string())
            .unwrap_or_default()
    }
}

// Future: SizeColumn, ModifiedColumn, etc. can be added and toggled via a vec in PanelState or App.

/// Simple column config for dynamic support (toggle like TC/Finder columns).
/// Extended with widths for grip/persist idea (per-column resize + save).
#[derive(Clone, Debug)]
pub struct ColumnConfig {
    pub show_git: bool,
    pub name_width: f32,
    pub git_width: f32,
    // TODO: order vec, more cols.
}

impl Default for ColumnConfig {
    fn default() -> Self {
        ColumnConfig { show_git: true, name_width: 260.0, git_width: 36.0 }
    }
}

/// Toggle a column (stub for UI).
pub fn toggle_column(config: &mut ColumnConfig, name: &str) {
    if name == "Git" {
        config.show_git = !config.show_git;
    }
}

/// Current active columns for a panel (simple vec for start; later per-tab config).
/// For now, to not break layout much, we keep the old show_git flag and conditionally include GitColumn.
pub fn active_columns(config: &ColumnConfig) -> Vec<Box<dyn FileColumn>> {
    crate::plugin::PluginRegistry::new().get_columns(config)
}

/// Width for a column name (used for layout/grip persist).
pub fn column_width(config: &ColumnConfig, name: &str) -> f32 {
    match name {
        "Git" => config.git_width,
        _ => config.name_width,
    }
}