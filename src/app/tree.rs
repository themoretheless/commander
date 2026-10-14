use super::*;

const TREE_CHILD_CACHE_LIMIT: usize = 2_048;

fn child_cache_key(path: &std::path::Path, show_hidden: bool) -> (PathBuf, bool) {
    (path.to_path_buf(), show_hidden)
}

fn cache_children(cache: &mut ChildCache, key: (PathBuf, bool), children: TreeChildren) {
    if !cache.contains_key(&key) && cache.len() >= TREE_CHILD_CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, children);
}

/// Shared child list: rows clone the `Arc`, not every child path, per frame.
pub(super) type TreeChildren = std::sync::Arc<[PathBuf]>;
pub(super) type ChildCache = std::collections::HashMap<(PathBuf, bool), TreeChildren>;

/// Child folders of `path`, read from disk only on a cache miss.
fn cached_children(
    cache: &mut ChildCache,
    path: &std::path::Path,
    show_hidden: bool,
) -> TreeChildren {
    let key = child_cache_key(path, show_hidden);
    if let Some(cached) = cache.get(&key) {
        return cached.clone();
    }
    let children = TreeChildren::from(PanelState::subdirs(path, show_hidden));
    cache_children(cache, key, children.clone());
    children
}

/// A click on a tree row toggles its expansion and drops its cached child
/// list, so expanding re-reads the folder (the cache is otherwise cleared
/// only by pane listing changes, which never cover folders outside both
/// panes).
fn toggle_tree_node(
    expanded: &mut std::collections::HashSet<PathBuf>,
    cache: &mut ChildCache,
    path: &std::path::Path,
    show_hidden: bool,
    has_children: bool,
) {
    cache.remove(&child_cache_key(path, show_hidden));
    if has_children && !expanded.remove(path) {
        expanded.insert(path.to_path_buf());
    }
}

/// Expand every ancestor of `path` so its row is visible. `path` itself
/// keeps its state: a tree click has just toggled it, and re-expanding it
/// here would make collapsing impossible.
pub(super) fn expand_ancestors(
    expanded: &mut std::collections::HashSet<PathBuf>,
    path: &std::path::Path,
) {
    for ancestor in path.ancestors().skip(1) {
        expanded.insert(ancestor.to_path_buf());
    }
}

/// Reveal `path` and open it too: used when the tree is shown, so the
/// current folder's subfolders are visible as before.
pub(super) fn expand_through(
    expanded: &mut std::collections::HashSet<PathBuf>,
    path: &std::path::Path,
) {
    expand_ancestors(expanded, path);
    expanded.insert(path.to_path_buf());
}

impl App {
    /// Render the global tree sidebar. Returns Some(path) if user clicked a folder.
    pub(crate) fn render_global_tree(
        &mut self,
        ui: &mut egui::Ui,
        t: &ThemeColors,
    ) -> Option<PathBuf> {
        let active_path = self.ws.active_panel_ref().current_path.clone();
        let show_hidden = self.ws.active_panel_ref().show_hidden();

        // Favorites rail: bookmarked directories with their quick-jump slots,
        // above the filesystem tree. Clicking one navigates the active panel.
        let rows = crate::bookmarks::rail_model(
            &self.ws.bookmarks,
            &self.ws.left.current_path,
            &self.ws.right.current_path,
        );
        let mut nav = Self::render_favorites(ui, t, &rows);

        let root = PathBuf::from("/");
        let tree_nav = Self::render_tree_node_recursive(
            ui,
            &root,
            0,
            t,
            &active_path,
            show_hidden,
            &mut self.tree_expanded,
            &mut self.tree_children_cache,
        );
        nav = nav.or(tree_nav);
        nav
    }

    /// Draw the Favorites rail from a pure [`rail_model`](crate::bookmarks::rail_model)
    /// row list. Returns the path of a clicked favorite, if any.
    fn render_favorites(
        ui: &mut egui::Ui,
        t: &ThemeColors,
        rows: &[crate::bookmarks::RailRow],
    ) -> Option<PathBuf> {
        if rows.is_empty() {
            return None;
        }
        let mut nav = None;
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("FAVORITES")
                .size(9.0)
                .color(t.text_muted),
        );
        let full_w = ui.available_width();
        for row in rows {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(full_w, 22.0), Sense::click());
            if row.is_current {
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::same(2),
                    t.bg_selected.linear_multiply(0.25),
                );
            } else if resp.hovered() {
                ui.painter().rect_filled(
                    rect,
                    CornerRadius::same(2),
                    t.bg_hover.linear_multiply(0.3),
                );
            }
            let mut x = rect.left() + 6.0;
            // Slot digit (fixed-width gutter so names align with/without a slot).
            let slot_text = row.slot.map(|n| n.to_string()).unwrap_or_default();
            ui.painter().text(
                egui::pos2(x, rect.center().y),
                egui::Align2::LEFT_CENTER,
                &slot_text,
                egui::FontId::proportional(11.0),
                t.accent,
            );
            x += 14.0;
            ui.painter().text(
                egui::pos2(x, rect.center().y),
                egui::Align2::LEFT_CENTER,
                &row.name,
                egui::FontId::proportional(12.0),
                if row.is_current {
                    t.text_primary
                } else {
                    t.text_secondary
                },
            );
            if resp.clicked() {
                nav = Some(row.path.clone());
            }
            resp.on_hover_text(row.path.display().to_string());
        }
        ui.add_space(6.0);
        nav
    }

    // Recursive walk threading mutable `expanded`/`cache` borrows through
    // each level; bundling the args would force those out of `App`.
    #[allow(clippy::too_many_arguments)]
    fn render_tree_node_recursive(
        ui: &mut egui::Ui,
        path: &std::path::Path,
        depth: usize,
        t: &ThemeColors,
        active_path: &std::path::Path,
        show_hidden: bool,
        expanded: &mut std::collections::HashSet<PathBuf>,
        cache: &mut ChildCache,
    ) -> Option<PathBuf> {
        let is_expanded = expanded.contains(path);
        let row_h = 22.0;
        let full_w = ui.available_width();
        let (row_rect, row_resp) = ui.allocate_exact_size(Vec2::new(full_w, row_h), Sense::click());

        // A row scrolled out of view keeps its layout slot but is neither
        // painted nor clickable, and needs its children only to lay out
        // an expanded subtree: no folder read or count for it.
        if !ui.is_rect_visible(row_rect) {
            if !is_expanded {
                return None;
            }
            let subdirs = cached_children(cache, path, show_hidden);
            let mut nav = None;
            for child in subdirs.iter() {
                if let Some(child_nav) = Self::render_tree_node_recursive(
                    ui,
                    child,
                    depth + 1,
                    t,
                    active_path,
                    show_hidden,
                    expanded,
                    cache,
                ) {
                    nav = Some(child_nav);
                }
            }
            return nav;
        }

        let mut nav = None;
        let is_current = active_path == path;
        let subdirs = cached_children(cache, path, show_hidden);
        let has_children = !subdirs.is_empty();

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());

        let indent = depth as f32 * 16.0;

        // Highlight current directory
        if is_current {
            ui.painter().rect_filled(
                row_rect,
                CornerRadius::same(2),
                t.bg_selected.linear_multiply(0.25),
            );
        }
        if row_resp.hovered() && !is_current {
            ui.painter().rect_filled(
                row_rect,
                CornerRadius::same(2),
                t.bg_hover.linear_multiply(0.3),
            );
        }

        // Tree guide lines
        if depth > 0 {
            let line_color = t.text_muted.linear_multiply(0.25);
            let line_w = 1.0_f32;
            let p = ui.painter();

            for d in 1..depth {
                let lx = row_rect.left() + (d as f32 - 1.0) * 16.0 + 4.0 + 5.0;
                p.line_segment(
                    [
                        egui::pos2(lx, row_rect.top()),
                        egui::pos2(lx, row_rect.bottom()),
                    ],
                    Stroke::new(line_w, line_color),
                );
            }

            let lx = row_rect.left() + (depth as f32 - 1.0) * 16.0 + 4.0 + 5.0;
            let cy = row_rect.center().y;
            p.line_segment(
                [egui::pos2(lx, row_rect.top()), egui::pos2(lx, cy)],
                Stroke::new(line_w, line_color),
            );
            let branch_end = row_rect.left() + indent + 4.0 + 2.0;
            p.line_segment(
                [egui::pos2(lx, cy), egui::pos2(branch_end, cy)],
                Stroke::new(line_w, line_color),
            );
        }

        // Folder icon with subdir count
        let text_left = row_rect.left() + indent + 4.0;
        {
            let p = ui.painter();
            let color = if is_current {
                Color32::from_rgb(220, 190, 80)
            } else {
                Color32::from_rgb(170, 150, 90)
            };
            let s = Stroke::new(1.0_f32, color);
            let ix = text_left;
            let iy = row_rect.center().y - 7.0;
            let iw = 16.0;
            let ih = 13.0;
            let tw = iw * 0.35;
            let th = 3.0;

            let tab = [
                egui::pos2(ix + 0.5, iy + th),
                egui::pos2(ix + 0.5, iy + 0.5),
                egui::pos2(ix + tw, iy + 0.5),
                egui::pos2(ix + tw + 1.5, iy + th),
            ];
            for i in 0..tab.len() - 1 {
                p.line_segment([tab[i], tab[i + 1]], s);
            }

            let body = egui::Rect::from_min_size(
                egui::pos2(ix + 0.5, iy + th),
                egui::vec2(iw - 1.0, ih - th - 0.5),
            );
            p.rect_stroke(body, CornerRadius::ZERO, s, egui::StrokeKind::Outside);

            let child_count = subdirs.len();
            if child_count > 0 {
                p.text(
                    body.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{}", child_count),
                    egui::FontId::proportional(7.0),
                    color,
                );
            }
        }

        let name_left = text_left + 19.0;
        let name_color = if is_current {
            t.accent
        } else {
            t.text_secondary
        };
        ui.painter().text(
            egui::pos2(name_left, row_rect.center().y),
            egui::Align2::LEFT_CENTER,
            &name,
            egui::FontId::proportional(12.0),
            name_color,
        );

        // Handle clicks
        if row_resp.clicked() {
            toggle_tree_node(expanded, cache, path, show_hidden, has_children);
            nav = Some(path.to_path_buf());
        }

        // Render children if expanded
        if is_expanded {
            for child in subdirs.iter() {
                if let Some(child_nav) = Self::render_tree_node_recursive(
                    ui,
                    child,
                    depth + 1,
                    t,
                    active_path,
                    show_hidden,
                    expanded,
                    cache,
                ) {
                    nav = Some(child_nav);
                }
            }
        }

        nav
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicking_an_expanded_node_collapses_it_and_it_stays_collapsed() {
        let path = PathBuf::from("/a/b");
        let mut expanded = std::collections::HashSet::from([
            PathBuf::from("/"),
            PathBuf::from("/a"),
            path.clone(),
        ]);
        let mut cache = ChildCache::new();
        toggle_tree_node(&mut expanded, &mut cache, &path, false, true);
        // The caller then reveals the navigated path.
        expand_ancestors(&mut expanded, &path);
        assert!(!expanded.contains(&path));
        assert!(expanded.contains(&PathBuf::from("/a")));

        toggle_tree_node(&mut expanded, &mut cache, &path, false, true);
        expand_ancestors(&mut expanded, &path);
        assert!(expanded.contains(&path));
    }

    #[test]
    fn showing_the_tree_opens_the_current_folder() {
        let path = PathBuf::from("/a/b");
        let mut expanded = std::collections::HashSet::new();
        expand_through(&mut expanded, &path);
        assert!(expanded.contains(&path));
        assert!(expanded.contains(&PathBuf::from("/a")));
        assert!(expanded.contains(&PathBuf::from("/")));
    }

    #[test]
    fn clicking_a_node_rereads_its_children() {
        let path = PathBuf::from("/outside/panes");
        let mut expanded = std::collections::HashSet::new();
        let mut cache = ChildCache::new();
        cache_children(
            &mut cache,
            child_cache_key(&path, false),
            TreeChildren::from(vec![PathBuf::from("/outside/panes/stale")]),
        );
        toggle_tree_node(&mut expanded, &mut cache, &path, false, false);
        assert!(!cache.contains_key(&child_cache_key(&path, false)));
        assert!(expanded.is_empty());
    }

    #[test]
    fn cached_children_are_shared_not_reread() {
        let temp = crate::testutil::TempDir::new();
        std::fs::create_dir(temp.path().join("sub")).expect("create sub");
        let mut cache = ChildCache::new();
        let first = cached_children(&mut cache, temp.path(), false);
        assert_eq!(first.len(), 1);
        std::fs::create_dir(temp.path().join("later")).expect("create later");
        let second = cached_children(&mut cache, temp.path(), false);
        assert!(std::sync::Arc::ptr_eq(&first, &second), "served from cache");
        cache.clear();
        assert_eq!(cached_children(&mut cache, temp.path(), false).len(), 2);
    }

    #[test]
    fn offscreen_collapsed_rows_do_not_read_their_folder() {
        let temp = crate::testutil::TempDir::new();
        let root = temp.path().to_path_buf();
        for index in 0..60 {
            std::fs::create_dir(root.join(format!("d{index:02}"))).expect("create dir");
        }
        let mut expanded = std::collections::HashSet::from([root.clone()]);
        let mut cache = ChildCache::new();
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(300.0, 200.0),
            )),
            ..Default::default()
        };
        let t = crate::theme::ThemeColors::for_preferences(
            crate::theme::ThemeMode::Dark,
            crate::accessibility::Preferences::default(),
        );
        crate::testutil::discard_egui_output(ctx.run_ui(input, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                App::render_tree_node_recursive(
                    ui,
                    &root,
                    0,
                    &t,
                    &root,
                    false,
                    &mut expanded,
                    &mut cache,
                );
            });
        }));
        assert!(cache.contains_key(&child_cache_key(&root, false)));
        assert!(cache.contains_key(&child_cache_key(&root.join("d00"), false)));
        // 61 rows of 22 px cannot fit in 200 px: the tail is never read.
        assert!(!cache.contains_key(&child_cache_key(&root.join("d59"), false)));
    }

    #[test]
    fn hidden_policy_is_part_of_the_tree_child_cache_key() {
        let path = std::path::Path::new("/tmp/example");
        assert_ne!(child_cache_key(path, false), child_cache_key(path, true));
    }

    #[test]
    fn tree_child_cache_is_bounded_under_path_and_policy_churn() {
        let mut cache = std::collections::HashMap::new();
        for index in 0..=TREE_CHILD_CACHE_LIMIT {
            cache_children(
                &mut cache,
                (PathBuf::from(format!("/tmp/{index}")), index % 2 == 0),
                TreeChildren::from(Vec::new()),
            );
        }

        assert!(cache.len() <= TREE_CHILD_CACHE_LIMIT);
        assert!(cache.contains_key(&(
            PathBuf::from(format!("/tmp/{TREE_CHILD_CACHE_LIMIT}")),
            TREE_CHILD_CACHE_LIMIT.is_multiple_of(2)
        )));
    }
}
