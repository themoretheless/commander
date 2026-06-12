use egui::{Color32, CornerRadius, FontDefinitions, FontFamily, Style, Visuals};

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
    pub bg_active_panel: Color32,

    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,

    pub accent: Color32,
    pub accent_green: Color32,
    pub accent_orange: Color32,
    pub accent_red: Color32,
    pub accent_purple: Color32,

    pub border: Color32,
    pub divider: Color32,
}

impl ThemeColors {
    pub fn light() -> Self {
        ThemeColors {
            bg_deep: Color32::from_rgb(245, 245, 248),
            bg_panel: Color32::from_rgb(252, 252, 254),
            bg_card: Color32::from_rgb(235, 235, 240),
            bg_hover: Color32::from_rgb(225, 225, 232),
            bg_selected: Color32::from_rgb(0, 100, 220),
            bg_toolbar: Color32::from_rgb(240, 240, 244),
            bg_active_panel: Color32::from_rgb(250, 250, 253),

            text_primary: Color32::from_rgb(30, 30, 35),
            text_secondary: Color32::from_rgb(90, 90, 105),
            text_muted: Color32::from_rgb(150, 150, 165),

            accent: Color32::from_rgb(0, 122, 255),
            accent_green: Color32::from_rgb(40, 185, 60),
            accent_orange: Color32::from_rgb(235, 145, 10),
            accent_red: Color32::from_rgb(235, 60, 50),
            accent_purple: Color32::from_rgb(160, 70, 210),

            border: Color32::from_rgb(210, 210, 218),
            divider: Color32::from_rgb(220, 220, 228),
        }
    }

    pub fn dark() -> Self {
        ThemeColors {
            bg_deep: Color32::from_rgb(16, 16, 20),
            bg_panel: Color32::from_rgb(24, 24, 30),
            bg_card: Color32::from_rgb(34, 34, 42),
            bg_hover: Color32::from_rgb(44, 44, 54),
            bg_selected: Color32::from_rgb(0, 100, 220),
            bg_toolbar: Color32::from_rgb(20, 20, 26),
            bg_active_panel: Color32::from_rgb(28, 28, 36),

            text_primary: Color32::from_rgb(230, 230, 240),
            text_secondary: Color32::from_rgb(150, 150, 170),
            text_muted: Color32::from_rgb(85, 85, 105),

            accent: Color32::from_rgb(0, 122, 255),
            accent_green: Color32::from_rgb(50, 215, 75),
            accent_orange: Color32::from_rgb(255, 159, 10),
            accent_red: Color32::from_rgb(255, 69, 58),
            accent_purple: Color32::from_rgb(175, 82, 222),

            border: Color32::from_rgb(50, 50, 60),
            divider: Color32::from_rgb(40, 40, 50),
        }
    }
}

pub const ROUNDING_SM: CornerRadius = CornerRadius::same(4);
pub const ROUNDING_MD: CornerRadius = CornerRadius::same(8);

pub fn apply_theme(ctx: &egui::Context, mode: ThemeMode) {
    let c = match mode {
        ThemeMode::Light => ThemeColors::light(),
        ThemeMode::Dark => ThemeColors::dark(),
    };

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
    style.spacing.scroll.bar_width = 6.0;
    style.spacing.scroll.floating = true;
    style.spacing.scroll.foreground_color = true;

    // Enable smooth animated scrolling
    style.animation_time = 0.15;

    ctx.set_style(style);

    let mut fonts = FontDefinitions::default();
    fonts.families.entry(FontFamily::Proportional).or_default();
    fonts.families.entry(FontFamily::Monospace).or_default();
    ctx.set_fonts(fonts);
}
