// SPDX-License-Identifier: Apache-2.0
//! Monotone cubic interpolation (Fritsch & Carlson, 1980, "Monotone Piecewise Cubic Interpolation",
//! SIAM J. Numer. Anal. 17(2)). Unlike a natural cubic spline it never overshoots, so a curve through
//! increasing points can't invert tones.

pub const LUT_SIZE: usize = 4096;

pub fn monotone_lut(points: &[[f64; 2]], size: usize) -> Vec<f32> {
    let mut pts: Vec<[f64; 2]> = points.iter().map(|p| [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)]).collect();
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-9);
    if pts.len() < 2 {
        return (0..size).map(|i| (i as f64 / (size - 1) as f64) as f32).collect();
    }
    let n = pts.len();
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
    (0..size)
        .map(|i| {
            let x = i as f64 / (size - 1) as f64;
            let y = if x <= pts[0][0] {
                pts[0][1]
            } else if x >= pts[n - 1][0] {
                pts[n - 1][1]
            } else {
                let k = pts.windows(2).position(|w| x < w[1][0]).unwrap();
                let h = pts[k + 1][0] - pts[k][0];
                let t = (x - pts[k][0]) / h;
                let (t2, t3) = (t * t, t * t * t);
                (2.0 * t3 - 3.0 * t2 + 1.0) * pts[k][1]
                    + (t3 - 2.0 * t2 + t) * h * m[k]
                    + (-2.0 * t3 + 3.0 * t2) * pts[k + 1][1]
                    + (t3 - t2) * h * m[k + 1]
            };
            y.clamp(0.0, 1.0) as f32
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
}
