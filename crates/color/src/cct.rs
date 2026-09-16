// SPDX-License-Identifier: Apache-2.0
//! Correlated colour temperature and tint ↔ CIE xy.
//!
//! The Planckian locus uses Krystek's rational approximation in CIE 1960 (u, v) (Krystek, 1985, "An
//! algorithm to calculate correlated colour temperature", Color Research & Application 10(1)). Tint is a
//! perpendicular offset from the locus of `tint / 3000` in (u, v). The slider names what happens to the
//! *image*, as every editor does: positive tint makes the photo more magenta, which means moving the
//! assumed illuminant toward green.

pub const TINT_SCALE: f64 = 3000.0;

fn locus_uv(t: f64) -> [f64; 2] {
    let t2 = t * t;
    let u = (0.860117757 + 1.54118254e-4 * t + 1.28641212e-7 * t2) / (1.0 + 8.42420235e-4 * t + 7.08145163e-7 * t2);
    let v = (0.317398726 + 4.22806245e-5 * t + 4.20481691e-8 * t2) / (1.0 - 2.89741816e-5 * t + 1.61456053e-7 * t2);
    [u, v]
}

/// Unit normal to the locus at `t`, oriented toward green (increasing v).
fn locus_normal(t: f64) -> [f64; 2] {
    let a = locus_uv(t * 0.999);
    let b = locus_uv(t * 1.001);
    let (du, dv) = (b[0] - a[0], b[1] - a[1]);
    let len = du.hypot(dv);
    let n = [-dv / len, du / len];
    if n[1] < 0.0 { [-n[0], -n[1]] } else { n }
}

fn uv_to_xy(uv: [f64; 2]) -> [f64; 2] {
    let d = 2.0 * uv[0] - 8.0 * uv[1] + 4.0;
    [3.0 * uv[0] / d, 2.0 * uv[1] / d]
}

fn xy_to_uv(xy: [f64; 2]) -> [f64; 2] {
    let d = -2.0 * xy[0] + 12.0 * xy[1] + 3.0;
    [4.0 * xy[0] / d, 6.0 * xy[1] / d]
}

pub fn temp_tint_to_xy(temperature_k: f64, tint: f64) -> [f64; 2] {
    let t = temperature_k.clamp(1000.0, 50000.0);
    let base = locus_uv(t);
    let n = locus_normal(t);
    let off = tint / TINT_SCALE;
    uv_to_xy([base[0] + n[0] * off, base[1] + n[1] * off])
}

pub fn xy_to_temp_tint(xy: [f64; 2]) -> (f64, f64) {
    let uv = xy_to_uv(xy);
    let dist = |mired: f64| {
        let p = locus_uv(1e6 / mired);
        (uv[0] - p[0]).hypot(uv[1] - p[1])
    };
    // Coarse scan in mired space (20–1000 ↔ 50,000–1,000 K), then golden-section refinement.
    let mut best = 20.0;
    let mut best_d = f64::MAX;
    let mut m = 20.0;
    while m <= 1000.0 {
        let d = dist(m);
        if d < best_d {
            best_d = d;
            best = m;
        }
        m += 5.0;
    }
    let (mut lo, mut hi) = ((best - 5.0).max(20.0), (best + 5.0).min(1000.0));
    let g = (5f64.sqrt() - 1.0) / 2.0;
    for _ in 0..60 {
        let a = hi - g * (hi - lo);
        let b = lo + g * (hi - lo);
        if dist(a) < dist(b) { hi = b } else { lo = a }
    }
    let t = 1e6 / ((lo + hi) / 2.0);
    let p = locus_uv(t);
    let n = locus_normal(t);
    let tint = ((uv[0] - p[0]) * n[0] + (uv[1] - p[1]) * n[1]) * TINT_SCALE;
    (t, tint)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d65_is_about_6500k_and_close_to_the_locus() {
        let (t, tint) = xy_to_temp_tint(crate::mat::D65);
        assert!((t - 6504.0).abs() < 25.0, "{t}");
        assert!(tint.abs() < 15.0, "{tint}");
    }

    /// Positive tint must make the image magenta, i.e. move the assumed illuminant toward green (higher v
    /// in CIE 1960). Getting this backwards makes the slider fight its own track gradient.
    #[test]
    fn positive_tint_moves_the_illuminant_toward_green() {
        let neutral = xy_to_uv(temp_tint_to_xy(5500.0, 0.0));
        let magenta_image = xy_to_uv(temp_tint_to_xy(5500.0, 40.0));
        assert!(magenta_image[1] > neutral[1], "{magenta_image:?} vs {neutral:?}");
    }

    #[test]
    fn round_trip() {
        for &(t, tint) in &[(2800.0, 0.0), (5000.0, 20.0), (7500.0, -35.0), (12000.0, 5.0)] {
            let (t2, tint2) = xy_to_temp_tint(temp_tint_to_xy(t, tint));
            assert!((t2 - t).abs() / t < 0.002, "{t} -> {t2}");
            assert!((tint2 - tint).abs() < 0.5, "{tint} -> {tint2}");
        }
    }
}
