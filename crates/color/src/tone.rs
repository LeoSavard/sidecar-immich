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
        if normalized(g, k, mid) > GREY_ENCODED {
            lo = mid
        } else {
            hi = mid
        }
    }
    (lo + hi) / 2.0
}

/// The display's headroom above middle grey, in stops: sRGB's white is 1.0 and grey is 0.18, so
/// `log2(1 / 0.18)` — 2.47 stops. Whatever the curve does, everything between grey and `white_ev` has to fit
/// in this, so a shape can only choose *where* the compression is spent (ROADMAP, "Tone curve above middle
/// grey").
pub const HEADROOM_STOPS: f64 = 2.473_931_188_332_41;

/// The candidate curve above middle grey: a straight segment in log-log — the identity in linear light, so a
/// stop of scene light is a stop of picture — then a shoulder that reaches white exactly at `white_ev`.
///
/// The shoulder is `v = k + (H − k)·(1 − (1 − t)ⁿ)`, `t = (u − k)/(W − k)`: it lands on white with no slope
/// left over, and its slope at the knee is `n(H − k)/(W − k)`. Asking that to be 1, so the straight segment
/// joins it without a kink, fixes the knee for a given order: `k = (nH − W)/(n − 1)`. The slope through the
/// shoulder is then exactly `(1 − t)ⁿ⁻¹`, so **`order` is the whole choice** — how far the straight segment
/// runs before the compression starts, against how hard the top is squeezed, out of a headroom that is fixed:
///
/// - `2` puts the knee just above grey and spreads the cost evenly over everything above it.
/// - `3` holds a full stop of scene light for a stop of picture out to +1.5 EV, and pays above +2 EV.
/// - higher still protects the midtones further and flattens the last stops almost completely.
///
/// A `white_ev` at or under `H` needs no shoulder at all; one so high that the knee would fall below grey
/// holds the knee at grey, and the segment gives up what it must.
pub fn straight_stops(u: f64, white_ev: f64, order: f64) -> f64 {
    let w = white_ev.max(1e-6);
    if w <= HEADROOM_STOPS {
        return u;
    }
    let n = order.max(1.000_001);
    let k = ((n * HEADROOM_STOPS - w) / (n - 1.0)).clamp(0.0, HEADROOM_STOPS);
    if u <= k {
        return u;
    }
    if u >= w {
        return HEADROOM_STOPS;
    }
    let t = (u - k) / (w - k);
    k + (HEADROOM_STOPS - k) * (1.0 - (1.0 - t).powf(n))
}

/// Where the straight segment gives way, in stops above middle grey (`straight_stops`).
pub fn straight_knee(white_ev: f64, order: f64) -> f64 {
    let (w, n) = (white_ev.max(1e-6), order.max(1.000_001));
    if w <= HEADROOM_STOPS {
        w
    } else {
        ((n * HEADROOM_STOPS - w) / (n - 1.0)).clamp(0.0, HEADROOM_STOPS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the two shapes do to a stop of scene light above grey: the log-log slope, which is the share of
    /// its contrast a detail keeps. 1 is "a stop in, a stop out".
    #[test]
    fn the_straight_segment_keeps_the_upper_midtones() {
        let (black_ev, white_ev) = (-8.0, 4.5);
        let k = 7.0;
        let p = pivot_for(black_ev, white_ev, k);
        // The logistic, in log-log: encoded → linear, against stops of scene light.
        let logistic_y = |u: f64| {
            let x = ((u - black_ev) / (white_ev - black_ev)).clamp(0.0, 1.0);
            srgb_decode(normalized(x, k, p))
        };
        // The candidate as it would actually be used: the logistic below grey, the straight segment above it.
        let straight_y = |u: f64, n: f64| {
            if u < 0.0 {
                logistic_y(u)
            } else {
                0.18 * 2f64.powf(straight_stops(u, white_ev, n))
            }
        };
        let slope =
            |f: &dyn Fn(f64) -> f64, u: f64| ((f(u + 0.05) / f(u - 0.05)).log2() / 0.1).abs();
        let orders = [2.0, 3.0, 4.0];
        println!("white_ev {white_ev}, headroom {HEADROOM_STOPS:.2} stops above grey");
        print!("  knee (stops above grey): ");
        for n in orders {
            print!("order {n:.0} at {:.2}   ", straight_knee(white_ev, n));
        }
        println!("\n\n  log-log slope — the share of its contrast a detail keeps");
        println!(
            "  {:>6}  {:>9}  {:>8}  {:>8}  {:>8}",
            "EV", "logistic", "order 2", "order 3", "order 4"
        );
        for u in [-2.0, -1.0, -0.5, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 4.0] {
            print!("  {u:>6.1}  {:>9.3}", slope(&logistic_y, u));
            for n in orders {
                print!("  {:>8.3}", slope(&|x| straight_y(x, n), u));
            }
            println!();
        }
        // The roadmap's measurements of the curve as it stands, which is what this is to be judged against.
        assert!(
            (slope(&logistic_y, 1.0) - 0.80).abs() < 0.08,
            "logistic at +1 EV: {}",
            slope(&logistic_y, 1.0)
        );
        assert!(
            (slope(&logistic_y, 2.0) - 0.57).abs() < 0.08,
            "logistic at +2 EV: {}",
            slope(&logistic_y, 2.0)
        );
        // Order 3 is the one the roadmap describes: a stop for a stop through the upper midtones, paid for
        // above +2 EV. Order 2's knee sits so low that it is already shouldering at +1.
        assert!(
            slope(&|x| straight_y(x, 3.0), 1.0) > 0.95,
            "order 3 at +1 EV"
        );
        assert!(
            slope(&|x| straight_y(x, 3.0), 3.0) < slope(&logistic_y, 3.0),
            "the shoulder is where it is paid for"
        );
        assert!(
            slope(&|x| straight_y(x, 2.0), 1.0) < 0.9,
            "order 2 is already into its shoulder at +1 EV"
        );
        // Shadows are the logistic's, untouched: the candidate only replaces the curve above grey.
        for n in orders {
            // Grey is still grey, and white is still white.
            assert!(straight_stops(0.0, white_ev, n).abs() < 1e-12);
            assert!((straight_stops(white_ev, white_ev, n) - HEADROOM_STOPS).abs() < 1e-12);
            // No kink at the knee: the straight segment meets the shoulder with the same slope.
            let k = straight_knee(white_ev, n);
            let d = |u: f64| {
                (straight_stops(u + 1e-6, white_ev, n) - straight_stops(u - 1e-6, white_ev, n))
                    / 2e-6
            };
            assert!(
                (d(k - 1e-5) - d(k + 1e-5)).abs() < 1e-3,
                "order {n}: {} vs {}",
                d(k - 1e-5),
                d(k + 1e-5)
            );
            // Monotone, so no tone ever goes backwards.
            let mut last = f64::MIN;
            for i in 0..=200 {
                let v = straight_stops(white_ev * i as f64 / 200.0, white_ev, n);
                assert!(v >= last - 1e-12, "order {n} is not monotone at {i}");
                last = v;
            }
        }
    }

    fn srgb_decode(e: f64) -> f64 {
        if e <= 0.04045 {
            e / 12.92
        } else {
            ((e + 0.055) / 1.055).powf(2.4)
        }
    }

    #[test]
    fn grey_is_preserved() {
        for &k in &[3.0, 6.0, 10.0] {
            let p = pivot_for(-10.0, 6.5, k);
            let g = 10.0 / 16.5;
            assert!((normalized(g, k, p) - GREY_ENCODED).abs() < 1e-9);
        }
    }
}
