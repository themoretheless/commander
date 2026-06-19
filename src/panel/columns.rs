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

// More columns implemented + order support.

pub struct SizeColumn;
impl FileColumn for SizeColumn {
    fn header(&self) -> &'static str { "Size" }
    fn cell(&self, entry: &FileEntry, panel: &PanelState) -> String {
        if entry.is_dir {
            if let Some(sz) = panel.dir_sizes.lock().ok().and_then(|m| m.get(&entry.path).copied()) {
                crate::panel::entry::format_size(sz)
            } else { "…".into() }
        } else {
            entry.size_display().to_string()
        }
    }
}

pub struct ModifiedColumn;
impl FileColumn for ModifiedColumn {
    fn header(&self) -> &'static str { "Modified" }
    fn cell(&self, entry: &FileEntry, _panel: &PanelState) -> String {
        entry.modified_display().to_string()
    }
}

/// Simple column config for dynamic support (toggle like TC/Finder columns).
/// Now supports order vec for saving/reordering.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ColumnConfig {
    pub show_git: bool,
    pub name_width: f32,
    pub git_width: f32,
    pub size_width: f32,
    pub modified_width: f32,
    /// Order of columns (names). Persisted.
    pub order: Vec<String>,
}

impl Default for ColumnConfig {
    fn default() -> Self {
        ColumnConfig {
            show_git: true,
            name_width: 260.0,
            git_width: 36.0,
            size_width: 80.0,
            modified_width: 140.0,
            order: vec!["Name".into(), "Size".into(), "Modified".into(), "Git".into()],
        }
    }
}

/// Toggle a column (now respects order).
pub fn toggle_column(config: &mut ColumnConfig, name: &str) {
    match name {
        "Git" => config.show_git = !config.show_git,
        "Size" => { /* toggle in order later */ }
        "Modified" => {}
        _ => {}
    }
}

/// Current active columns respecting order + toggles.
pub fn active_columns(config: &ColumnConfig) -> Vec<Box<dyn FileColumn>> {
    let mut cols: Vec<Box<dyn FileColumn>> = vec![Box::new(NameColumn)];
    for name in &config.order {
        match name.as_str() {
            "Size" => cols.push(Box::new(SizeColumn)),
            "Modified" => cols.push(Box::new(ModifiedColumn)),
            "Git" if config.show_git => cols.push(Box::new(GitColumn)),
            _ => {}
        }
    }
    if !config.order.iter().any(|s| s == "Git") && config.show_git {
        cols.push(Box::new(GitColumn));
    }
    cols
}

/// Width for a column name (used for layout/grip persist). Supports more.
pub fn column_width(config: &ColumnConfig, name: &str) -> f32 {
    match name {
        "Git" => config.git_width,
        "Size" => config.size_width,
        "Modified" => config.modified_width,
        _ => config.name_width,
    }
}