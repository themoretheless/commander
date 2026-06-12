use super::*;
use crate::panel::PreviewContent;

impl App {
    pub(crate) fn preload_images(&mut self, ctx: &egui::Context) {
        // Find which panel has preview open, preload from the other panel's file list
        let has_image_preview = |p: &PanelState| matches!(p.preview, Some(PreviewContent::Image(_)));

        let (source, target) = if has_image_preview(&self.right) || self.right.preview.is_some() {
            (&self.left, &mut self.right)
        } else if has_image_preview(&self.left) || self.left.preview.is_some() {
            (&self.right, &mut self.left)
        } else {
            return;
        };

        let entries = source.filtered_entries();
        let cur = source.cursor.saturating_sub(1);

        // Sync preview with current cursor position
        if let Some(entry) = entries.get(cur) {
            let new_preview = Self::make_preview(entry);
            let same = match (&target.preview, &new_preview) {
                (Some(PreviewContent::Image(a)), Some(PreviewContent::Image(b))) => a == b,
                (Some(PreviewContent::Text { path: a, .. }), Some(PreviewContent::Text { path: b, .. })) => a == b,
                (None, None) => true,
                _ => false,
            };
            if !same {
                target.preview = new_preview;
            }
        }

        // Image preloading only if current preview is image
        if !has_image_preview(target) {
            return;
        }

        let start = cur.saturating_sub(10);
        let (_, current_bytes) = self.image_cache.stats();
        let budget = 1024 * 1024 * 1024; // 1 GB
        let forward = if current_bytes < budget {
            let remaining = budget - current_bytes;
            let extra = remaining / (5 * 1024 * 1024);
            50 + extra
        } else {
            50
        };
        let end = (cur + 1 + forward).min(entries.len());
        let paths: Vec<PathBuf> = entries[start..end]
            .iter()
            .filter(|e| e.is_image())
            .map(|e| e.path.clone())
            .collect();

        self.image_cache.preload(ctx, &paths, &source.current_path);
    }
}
