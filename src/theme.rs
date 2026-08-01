use egui::{Color32, CornerRadius, Style, Visuals};

#[derive(Clone, Copy, PartialEq)]
pub enum ThemeMode {
    Light,
    Dark,
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

    pub border: Color32,
}

/// Selection fill is intentionally identical in the standard light and dark
/// themes: selected rows always draw white text on this blue, so the pair must
/// keep the same contrast in both modes. The high-contrast themes override it.
const SELECTION_BLUE: Color32 = Color32::from_rgb(0, 100, 220);

/// Warning amber is intentionally identical in the standard light and dark
/// themes: conflict and pending badges must read the same in both modes. The
/// high-contrast themes override it with darker, more legible variants.
const WARNING_AMBER: Color32 = Color32::from_rgb(230, 160, 40);

impl ThemeColors {
    pub fn for_preferences(
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

    pub fn light() -> Self {
        ThemeColors {
            bg_deep: Color32::from_rgb(245, 245, 248),
            bg_panel: Color32::from_rgb(252, 252, 254),
            bg_card: Color32::from_rgb(235, 235, 240),
            bg_hover: Color32::from_rgb(225, 225, 232),
            bg_selected: SELECTION_BLUE,
            bg_toolbar: Color32::from_rgb(240, 240, 244),

            text_primary: Color32::from_rgb(30, 30, 35),
            text_secondary: Color32::from_rgb(90, 90, 105),
            text_muted: Color32::from_rgb(150, 150, 165),

            accent: Color32::from_rgb(0, 122, 255),
            accent_red: Color32::from_rgb(235, 60, 50),
            accent_purple: Color32::from_rgb(160, 70, 210),
            accent_warning: WARNING_AMBER,

            border: Color32::from_rgb(210, 210, 218),
        }
    }

    pub fn dark() -> Self {
        ThemeColors {
            bg_deep: Color32::from_rgb(16, 16, 20),
            bg_panel: Color32::from_rgb(24, 24, 30),
            bg_card: Color32::from_rgb(34, 34, 42),
            bg_hover: Color32::from_rgb(44, 44, 54),
            bg_selected: SELECTION_BLUE,
            bg_toolbar: Color32::from_rgb(20, 20, 26),

            text_primary: Color32::from_rgb(230, 230, 240),
            text_secondary: Color32::from_rgb(150, 150, 170),
            text_muted: Color32::from_rgb(85, 85, 105),

            accent: Color32::from_rgb(0, 122, 255),
            accent_red: Color32::from_rgb(255, 69, 58),
            accent_purple: Color32::from_rgb(175, 82, 222),
            accent_warning: WARNING_AMBER,

            border: Color32::from_rgb(50, 50, 60),
        }
    }

    pub fn high_contrast_light() -> Self {
        ThemeColors {
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
            border: Color32::from_rgb(0, 0, 0),
        }
    }

    pub fn high_contrast_dark() -> Self {
        ThemeColors {
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
    visuals.widgets.inactive.fg_stroke.color = c.text_primary;
    visuals.widgets.inactive.corner_radius = ROUNDING_SM;

    visuals.widgets.hovered.bg_fill = c.bg_hover;
    visuals.widgets.hovered.fg_stroke.color = c.text_primary;
    visuals.widgets.hovered.corner_radius = ROUNDING_SM;

    visuals.widgets.active.bg_fill = c.accent;
    visuals.widgets.active.fg_stroke.color = Color32::WHITE;
    visuals.widgets.active.corner_radius = ROUNDING_SM;

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

    let theme = match mode {
        ThemeMode::Light => egui::Theme::Light,
        ThemeMode::Dark => egui::Theme::Dark,
    };
    ctx.set_style_of(theme, style);
    ctx.set_theme(theme);
}

#[cfg(test)]
mod tests {
    use super::*;

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
