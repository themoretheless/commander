//! Pure squarified-treemap layout. Turns a list of weights into rectangles
//! filling a region, keeping each tile's aspect ratio as near 1 as possible.
//! No egui types, so the geometry is unit-tested directly.

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn area(&self) -> f64 {
        self.w * self.h
    }
}

/// Lay out `weights` as a squarified treemap inside `rect`. The returned rects
/// are in input order and their areas are proportional to the weights (summing
/// to the region area). Non-positive weights are treated as 0; an empty or
/// all-zero input yields an empty layout.
pub fn squarify(weights: &[f64], rect: Rect) -> Vec<Rect> {
    let n = weights.len();
    if n == 0 || rect.w <= 0.0 || rect.h <= 0.0 {
        return Vec::new();
    }
    let total: f64 = weights.iter().map(|w| w.max(0.0)).sum();
    if total <= 0.0 {
        return Vec::new();
    }

    // Scale weights to areas that sum to the region area.
    let scale = rect.area() / total;
    let areas: Vec<f64> = weights.iter().map(|w| w.max(0.0) * scale).collect();

    let mut out = vec![
        Rect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        };
        n
    ];
    let mut free = rect;
    let mut i = 0; // next area index to place

    while i < n {
        // Skip zero-area items (give them a degenerate rect at the origin).
        if areas[i] <= 0.0 {
            out[i] = Rect {
                x: free.x,
                y: free.y,
                w: 0.0,
                h: 0.0,
            };
            i += 1;
            continue;
        }
        let side = free.w.min(free.h); // lay the row along the shorter side
        // Grow the current row while it improves (lowers) the worst aspect.
        let mut row_end = i + 1;
        let mut row_sum = areas[i];
        let mut best = worst_ratio_sum(areas[i], areas[i], areas[i], side);
        while row_end < n && areas[row_end] > 0.0 {
            let new_sum = row_sum + areas[row_end];
            let row_min = areas[i..=row_end]
                .iter()
                .copied()
                .fold(f64::INFINITY, f64::min);
            let row_max = areas[i..=row_end].iter().copied().fold(0.0, f64::max);
            let cand = worst_ratio_sum(new_sum, row_min, row_max, side);
            if cand <= best {
                best = cand;
                row_sum = new_sum;
                row_end += 1;
            } else {
                break;
            }
        }

        // Place the row [i, row_end) along the shorter side, stacking tiles
        // along the longer side, then shrink the free rect.
        let thickness = row_sum / side; // depth into the longer side
        if free.w <= free.h {
            // Row spans the full width; tiles vary in width.
            let mut x = free.x;
            for (k, a) in areas[i..row_end].iter().enumerate() {
                let w = a / thickness;
                out[i + k] = Rect {
                    x,
                    y: free.y,
                    w,
                    h: thickness,
                };
                x += w;
            }
            free = Rect {
                x: free.x,
                y: free.y + thickness,
                w: free.w,
                h: free.h - thickness,
            };
        } else {
            // Row spans the full height; tiles vary in height.
            let mut y = free.y;
            for (k, a) in areas[i..row_end].iter().enumerate() {
                let h = a / thickness;
                out[i + k] = Rect {
                    x: free.x,
                    y,
                    w: thickness,
                    h,
                };
                y += h;
            }
            free = Rect {
                x: free.x + thickness,
                y: free.y,
                w: free.w - thickness,
                h: free.h,
            };
        }
        i = row_end;
    }

    out
}

/// Worst aspect ratio (>= 1) of a row whose tiles total `sum`, with smallest
/// and largest tile areas `min`/`max`, laid along length `side`.
fn worst_ratio_sum(sum: f64, min: f64, max: f64, side: f64) -> f64 {
    if sum <= 0.0 || side <= 0.0 {
        return f64::INFINITY;
    }
    let s2 = side * side;
    let sum2 = sum * sum;
    (s2 * max / sum2).max(sum2 / (s2 * min))
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 80.0,
    };

    fn within(inner: &Rect, outer: &Rect, eps: f64) -> bool {
        inner.x >= outer.x - eps
            && inner.y >= outer.y - eps
            && inner.x + inner.w <= outer.x + outer.w + eps
            && inner.y + inner.h <= outer.y + outer.h + eps
    }

    #[test]
    fn empty_and_zero_inputs_yield_empty() {
        assert!(squarify(&[], R).is_empty());
        assert!(squarify(&[0.0, 0.0], R).is_empty());
    }

    #[test]
    fn single_weight_fills_the_rect() {
        let tiles = squarify(&[5.0], R);
        assert_eq!(tiles.len(), 1);
        assert!((tiles[0].area() - R.area()).abs() < 1e-6);
        assert!(within(&tiles[0], &R, 1e-6));
    }

    #[test]
    fn length_matches_and_tiles_stay_within_bounds() {
        let weights = [10.0, 6.0, 4.0, 3.0, 2.0, 1.0, 1.0];
        let tiles = squarify(&weights, R);
        assert_eq!(tiles.len(), weights.len());
        for t in &tiles {
            assert!(within(t, &R, 1e-6), "tile {t:?} escaped {R:?}");
        }
    }

    #[test]
    fn total_area_is_conserved() {
        let weights = [10.0, 6.0, 4.0, 3.0, 2.0, 1.0];
        let tiles = squarify(&weights, R);
        let total: f64 = tiles.iter().map(|t| t.area()).sum();
        assert!(
            (total - R.area()).abs() < 1e-3,
            "area {total} vs {}",
            R.area()
        );
    }

    #[test]
    fn areas_are_proportional_to_weights() {
        let weights = [3.0, 1.0];
        let tiles = squarify(&weights, R);
        let ratio = tiles[0].area() / tiles[1].area();
        assert!((ratio - 3.0).abs() < 1e-6, "ratio {ratio}");
    }

    #[test]
    fn uniform_weights_are_reasonably_square() {
        let weights = [1.0; 9];
        let tiles = squarify(
            &weights,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 90.0,
                h: 90.0,
            },
        );
        for t in &tiles {
            let aspect = (t.w / t.h).max(t.h / t.w);
            assert!(aspect <= 3.0, "tile aspect {aspect} too oblong: {t:?}");
        }
    }
}
