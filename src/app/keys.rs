use super::*;

impl App {
    pub(crate) fn handle_keys(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            // Tab — switch panel
            if i.key_pressed(egui::Key::Tab) {
                self.active = match self.active {
                    ActivePanel::Left => ActivePanel::Right,
                    ActivePanel::Right => ActivePanel::Left,
                };
            }

            // Up / Down — move cursor (0 = ".." row, 1.. = files)
            if i.key_pressed(egui::Key::ArrowUp) {
                let panel = match self.active {
                    ActivePanel::Left => &mut self.left,
                    ActivePanel::Right => &mut self.right,
                };
                if panel.cursor > 0 {
                    panel.cursor -= 1;
                    panel.scroll_to_cursor = true;
                }
            }
            if i.key_pressed(egui::Key::ArrowDown) {
                let panel = match self.active {
                    ActivePanel::Left => &mut self.left,
                    ActivePanel::Right => &mut self.right,
                };
                let max = panel.filtered_entries().len();
                if panel.cursor < max {
                    panel.cursor += 1;
                    panel.scroll_to_cursor = true;
                }
            }

            // Update preview if open and cursor moved
            if i.key_pressed(egui::Key::ArrowUp) || i.key_pressed(egui::Key::ArrowDown) {
                let other_has_preview = match self.active {
                    ActivePanel::Left => self.right.preview.is_some(),
                    ActivePanel::Right => self.left.preview.is_some(),
                };
                if other_has_preview {
                    // Handled in preload_images
                }
            }

            // Enter — ".." goes up, otherwise open dir/file
            if i.key_pressed(egui::Key::Enter) {
                let panel = match self.active {
                    ActivePanel::Left => &mut self.left,
                    ActivePanel::Right => &mut self.right,
                };
                if panel.cursor == 0 {
                    panel.go_up();
                } else {
                    let file_idx = panel.cursor - 1;
                    if let Some(entry) = panel.filtered_entries().get(file_idx).cloned() {
                        if entry.is_dir {
                            let path = entry.path.clone();
                            panel.navigate_to(path);
                        } else {
                            let _ = open::that(&entry.path);
                        }
                    }
                }
            }

            // Backspace — go up
            if i.key_pressed(egui::Key::Backspace) {
                match self.active {
                    ActivePanel::Left => self.left.go_up(),
                    ActivePanel::Right => self.right.go_up(),
                };
            }

            // Space — toggle select (skip ".." row)
            if i.key_pressed(egui::Key::Space) {
                let panel = match self.active {
                    ActivePanel::Left => &mut self.left,
                    ActivePanel::Right => &mut self.right,
                };
                if panel.cursor > 0 {
                    let file_idx = panel.cursor - 1;
                    panel.toggle_select(file_idx);
                }
                let max = panel.filtered_entries().len();
                if panel.cursor < max {
                    panel.cursor += 1;
                }
            }

            // F3 — toggle preview in other panel
            if i.key_pressed(egui::Key::F3) {
                let other = match self.active {
                    ActivePanel::Left => &mut self.right,
                    ActivePanel::Right => &mut self.left,
                };
                if other.preview.is_some() {
                    other.preview = None;
                } else {
                    let preview = {
                        let panel = match self.active {
                            ActivePanel::Left => &self.left,
                            ActivePanel::Right => &self.right,
                        };
                        panel.filtered_entries()
                            .get(panel.cursor.saturating_sub(1))
                            .map(|e| Self::make_preview(e))
                            .flatten()
                    };
                    let other = match self.active {
                        ActivePanel::Left => &mut self.right,
                        ActivePanel::Right => &mut self.left,
                    };
                    other.preview = preview;
                }
            }

            // F5 — copy (show confirmation)
            if i.key_pressed(egui::Key::F5) {
                self.request_copy();
            }

            // F6 — move (show confirmation)
            if i.key_pressed(egui::Key::F6) {
                self.request_move();
            }

            // F7 — new dir
            if i.key_pressed(egui::Key::F7) {
                self.create_dir();
            }

            // F8 / Delete — delete (show confirmation)
            if i.key_pressed(egui::Key::F8) || i.key_pressed(egui::Key::Delete) {
                self.request_delete();
            }

            // Cmd+A — select all
            if i.modifiers.command && i.key_pressed(egui::Key::A) {
                match self.active {
                    ActivePanel::Left => self.left.select_all(),
                    ActivePanel::Right => self.right.select_all(),
                };
            }

            // Cmd+H — toggle hidden
            if i.modifiers.command && i.key_pressed(egui::Key::H) {
                let panel = match self.active {
                    ActivePanel::Left => &mut self.left,
                    ActivePanel::Right => &mut self.right,
                };
                panel.show_hidden = !panel.show_hidden;
                panel.refresh();
            }
        });
    }
}
