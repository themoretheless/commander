use egui::{Color32, CornerRadius, Style, Visuals};

#[derive(Clone, Copy, PartialEq)]
pub enum ThemeMode {
    Light,
    Dark,
}

/// The palette that follows the OS appearance: `system` is egui's reading
/// of it, and `None` (the platform does not report one) keeps `current`.
pub const fn follow_system(system: Option<egui::Theme>, current: ThemeMode) -> ThemeMode {
    match system {
        Some(egui::Theme::Dark) => ThemeMode::Dark,
        Some(egui::Theme::Light) => ThemeMode::Light,
        None => current,
    }
}

#[derive(Clone, Copy)]
pub struct ThemeColors {
    pub bg_deep: Color32,
    pub bg_panel: Color32,
    pub bg_card: Color32,
    pub bg_hover: Color32,
    pub bg_selected: Color32,
    pub bg_toolbar: Color32,

    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,

    pub accent: Color32,
    pub accent_red: Color32,
    pub accent_purple: Color32,
    /// Conflicts, pending moves and other "attention" highlights.
    pub accent_warning: Color32,
    /// Folder icon outline and its item count: at least 4.5:1 on the panel
    /// and hover backgrounds, so the count reads as text.
    pub folder: Color32,

    pub border: Color32,
}

impl ThemeColors {
    /// Whether this is a dark palette, judged by the panel background, so a
    /// view can pick the matching toned file-kind palette.
    pub const fn is_dark(&self) -> bool {
        (self.bg_panel.r() as u16 + self.bg_panel.g() as u16 + self.bg_panel.b() as u16) < 384
    }

    pub const fn for_preferences(
        mode: ThemeMode,
        preferences: crate::accessibility::Preferences,
    ) -> Self {
        match (mode, preferences.high_contrast) {
            (ThemeMode::Light, false) => Self::light(),
            (ThemeMode::Dark, false) => Self::dark(),
            (ThemeMode::Light, true) => Self::high_contrast_light(),
            (ThemeMode::Dark, true) => Self::high_contrast_dark(),
        }
    }

    /// Warm paper: neutral surfaces with a slight warm cast; accents dark enough
    /// for white button text.
    pub const fn light() -> Self {
        Self {
            bg_deep: Color32::from_rgb(241, 238, 233),
            bg_panel: Color32::from_rgb(253, 252, 249),
            bg_card: Color32::from_rgb(241, 238, 233),
            bg_hover: Color32::from_rgb(235, 231, 226),
            bg_selected: Color32::from_rgb(22, 102, 170),
            bg_toolbar: Color32::from_rgb(246, 243, 239),
            text_primary: Color32::from_rgb(46, 42, 38),
            text_secondary: Color32::from_rgb(87, 82, 77),
            text_muted: Color32::from_rgb(108, 104, 100),
            accent: Color32::from_rgb(22, 102, 170),
            accent_red: Color32::from_rgb(182, 50, 45),
            accent_purple: Color32::from_rgb(117, 77, 158),
            accent_warning: Color32::from_rgb(162, 101, 10),
            folder: Color32::from_rgb(128, 96, 24),
            border: Color32::from_rgb(217, 212, 206),
        }
    }

    /// Warm graphite: brown-tinted near-black surfaces with warm off-white text;
    /// accents stay dark enough for white button text.
    pub const fn dark() -> Self {
        Self {
            bg_deep: Color32::from_rgb(19, 17, 15),
            bg_panel: Color32::from_rgb(27, 24, 21),
            bg_card: Color32::from_rgb(36, 33, 30),
            bg_hover: Color32::from_rgb(45, 41, 37),
            bg_selected: Color32::from_rgb(49, 120, 186),
            bg_toolbar: Color32::from_rgb(24, 22, 19),
            text_primary: Color32::from_rgb(237, 233, 226),
            text_secondary: Color32::from_rgb(185, 181, 176),
            text_muted: Color32::from_rgb(146, 142, 137),
            accent: Color32::from_rgb(49, 120, 186),
            accent_red: Color32::from_rgb(202, 69, 61),
            accent_purple: Color32::from_rgb(140, 100, 183),
            accent_warning: Color32::from_rgb(191, 123, 31),
            folder: Color32::from_rgb(200, 175, 100),
            border: Color32::from_rgb(52, 48, 44),
        }
    }

    pub const fn high_contrast_light() -> Self {
        Self {
            bg_deep: Color32::from_rgb(255, 255, 255),
            bg_panel: Color32::from_rgb(255, 255, 255),
            bg_card: Color32::from_rgb(230, 230, 230),
            bg_hover: Color32::from_rgb(210, 225, 245),
            bg_selected: Color32::from_rgb(0, 70, 170),
            bg_toolbar: Color32::from_rgb(242, 242, 242),
            text_primary: Color32::from_rgb(0, 0, 0),
            text_secondary: Color32::from_rgb(40, 40, 40),
            text_muted: Color32::from_rgb(80, 80, 80),
            accent: Color32::from_rgb(0, 70, 170),
            accent_red: Color32::from_rgb(170, 0, 25),
            accent_purple: Color32::from_rgb(100, 20, 145),
            accent_warning: Color32::from_rgb(105, 65, 0),
            folder: Color32::from_rgb(100, 72, 0),
            border: Color32::from_rgb(0, 0, 0),
        }
    }

    pub const fn high_contrast_dark() -> Self {
        Self {
            bg_deep: Color32::from_rgb(0, 0, 0),
            bg_panel: Color32::from_rgb(5, 5, 5),
            bg_card: Color32::from_rgb(30, 30, 30),
            bg_hover: Color32::from_rgb(48, 55, 64),
            bg_selected: Color32::from_rgb(55, 145, 255),
            bg_toolbar: Color32::from_rgb(10, 10, 10),
            text_primary: Color32::from_rgb(255, 255, 255),
            text_secondary: Color32::from_rgb(225, 225, 225),
            text_muted: Color32::from_rgb(185, 185, 185),
            // These semantic colors sit in the narrow luminance band that is
            // legible both as text on black and under white button text.
            accent: Color32::from_rgb(0, 116, 232),
            accent_red: Color32::from_rgb(220, 55, 55),
            accent_purple: Color32::from_rgb(162, 86, 194),
            accent_warning: Color32::from_rgb(154, 111, 0),
            folder: Color32::from_rgb(230, 200, 100),
            border: Color32::from_rgb(235, 235, 235),
        }
    }
}

pub const ROUNDING_SM: CornerRadius = CornerRadius::same(4);
pub const ROUNDING_MD: CornerRadius = CornerRadius::same(8);

pub fn apply_theme(
    ctx: &egui::Context,
    mode: ThemeMode,
    preferences: crate::accessibility::Preferences,
) {
    let style = theme_style(mode, preferences);
    let theme = match mode {
        ThemeMode::Light => egui::Theme::Light,
        ThemeMode::Dark => egui::Theme::Dark,
    };
    ctx.set_style_of(theme, style);
    ctx.set_theme(theme);
}

/// The egui style for `mode` and `preferences`, without touching a context.
fn theme_style(mode: ThemeMode, preferences: crate::accessibility::Preferences) -> Style {
    let c = ThemeColors::for_preferences(mode, preferences);

    let mut style = Style::default();
    let mut visuals = match mode {
        ThemeMode::Light => Visuals::light(),
        ThemeMode::Dark => Visuals::dark(),
    };

    visuals.window_fill = c.bg_deep;
    visuals.panel_fill = c.bg_panel;
    visuals.faint_bg_color = c.bg_card;
    visuals.extreme_bg_color = c.bg_deep;
    visuals.override_text_color = Some(c.text_primary);

    visuals.widgets.noninteractive.bg_fill = c.bg_panel;
    visuals.widgets.noninteractive.fg_stroke.color = c.text_secondary;
    visuals.widgets.noninteractive.corner_radius = ROUNDING_MD;

    visuals.widgets.inactive.bg_fill = c.bg_card;
    visuals.widgets.inactive.weak_bg_fill = c.bg_card;
    visuals.widgets.inactive.fg_stroke.color = c.text_primary;
    visuals.widgets.inactive.corner_radius = ROUNDING_SM;

    visuals.widgets.hovered.bg_fill = c.bg_hover;
    visuals.widgets.hovered.weak_bg_fill = c.bg_hover;
    visuals.widgets.hovered.fg_stroke.color = c.text_primary;
    visuals.widgets.hovered.corner_radius = ROUNDING_SM;

    visuals.widgets.active.bg_fill = c.accent;
    visuals.widgets.active.weak_bg_fill = c.accent;
    visuals.widgets.active.fg_stroke.color = Color32::WHITE;
    visuals.widgets.active.corner_radius = ROUNDING_SM;

    // An open combo box or menu button reads like a hovered one rather than
    // keeping egui's gray defaults.
    visuals.widgets.open = visuals.widgets.hovered;

    // Separators, frames and window edges use the palette border. High
    // contrast also outlines idle controls so buttons stay distinguishable.
    let border_width = if preferences.high_contrast { 1.5 } else { 1.0 };
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(border_width, c.border);
    visuals.window_stroke = egui::Stroke::new(border_width, c.border);
    if preferences.high_contrast {
        visuals.widgets.inactive.bg_stroke = egui::Stroke::new(border_width, c.border);
    }

    visuals.selection.bg_fill = c.accent.linear_multiply(0.3);
    visuals.selection.stroke.color = c.accent;

    visuals.window_corner_radius = ROUNDING_MD;

    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.window_margin = egui::Margin::same(0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.interact_size.y = crate::accessibility::MIN_CONTROL_POINTS;
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.scroll.floating = true;
    style.spacing.scroll.foreground_color = true;

    style.animation_time = if preferences.reduced_motion {
        0.0
    } else {
        0.15
    };
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn following_the_system_tracks_its_appearance() {
        assert!(follow_system(Some(egui::Theme::Dark), ThemeMode::Light) == ThemeMode::Dark);
        assert!(follow_system(Some(egui::Theme::Light), ThemeMode::Dark) == ThemeMode::Light);
        // No reported appearance: keep the palette in use.
        assert!(follow_system(None, ThemeMode::Dark) == ThemeMode::Dark);
        assert!(follow_system(None, ThemeMode::Light) == ThemeMode::Light);
    }

    #[test]
    fn every_palette_knows_whether_it_is_dark() {
        assert!(!ThemeColors::light().is_dark());
        assert!(!ThemeColors::high_contrast_light().is_dark());
        assert!(ThemeColors::dark().is_dark());
        assert!(ThemeColors::high_contrast_dark().is_dark());
    }

    fn hex(color: Color32) -> String {
        format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b())
    }

    fn semantic_snapshot(colors: ThemeColors) -> String {
        format!(
            "focus={} selection={} diff={} disabled={}",
            hex(colors.text_primary),
            hex(colors.accent_purple),
            hex(colors.accent_warning),
            hex(colors.text_muted)
        )
    }

    fn luminance(color: Color32) -> f64 {
        let linear = |channel: u8| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
    }

    fn contrast(left: Color32, right: Color32) -> f64 {
        let (bright, dark) = {
            let left = luminance(left);
            let right = luminance(right);
            if left > right {
                (left, right)
            } else {
                (right, left)
            }
        };
        (bright + 0.05) / (dark + 0.05)
    }

    #[test]
    fn standard_themes_keep_text_legible_and_button_text_readable() {
        for colors in [ThemeColors::light(), ThemeColors::dark()] {
            assert!(contrast(colors.text_primary, colors.bg_panel) >= 7.0);
            assert!(contrast(colors.text_secondary, colors.bg_panel) >= 4.5);
            for surface in [colors.bg_panel, colors.bg_card] {
                assert!(contrast(colors.text_muted, surface) >= 4.5);
            }
            for fill in [colors.accent, colors.accent_red, colors.accent_purple] {
                assert!(contrast(Color32::WHITE, fill) >= 4.5);
            }
            assert!(contrast(Color32::WHITE, colors.accent_warning) >= 3.0);
        }
    }

    fn preferences(high_contrast: bool) -> crate::accessibility::Preferences {
        crate::accessibility::Preferences {
            high_contrast,
            ..crate::accessibility::Preferences::default()
        }
    }

    #[test]
    fn frames_and_open_widgets_follow_the_palette() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            for high_contrast in [false, true] {
                let prefs = preferences(high_contrast);
                let colors = ThemeColors::for_preferences(mode, prefs);
                let visuals = theme_style(mode, prefs).visuals;
                assert_eq!(visuals.window_stroke.color, colors.border);
                assert_eq!(
                    visuals.widgets.noninteractive.bg_stroke.color,
                    colors.border
                );
                assert_eq!(visuals.widgets.open.bg_fill, colors.bg_hover);
                assert_eq!(visuals.widgets.open.fg_stroke.color, colors.text_primary);
                for (widget, expected) in [
                    (&visuals.widgets.inactive, colors.bg_card),
                    (&visuals.widgets.hovered, colors.bg_hover),
                    (&visuals.widgets.active, colors.accent),
                    (&visuals.widgets.open, colors.bg_hover),
                ] {
                    assert_eq!(widget.weak_bg_fill, expected);
                }
                // Idle buttons get an outline only in high contrast.
                assert_eq!(
                    visuals.widgets.inactive.bg_stroke.color == colors.border,
                    high_contrast
                );
            }
        }
    }

    #[test]
    fn high_contrast_semantic_snapshots_cover_required_states() {
        assert_eq!(
            semantic_snapshot(ThemeColors::high_contrast_light()),
            "focus=#000000 selection=#641491 diff=#694100 disabled=#505050"
        );
        assert_eq!(
            semantic_snapshot(ThemeColors::high_contrast_dark()),
            "focus=#FFFFFF selection=#A256C2 diff=#9A6F00 disabled=#B9B9B9"
        );
    }

    #[test]
    fn folder_icons_read_on_every_theme() {
        for colors in [
            ThemeColors::light(),
            ThemeColors::dark(),
            ThemeColors::high_contrast_light(),
            ThemeColors::high_contrast_dark(),
        ] {
            for surface in [colors.bg_panel, colors.bg_hover] {
                assert!(contrast(colors.folder, surface) >= 4.5);
            }
        }
    }

    #[test]
    fn high_contrast_text_roles_clear_wcag_normal_text_ratio() {
        for colors in [
            ThemeColors::high_contrast_light(),
            ThemeColors::high_contrast_dark(),
        ] {
            assert!(contrast(colors.text_primary, colors.bg_panel) >= 7.0);
            assert!(contrast(colors.text_secondary, colors.bg_panel) >= 4.5);
            assert!(contrast(colors.text_muted, colors.bg_panel) >= 4.5);
            for semantic in [
                colors.accent,
                colors.accent_red,
                colors.accent_purple,
                colors.accent_warning,
            ] {
                assert!(contrast(semantic, colors.bg_panel) >= 4.5);
                assert!(contrast(Color32::WHITE, semantic) >= 4.5);
            }
        }
    }
}
