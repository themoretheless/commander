//! UI layer: renders the [`Workspace`] and forwards input to it.
//! All file-manager behaviour lives in `crate::workspace`; this module
//! owns only presentation state (theme, zoom, image cache, tree widget).

mod confirm_dialog;
mod file_list;
mod keys;
mod preload;
mod rename_dialog;
mod render;
mod toolbar;
mod transfer_dialog;
mod tree;
mod update;

use egui::{Align, Color32, CornerRadius, Frame, Layout, Margin, Sense, Stroke, Vec2};
use std::path::PathBuf;

use crate::panel::{PanelState, SortColumn, format_size};
use crate::theme::{ThemeColors, ThemeMode, apply_theme};
pub(crate) use crate::transfer::{CopyMethod, OverwritePolicy, TransferKind};
pub(crate) use crate::workspace::{ActivePanel, PendingOp, Workspace};

pub struct App {
    /// UI-independent application core (panels, ops, transfers).
    pub ws: Workspace,
    pub ui_scale: f32,
    pub theme_mode: ThemeMode,
    pub colors: ThemeColors,
    pub(crate) prev_window_width: f32,
    pub(crate) image_cache: crate::image_cache::ImageCache,
    pub(crate) show_tree: bool,
    pub(crate) tree_expanded: std::collections::HashSet<PathBuf>,
    pub(crate) tree_children_cache: std::collections::HashMap<PathBuf, Vec<PathBuf>>,
    pub(crate) tree_width: f32,
    /// Active inline rename: the entry being renamed and the edit buffer.
    pub(crate) renaming: Option<RenameState>,
    /// Type-ahead buffer and the input time of its last keystroke (seconds,
    /// from egui). Expires after a short idle.
    pub(crate) type_ahead: Option<(String, f64)>,
    /// Paint relative size occupancy bars behind file rows.
    pub(crate) show_size_bars: bool,
}

/// UI state for the rename editor.
pub(crate) struct RenameState {
    pub path: PathBuf,
    pub buffer: String,
    pub error: Option<String>,
    /// Set once so the text field grabs focus on the first frame.
    pub focused: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mode = if cc.egui_ctx.style().visuals.dark_mode {
            ThemeMode::Dark
        } else {
            // Detect system dark mode via macOS defaults
            let is_dark = std::process::Command::new("defaults")
                .args(["read", "-g", "AppleInterfaceStyle"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains("Dark"))
                .unwrap_or(false);
            if is_dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            }
        };
        apply_theme(&cc.egui_ctx, mode);
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));

        App {
            ws: Workspace::new(home.clone(), home),
            ui_scale: 1.0,
            theme_mode: mode,
            colors: match mode {
                ThemeMode::Light => ThemeColors::light(),
                ThemeMode::Dark => ThemeColors::dark(),
            },
            prev_window_width: 0.0,
            image_cache: crate::image_cache::ImageCache::new(),
            show_tree: false,
            tree_expanded: std::collections::HashSet::new(),
            tree_children_cache: std::collections::HashMap::new(),
            tree_width: 200.0,
            renaming: None,
            type_ahead: None,
            show_size_bars: false,
        }
    }

    /// Confirm the pending operation; progress wakes the UI via repaint.
    pub(crate) fn confirm_pending_op(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        self.ws.confirm_pending_op(move || ctx.request_repaint());
    }

    pub(crate) fn tree_expand_to_path(&mut self, path: &std::path::Path) {
        let mut p = path.to_path_buf();
        loop {
            self.tree_expanded.insert(p.clone());
            match p.parent() {
                Some(parent) if parent != p => p = parent.to_path_buf(),
                _ => break,
            }
        }
    }
}
