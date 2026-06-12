mod keys;
mod toolbar;
mod render;
mod file_list;
mod tree;
mod file_ops;
mod preload;
mod update;

use egui::{
    Align, Color32, CornerRadius, Frame, Layout, Margin, Sense, Stroke, Vec2,
};
use std::path::PathBuf;

use crate::panel::{format_size, PanelState, SortColumn};
use crate::theme::{ThemeColors, ThemeMode, apply_theme};

#[derive(PartialEq, Clone, Copy)]
pub enum ActivePanel {
    Left,
    Right,
}

/// What to do when destination file already exists.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum OverwritePolicy {
    Ask,
    OverwriteAll,
    SkipAll,
}

/// Copy method.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum CopyMethod {
    /// Byte-by-byte with 1MB buffer, full progress tracking.
    Buffered,
    /// Native macOS copyfile() with APFS clone support, xattr/ACL preservation.
    Native,
}

/// Pending file operation awaiting user confirmation.
pub(crate) type FlatList = std::sync::Arc<std::sync::Mutex<Option<Vec<crate::app::file_ops::FlatFileEntry>>>>;

#[derive(Clone)]
pub(crate) enum PendingOp {
    Copy {
        entries: Vec<crate::panel::FileEntry>,
        target: PathBuf,
        conflicts: Vec<String>,
        policy: OverwritePolicy,
        method: CopyMethod,
        flat: FlatList,
    },
    Move {
        entries: Vec<crate::panel::FileEntry>,
        target: PathBuf,
        conflicts: Vec<String>,
        policy: OverwritePolicy,
        method: CopyMethod,
        flat: FlatList,
    },
    Delete {
        entries: Vec<crate::panel::FileEntry>,
        flat: FlatList,
    },
}

/// Live transfer progress shared between background thread and UI.
#[derive(Clone)]
pub(crate) struct TransferProgress {
    pub total_bytes: u64,
    pub copied_bytes: u64,
    pub current_file: String,
    pub current_file_size: u64,
    pub current_file_copied: u64,
    pub files_done: usize,
    pub files_total: usize,
    pub speed_samples: Vec<(f64, f64)>,  // (timestamp_secs, bytes_at_that_time)
    pub started_at: std::time::Instant,
    pub finished: bool,
    pub cancelled: bool,
}

impl TransferProgress {
    pub fn new(total_bytes: u64, files_total: usize) -> Self {
        Self {
            total_bytes,
            copied_bytes: 0,
            current_file: String::new(),
            current_file_size: 0,
            current_file_copied: 0,
            files_done: 0,
            files_total,
            speed_samples: vec![(0.0, 0.0)],
            started_at: std::time::Instant::now(),
            finished: false,
            cancelled: false,
        }
    }

    /// Current speed in bytes/sec (averaged over last 2 seconds).
    pub fn speed_bps(&self) -> f64 {
        if self.speed_samples.len() < 2 {
            return 0.0;
        }
        let now = self.started_at.elapsed().as_secs_f64();
        // Find sample ~2 seconds ago
        let window = 2.0;
        let cutoff = now - window;
        let old = self.speed_samples.iter()
            .rev()
            .find(|(t, _)| *t <= cutoff)
            .unwrap_or(&self.speed_samples[0]);
        let dt = now - old.0;
        if dt < 0.01 { return 0.0; }
        (self.copied_bytes as f64 - old.1) / dt
    }

    /// Estimated time remaining in seconds.
    pub fn eta_secs(&self) -> f64 {
        let speed = self.speed_bps();
        if speed < 1.0 { return 0.0; }
        let remaining = self.total_bytes.saturating_sub(self.copied_bytes) as f64;
        remaining / speed
    }

    /// Record a speed sample (call periodically from copy thread).
    pub fn record_sample(&mut self) {
        let t = self.started_at.elapsed().as_secs_f64();
        self.speed_samples.push((t, self.copied_bytes as f64));
        // Keep last 120 samples (~60 seconds at 2Hz)
        if self.speed_samples.len() > 120 {
            self.speed_samples.remove(0);
        }
    }
}

pub(crate) type TransferState = std::sync::Arc<std::sync::Mutex<TransferProgress>>;

pub struct App {
    pub left: PanelState,
    pub right: PanelState,
    pub active: ActivePanel,
    pub show_confirm_delete: bool,
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
            show_confirm_delete: false,
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

    pub(crate) fn tree_subdirs_cached(&mut self, path: &std::path::Path) -> Vec<PathBuf> {
        if let Some(cached) = self.tree_children_cache.get(path) {
            return cached.clone();
        }
        let active = self.active_panel();
        let dirs = PanelState::subdirs(path, active.show_hidden);
        self.tree_children_cache.insert(path.to_path_buf(), dirs.clone());
        dirs
    }

    pub(crate) fn invalidate_tree_cache(&mut self) {
        self.tree_children_cache.clear();
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
