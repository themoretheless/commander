use super::*;

/// The active path elided at the start ("...tail") so that it measures at
/// most `max_width` by `width_of`. Measuring the real text keeps wide glyphs
/// (CJK, emoji) inside the trigger, and the tail is cut on grapheme
/// boundaries so a decomposed accent never dangles after the ellipsis.
/// Returns the text and whether it was shortened.
fn compact_path(
    path: &std::path::Path,
    max_width: f32,
    width_of: impl Fn(&str) -> f32,
) -> (String, bool) {
    use unicode_segmentation::UnicodeSegmentation;

    let full = path.display().to_string();
    if width_of(&full) <= max_width {
        return (full, false);
    }
    let starts: Vec<usize> = full.grapheme_indices(true).map(|(at, _)| at).collect();
    if starts.len() < 2 {
        return (full, false);
    }
    let elided = |index: usize| format!("...{}", &full[starts[index]..]);
    // Dropping more leading graphemes only narrows the text, so search for
    // the longest tail that fits; the last grapheme is kept regardless.
    let (mut lo, mut hi) = (1, starts.len() - 1);
    while lo < hi {
        let mid = usize::midpoint(lo, hi);
        if width_of(&elided(mid)) <= max_width {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    (elided(lo), true)
}

/// The omnibar trigger's accessible name: its purpose, then the folder it
/// shows in full (the visible text is a glyph and a possibly elided path).
fn omnibar_trigger_label(full_path: &str) -> String {
    format!("Go to folder or command, current folder: {full_path}")
}

/// The trigger's hover text, led by the full path when the visible one is
/// elided.
fn omnibar_trigger_hover(full_path: &str, truncated: bool) -> String {
    omnibar_trigger_hover_for(full_path, truncated, cfg!(target_os = "macos"))
}

fn omnibar_trigger_hover_for(full_path: &str, truncated: bool, mac: bool) -> String {
    let [palette, go_to, recent] =
        ["Cmd+K", "Cmd+L", "Cmd+P"].map(|shortcut| platform_shortcut_for(shortcut, mac));
    let hint = format!("Go to folder, run a command, search ({palette}, {go_to}, {recent})");
    if truncated {
        format!("{full_path}\n{hint}")
    } else {
        hint
    }
}

/// A catalog shortcut ("Cmd+J") as it is pressed on this platform: egui's
/// command modifier is Ctrl outside macOS.
pub(super) fn platform_shortcut(shortcut: &str) -> String {
    platform_shortcut_for(shortcut, cfg!(target_os = "macos"))
}

fn platform_shortcut_for(shortcut: &str, mac: bool) -> String {
    if mac {
        shortcut.to_string()
    } else {
        shortcut.replace("Cmd+", "Ctrl+")
    }
}

/// The shortcut the command palette lists for `command`, for this platform.
pub(super) fn command_shortcut(command: crate::command::Command) -> Option<String> {
    crate::command::CATALOG
        .iter()
        .find(|entry| entry.command == command && !entry.shortcut.is_empty())
        .map(|entry| platform_shortcut(entry.shortcut))
}

/// A menu button showing `command`'s shortcut on its right edge.
fn menu_item(label: &str, command: crate::command::Command) -> egui::Button<'_> {
    let button = egui::Button::new(label);
    match command_shortcut(command) {
        Some(shortcut) => button.shortcut_text(shortcut),
        None => button,
    }
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
                    let painter = ui.painter().clone();
                    let (shown_path, truncated) =
                        compact_path(&active_path, (width - 60.0).max(56.0), |text| {
                            painter
                                .layout_no_wrap(
                                    text.to_string(),
                                    egui::FontId::proportional(12.0),
                                    t.text_secondary,
                                )
                                .size()
                                .x
                        });
                    let full_path = active_path.display().to_string();
                    let trigger = ui
                        .add_sized(
                            Vec2::new(width, crate::accessibility::MIN_CONTROL_POINTS),
                            egui::Button::new(
                                egui::RichText::new(format!("\u{2315}  {shown_path}"))
                                    .size(12.0)
                                    .color(t.text_secondary),
                            )
                            .fill(t.bg_card)
                            .stroke(Stroke::NONE)
                            .corner_radius(crate::theme::ROUNDING_MD),
                        )
                        .on_hover_text(omnibar_trigger_hover(&full_path, truncated));
                    trigger.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            true,
                            omnibar_trigger_label(&full_path),
                        )
                    });
                    if trigger.clicked() {
                        self.ws.execute(crate::command::Command::BeginPalette);
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let menu = ui
                            .menu_button(egui::RichText::new("\u{2026}").size(16.0), |ui| {
                                // Only the open menu needs command availability;
                                // building its context walks the selection, so
                                // skip it on the frames where the menu is closed.
                                let actions = ToolbarCommands::capture(&self.ws);
                                self.view_menu(ui, ctx, actions);
                            })
                            .response
                            .on_hover_text("View and settings");
                        // The visible text is an ellipsis glyph; name the purpose.
                        menu.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                true,
                                "View and settings",
                            )
                        });
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
                            self.show_compare,
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
            .add_enabled(
                actions.create_dir.enabled,
                menu_item("New folder", crate::command::Command::CreateDir),
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
                menu_item(
                    "Move into highlighted folder",
                    crate::command::Command::MoveIntoCursorFolder,
                ),
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
                menu_item(
                    "Copy into highlighted folder",
                    crate::command::Command::CopyIntoCursorFolder,
                ),
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
            .on_disabled_hover_text("Type in the panel filter or pick a filter chip first")
            .clicked()
        {
            self.save_active_filter_as_smart_folder(ctx);
            ui.close();
        }

        ui.separator();
        // Hidden files and density belong to the active panel; say so, since
        // the checkboxes around them are app-wide.
        let mut show_hidden = self.ws.active_panel_ref().show_hidden();
        let hidden = ui.checkbox(&mut show_hidden, "Show hidden files (this panel)");
        let hidden = match command_shortcut(crate::command::Command::ToggleHidden) {
            Some(shortcut) => hidden.on_hover_text(format!("Shortcut: {shortcut}")),
            None => hidden,
        };
        if hidden.changed() {
            self.ws.execute(crate::command::Command::ToggleHidden);
        }
        if ui
            .checkbox(&mut self.show_tree, "Show folder sidebar")
            .changed()
            && self.show_tree
        {
            // Open on the current folder, as the pane's sidebar toggle does.
            let path = self.ws.active_panel_ref().current_path.clone();
            self.tree_reveal_path(&path);
        }
        ui.checkbox(&mut self.show_size_bars, "Show size bars");
        ui.checkbox(&mut self.show_key_bar, "Always show key bar");
        let density_label = format!(
            "Row density (this panel): {}",
            crate::density::label(self.ws.active_panel_ref().density())
        );
        if ui
            .add(menu_item(
                &density_label,
                crate::command::Command::CycleDensity,
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
        let text_size = ui.label("Text size");
        let mut scale = self.ui_scale;
        if ui
            .add(
                egui::Slider::new(&mut scale, 0.8..=2.0)
                    .step_by(0.05)
                    .show_value(true),
            )
            .labelled_by(text_size.id)
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
        self.ui_scale = normalized_ui_scale(scale);
        ctx.set_zoom_factor(self.ui_scale);
    }

    /// Adopt a zoom changed outside the Text size slider (egui's built-in
    /// Cmd/Ctrl +/- keys), so the slider and the saved session follow it.
    /// Runs at frame start, after egui applied any pending zoom change.
    pub(crate) fn sync_ui_scale_with_zoom(&mut self, ctx: &egui::Context) {
        if let Some(scale) = zoom_to_adopt(self.ui_scale, ctx.zoom_factor()) {
            self.set_ui_scale(ctx, scale);
        }
    }
}

/// The text scale clamped to the supported range, snapping near-1.0 to 1.0.
fn normalized_ui_scale(scale: f32) -> f32 {
    let scale = crate::accessibility::sanitize_text_scale(scale);
    if (scale - 1.0).abs() < 0.03 {
        1.0
    } else {
        scale
    }
}

/// The live zoom factor when it no longer matches the stored text scale.
fn zoom_to_adopt(ui_scale: f32, zoom: f32) -> Option<f32> {
    ((zoom - ui_scale).abs() > 1e-3).then_some(zoom)
}

#[cfg(test)]
mod tests {
    use super::{
        command_shortcut, compact_path, normalized_ui_scale, omnibar_trigger_hover,
        omnibar_trigger_hover_for, omnibar_trigger_label, platform_shortcut_for, zoom_to_adopt,
    };
    use std::path::Path;

    /// A stand-in for text measurement: CJK ideographs are twice as wide as
    /// other characters, and combining marks take no width.
    fn width(text: &str) -> f32 {
        text.chars()
            .map(|c| match c {
                '\u{0300}'..='\u{036f}' => 0.0,
                '\u{4e00}'..='\u{9fff}' => 14.0,
                _ => 7.0,
            })
            .sum()
    }

    #[test]
    fn compact_path_keeps_short_paths_whole() {
        let (text, truncated) = compact_path(Path::new("/tmp/a"), 100.0, width);
        assert_eq!(text, "/tmp/a");
        assert!(!truncated);
    }

    #[test]
    fn compact_path_boundaries() {
        let per_char = |text: &str| text.chars().count() as f32 * 7.0;
        // Exactly as wide as the room: kept whole.
        let (text, truncated) = compact_path(Path::new("/tmp/ab"), 49.0, per_char);
        assert_eq!((text.as_str(), truncated), ("/tmp/ab", false));
        // One unit narrower: elided, still within the room.
        let (text, truncated) = compact_path(Path::new("/tmp/ab"), 48.0, per_char);
        assert!(truncated);
        assert!(text.starts_with("...") && per_char(&text) <= 48.0, "{text}");
        // A single grapheme cannot be shortened, however little room there is.
        assert_eq!(
            compact_path(Path::new("/"), 1.0, per_char),
            ("/".to_string(), false)
        );
        // With no room for any tail the last grapheme is still kept.
        let (text, truncated) = compact_path(Path::new("/a/b"), 0.0, per_char);
        assert_eq!((text.as_str(), truncated), ("...b", true));
    }

    #[test]
    fn compact_path_fits_wide_glyphs_by_measured_width() {
        let path = Path::new(
            "/home/\u{6587}\u{6863}/\u{9879}\u{76ee}/\u{65b0}\u{5efa}\u{6587}\u{4ef6}\u{5939}",
        );
        let (text, truncated) = compact_path(path, 70.0, width);
        assert!(truncated);
        assert!(text.starts_with("..."));
        assert!(width(&text) <= 70.0, "{text} is {} wide", width(&text));
        assert!(text.ends_with("\u{5939}"));
    }

    #[test]
    fn compact_path_never_starts_the_tail_with_a_combining_mark() {
        // "cafe\u{301}" is an NFD "café". With room for one more character
        // after the ellipsis, a per-char cut would keep only the accent.
        let per_char = |text: &str| text.chars().count() as f32 * 7.0;
        let (text, truncated) = compact_path(Path::new("/a/cafe\u{301}"), 28.0, per_char);
        assert!(truncated);
        assert_eq!(text, "...e\u{301}");
    }

    #[test]
    fn omnibar_trigger_names_its_purpose_and_full_path() {
        assert_eq!(
            omnibar_trigger_label("/a/b"),
            "Go to folder or command, current folder: /a/b"
        );
        assert!(omnibar_trigger_hover("/deep/path", true).starts_with("/deep/path\n"));
        assert!(!omnibar_trigger_hover("/deep/path", false).contains("/deep/path"));
        assert!(omnibar_trigger_hover_for("/p", false, false).contains("(Ctrl+K, Ctrl+L, Ctrl+P)"));
        assert!(omnibar_trigger_hover_for("/p", false, true).contains("(Cmd+K, Cmd+L, Cmd+P)"));
    }

    #[test]
    fn shortcuts_use_the_platform_command_modifier() {
        assert_eq!(platform_shortcut_for("Cmd+Shift+D", false), "Ctrl+Shift+D");
        assert_eq!(platform_shortcut_for("Cmd+J", true), "Cmd+J");
        assert_eq!(platform_shortcut_for("F7", false), "F7");
        assert_eq!(
            command_shortcut(crate::command::Command::CreateDir).as_deref(),
            Some("F7")
        );
        assert_eq!(
            command_shortcut(crate::command::Command::FindDuplicates),
            None
        );
    }

    #[test]
    fn keyboard_zoom_is_adopted_into_the_stored_scale() {
        assert_eq!(zoom_to_adopt(1.0, 1.1), Some(1.1));
        assert_eq!(zoom_to_adopt(1.25, 1.25), None);
        // Out-of-range keyboard zoom is clamped back to the slider range.
        assert_eq!(zoom_to_adopt(1.0, 3.0).map(normalized_ui_scale), Some(2.0));
        assert_eq!(normalized_ui_scale(1.02), 1.0);
    }

    #[test]
    fn adopted_zoom_matches_egui_after_a_keyboard_zoom_pass() {
        let ctx = egui::Context::default();
        ctx.set_zoom_factor(1.5);
        crate::testutil::discard_egui_output(ctx.run_ui(egui::RawInput::default(), |_| {}));
        assert_eq!(zoom_to_adopt(1.0, ctx.zoom_factor()), Some(1.5));
    }
}
