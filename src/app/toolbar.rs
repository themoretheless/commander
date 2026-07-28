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

#[derive(Clone, Copy)]
struct ToolbarCommands {
    copy: crate::command::CommandAvailability,
    move_items: crate::command::CommandAvailability,
    move_in: crate::command::CommandAvailability,
    copy_in: crate::command::CommandAvailability,
    create_dir: crate::command::CommandAvailability,
    delete: crate::command::CommandAvailability,
}

impl ToolbarCommands {
    fn capture(workspace: &Workspace) -> Self {
        use crate::command::{Command, availability};

        let context = workspace.action_bar_command_context();
        Self {
            copy: availability(Command::RequestCopy, &context),
            move_items: availability(Command::RequestMove, &context),
            move_in: availability(Command::MoveIntoCursorFolder, &context),
            copy_in: availability(Command::CopyIntoCursorFolder, &context),
            create_dir: availability(Command::CreateDir, &context),
            delete: availability(Command::RequestDelete, &context),
        }
    }
}

impl App {
    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let t = self.colors;
        let active_side = match self.ws.active {
            ActivePanel::Left => "LEFT",
            ActivePanel::Right => "RIGHT",
        };
        let active_path = compact_path(&self.ws.active_panel_ref().current_path, 54);

        if crate::accessibility::toolbar_mode(ui.available_width())
            == crate::accessibility::ToolbarMode::Compact
        {
            self.compact_toolbar(ui, ctx, t, active_side, &active_path);
            return;
        }

        let actions = ToolbarCommands::capture(&self.ws);

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
                    let btn = |ui: &mut egui::Ui,
                               label: &str,
                               shortcut: &str,
                               enabled: bool,
                               disabled_reason: &str|
                     -> bool {
                        ui.add_enabled(
                            enabled,
                            egui::Button::new(
                                egui::RichText::new(label).size(12.0).color(t.text_primary),
                            )
                            .fill(t.bg_card)
                            .corner_radius(crate::theme::ROUNDING_SM),
                        )
                        .on_hover_text(shortcut)
                        .on_disabled_hover_text(disabled_reason)
                        .clicked()
                    };

                    if btn(
                        ui,
                        "Copy",
                        "F5  Copy selected to other panel",
                        actions.copy.enabled,
                        actions.copy.reason.unwrap_or_default(),
                    ) {
                        self.ws.request_copy();
                    }
                    if btn(
                        ui,
                        "Move",
                        "F6  Move selected to other panel",
                        actions.move_items.enabled,
                        actions.move_items.reason.unwrap_or_default(),
                    ) {
                        self.ws.request_move();
                    }
                    if btn(
                        ui,
                        "Move In",
                        &format!(
                            "{}  Move selection into highlighted folder",
                            crate::accessibility::drag_alternative(
                                crate::accessibility::DragWorkflow::MoveToHighlightedFolder
                            )
                            .keyboard
                        ),
                        actions.move_in.enabled,
                        actions.move_in.reason.unwrap_or_default(),
                    ) {
                        self.ws
                            .execute(crate::command::Command::MoveIntoCursorFolder);
                    }
                    if btn(
                        ui,
                        "Copy In",
                        &format!(
                            "{}  Copy selection into highlighted folder",
                            crate::accessibility::drag_alternative(
                                crate::accessibility::DragWorkflow::CopyToHighlightedFolder
                            )
                            .keyboard
                        ),
                        actions.copy_in.enabled,
                        actions.copy_in.reason.unwrap_or_default(),
                    ) {
                        self.ws
                            .execute(crate::command::Command::CopyIntoCursorFolder);
                    }
                    if btn(
                        ui,
                        "New Folder",
                        "F7  Create new directory",
                        actions.create_dir.enabled,
                        actions.create_dir.reason.unwrap_or_default(),
                    ) {
                        self.ws.create_dir();
                    }
                    if btn(
                        ui,
                        "Delete",
                        "F8  Move to Trash",
                        actions.delete.enabled,
                        actions.delete.reason.unwrap_or_default(),
                    ) {
                        self.ws.request_delete();
                    }
                    if btn(ui, "\u{2318}K", "Open command palette", true, "") {
                        self.ws.execute(crate::command::Command::BeginPalette);
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(12.0);

                        let diagnostics_fill = if self.show_developer_panel {
                            t.accent.linear_multiply(0.3)
                        } else {
                            t.bg_card
                        };
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("⚙").size(14.0))
                                    .fill(diagnostics_fill)
                                    .corner_radius(crate::theme::ROUNDING_SM),
                            )
                            .on_hover_text("Developer diagnostics")
                            .clicked()
                        {
                            self.show_developer_panel = !self.show_developer_panel;
                        }

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
                            self.switch_theme(ctx);
                        }

                        // Hidden files toggle
                        let active_hidden = self.ws.active_panel_ref().show_hidden();
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
                            self.ws.execute(crate::command::Command::ToggleHidden);
                        }

                        // Density cycle (active panel)
                        let density = self.ws.active_panel_ref().density();
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
                            self.ws
                                .active_panel()
                                .set_density(crate::density::cycle(density, 1));
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
                        if crate::app::glyphs::toolbar_compare_button(ui, cmp_fill, t.text_primary)
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

    fn compact_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        t: ThemeColors,
        active_side: &str,
        active_path: &str,
    ) {
        let actions = ToolbarCommands::capture(&self.ws);
        Frame::NONE
            .fill(t.bg_toolbar)
            .inner_margin(Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(52.0);
                    ui.label(
                        egui::RichText::new("Commander")
                            .size(12.0)
                            .strong()
                            .color(t.text_primary),
                    );
                    ui.label(
                        egui::RichText::new(if active_side == "LEFT" { "L" } else { "R" })
                            .size(10.0)
                            .strong()
                            .color(t.accent),
                    );

                    let path_width = (ui.available_width() - 190.0).clamp(0.0, 180.0);
                    if path_width >= 40.0 {
                        ui.add_sized(
                            Vec2::new(path_width, crate::accessibility::MIN_CONTROL_POINTS),
                            egui::Label::new(
                                egui::RichText::new(active_path)
                                    .size(10.0)
                                    .color(t.text_muted),
                            )
                            .truncate(),
                        );
                    }

                    let command = |ui: &mut egui::Ui,
                                   label: &str,
                                   tooltip: &str,
                                   enabled: bool,
                                   disabled_reason: &str| {
                        ui.add_enabled(
                            enabled,
                            egui::Button::new(
                                egui::RichText::new(label).size(11.0).color(t.text_primary),
                            )
                            .fill(t.bg_card)
                            .corner_radius(crate::theme::ROUNDING_SM)
                            .min_size(Vec2::new(32.0, crate::accessibility::MIN_CONTROL_POINTS)),
                        )
                        .on_hover_text(tooltip)
                        .on_disabled_hover_text(disabled_reason)
                        .clicked()
                    };
                    if command(
                        ui,
                        "F5",
                        "Copy to other panel",
                        actions.copy.enabled,
                        actions.copy.reason.unwrap_or_default(),
                    ) {
                        self.ws.request_copy();
                    }
                    if command(
                        ui,
                        "F6",
                        "Move to other panel",
                        actions.move_items.enabled,
                        actions.move_items.reason.unwrap_or_default(),
                    ) {
                        self.ws.request_move();
                    }
                    if command(
                        ui,
                        "F8",
                        "Move to Trash",
                        actions.delete.enabled,
                        actions.delete.reason.unwrap_or_default(),
                    ) {
                        self.ws.request_delete();
                    }
                    if command(ui, "⌘K", "Open command palette", true, "") {
                        self.ws.execute(crate::command::Command::BeginPalette);
                    }

                    ui.menu_button(egui::RichText::new("…").size(18.0), |ui| {
                        if ui
                            .add_enabled(
                                actions.create_dir.enabled,
                                egui::Button::new("New Folder"),
                            )
                            .on_disabled_hover_text(actions.create_dir.reason.unwrap_or_default())
                            .clicked()
                        {
                            self.ws.create_dir();
                            ui.close();
                        }

                        if ui
                            .add_enabled(
                                actions.move_in.enabled,
                                egui::Button::new("Move into highlighted folder"),
                            )
                            .on_disabled_hover_text(actions.move_in.reason.unwrap_or_default())
                            .clicked()
                        {
                            self.ws
                                .execute(crate::command::Command::MoveIntoCursorFolder);
                            ui.close();
                        }
                        if ui
                            .add_enabled(
                                actions.copy_in.enabled,
                                egui::Button::new("Copy into highlighted folder"),
                            )
                            .on_disabled_hover_text(actions.copy_in.reason.unwrap_or_default())
                            .clicked()
                        {
                            self.ws
                                .execute(crate::command::Command::CopyIntoCursorFolder);
                            ui.close();
                        }

                        ui.separator();
                        let mut show_hidden = self.ws.active_panel_ref().show_hidden();
                        if ui.checkbox(&mut show_hidden, "Show hidden files").changed() {
                            self.ws.execute(crate::command::Command::ToggleHidden);
                        }
                        if ui
                            .button(format!(
                                "Row density: {}",
                                crate::density::label(self.ws.active_panel_ref().density())
                            ))
                            .clicked()
                        {
                            let density = self.ws.active_panel_ref().density();
                            self.ws
                                .active_panel()
                                .set_density(crate::density::cycle(density, 1));
                        }
                        ui.checkbox(&mut self.show_size_bars, "Show size bars");
                        ui.checkbox(&mut self.show_compare, "Compare panels");
                        ui.checkbox(&mut self.show_developer_panel, "Developer diagnostics");

                        ui.separator();
                        let theme_label = match self.theme_mode {
                            ThemeMode::Light => "Use dark theme",
                            ThemeMode::Dark => "Use light theme",
                        };
                        if ui.button(theme_label).clicked() {
                            self.switch_theme(ctx);
                        }
                        ui.label("Text size");
                        let mut scale = self.ui_scale;
                        if ui
                            .add(
                                egui::Slider::new(&mut scale, 0.8..=2.0)
                                    .step_by(0.05)
                                    .show_value(true),
                            )
                            .changed()
                        {
                            self.set_ui_scale(ctx, scale);
                        }
                        if ui.button("Refresh both panels").clicked() {
                            self.ws.left.refresh();
                            self.ws.right.refresh();
                            ui.close();
                        }
                    });
                });
            });
    }

    fn switch_theme(&mut self, ctx: &egui::Context) {
        self.theme_mode = match self.theme_mode {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
        };
        self.colors = ThemeColors::for_preferences(self.theme_mode, self.accessibility_preferences);
        apply_theme(ctx, self.theme_mode, self.accessibility_preferences);
    }

    fn set_ui_scale(&mut self, ctx: &egui::Context, scale: f32) {
        self.ui_scale = crate::accessibility::sanitize_text_scale(scale);
        if (self.ui_scale - 1.0).abs() < 0.03 {
            self.ui_scale = 1.0;
        }
        ctx.set_zoom_factor(self.ui_scale);
    }
}
