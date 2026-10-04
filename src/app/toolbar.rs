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
    move_in: crate::command::CommandAvailability,
    copy_in: crate::command::CommandAvailability,
    create_dir: crate::command::CommandAvailability,
}

impl ToolbarCommands {
    fn capture(workspace: &Workspace) -> Self {
        use crate::command::{Command, availability};

        let context = workspace.action_bar_command_context();
        Self {
            move_in: availability(Command::MoveIntoCursorFolder, &context),
            copy_in: availability(Command::CopyIntoCursorFolder, &context),
            create_dir: availability(Command::CreateDir, &context),
        }
    }
}

impl App {
    /// The only persistent chrome above the panels: an omnibar trigger
    /// showing where the active panel is, a Compare toggle, and one menu
    /// for view settings. File actions live on the key bar (F5..F8).
    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let t = self.colors;
        let actions = ToolbarCommands::capture(&self.ws);
        let active_path = self.ws.active_panel_ref().current_path.clone();
        Frame::NONE
            .fill(t.bg_toolbar)
            .inner_margin(Margin::symmetric(10, 5))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Room for the macOS traffic lights (title bar is hidden).
                    if cfg!(target_os = "macos") {
                        ui.add_space(68.0);
                    }
                    let menu_width = 96.0;
                    let width = (ui.available_width() - menu_width).clamp(120.0, 560.0);
                    let lead = (ui.available_width() - menu_width - width).max(0.0) / 2.0;
                    ui.add_space(lead);
                    let max_chars = ((width - 60.0) / 7.0).max(8.0) as usize;
                    let trigger = ui
                        .add_sized(
                            Vec2::new(width, crate::accessibility::MIN_CONTROL_POINTS),
                            egui::Button::new(
                                egui::RichText::new(format!(
                                    "\u{2315}  {}",
                                    compact_path(&active_path, max_chars)
                                ))
                                .size(12.0)
                                .color(t.text_secondary),
                            )
                            .fill(t.bg_card)
                            .stroke(Stroke::NONE)
                            .corner_radius(crate::theme::ROUNDING_MD),
                        )
                        .on_hover_text(
                            "Go to folder, run a command, search (\u{2318}K, \u{2318}L, \u{2318}P)",
                        );
                    if trigger.clicked() {
                        self.ws.execute(crate::command::Command::BeginPalette);
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.menu_button(egui::RichText::new("\u{2026}").size(16.0), |ui| {
                            self.view_menu(ui, ctx, actions);
                        })
                        .response
                        .on_hover_text("View and settings");
                        let compare_fill = if self.show_compare {
                            t.accent.linear_multiply(0.18)
                        } else {
                            Color32::TRANSPARENT
                        };
                        let compare_color = if self.show_compare {
                            t.accent
                        } else {
                            t.text_muted
                        };
                        let compare = crate::app::glyphs::toolbar_compare_button(
                            ui,
                            compare_fill,
                            compare_color,
                        )
                        .on_hover_text("Compare panels: highlight differences");
                        if compare.clicked() {
                            self.show_compare = !self.show_compare;
                        }
                    });
                });
            });
    }

    fn view_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, actions: ToolbarCommands) {
        if ui
            .add_enabled(actions.create_dir.enabled, egui::Button::new("New folder"))
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
            .on_hover_text(
                crate::accessibility::drag_alternative(
                    crate::accessibility::DragWorkflow::MoveToHighlightedFolder,
                )
                .keyboard,
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
            .on_hover_text(
                crate::accessibility::drag_alternative(
                    crate::accessibility::DragWorkflow::CopyToHighlightedFolder,
                )
                .keyboard,
            )
            .on_disabled_hover_text(actions.copy_in.reason.unwrap_or_default())
            .clicked()
        {
            self.ws
                .execute(crate::command::Command::CopyIntoCursorFolder);
            ui.close();
        }

        let filtered = {
            let active = self.ws.active_panel_ref();
            crate::panel::filter_is_active(active.search_query(), &active.facets())
        };
        if ui
            .add_enabled(filtered, egui::Button::new("Save filter as search"))
            .on_disabled_hover_text("Type in the panel filter first")
            .clicked()
        {
            self.save_active_filter_as_smart_folder(ctx);
            ui.close();
        }

        ui.separator();
        let mut show_hidden = self.ws.active_panel_ref().show_hidden();
        if ui.checkbox(&mut show_hidden, "Show hidden files").changed() {
            self.ws.execute(crate::command::Command::ToggleHidden);
        }
        ui.checkbox(&mut self.show_tree, "Show folder sidebar");
        ui.checkbox(&mut self.show_size_bars, "Show size bars");
        ui.checkbox(&mut self.show_key_bar, "Always show key bar");
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
                .set_density(crate::density::cycle(density));
        }

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
        ui.separator();
        ui.checkbox(&mut self.show_developer_panel, "Diagnostics");
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
