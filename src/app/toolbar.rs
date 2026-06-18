use super::*;

impl App {
    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let t = self.colors;
        crate::app::ui_common::section_frame(&t)
            .fill(t.bg_toolbar)
            .inner_margin(Margin::symmetric(12, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Traffic light space (macOS native)
                    ui.add_space(68.0);

                    ui.label(
                        egui::RichText::new("Commander")
                            .size(13.0)
                            .strong()
                            .color(t.text_primary),
                    );

                    ui.add_space(20.0);

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

                    if btn(ui, "\u{1f4cb}  Copy", "Copy selected to other panel") {
                        self.ws.request_copy();
                    }
                    if btn(ui, "\u{1f4e6}  Move", "Move selected to other panel") {
                        self.ws.request_move();
                    }
                    if btn(ui, "\u{1f4c1}  New Dir", "Create new directory") {
                        self.ws.create_dir();
                    }
                    if btn(ui, "\u{1f5d1}  Delete", "Move to trash") {
                        self.ws.request_delete();
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
                            .clicked()
                        {
                            // PR1 tabs: refresh the (current single) active tab per side
                            self.ws.left.tabs[self.ws.left.active].state.refresh();
                            self.ws.right.tabs[self.ws.right.active].state.refresh();
                        }
                    });
                });
            });
    }
}
