mod keys;
mod toolbar;
mod render;
mod file_list;
mod tree;
mod file_ops;
mod preload;
mod update;
mod confirm_dialog;
mod transfer_dialog;

use egui::{
    Align, Color32, CornerRadius, Frame, Layout, Margin, Sense, Stroke, Vec2,
};
use std::path::PathBuf;

use crate::panel::{format_size, PanelState, SortColumn};
use crate::scan::FlatList;
use crate::theme::{ThemeColors, ThemeMode, apply_theme};
pub(crate) use crate::transfer::{
    CopyMethod, OverwritePolicy, TransferKind, TransferState,
};
pub use crate::transfer::TransferProgress;

#[derive(PartialEq, Clone, Copy)]
pub enum ActivePanel {
    Left,
    Right,
}

/// A copy/move awaiting user confirmation in the dialog.
#[derive(Clone)]
pub(crate) struct PendingTransfer {
    pub kind: TransferKind,
    pub entries: Vec<crate::panel::FileEntry>,
    pub target: PathBuf,
    pub conflicts: Vec<String>,
    pub policy: OverwritePolicy,
    pub method: CopyMethod,
    pub flat: FlatList,
}

/// Pending file operation awaiting user confirmation.
#[derive(Clone)]
pub(crate) enum PendingOp {
    Transfer(PendingTransfer),
    Delete {
        entries: Vec<crate::panel::FileEntry>,
        flat: FlatList,
    },
}

pub struct App {
    pub left: PanelState,
    pub right: PanelState,
    pub active: ActivePanel,
    pub(crate) pending_op: Option<PendingOp>,
    pub(crate) active_transfer: Option<TransferState>,
    pub ui_scale: f32,
    pub theme_mode: ThemeMode,
    pub colors: ThemeColors,
    pub(crate) prev_window_width: f32,
    pub(crate) image_cache: crate::image_cache::ImageCache,
    pub(crate) show_tree: bool,
    pub(crate) tree_expanded: std::collections::HashSet<PathBuf>,
    pub(crate) tree_children_cache: std::collections::HashMap<PathBuf, Vec<PathBuf>>,
    pub(crate) tree_width: f32,
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
            if is_dark { ThemeMode::Dark } else { ThemeMode::Light }
        };
        apply_theme(&cc.egui_ctx, mode);
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));

        App {
            left: PanelState::new(home.clone()),
            right: PanelState::new(home),
            active: ActivePanel::Left,
            pending_op: None,
            active_transfer: None,
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
        }
    }

    pub(crate) fn active_panel(&mut self) -> &mut PanelState {
        match self.active {
            ActivePanel::Left => &mut self.left,
            ActivePanel::Right => &mut self.right,
        }
    }

    pub(crate) fn inactive_panel(&self) -> &PanelState {
        match self.active {
            ActivePanel::Left => &self.right,
            ActivePanel::Right => &self.left,
        }
    }

    /// The panel opposite to the active one, mutable.
    pub(crate) fn inactive_panel_mut(&mut self) -> &mut PanelState {
        match self.active {
            ActivePanel::Left => &mut self.right,
            ActivePanel::Right => &mut self.left,
        }
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

    /// Create preview content for a file entry.
    pub(crate) fn make_preview(entry: &crate::panel::FileEntry) -> Option<crate::panel::PreviewContent> {
        use crate::panel::PreviewContent;
        if entry.is_dir {
            return None;
        }
        if entry.is_image() {
            Some(PreviewContent::Image(entry.path.clone()))
        } else {
            // Try to read as text (limit to 1MB)
            let Ok(meta) = std::fs::metadata(&entry.path) else { return None };
            if meta.len() > 1024 * 1024 {
                return None; // Too large
            }
            let Ok(content) = std::fs::read_to_string(&entry.path) else { return None };
            Some(PreviewContent::Text {
                path: entry.path.clone(),
                content,
            })
        }
    }
}
