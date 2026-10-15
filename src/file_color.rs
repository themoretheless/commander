//! Pure semantic colors for file kinds. No egui types, so the palette and its
//! contrast and color-vision separation are unit-tested. Reuses the [`Kind`]
//! classifier from `selection_summary`.

use crate::selection_summary::Kind;

/// Stub and glyph colors per kind (never text), as `(dark, light)`. The six hued kinds were
/// searched in OKLCH so each clears 4.5:1 on plain, zebra and hovered rows,
/// at least the 3:1 graphics bar on cursor and selected rows in every theme,
/// and stays apart under deuteranopia, protanopia and tritanopia; Folder and
/// Other are warm neutrals.
type Rgb = (u8, u8, u8);

const PALETTE: [(Kind, Rgb, Rgb); 8] = [
    (Kind::Folder, (0xb9, 0xb5, 0xb0), (0x57, 0x52, 0x4d)),
    (Kind::Document, (0x98, 0xcf, 0xfb), (0x14, 0x64, 0xa8)),
    (Kind::Image, (0x8d, 0xbb, 0x82), (0x00, 0x61, 0x44)),
    (Kind::Video, (0xbe, 0xa4, 0xf6), (0x5b, 0x43, 0x7f)),
    (Kind::Audio, (0xcc, 0x93, 0x3f), (0x71, 0x3f, 0x01)),
    (Kind::Code, (0xef, 0xa6, 0xb2), (0x96, 0x35, 0x3a)),
    (Kind::Archive, (0xd2, 0xc1, 0x5f), (0x7f, 0x63, 0x0d)),
    (Kind::Other, (0xa2, 0x9e, 0x99), (0x6a, 0x68, 0x66)),
];

/// An `(r, g, b)` color for `kind`, themed for `dark` mode.
pub fn kind_color(kind: Kind, dark: bool) -> Rgb {
    let (_, on_dark, on_light) = PALETTE
        .iter()
        .find(|(entry, _, _)| *entry == kind)
        .copied()
        .unwrap_or(PALETTE[PALETTE.len() - 1]);
    if dark { on_dark } else { on_light }
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

    // Hand-copied row backgrounds (panel, cursor, selected, hover) from an
    // older palette. They no longer match what file_list paints; the
    // ThemeColors-based tests (`hued_kinds_stay_legible_on_the_real_theme_rows`
    // and `every_stub_clears_three_to_one_in_every_theme`) are the
    // authoritative contrast checks. These stay as an extra fixed fixture.
    const DARK_ROWS: [(u8, u8, u8); 4] = [
        (0x1b, 0x18, 0x15),
        (0x2d, 0x29, 0x25),
        (0x27, 0x24, 0x1f),
        (0x23, 0x20, 0x1d),
    ];
    const LIGHT_ROWS: [(u8, u8, u8); 4] = [
        (0xfd, 0xfc, 0xf9),
        (0xeb, 0xe7, 0xe2),
        (0xf1, 0xee, 0xe9),
        (0xf6, 0xf3, 0xef),
    ];
    const DARK_BG: (u8, u8, u8) = DARK_ROWS[0];
    const LIGHT_BG: (u8, u8, u8) = LIGHT_ROWS[0];

    use crate::theme::ThemeColors;
    use egui::Color32;

    /// Every theme a row can be painted in, with whether it uses the dark
    /// half of the palette.
    fn themes() -> [(ThemeColors, bool); 4] {
        [
            (ThemeColors::dark(), true),
            (ThemeColors::light(), false),
            (ThemeColors::high_contrast_dark(), true),
            (ThemeColors::high_contrast_light(), false),
        ]
    }

    fn rgb(color: Color32) -> Rgb {
        (color.r(), color.g(), color.b())
    }

    /// Opaque row backgrounds as `app::file_list` composites them over the
    /// panel, each also under the hover tint: `(plain rows, highlighted
    /// rows)`, where plain is the panel and zebra fill and highlighted is
    /// the cursor (active and inactive pane) and selected fill. The fill
    /// factors mirror the row painter.
    fn row_backgrounds(colors: ThemeColors) -> (Vec<Rgb>, Vec<Rgb>) {
        let panel = colors.bg_panel;
        let hover = colors.bg_hover.linear_multiply(0.3);
        let composite = |fills: &[Color32]| -> Vec<Rgb> {
            fills
                .iter()
                .flat_map(|&fill| {
                    let row = panel.blend(fill);
                    [rgb(row), rgb(row.blend(hover))]
                })
                .collect()
        };
        (
            composite(&[Color32::TRANSPARENT, colors.bg_card.linear_multiply(0.3)]),
            composite(&[
                colors.bg_selected.linear_multiply(0.22),
                colors.bg_selected.linear_multiply(0.08),
                colors.accent_purple.linear_multiply(0.15),
            ]),
        )
    }

    #[test]
    fn hued_kinds_stay_legible_on_the_real_theme_rows() {
        for (colors, dark) in themes() {
            let (plain, highlighted) = row_backgrounds(colors);
            for &k in &HUED {
                let color = kind_color(k, dark);
                for &row in &plain {
                    let ratio = contrast_ratio(color, row);
                    assert!(ratio >= 4.5, "{k:?} dark={dark} on {row:?}: {ratio}");
                }
                // The stub and glyph are graphics: 3:1 is the WCAG bar on the
                // tinted cursor and selection rows.
                for &row in &highlighted {
                    let ratio = contrast_ratio(color, row);
                    assert!(ratio >= 3.0, "{k:?} dark={dark} on {row:?}: {ratio}");
                }
            }
        }
    }

    #[test]
    fn every_stub_clears_three_to_one_in_every_theme() {
        for &k in &ALL {
            for (colors, dark) in themes() {
                let stub = kind_color(k, dark);
                let ratio = contrast_ratio(stub, rgb(colors.bg_panel));
                assert!(ratio >= 3.0, "{k:?} dark={dark} {stub:?} ratio {ratio}");
            }
        }
    }

    /// Machado et al. 2009 dichromacy matrices (severity 1) on linear RGB.
    const CVD: [[[f32; 3]; 3]; 3] = [
        [
            [0.367_322, 0.860_646, -0.227_968],
            [0.280_085, 0.672_501, 0.047_413],
            [-0.011_82, 0.042_94, 0.968_881],
        ],
        [
            [0.152_286, 1.052_583, -0.204_868],
            [0.114_503, 0.786_281, 0.099_216],
            [-0.003_882, -0.048_116, 1.051_998],
        ],
        [
            [1.255_528, -0.076_749, -0.178_779],
            [-0.078_411, 0.930_809, 0.147_602],
            [0.004_733, 0.691_367, 0.303_9],
        ],
    ];

    fn linear((r, g, b): (u8, u8, u8)) -> [f32; 3] {
        let lin = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.040_45 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        [lin(r), lin(g), lin(b)]
    }

    fn oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
        let (r, g, b) = (r.max(0.0), g.max(0.0), b.max(0.0));
        let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_99 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        [
            0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
            0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
        ]
    }

    fn simulate(m: &[[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|i| m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2])
    }

    fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    const HUED: [Kind; 7] = [
        Kind::Document,
        Kind::Image,
        Kind::Video,
        Kind::Audio,
        Kind::Code,
        Kind::Archive,
        Kind::Other,
    ];

    #[test]
    fn hued_kinds_clear_text_contrast_on_every_row_background() {
        for &k in &HUED {
            for (dark, rows) in [(true, DARK_ROWS), (false, LIGHT_ROWS)] {
                for row in rows {
                    let ratio = contrast_ratio(kind_color(k, dark), row);
                    assert!(ratio >= 4.5, "{k:?} dark={dark} on {row:?}: {ratio}");
                }
            }
        }
    }

    #[test]
    fn kinds_stay_distinct_under_color_vision_deficiencies() {
        for dark in [true, false] {
            for matrix in [None, Some(&CVD[0]), Some(&CVD[1]), Some(&CVD[2])] {
                let seen = HUED.map(|k| {
                    let c = linear(kind_color(k, dark));
                    oklab(matrix.map_or(c, |m| simulate(m, c)))
                });
                for i in 0..seen.len() {
                    for j in i + 1..seen.len() {
                        let d = distance(seen[i], seen[j]);
                        assert!(
                            d >= 0.05,
                            "{:?}/{:?} dark={dark} cvd={matrix:?}: {d}",
                            HUED[i],
                            HUED[j]
                        );
                    }
                }
            }
        }
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
                spread(kind_color(Kind::Other, dark)) <= 12,
                "other should stay a warm neutral"
            );
        }
    }
}
