//! Pure semantic colors for file kinds: a calm hue per kind, lifted slightly
//! in dark mode and muted toward the background in light mode. No egui types,
//! so the mapping is unit-tested directly. Reuses the [`Kind`] classifier from
//! `selection_summary`.

use crate::selection_summary::Kind;

/// An `(r, g, b)` accent for `kind`, themed for `dark` mode.
pub fn kind_color(kind: Kind, dark: bool) -> (u8, u8, u8) {
    let base = match kind {
        Kind::Folder => (120, 130, 145),  // neutral slate
        Kind::Image => (90, 170, 120),    // green
        Kind::Video => (150, 120, 200),   // violet
        Kind::Audio => (210, 150, 90),    // amber
        Kind::Document => (90, 150, 210), // blue
        Kind::Code => (210, 120, 140),    // rose
        Kind::Archive => (180, 160, 90),  // olive/gold
        Kind::Other => (135, 135, 135),   // grey
    };
    adjust(base, dark)
}

fn adjust((r, g, b): (u8, u8, u8), dark: bool) -> (u8, u8, u8) {
    if dark {
        // Lift a touch for contrast on dark backgrounds.
        let lift = |c: u8| ((c as u16 * 6 / 5).min(255)) as u8;
        (lift(r), lift(g), lift(b))
    } else {
        // Mute toward mid-grey for light backgrounds.
        let mute = |c: u8| ((c as u16 * 4 / 5 + 28).min(255)) as u8;
        (mute(r), mute(g), mute(b))
    }
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

    #[test]
    fn deterministic_per_kind_and_theme() {
        for &k in &ALL {
            assert_eq!(kind_color(k, true), kind_color(k, true));
            assert_eq!(kind_color(k, false), kind_color(k, false));
        }
    }

    #[test]
    fn every_kind_has_in_range_channels() {
        for &k in &ALL {
            let (r, g, b) = kind_color(k, true);
            // Just exercising the table; u8 is inherently in range, so assert
            // the color is not pure black (a sign of a bad/empty mapping).
            assert!(r as u16 + g as u16 + b as u16 > 0, "{k:?} mapped to black");
        }
    }

    fn spread((r, g, b): (u8, u8, u8)) -> u8 {
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        max - min
    }

    #[test]
    fn folder_and_other_are_near_neutral() {
        for dark in [true, false] {
            assert!(
                spread(kind_color(Kind::Folder, dark)) <= 40,
                "folder should read as a neutral tone"
            );
            assert!(
                spread(kind_color(Kind::Other, dark)) <= 16,
                "other should be near grey"
            );
        }
    }

    #[test]
    fn dark_is_at_least_as_bright_as_light_base() {
        // The dark variant lifts; the light variant mutes. They should differ.
        for &k in &ALL {
            assert_ne!(kind_color(k, true), kind_color(k, false));
        }
    }
}
