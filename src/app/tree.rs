use super::*;

impl App {
    /// Render the global tree sidebar. Returns Some(path) if user clicked a folder.
    pub(crate) fn render_global_tree(
        &mut self,
        ui: &mut egui::Ui,
        t: &ThemeColors,
    ) -> Option<PathBuf> {
        let active_path = self.ws.active_panel_ref().current_path.clone();
        let show_hidden = self.ws.active_panel_ref().show_hidden;
        let root = PathBuf::from("/");
        Self::render_tree_node_recursive(
            ui,
            &root,
            0,
            t,
            &active_path,
            show_hidden,
            &mut self.tree_expanded,
            &mut self.tree_children_cache,
        )
    }

    fn render_tree_node_recursive(
        ui: &mut egui::Ui,
        path: &std::path::Path,
        depth: usize,
        t: &ThemeColors,
        active_path: &std::path::Path,
        show_hidden: bool,
        expanded: &mut std::collections::HashSet<PathBuf>,
        cache: &mut std::collections::HashMap<PathBuf, Vec<PathBuf>>,
    ) -> Option<PathBuf> {
        let mut nav = None;
        let is_current = active_path == path;
        let is_expanded = expanded.contains(path);

        // Get subdirs (cached)
        let subdirs = if let Some(cached) = cache.get(path) {
            cached.clone()
        } else {
            let dirs = PanelState::subdirs(path, show_hidden);
            cache.insert(path.to_path_buf(), dirs.clone());
            dirs
        };
        let has_children = !subdirs.is_empty();

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "/".to_string());

        let indent = depth as f32 * 16.0;
        let row_h = 22.0;
        let full_w = ui.available_width();
        let (row_rect, row_resp) = ui.allocate_exact_size(Vec2::new(full_w, row_h), Sense::click());

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
            let line_w = 1.0;
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
            let s = Stroke::new(1.0, color);
            let ix = text_left;
            let iy = row_rect.center().y - 7.0;
            let iw = 16.0;
            let ih = 13.0;
            let tw = iw * 0.35;
            let th = 3.0;

            let tab = vec![
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
            if has_children {
                if is_expanded {
                    expanded.remove(path);
                } else {
                    expanded.insert(path.to_path_buf());
                }
            }
            nav = Some(path.to_path_buf());
        }

        // Render children if expanded
        if is_expanded {
            for child in &subdirs {
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
