use super::*;

fn compact_path(path: &std::path::Path, max_chars: usize) -> String {
    let full = path.display().to_string();
    if full.chars().count() <= max_chars {
        return full;
    }
    let keep = max_chars.saturating_sub(3);
    let tail: String = full
        .chars()
        .rev()
        .take(keep)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("...{tail}")
}

impl App {
    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let t = self.colors;
        let active_side = match self.ws.active {
            ActivePanel::Left => "LEFT",
            ActivePanel::Right => "RIGHT",
        };
        let active_path = compact_path(&self.ws.active_panel_ref().current_path, 54);

        Frame::NONE
            .fill(t.bg_toolbar)
            .inner_margin(Margin::symmetric(12, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Traffic light space
                    ui.add_space(68.0);

                    ui.label(
                        egui::RichText::new("Commander")
                            .size(13.0)
                            .strong()
                            .color(t.text_primary),
                    );

                    ui.add_space(14.0);
                    Frame::NONE
                        .fill(t.accent.linear_multiply(0.14))
                        .corner_radius(crate::theme::ROUNDING_SM)
                        .inner_margin(Margin::symmetric(7, 2))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(active_side)
                                    .size(10.0)
                                    .strong()
                                    .color(t.accent),
                            );
                        });
                    ui.label(
                        egui::RichText::new(active_path)
                            .size(11.0)
                            .color(t.text_muted),
                    );

                    ui.add_space(14.0);

                    // Action buttons
                    let btn = |ui: &mut egui::Ui, label: &str, shortcut: &str| -> bool {
                        ui.add(
                            egui::Button::new(
                                egui::RichText::new(label).size(12.0).color(t.text_primary),
                            )
                            .fill(t.bg_card)
                            .corner_radius(crate::theme::ROUNDING_SM),
                        )
                        .on_hover_text(shortcut)
                        .clicked()
                    };

                    if btn(ui, "Copy", "F5  Copy selected to other panel") {
                        self.ws.request_copy();
                    }
                    if btn(ui, "Move", "F6  Move selected to other panel") {
                        self.ws.request_move();
                    }
                    if btn(ui, "New Folder", "F7  Create new directory") {
                        self.ws.create_dir();
                    }
                    if btn(ui, "Delete", "F8  Move to Trash") {
                        self.ws.request_delete();
                    }
                    if btn(ui, "\u{2318}K", "Open command palette") {
                        self.ws.palette_request = true;
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(12.0);

                        // Theme toggle
                        let theme_icon = match self.theme_mode {
                            ThemeMode::Light => "\u{1f319}",
                            ThemeMode::Dark => "\u{2600}\u{fe0f}",
                        };
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new(theme_icon).size(14.0))
                                    .fill(t.bg_card)
                                    .corner_radius(crate::theme::ROUNDING_SM),
                            )
                            .on_hover_text("Switch theme")
                            .clicked()
                        {
                            self.theme_mode = match self.theme_mode {
                                ThemeMode::Light => ThemeMode::Dark,
                                ThemeMode::Dark => ThemeMode::Light,
                            };
                            self.colors = match self.theme_mode {
                                ThemeMode::Light => ThemeColors::light(),
                                ThemeMode::Dark => ThemeColors::dark(),
                            };
                            apply_theme(ctx, self.theme_mode);
                        }

                        // Hidden files toggle
                        let active_hidden = self.ws.active_panel_ref().show_hidden;
                        let hidden_icon = if active_hidden {
                            "\u{1f441}"
                        } else {
                            "\u{1f441}\u{200d}\u{1f5e8}"
                        };
                        let hidden_fill = if active_hidden {
                            t.accent.linear_multiply(0.3)
                        } else {
                            t.bg_card
                        };
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new(hidden_icon).size(14.0))
                                    .fill(hidden_fill)
                                    .corner_radius(crate::theme::ROUNDING_SM),
                            )
                            .on_hover_text("Toggle hidden files (\u{2318}H)")
                            .clicked()
                        {
                            let panel = self.ws.active_panel();
                            panel.show_hidden = !panel.show_hidden;
                            panel.refresh();
                        }

                        // Density cycle (active panel)
                        let density = self.ws.active_panel_ref().density;
                        let density_label = crate::density::short_label(density);
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(format!("Rows {density_label}"))
                                        .size(12.0)
                                        .color(t.text_primary),
                                )
                                .fill(t.bg_card)
                                .corner_radius(crate::theme::ROUNDING_SM),
                            )
                            .on_hover_text(format!(
                                "List density: {} (\u{2318}\u{21e7}D)",
                                crate::density::label(density)
                            ))
                            .clicked()
                        {
                            self.ws.active_panel().density = crate::density::cycle(density, 1);
                        }

                        // Size-bars toggle
                        let bars_fill = if self.show_size_bars {
                            t.accent.linear_multiply(0.3)
                        } else {
                            t.bg_card
                        };
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("\u{1f4ca}").size(14.0))
                                    .fill(bars_fill)
                                    .corner_radius(crate::theme::ROUNDING_SM),
                            )
                            .on_hover_text("Toggle size bars")
                            .clicked()
                        {
                            self.show_size_bars = !self.show_size_bars;
                        }

                        // Folder-compare toggle
                        let cmp_fill = if self.show_compare {
                            t.accent.linear_multiply(0.3)
                        } else {
                            t.bg_card
                        };
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("\u{21c4}").size(14.0))
                                    .fill(cmp_fill)
                                    .corner_radius(crate::theme::ROUNDING_SM),
                            )
                            .on_hover_text("Compare panels (highlight differences)")
                            .clicked()
                        {
                            self.show_compare = !self.show_compare;
                        }

                        // Refresh button
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new("\u{27f3}")
                                        .size(14.0)
                                        .color(t.text_primary),
                                )
                                .fill(t.bg_card)
                                .corner_radius(crate::theme::ROUNDING_SM),
                            )
                            .on_hover_text("Refresh both panels")
                            .clicked()
                        {
                            self.ws.left.refresh();
                            self.ws.right.refresh();
                        }
                    });
                });
            });
    }
}
