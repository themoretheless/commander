//! List density: a tiny pure lookup from a named tier to the row metrics the
//! file list renders with. No egui types here, so the tier ordering and the
//! metric monotonicity are unit-tested directly.

use serde::{Deserialize, Serialize};

/// How much breathing room the file list gives each row.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Density {
    /// Roomy rows, fewest on screen.
    Spacious,
    /// The default look (unchanged from before density modes existed).
    #[default]
    Comfortable,
    /// Tight rows, most on screen.
    Compact,
}

/// Concrete per-row sizes for a [`Density`]. Point sizes feed egui font sizes;
/// `row_pad_y` is the vertical inner margin of each row.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DensityMetrics {
    pub row_pad_y: f32,
    pub icon_pt: f32,
    pub name_pt: f32,
    pub meta_pt: f32,
}

/// The row metrics for `density`. Comfortable matches the sizes the list used
/// before density modes, so the default look is unchanged.
pub fn metrics(density: Density) -> DensityMetrics {
    match density {
        Density::Spacious => DensityMetrics {
            row_pad_y: 7.0,
            icon_pt: 16.0,
            name_pt: 14.0,
            meta_pt: 12.0,
        },
        Density::Comfortable => DensityMetrics {
            row_pad_y: 4.0,
            icon_pt: 14.0,
            name_pt: 13.0,
            meta_pt: 11.0,
        },
        Density::Compact => DensityMetrics {
            row_pad_y: 2.0,
            icon_pt: 12.0,
            name_pt: 12.0,
            meta_pt: 10.0,
        },
    }
}

pub fn label(density: Density) -> &'static str {
    match density {
        Density::Compact => "Compact",
        Density::Comfortable => "Comfortable",
        Density::Spacious => "Spacious",
    }
}

pub fn short_label(density: Density) -> &'static str {
    match density {
        Density::Compact => "C",
        Density::Comfortable => "M",
        Density::Spacious => "S",
    }
}

/// Densest-to-roomiest order, for cycling.
const ORDER: [Density; 3] = [Density::Compact, Density::Comfortable, Density::Spacious];

/// Step to the next density tier, toward Spacious, wrapping to Compact.
pub fn cycle(density: Density) -> Density {
    let idx = ORDER.iter().position(|&d| d == density).unwrap_or(1);
    ORDER[(idx + 1) % ORDER.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_are_monotonic_compact_to_spacious() {
        let c = metrics(Density::Compact);
        let m = metrics(Density::Comfortable);
        let s = metrics(Density::Spacious);
        assert!(c.row_pad_y < m.row_pad_y && m.row_pad_y < s.row_pad_y);
        assert!(c.icon_pt < m.icon_pt && m.icon_pt < s.icon_pt);
        assert!(c.name_pt < m.name_pt && m.name_pt < s.name_pt);
        assert!(c.meta_pt < m.meta_pt && m.meta_pt < s.meta_pt);
    }

    #[test]
    fn comfortable_is_the_default() {
        assert_eq!(Density::default(), Density::Comfortable);
    }

    #[test]
    fn labels_are_stable_for_toolbar_copy() {
        assert_eq!(label(Density::Compact), "Compact");
        assert_eq!(short_label(Density::Comfortable), "M");
        assert_eq!(label(Density::Spacious), "Spacious");
    }

    #[test]
    fn cycle_steps_toward_spacious_and_wraps() {
        assert_eq!(cycle(Density::Compact), Density::Comfortable);
        assert_eq!(cycle(Density::Comfortable), Density::Spacious);
        assert_eq!(cycle(Density::Spacious), Density::Compact);
    }
}
