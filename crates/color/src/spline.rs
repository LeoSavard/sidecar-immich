// SPDX-License-Identifier: Apache-2.0
//! Monotone cubic interpolation (Fritsch & Carlson, 1980, "Monotone Piecewise Cubic Interpolation",
//! SIAM J. Numer. Anal. 17(2)). Unlike a natural cubic spline it never overshoots, so a curve through
//! increasing points can't invert tones.

pub const LUT_SIZE: usize = 4096;

/// A monotone cubic through points, tangents computed once. Held flat past either end.
pub struct Monotone {
    pts: Vec<[f64; 2]>,
    m: Vec<f64>,
}

impl Monotone {
    /// Through `points` in order of `x`, duplicates of an `x` dropped; `None` below two distinct points.
    pub fn new(points: &[[f64; 2]]) -> Option<Self> {
        let mut pts = points.to_vec();
        pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
        pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-9);
        let n = pts.len();
        if n < 2 {
            return None;
        }
        let d: Vec<f64> = (0..n - 1).map(|k| (pts[k + 1][1] - pts[k][1]) / (pts[k + 1][0] - pts[k][0])).collect();
        let mut m = vec![0.0; n];
        m[0] = d[0];
        m[n - 1] = d[n - 2];
        for k in 1..n - 1 {
            m[k] = if d[k - 1] * d[k] <= 0.0 { 0.0 } else { (d[k - 1] + d[k]) / 2.0 };
        }
        for k in 0..n - 1 {
            if d[k] == 0.0 {
                m[k] = 0.0;
                m[k + 1] = 0.0;
                continue;
            }
            let (a, b) = (m[k] / d[k], m[k + 1] / d[k]);
            let s = a * a + b * b;
            if s > 9.0 {
                let tau = 3.0 / s.sqrt();
                m[k] = tau * a * d[k];
                m[k + 1] = tau * b * d[k];
            }
        }
        Some(Self { pts, m })
    }

    pub fn eval(&self, x: f64) -> f64 {
        let (pts, m, n) = (&self.pts, &self.m, self.pts.len());
        if x <= pts[0][0] {
            return pts[0][1];
        }
        if x >= pts[n - 1][0] {
            return pts[n - 1][1];
        }
        let k = pts.windows(2).position(|w| x < w[1][0]).expect("x is inside the points");
        let h = pts[k + 1][0] - pts[k][0];
        let t = (x - pts[k][0]) / h;
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * pts[k][1] + (t3 - 2.0 * t2 + t) * h * m[k] + (-2.0 * t3 + 3.0 * t2) * pts[k + 1][1] + (t3 - t2) * h * m[k + 1]
    }
}

/// The tone curve's lookup table: `size` samples over 0…1 of the monotone cubic through `points` (clamped to
/// the unit square), or the identity when there aren't two points.
pub fn monotone_lut(points: &[[f64; 2]], size: usize) -> Vec<f32> {
    let clamped: Vec<[f64; 2]> = points.iter().map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)]).collect();
    let curve = Monotone::new(&clamped);
    (0..size)
        .map(|i| {
            let x = i as f64 / (size - 1) as f64;
            curve.as_ref().map_or(x, |c| c.eval(x).clamp(0.0, 1.0)) as f32
        })
        .collect()
}

/// How much Highlights and Shadows move their curve point, ±100 being ±this. Kept here, not just in the web
/// app, because `migrate_highlight_shadow_anchors` (params.rs) needs the exact value an old sidecar's curve
/// point was written with to carry its strength forward unchanged.
pub const HIGHLIGHT_SHADOW_SCALE: f64 = 0.15;

/// How much of the Highlights (1.0) vs. Shadows (0.0) bump a point at `x` feels: a raised-cosine ease between
/// the two sliders' own anchors (0.25, 0.75) — 0 or 1 at and past each anchor, so a slider still gets its full
/// effect exactly where its curve point used to sit, but nothing changes abruptly at the midpoint the way a
/// hard split there would. The same shape the colour mixer's hue bands already use for the same reason.
fn highlight_weight(x: f64) -> f64 {
    let t = ((x - 0.25) / 0.5).clamp(0.0, 1.0);
    0.5 - 0.5 * (std::f64::consts::PI * t).cos()
}

/// Where Highlights and Shadows have their full effect: the points their curve points used to sit at.
const SHADOW_ANCHOR: f64 = 0.25;
const HIGHLIGHT_ANCHOR: f64 = 0.75;

/// How much of the bump survives at `x`: all of it between the two anchors, easing to **nothing** at pure
/// black and pure white.
///
/// Without this the bump is a pedestal rather than a bump. `highlight_weight` splits the range *between* the
/// two sliders and so sums to 1 everywhere, including at the ends — Shadows applied whole at x = 0, Highlights
/// whole at x = 1. Lifting Shadows then lifted black off zero, and pulling Highlights down pulled white off
/// one; the other signs clamped instead, crushing a band at each end into solid black or white. Either way the
/// histogram's ends moved or clipped when the sliders were meant to move the tones between them.
///
/// The taper leaves everything between the anchors exactly as it was — the sliders keep their strength where
/// they are aimed, and their midpoint blend is untouched — and only brings the tails home.
fn ends_taper(x: f64) -> f64 {
    let t = if x < SHADOW_ANCHOR {
        x / SHADOW_ANCHOR
    } else if x > HIGHLIGHT_ANCHOR {
        (1.0 - x) / (1.0 - HIGHLIGHT_ANCHOR)
    } else {
        return 1.0;
    };
    0.5 - 0.5 * (std::f64::consts::PI * t.clamp(0.0, 1.0)).cos()
}

/// The tone curve's LUT, with Highlights and Shadows as a smooth additive bump instead of literal points in
/// `points` — one LUT either way, so the develop pass samples exactly what this returns. Calling this with
/// `points: &[]` is the curve Highlights/Shadows alone would produce, which is what a "ghost" preview of the
/// sliders, independent of any manually-drawn curve, needs; see docs/ROADMAP.md.
pub fn tone_curve_lut(points: &[[f64; 2]], highlights: f64, shadows: f64, size: usize) -> Vec<f32> {
    let base = monotone_lut(points, size);
    if highlights == 0.0 && shadows == 0.0 {
        return base;
    }
    let h = highlights.clamp(-100.0, 100.0) / 100.0 * HIGHLIGHT_SHADOW_SCALE;
    let s = shadows.clamp(-100.0, 100.0) / 100.0 * HIGHLIGHT_SHADOW_SCALE;
    base.iter()
        .enumerate()
        .map(|(i, &y)| {
            let x = i as f64 / (size - 1) as f64;
            let w = highlight_weight(x);
            (y as f64 + ends_taper(x) * (h * w + s * (1.0 - w))).clamp(0.0, 1.0) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_monotone_s_curve() {
        let id = monotone_lut(&[], 5);
        assert_eq!(id, vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        let s = monotone_lut(&[[0.0, 0.0], [0.25, 0.15], [0.75, 0.9], [1.0, 1.0]], LUT_SIZE);
        assert!(s.windows(2).all(|w| w[1] >= w[0]));
        assert!((s[LUT_SIZE / 4] - 0.15).abs() < 0.01);
    }

    /// Black stays black and white stays white, whatever Highlights and Shadows are set to. They move the
    /// tones *between* the ends; they are not a black- or white-point control, and nothing should clip or walk
    /// off the end of the histogram because one of them moved.
    #[test]
    fn the_ends_are_pinned_whatever_the_sliders_do() {
        let n = 4096;
        for h in [-100.0, -60.0, 0.0, 60.0, 100.0] {
            for s in [-100.0, -60.0, 0.0, 60.0, 100.0] {
                let lut = tone_curve_lut(&[], h, s, n);
                assert!(lut[0].abs() < 1e-6, "highlights {h}, shadows {s}: black became {}", lut[0]);
                assert!((lut[n - 1] - 1.0).abs() < 1e-6, "highlights {h}, shadows {s}: white became {}", lut[n - 1]);
                // Nor may they clip a band at either end flat, which is the same fault wearing a clamp.
                assert!(lut[1] > lut[0] && lut[n - 1] > lut[n - 2], "highlights {h}, shadows {s}: an end is flat");
                assert!(lut.windows(2).all(|w| w[1] >= w[0]), "highlights {h}, shadows {s}: not monotone");
            }
        }
        // The ends are pinned for a drawn curve too, not just the identity.
        let drawn = [[0.0, 0.0], [0.5, 0.42], [1.0, 1.0]];
        let lut = tone_curve_lut(&drawn, 100.0, -100.0, n);
        assert!(lut[0].abs() < 1e-6 && (lut[n - 1] - 1.0).abs() < 1e-6, "a drawn curve's ends moved: {} {}", lut[0], lut[n - 1]);
    }

    #[test]
    fn highlight_weight_is_a_complementary_raised_cosine() {
        assert_eq!(highlight_weight(0.0), 0.0);
        assert_eq!(highlight_weight(0.25), 0.0);
        assert_eq!(highlight_weight(0.75), 1.0);
        assert_eq!(highlight_weight(1.0), 1.0);
        assert!((highlight_weight(0.5) - 0.5).abs() < 1e-9, "the midpoint is where the two are equal, not a seam");
        for x in [0.0, 0.1, 0.25, 0.4, 0.5, 0.6, 0.75, 0.9, 1.0] {
            let w = highlight_weight(x);
            assert!((0.0..=1.0).contains(&w));
        }
    }

    #[test]
    fn a_slider_at_its_own_anchor_matches_the_old_point_based_strength_exactly() {
        // At x=0.75 (Highlights' own anchor), highlight_weight is 1 and the Shadows term drops out — so a
        // Highlights slider of `v` must land exactly where the old mechanism's curve point did:
        // `x + v/100 * HIGHLIGHT_SHADOW_SCALE` (edit-view.ts's `withCurvePoint`, before this change).
        for v in [-100.0, -42.0, 0.0, 17.0, 100.0] {
            let lut = tone_curve_lut(&[], v, 0.0, LUT_SIZE);
            let expected = 0.75 + v / 100.0 * HIGHLIGHT_SHADOW_SCALE;
            assert!((lut[(0.75 * (LUT_SIZE - 1) as f64).round() as usize] as f64 - expected).abs() < 1e-3, "highlights={v}");
        }
        for v in [-100.0, -42.0, 0.0, 17.0, 100.0] {
            let lut = tone_curve_lut(&[], 0.0, v, LUT_SIZE);
            let expected = 0.25 + v / 100.0 * HIGHLIGHT_SHADOW_SCALE;
            assert!((lut[(0.25 * (LUT_SIZE - 1) as f64).round() as usize] as f64 - expected).abs() < 1e-3, "shadows={v}");
        }
    }

    #[test]
    fn zero_highlights_and_shadows_is_exactly_the_manual_curve() {
        let points = [[0.0, 0.0], [0.3, 0.2], [0.7, 0.85], [1.0, 1.0]];
        assert_eq!(tone_curve_lut(&points, 0.0, 0.0, LUT_SIZE), monotone_lut(&points, LUT_SIZE));
    }

    #[test]
    fn the_bump_never_exceeds_the_unit_range() {
        let lut = tone_curve_lut(&[[0.0, 0.9], [1.0, 1.0]], 100.0, -100.0, LUT_SIZE);
        assert!(lut.iter().all(|&v| (0.0..=1.0).contains(&v)));
    }
}
