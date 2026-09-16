// SPDX-License-Identifier: Apache-2.0
//! Base tone curve: a normalized logistic over log-encoded luminance. The contrast slider sets steepness; the
//! pivot is solved so that 18 % grey always lands on its sRGB-encoded value, i.e. contrast pivots on middle grey.

pub const GREY_ENCODED: f64 = 0.461_356_129_5;

fn logistic(x: f64, k: f64, p: f64) -> f64 {
    1.0 / (1.0 + (-k * (x - p)).exp())
}

pub fn normalized(x: f64, k: f64, p: f64) -> f64 {
    let (s0, s1) = (logistic(0.0, k, p), logistic(1.0, k, p));
    (logistic(x, k, p) - s0) / (s1 - s0)
}

/// Pivot such that `normalized(grey position, k, pivot) == GREY_ENCODED`.
pub fn pivot_for(black_ev: f64, white_ev: f64, k: f64) -> f64 {
    let g = -black_ev / (white_ev - black_ev);
    let (mut lo, mut hi) = (-2.0, 3.0);
    for _ in 0..100 {
        let mid = (lo + hi) / 2.0;
        if normalized(g, k, mid) > GREY_ENCODED { lo = mid } else { hi = mid }
    }
    (lo + hi) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_is_preserved() {
        for &k in &[3.0, 6.0, 10.0] {
            let p = pivot_for(-10.0, 6.5, k);
            let g = 10.0 / 16.5;
            assert!((normalized(g, k, p) - GREY_ENCODED).abs() < 1e-9);
        }
    }
}
