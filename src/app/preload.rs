use super::*;
use crate::panel::PreviewContent;

impl App {
    pub(crate) fn preload_images(&mut self, ctx: &egui::Context) {
        // Preview content follows the cursor; the core caches it by path,
        // so this is free while the cursor doesn't move.
        self.ws.sync_preview();

        // Texture preloading is a UI concern: warm the image cache around
        // the cursor while an image preview is open.
        // PR1 tabs model + per review: only the *active tab* on a side can host the visible preview.
        // A preview on an inactive tab of a side must not affect the opposite side.
        let (source, target) = if self.ws.right.tabs[self.ws.right.active].state.preview.is_some() {
            (&self.ws.left.tabs[self.ws.left.active].state, &self.ws.right.tabs[self.ws.right.active].state)
        } else if self.ws.left.tabs[self.ws.left.active].state.preview.is_some() {
            (&self.ws.right.tabs[self.ws.right.active].state, &self.ws.left.tabs[self.ws.left.active].state)
        } else {
            return;
        };
        if !matches!(target.preview, Some(PreviewContent::Image(_))) {
            return;
        }

        let entries = source.filtered_entries();
        let cur = source.cursor().saturating_sub(1);
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
