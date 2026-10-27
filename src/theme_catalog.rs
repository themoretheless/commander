//! Strata's MIT-licensed palettes mapped to Commander's semantic colors.
use crate::theme::{ThemeColors, ThemeMode};
use egui::Color32;
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Deserialize)]
pub struct Palette {
    pub id: String,
    pub name: String,
    background: String,
    surface: String,
    text: String,
    accent: String,
    danger: String,
    muted: String,
    highlight: String,
    border: String,
    dim_text: String,
    syntax_keyword: String,
    syntax_type: String,
}

fn hex(value: &str) -> Option<Color32> {
    let bytes = value.strip_prefix('#')?;
    if bytes.len() != 6 || !bytes.is_ascii() {
        return None;
    }
    Some(Color32::from_rgb(
        u8::from_str_radix(&bytes[..2], 16).ok()?,
        u8::from_str_radix(&bytes[2..4], 16).ok()?,
        u8::from_str_radix(&bytes[4..], 16).ok()?,
    ))
}

pub fn palettes() -> &'static [Palette] {
    static PALETTES: OnceLock<Vec<Palette>> = OnceLock::new();
    PALETTES.get_or_init(|| {
        serde_json::from_str(include_str!("../data/themes/strata.json"))
            .expect("bundled theme catalog is tested")
    })
}

pub fn find(id: &str) -> Option<&'static Palette> {
    palettes().iter().find(|palette| palette.id == id)
}

impl Palette {
    pub fn colors(&self) -> ThemeColors {
        let color = |value: &str| hex(value).expect("bundled palette colors are tested");
        ThemeColors {
            bg_deep: color(&self.muted),
            bg_panel: color(&self.background),
            bg_card: color(&self.surface),
            bg_hover: color(&self.highlight),
            bg_selected: color(&self.accent),
            bg_toolbar: color(&self.surface),
            text_primary: color(&self.text),
            text_secondary: color(&self.text),
            text_muted: color(&self.dim_text),
            accent: color(&self.accent),
            accent_red: color(&self.danger),
            accent_purple: color(&self.syntax_keyword),
            accent_warning: color(&self.syntax_type),
            folder: color(&self.accent),
            border: color(&self.border),
        }
    }
}

pub fn resolve(
    id: Option<&str>,
    mode: ThemeMode,
    preferences: crate::accessibility::Preferences,
) -> ThemeColors {
    if !preferences.high_contrast
        && let Some(palette) = id.and_then(find)
    {
        return palette.colors();
    }
    ThemeColors::for_preferences(mode, preferences)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_catalog_has_unique_ids_and_valid_colors() {
        let catalog = palettes();
        assert_eq!(catalog.len(), 95);
        let mut ids = std::collections::HashSet::new();
        for palette in catalog {
            assert!(ids.insert(&palette.id));
            assert!(!palette.name.is_empty());
            for value in [
                &palette.background,
                &palette.surface,
                &palette.text,
                &palette.accent,
                &palette.danger,
                &palette.muted,
                &palette.highlight,
                &palette.border,
                &palette.dim_text,
                &palette.syntax_keyword,
                &palette.syntax_type,
            ] {
                assert!(hex(value).is_some(), "{}: {value}", palette.id);
            }
            let _ = palette.colors();
        }
    }
    #[test]
    fn accessibility_overrides_catalog_and_missing_ids_fall_back() {
        let preferences = crate::accessibility::Preferences {
            high_contrast: true,
            reduced_motion: true,
        };
        assert_eq!(
            resolve(Some(&palettes()[0].id), ThemeMode::Dark, preferences).text_primary,
            ThemeColors::high_contrast_dark().text_primary
        );
        assert_eq!(
            resolve(
                Some("missing"),
                ThemeMode::Light,
                crate::accessibility::Preferences::default()
            )
            .bg_panel,
            ThemeColors::light().bg_panel
        );
    }
}
