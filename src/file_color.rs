//! Pure semantic colors for file kinds, derived from a perceptual OKLCH model
//! so hues stay balanced and legible across light and dark themes. No egui
//! types, so the conversion, the palette and its WCAG contrast are all
//! unit-tested. Reuses the [`Kind`] classifier from `selection_summary`.

use crate::selection_summary::Kind;

/// Hue (OKLCH degrees) and chroma per kind. Lightness is chosen per theme.
/// Folder is a faint slate, Other a true neutral.
fn hue_chroma(kind: Kind) -> (f32, f32) {
    match kind {
        Kind::Folder => (255.0, 0.022),
        Kind::Image => (150.0, 0.13),
        Kind::Video => (300.0, 0.13),
        Kind::Audio => (65.0, 0.12),
        Kind::Document => (250.0, 0.13),
        Kind::Code => (25.0, 0.14),
        Kind::Archive => (95.0, 0.115),
        Kind::Other => (0.0, 0.0),
    }
}

/// An `(r, g, b)` accent for `kind`, themed for `dark` mode. Dark uses a high
/// lightness (bright on a dark panel); light uses a low lightness (dark enough
/// to stay legible on a near-white panel).
pub fn kind_color(kind: Kind, dark: bool) -> (u8, u8, u8) {
    let (h, c) = hue_chroma(kind);
    let l = if dark { 0.74 } else { 0.48 };
    oklch_to_srgb(l, c, h)
}

/// Convert OKLCH (lightness 0..1, chroma, hue in degrees) to an sRGB byte
/// triple, clamping out-of-gamut results.
pub fn oklch_to_srgb(l: f32, c: f32, h_deg: f32) -> (u8, u8, u8) {
    let h = h_deg.to_radians();
    let (a, b) = (c * h.cos(), c * h.sin());

    // OKLab -> LMS' -> LMS.
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (lc, mc, sc) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);

    // LMS -> linear sRGB.
    let r = 4.076_741_7 * lc - 3.307_711_6 * mc + 0.230_969_94 * sc;
    let g = -1.268_438 * lc + 2.609_757_4 * mc - 0.341_319_38 * sc;
    let bl = -0.004_196_086_3 * lc - 0.703_418_6 * mc + 1.707_614_7 * sc;

    (encode(r), encode(g), encode(bl))
}

/// Linear-light channel -> gamma sRGB byte.
fn encode(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// WCAG relative luminance of an sRGB byte triple, in `[0, 1]`. Used to verify
/// the palette's contrast in tests (not needed at runtime).
#[cfg(test)]
pub fn relative_luminance((r, g, b): (u8, u8, u8)) -> f32 {
    let lin = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG contrast ratio between two colors (>= 1.0, up to 21.0). Test-only: it
/// proves the OKLCH palette clears 3:1 against each panel background.
#[cfg(test)]
pub fn contrast_ratio(a: (u8, u8, u8), b: (u8, u8, u8)) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Kind; 8] = [
        Kind::Folder,
        Kind::Image,
        Kind::Video,
        Kind::Audio,
        Kind::Document,
        Kind::Code,
        Kind::Archive,
        Kind::Other,
    ];

    // The two themes' panel backgrounds (see theme.rs).
    const LIGHT_BG: (u8, u8, u8) = (252, 252, 254);
    const DARK_BG: (u8, u8, u8) = (24, 24, 30);

    fn close(a: (u8, u8, u8), b: (u8, u8, u8), tol: i32) -> bool {
        (a.0 as i32 - b.0 as i32).abs() <= tol
            && (a.1 as i32 - b.1 as i32).abs() <= tol
            && (a.2 as i32 - b.2 as i32).abs() <= tol
    }

    #[test]
    fn oklch_matches_known_references() {
        assert!(close(oklch_to_srgb(1.0, 0.0, 0.0), (255, 255, 255), 1));
        assert!(close(oklch_to_srgb(0.0, 0.0, 0.0), (0, 0, 0), 1));
        // sRGB red per the OKLab reference (L 0.6279, C 0.2576, h 29.23 deg).
        assert!(
            close(oklch_to_srgb(0.627_95, 0.257_6, 29.23), (255, 0, 0), 3),
            "got {:?}",
            oklch_to_srgb(0.627_95, 0.257_6, 29.23)
        );
    }

    #[test]
    fn luminance_and_contrast_endpoints() {
        assert!(relative_luminance((0, 0, 0)) < 1e-6);
        assert!((relative_luminance((255, 255, 255)) - 1.0).abs() < 1e-3);
        assert!((contrast_ratio((0, 0, 0), (255, 255, 255)) - 21.0).abs() < 0.2);
    }

    #[test]
    fn every_stub_clears_three_to_one_against_its_panel() {
        for &k in &ALL {
            let dark = kind_color(k, true);
            let light = kind_color(k, false);
            assert!(
                contrast_ratio(dark, DARK_BG) >= 3.0,
                "{k:?} dark {dark:?} ratio {}",
                contrast_ratio(dark, DARK_BG)
            );
            assert!(
                contrast_ratio(light, LIGHT_BG) >= 3.0,
                "{k:?} light {light:?} ratio {}",
                contrast_ratio(light, LIGHT_BG)
            );
        }
    }

    #[test]
    fn deterministic() {
        for &k in &ALL {
            assert_eq!(kind_color(k, true), kind_color(k, true));
            assert_eq!(kind_color(k, false), kind_color(k, false));
        }
    }

    fn spread((r, g, b): (u8, u8, u8)) -> u8 {
        r.max(g).max(b) - r.min(g).min(b)
    }

    #[test]
    fn folder_and_other_stay_near_neutral() {
        for dark in [true, false] {
            assert!(
                spread(kind_color(Kind::Folder, dark)) <= 45,
                "folder spread {}",
                spread(kind_color(Kind::Folder, dark))
            );
            assert!(
                spread(kind_color(Kind::Other, dark)) <= 4,
                "other should be grey"
            );
        }
    }
}
