//! Config and settings module.
//! SRP: central place for UI prefs, layout, column config, persistable.
//! Inspired by Path Finder prefs, Total Commander ini, VSCode settings.
//! Future: serde for save/load, defaults.

use crate::density::Density;
use crate::panel::ColumnConfig;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub ui_scale: f32,
    pub density: Density,
    pub show_tree: bool,
    pub tree_width: f32,
    pub show_size_bars: bool,
    pub show_compare: bool,
    pub show_git_status: bool,
    pub column_config: ColumnConfig,
    // TODO: more, like theme, bookmarks, etc. (git show etc persisted via session).
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            ui_scale: 1.0,
            density: Density::default(),
            show_tree: true,
            tree_width: 200.0,
            show_size_bars: false,
            show_compare: false,
            show_git_status: true,
            column_config: ColumnConfig::default(),
        }
    }
}

// Persist wired (see app to_session + session fields); more fields easy to add.
