use super::*;
use crate::image_cache::MAX_CACHE_BYTES;
use crate::panel::PreviewContent;

const PRELOAD_BACK_WINDOW: usize = 10;
const PRELOAD_TARGET_FORWARD_WINDOW: usize = 50;
const PRELOAD_SCAN_ROWS: usize = 1_000;
const ASSUMED_PRELOAD_IMAGE_BYTES: usize = 5 * 1024 * 1024;

fn forward_preload_window(current_cache_bytes: usize) -> usize {
    let remaining_bytes = MAX_CACHE_BYTES.saturating_sub(current_cache_bytes);
    let remaining_image_slots = remaining_bytes / ASSUMED_PRELOAD_IMAGE_BYTES;
    remaining_image_slots.min(PRELOAD_TARGET_FORWARD_WINDOW)
}

fn push_unique_index(
    ordered: &mut Vec<usize>,
    seen: &mut std::collections::HashSet<usize>,
    index: usize,
    total: usize,
    limit: usize,
) {
    if index < total && ordered.len() < limit && seen.insert(index) {
        ordered.push(index);
    }
}

fn prioritized_indices(
    total: usize,
    visible_start: usize,
    visible_rows: usize,
    cursor: usize,
) -> Vec<usize> {
    if total == 0 {
        return Vec::new();
    }
    let limit = total.min(PRELOAD_SCAN_ROWS);
    let cursor = cursor.min(total - 1);
    let start = visible_start.min(total);
    let end = start.saturating_add(visible_rows.max(1)).min(total);
    let mut ordered = Vec::with_capacity(limit);
    let mut seen = std::collections::HashSet::with_capacity(limit);

    push_unique_index(&mut ordered, &mut seen, cursor, total, limit);
    let mut visible: Vec<_> = (start..end).collect();
    visible.sort_by_key(|index| index.abs_diff(cursor));
    for index in visible {
        push_unique_index(&mut ordered, &mut seen, index, total, limit);
    }

    let mut distance = 0usize;
    while ordered.len() < limit {
        let before = start.checked_sub(distance.saturating_add(1));
        let after = end.saturating_add(distance);
        let prior_len = ordered.len();
        if after < total {
            push_unique_index(&mut ordered, &mut seen, after, total, limit);
        }
        if let Some(before) = before {
            push_unique_index(&mut ordered, &mut seen, before, total, limit);
        }
        if ordered.len() == prior_len && after >= total && before.is_none() {
            break;
        }
        distance = distance.saturating_add(1);
    }
    ordered
}

#[derive(Clone, Copy)]
enum PreviewPane {
    Left,
    Right,
}

fn preview_mut(ws: &mut Workspace, pane: PreviewPane) -> &mut Option<PreviewContent> {
    match pane {
        PreviewPane::Left => &mut ws.left.preview,
        PreviewPane::Right => &mut ws.right.preview,
    }
}

impl App {
    pub(crate) fn preload_images(&mut self, ctx: &egui::Context) {
        // Workspace synchronization now creates only image markers or pending
        // text identities; it never reads text on the UI thread.
        self.ws.sync_preview();

        let target_pane = if self.ws.right.preview.is_some() {
            PreviewPane::Right
        } else if self.ws.left.preview.is_some() {
            PreviewPane::Left
        } else {
            self.image_cache.cancel_text_preview();
            return;
        };

        let (text_identity, image_open, info_open) = {
            let target = match target_pane {
                PreviewPane::Right => &self.ws.right,
                PreviewPane::Left => &self.ws.left,
            };
            let text_identity = match target.preview.as_ref() {
                Some(PreviewContent::Pending(identity))
                | Some(PreviewContent::Text { identity, .. }) => Some(identity.clone()),
                Some(PreviewContent::Image(_)) | Some(PreviewContent::Info(_)) => None,
                None => unreachable!("preview pane was selected from a non-empty preview"),
            };
            (
                text_identity,
                matches!(target.preview, Some(PreviewContent::Image(_))),
                matches!(target.preview, Some(PreviewContent::Info(_))),
            )
        };

        if info_open {
            self.image_cache.cancel_text_preview();
            return;
        }
        if let Some(identity) = text_identity {
            let poll = self.image_cache.request_text_preview(ctx, &identity);
            match poll {
                crate::image_cache::TextPreviewPoll::Ready(content) => {
                    *preview_mut(&mut self.ws, target_pane) =
                        Some(PreviewContent::Text { identity, content });
                }
                crate::image_cache::TextPreviewPoll::Loading
                | crate::image_cache::TextPreviewPoll::Failed(_)
                | crate::image_cache::TextPreviewPoll::Current => {}
            }
            return;
        }

        self.image_cache.cancel_text_preview();
        if !image_open {
            return;
        }

        // Texture preloading is a UI concern: warm the image cache around
        // the cursor while an image preview is open.
        let source = match target_pane {
            PreviewPane::Right => &self.ws.left,
            PreviewPane::Left => &self.ws.right,
        };

        let Some(cursor) = source.cursor().checked_sub(1) else {
            return;
        };
        let image_stats = self.image_cache.stats();
        let forward = forward_preload_window(image_stats.bytes);
        let slots = PRELOAD_BACK_WINDOW.saturating_add(forward);
        let paths: Vec<PathBuf> = prioritized_indices(
            source.filtered_count(),
            source.scroll_anchor(),
            source.page_rows(),
            cursor,
        )
        .into_iter()
        .filter_map(|index| source.filtered_get(index))
        .filter(|e| e.is_image())
        .take(slots)
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

    #[test]
    fn visible_rows_are_prioritized_before_nearby_offscreen_rows() {
        let order = prioritized_indices(20, 5, 4, 7);

        assert_eq!(order[0], 7);
        let visible_positions: Vec<_> = (5..9)
            .map(|index| {
                order
                    .iter()
                    .position(|candidate| *candidate == index)
                    .unwrap()
            })
            .collect();
        let first_offscreen = order.iter().position(|index| *index == 9).unwrap();
        assert!(
            visible_positions
                .into_iter()
                .all(|position| position < first_offscreen)
        );
    }

    #[test]
    fn priority_order_is_unique_bounded_and_covers_small_lists() {
        let order = prioritized_indices(12, 99, 0, 99);
        let unique: std::collections::HashSet<_> = order.iter().copied().collect();

        assert_eq!(order.len(), 12);
        assert_eq!(unique.len(), order.len());
        assert_eq!(order[0], 11);
    }
}
