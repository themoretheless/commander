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

/// Lay out `text` on one line, cut to `max_width` with a trailing "…".
fn elided_galley(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    ui.painter().layout_job(job)
}

/// Paint a one-line label left-aligned at `left`, vertically centred in
/// `row`, elided at the row's right edge.
fn paint_row_label(
    ui: &egui::Ui,
    row: egui::Rect,
    left: f32,
    text: &str,
    size: f32,
    color: Color32,
) {
    let galley = elided_galley(
        ui,
        text,
        egui::FontId::proportional(size),
        color,
        row.right() - left - 4.0,
    );
    let pos = egui::pos2(left, row.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, color);
}

/// The keyboard focus ring for a painter-drawn sidebar row.
fn paint_focus_ring(ui: &egui::Ui, response: &egui::Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.shrink(1.0),
            CornerRadius::same(2),
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
}

/// A favorite's tooltip: its path, plus the Cmd+digit jump when it has a slot.
fn favorite_tooltip(path: &std::path::Path, slot: Option<u8>) -> String {
    match slot {
        Some(n) => format!("{}  (Cmd+{n})", path.display()),
        None => path.display().to_string(),
    }
}

/// The subfolder count drawn inside a tree folder icon; capped so it fits.
fn folder_count_label(count: usize) -> String {
    if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    }
}

/// Folder icon outline and count colours from the theme, so they follow the
/// light, dark and high-contrast palettes.
const fn tree_icon_colors(t: &ThemeColors, is_current: bool) -> (Color32, Color32) {
    if is_current {
        (t.accent, t.text_primary)
    } else {
        (t.accent_warning, t.text_secondary)
    }
}

/// Remember the folder the tree last showed as active; true when it changed
/// (or on first sight), so the tree should reveal and scroll to it.
fn note_active_path(last: &mut Option<PathBuf>, active: &std::path::Path) -> bool {
    if last.as_deref() == Some(active) {
        return false;
    }
    *last = Some(active.to_path_buf());
    true
}

/// Guide lines for a row whose ancestors' "has a later sibling" flags are
/// `continues` (outermost first, the row's own flag last): the columns that
/// run the full row height, and whether the row's own connector continues
/// below it to a later sibling.
fn guide_lines(continues: &[bool]) -> (Vec<usize>, bool) {
    let Some((own, ancestors)) = continues.split_last() else {
        return (Vec::new(), false);
    };
    let columns = ancestors
        .iter()
        .enumerate()
        .filter_map(|(column, more)| more.then_some(column))
        .collect();
    (columns, *own)
}

/// Pixels between tree levels.
const TREE_INDENT: f32 = 16.0;
/// Room for the disclosure triangle ahead of each folder icon.
const DISCLOSURE_WIDTH: f32 = 10.0;

/// State threaded through the tree's recursive walk.
struct TreeWalk<'a> {
    t: &'a ThemeColors,
    active_path: &'a std::path::Path,
    show_hidden: bool,
    expanded: &'a mut std::collections::HashSet<PathBuf>,
    cache: &'a mut ChildCache,
    /// Per level below the root: whether that ancestor (and, last, the
    /// current row) has a later sibling, so its guide line continues.
    continues: Vec<bool>,
    /// Scroll the active folder's row into view this frame.
    reveal_active: bool,
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

        // Follow navigation from the panes too: when the active folder
        // changes, open its ancestors and scroll its row into view.
        let last_id = egui::Id::new("tree_last_active_path");
        let mut last = ui
            .data(|d| d.get_temp::<Option<PathBuf>>(last_id))
            .flatten();
        let reveal_active = note_active_path(&mut last, &active_path);
        if reveal_active {
            ui.data_mut(|d| d.insert_temp(last_id, last));
            expand_ancestors(&mut self.tree_expanded, &active_path);
        }

        // Favorites rail: bookmarked directories with their quick-jump slots,
        // above the filesystem tree. Clicking one navigates the active panel.
        let rows = crate::bookmarks::rail_model(
            &self.ws.bookmarks,
            &self.ws.left.current_path,
            &self.ws.right.current_path,
        );
        let mut nav = Self::render_favorites(ui, t, &rows);

        let root = PathBuf::from("/");
        let mut walk = TreeWalk {
            t,
            active_path: &active_path,
            show_hidden,
            expanded: &mut self.tree_expanded,
            cache: &mut self.tree_children_cache,
            continues: Vec::new(),
            reveal_active,
        };
        let tree_nav = Self::render_tree_node_recursive(ui, &root, &mut walk);
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
            paint_focus_ring(ui, &resp);
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
            let name_color = if row.is_current {
                t.text_primary
            } else {
                t.text_secondary
            };
            paint_row_label(ui, rect, x, &row.name, 12.0, name_color);
            resp.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &row.name)
            });
            if resp.clicked() {
                nav = Some(row.path.clone());
            }
            resp.on_hover_text(favorite_tooltip(&row.path, row.slot));
        }
        ui.add_space(6.0);
        nav
    }

    fn render_tree_node_recursive(
        ui: &mut egui::Ui,
        path: &std::path::Path,
        walk: &mut TreeWalk<'_>,
    ) -> Option<PathBuf> {
        let depth = walk.continues.len();
        let is_expanded = walk.expanded.contains(path);
        let is_current = walk.active_path == path;
        let row_h = 22.0;
        let full_w = ui.available_width();
        let (row_rect, row_resp) = ui.allocate_exact_size(Vec2::new(full_w, row_h), Sense::click());
        if is_current && walk.reveal_active {
            ui.scroll_to_rect(row_rect, None);
        }

        // A row scrolled out of view keeps its layout slot but is neither
        // painted nor clickable, and needs its children only to lay out
        // an expanded subtree: no folder read or count for it.
        if !ui.is_rect_visible(row_rect) {
            if !is_expanded {
                return None;
            }
            let subdirs = cached_children(walk.cache, path, walk.show_hidden);
            return Self::render_tree_children(ui, &subdirs, walk);
        }

        let mut nav = None;
        let t = walk.t;
        let subdirs = cached_children(walk.cache, path, walk.show_hidden);
        let has_children = !subdirs.is_empty();

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());

        let indent = depth as f32 * TREE_INDENT;

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
        paint_focus_ring(ui, &row_resp);

        // Tree guide lines: an ancestor's column runs on only while that
        // ancestor has a later sibling, so lines stop at last children.
        if depth > 0 {
            let line_color = t.text_muted.linear_multiply(0.25);
            let stroke = Stroke::new(1.0_f32, line_color);
            let p = ui.painter();
            let column_x = |column: usize| row_rect.left() + column as f32 * TREE_INDENT + 9.0;
            let (full_columns, own_continues) = guide_lines(&walk.continues);
            for column in full_columns {
                let lx = column_x(column);
                p.line_segment(
                    [
                        egui::pos2(lx, row_rect.top()),
                        egui::pos2(lx, row_rect.bottom()),
                    ],
                    stroke,
                );
            }

            let lx = column_x(depth - 1);
            let cy = row_rect.center().y;
            let own_bottom = if own_continues { row_rect.bottom() } else { cy };
            p.line_segment(
                [egui::pos2(lx, row_rect.top()), egui::pos2(lx, own_bottom)],
                stroke,
            );
            // The branch meets the disclosure triangle, or the folder icon
            // of a row without one.
            let branch_end = if has_children {
                row_rect.left() + indent + 4.0 + 2.0
            } else {
                row_rect.left() + indent + 4.0 + DISCLOSURE_WIDTH - 2.0
            };
            p.line_segment([egui::pos2(lx, cy), egui::pos2(branch_end, cy)], stroke);
        }

        // Disclosure triangle: points right when collapsed, down when open.
        let disclosure_left = row_rect.left() + indent + 4.0;
        if has_children {
            let center = egui::pos2(disclosure_left + 4.0, row_rect.center().y);
            let points = if is_expanded {
                vec![
                    center + egui::vec2(-3.5, -2.0),
                    center + egui::vec2(3.5, -2.0),
                    center + egui::vec2(0.0, 2.5),
                ]
            } else {
                vec![
                    center + egui::vec2(-2.0, -3.5),
                    center + egui::vec2(2.5, 0.0),
                    center + egui::vec2(-2.0, 3.5),
                ]
            };
            ui.painter().add(egui::Shape::convex_polygon(
                points,
                t.text_muted,
                Stroke::NONE,
            ));
        }

        // Folder icon with subdir count
        let text_left = disclosure_left + DISCLOSURE_WIDTH;
        {
            let p = ui.painter();
            let (color, count_color) = tree_icon_colors(t, is_current);
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
                    folder_count_label(child_count),
                    egui::FontId::proportional(7.0),
                    count_color,
                );
            }
        }

        let name_left = text_left + 19.0;
        let name_color = if is_current {
            t.accent
        } else {
            t.text_secondary
        };
        paint_row_label(ui, row_rect, name_left, &name, 12.0, name_color);

        row_resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &name)
        });
        ui.ctx().accesskit_node_builder(row_resp.id, |node| {
            node.set_role(egui::accesskit::Role::TreeItem);
            node.set_selected(is_current);
            if has_children {
                node.set_expanded(is_expanded);
            }
        });

        // Handle clicks
        if row_resp.clicked() {
            toggle_tree_node(
                walk.expanded,
                walk.cache,
                path,
                walk.show_hidden,
                has_children,
            );
            nav = Some(path.to_path_buf());
        }
        row_resp.on_hover_text(path.display().to_string());

        // Render children if expanded
        if is_expanded {
            nav = Self::render_tree_children(ui, &subdirs, walk).or(nav);
        }

        nav
    }

    /// Render `children` one level deeper, tracking which of them has a
    /// later sibling for the guide lines. Returns a clicked folder, if any.
    fn render_tree_children(
        ui: &mut egui::Ui,
        children: &[PathBuf],
        walk: &mut TreeWalk<'_>,
    ) -> Option<PathBuf> {
        let mut nav = None;
        for (index, child) in children.iter().enumerate() {
            walk.continues.push(index + 1 < children.len());
            if let Some(child_nav) = Self::render_tree_node_recursive(ui, child, walk) {
                nav = Some(child_nav);
            }
            walk.continues.pop();
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
                let mut walk = TreeWalk {
                    t: &t,
                    active_path: &root,
                    show_hidden: false,
                    expanded: &mut expanded,
                    cache: &mut cache,
                    continues: Vec::new(),
                    reveal_active: false,
                };
                App::render_tree_node_recursive(ui, &root, &mut walk);
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

    #[test]
    fn guide_lines_stop_below_last_children() {
        // Root-level row: only its own connector.
        assert_eq!(guide_lines(&[]), (Vec::new(), false));
        assert_eq!(guide_lines(&[true]), (Vec::new(), true));
        // Under a last child (level 1 has no later sibling) that column is
        // not drawn, while level 2's continues.
        assert_eq!(guide_lines(&[false, true, false]), (vec![1], false));
        assert_eq!(guide_lines(&[true, true, true]), (vec![0, 1], true));
    }

    #[test]
    fn large_subfolder_counts_are_capped_to_fit_the_icon() {
        assert_eq!(folder_count_label(7), "7");
        assert_eq!(folder_count_label(99), "99");
        assert_eq!(folder_count_label(100), "99+");
        assert_eq!(folder_count_label(4_812), "99+");
    }

    #[test]
    fn favorite_tooltips_mention_the_jump_shortcut() {
        let path = std::path::Path::new("/work");
        assert_eq!(favorite_tooltip(path, Some(3)), "/work  (Cmd+3)");
        assert_eq!(favorite_tooltip(path, None), "/work");
    }

    #[test]
    fn the_tree_follows_the_active_folder_once_per_change() {
        let mut last = None;
        assert!(note_active_path(&mut last, std::path::Path::new("/a")));
        assert!(!note_active_path(&mut last, std::path::Path::new("/a")));
        assert!(note_active_path(&mut last, std::path::Path::new("/a/b")));
        assert_eq!(last, Some(PathBuf::from("/a/b")));
    }

    fn contrast(left: Color32, right: Color32) -> f32 {
        let luminance = |color: Color32| {
            let linear = |channel: u8| {
                let value = f32::from(channel) / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
        };
        let (a, b) = (luminance(left), luminance(right));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn tree_folder_icons_are_legible_in_every_theme() {
        use crate::theme::ThemeMode;
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            for high_contrast in [false, true] {
                let t = ThemeColors::for_preferences(
                    mode,
                    crate::accessibility::Preferences {
                        high_contrast,
                        ..Default::default()
                    },
                );
                for is_current in [false, true] {
                    let (outline, count) = tree_icon_colors(&t, is_current);
                    // The current row sits on a tinted highlight; the
                    // plain sidebar background is the worst case for both.
                    assert!(
                        contrast(outline, t.bg_deep) >= 3.0,
                        "high contrast {high_contrast}, current {is_current}"
                    );
                    assert!(
                        contrast(count, t.bg_deep) >= 4.5,
                        "high contrast {high_contrast}, current {is_current}"
                    );
                }
            }
        }
    }

    #[test]
    fn long_sidebar_names_are_elided_to_the_row() {
        let ctx = egui::Context::default();
        let mut elided = None;
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
            let galley = elided_galley(
                ui,
                &"very long folder name ".repeat(20),
                egui::FontId::proportional(12.0),
                Color32::WHITE,
                120.0,
            );
            elided = Some((galley.elided, galley.size().x));
        }));
        let (was_elided, width) = elided.expect("ran");
        assert!(was_elided);
        assert!(width <= 120.0);
    }

    #[test]
    fn tree_rows_publish_name_and_expanded_state() {
        let temp = crate::testutil::TempDir::new();
        let root = temp.path().to_path_buf();
        std::fs::create_dir(root.join("child")).expect("create child");
        let mut expanded = std::collections::HashSet::from([root.clone()]);
        let mut cache = ChildCache::new();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let t = ThemeColors::dark();
        let output =
            crate::testutil::take_egui_output(ctx.run_ui(egui::RawInput::default(), |ui| {
                let mut walk = TreeWalk {
                    t: &t,
                    active_path: &root,
                    show_hidden: false,
                    expanded: &mut expanded,
                    cache: &mut cache,
                    continues: Vec::new(),
                    reveal_active: false,
                };
                App::render_tree_node_recursive(ui, &root, &mut walk);
            }));
        let nodes = output
            .platform_output
            .accesskit_update
            .expect("headless AccessKit output")
            .nodes;
        let item = |label: &str| {
            nodes
                .iter()
                .map(|(_, node)| node)
                .find(|node| node.label() == Some(label))
                .unwrap_or_else(|| panic!("no tree item {label:?}"))
        };
        let root_name = root.file_name().expect("named").to_string_lossy();
        let root_item = item(&root_name);
        assert_eq!(root_item.role(), egui::accesskit::Role::TreeItem);
        assert_eq!(root_item.is_expanded(), Some(true));
        assert_eq!(root_item.is_selected(), Some(true));
        assert_eq!(item("child").is_expanded(), None);
    }
}
