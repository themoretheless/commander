use super::*;
use crate::image_cache::MAX_CACHE_BYTES;
use crate::panel::PreviewContent;

const PRELOAD_BACK_WINDOW: usize = 10;
const PRELOAD_TARGET_FORWARD_WINDOW: usize = 50;
const ASSUMED_PRELOAD_IMAGE_BYTES: usize = 5 * 1024 * 1024;

fn forward_preload_window(current_cache_bytes: usize) -> usize {
    let remaining_bytes = MAX_CACHE_BYTES.saturating_sub(current_cache_bytes);
    let remaining_image_slots = remaining_bytes / ASSUMED_PRELOAD_IMAGE_BYTES;
    remaining_image_slots.min(PRELOAD_TARGET_FORWARD_WINDOW)
}

impl App {
    pub(crate) fn preload_images(&mut self, ctx: &egui::Context) {
        // Preview content follows the cursor; the core caches it by path,
        // so this is free while the cursor doesn't move.
        self.ws.sync_preview();

        // Texture preloading is a UI concern: warm the image cache around
        // the cursor while an image preview is open.
        let (source, target) = if self.ws.right.preview.is_some() {
            (&self.ws.left, &self.ws.right)
        } else if self.ws.left.preview.is_some() {
            (&self.ws.right, &self.ws.left)
        } else {
            return;
        };
        if !matches!(target.preview, Some(PreviewContent::Image(_))) {
            return;
        }

        let cur = source.cursor.saturating_sub(1);
        let start = cur.saturating_sub(PRELOAD_BACK_WINDOW);
        let (_, current_bytes) = self.image_cache.stats();
        let forward = forward_preload_window(current_bytes);
        let end = cur
            .saturating_add(1)
            .saturating_add(forward)
            .min(source.filtered_count());
        let paths: Vec<PathBuf> = (start..end)
            .filter_map(|i| source.filtered_get(i))
            .filter(|e| e.is_image())
            .map(|e| e.path.clone())
            .collect();

        self.image_cache.preload(ctx, &paths, &source.current_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_preload_window_is_capped_by_target_window() {
        assert_eq!(forward_preload_window(0), PRELOAD_TARGET_FORWARD_WINDOW);
    }

    #[test]
    fn forward_preload_window_shrinks_with_remaining_cache_budget() {
        let nearly_full = MAX_CACHE_BYTES - ASSUMED_PRELOAD_IMAGE_BYTES * 3;
        assert_eq!(forward_preload_window(nearly_full), 3);
    }

    #[test]
    fn forward_preload_window_stops_when_cache_is_full() {
        assert_eq!(forward_preload_window(MAX_CACHE_BYTES), 0);
        assert_eq!(forward_preload_window(MAX_CACHE_BYTES + 1), 0);
    }
}
