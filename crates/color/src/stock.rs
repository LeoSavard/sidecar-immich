// SPDX-License-Identifier: Apache-2.0
//! Named black-and-white stocks' characteristic curves, from their makers' datasheets (DESIGN.md §7, "Stocks").
//!
//! A datasheet's curve is density over relative log exposure: fog, a toe, a straight line and, for some stocks,
//! a shoulder. Sidecar keeps its shape and not its numbers: the straight line's gradient is the print's to set
//! (Contrast does that), so what a stock brings is where its toe flattens the shadows into fog, how gradually,
//! and its shoulder. The curve is fitted as `fog + G·w·ln(1 + exp((log E − e₀)/w))` (a toe that becomes the
//! straight line), then expressed in stops from middle grey, which the zone system places 1.2 log E (four
//! stops) above the speed point at fog + 0.1.

/// A stock's toe in stops from middle grey: where it is and how wide it bends. `DevelopParams.stock`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Toe {
    pub at: f64,
    pub width: f64,
}

/// One stop in log₁₀ exposure.
const STOP: f64 = std::f64::consts::LOG10_2;

/// Middle grey above the speed point (fog + 0.1), in log exposure: Zone V four stops over Zone I.
const GREY_ABOVE_SPEED_POINT: f64 = 1.2;

/// A curve fitted to a datasheet: fog, gradient, the toe's log exposure and width.
struct Fit {
    /// Only the fit's test reads it: the toe's shape doesn't depend on it.
    #[cfg_attr(not(test), allow(dead_code))]
    fog: f64,
    gradient: f64,
    e0: f64,
    w: f64,
}

impl Fit {
    #[cfg(test)]
    fn density(&self, log_e: f64) -> f64 {
        self.fog + self.gradient * self.w * ((log_e - self.e0) / self.w).exp().ln_1p()
    }

    /// The log exposure where density is fog + 0.1.
    fn speed_point(&self) -> f64 {
        // fog + G·w·ln(1 + eᵘ) = fog + 0.1 ⇒ u = ln(exp(0.1 / (G·w)) − 1).
        self.e0 + self.w * ((0.1 / (self.gradient * self.w)).exp() - 1.0).ln()
    }

    fn toe(&self) -> Toe {
        let grey = self.speed_point() + GREY_ABOVE_SPEED_POINT;
        Toe { at: (self.e0 - grey) / STOP, width: self.w / STOP }
    }
}

/// ILFORD HP5 Plus, developed in ILFOTEC HC 1+31 for 6½ minutes at 20 °C (HARMAN technology's technical
/// information sheet, Nov 2018, page 5), traced from the sheet's graph: (relative log exposure, density).
pub const HP5_PLUS_CURVE: [(f64, f64); 40] = [
    (0.1, 0.179), (0.2, 0.179), (0.3, 0.179), (0.4, 0.179), (0.5, 0.179), (0.6, 0.188), (0.7, 0.199), (0.8, 0.210),
    (0.9, 0.230), (1.0, 0.253), (1.1, 0.285), (1.2, 0.319), (1.3, 0.361), (1.4, 0.410), (1.5, 0.464), (1.6, 0.518),
    (1.7, 0.588), (1.8, 0.656), (1.9, 0.724), (2.0, 0.791), (2.1, 0.858), (2.2, 0.924), (2.3, 0.987), (2.4, 1.050),
    (2.5, 1.116), (2.6, 1.182), (2.7, 1.246), (2.8, 1.308), (2.9, 1.374), (3.0, 1.437), (3.1, 1.499), (3.2, 1.564),
    (3.3, 1.627), (3.4, 1.690), (3.5, 1.754), (3.6, 1.818), (3.7, 1.880), (3.8, 1.941), (3.9, 2.002), (4.0, 2.067),
];

/// HP5 Plus's curve, fitted to `HP5_PLUS_CURVE` by least squares (RMS 0.0065 density); no shoulder within the
/// sheet's 4 log E.
const HP5_PLUS: Fit = Fit { fog: 0.182, gradient: 0.645, e0: 1.06, w: 0.18 };

/// The toe of a named stock, by film id; `None` for the film classes, which have no one curve.
pub fn toe(film: &str) -> Option<Toe> {
    match film {
        "hp5-plus" => Some(HP5_PLUS.toe()),
        _ => None,
    }
}

/// Where a toe's shadows end up, in stops from middle grey: the fog the deepest shadows flatten into.
pub fn floor(toe: &Toe) -> f64 {
    -toe.width * (-toe.at / toe.width).exp().ln_1p()
}

/// The stops from middle grey a print gives exposure `x` (stops from grey) through `toe`: the straight line
/// through grey, bent into the fog below the toe. `develop/stock.wgsl` is the same.
pub fn print_stops(toe: &Toe, x: f64) -> f64 {
    let sp = |u: f64| u.exp().ln_1p();
    toe.width * (sp((x - toe.at) / toe.width) - sp(-toe.at / toe.width))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fit follows the traced curve within 0.02 density.
    #[test]
    fn hp5_fit_follows_the_datasheet() {
        let worst = HP5_PLUS_CURVE.iter().map(|&(e, d)| (HP5_PLUS.density(e) - d).abs()).fold(0.0, f64::max);
        assert!(worst < 0.02, "worst {worst}");
    }

    /// HP5's toe sits about four stops under grey: Zone I is at the speed point by construction, the shadows
    /// flatten below it, and grey and the straight line above it are kept.
    #[test]
    fn hp5_toe_sits_where_zone_one_is() {
        let t = toe("hp5-plus").unwrap();
        assert!((-4.5..-3.5).contains(&t.at), "toe at {}", t.at);
        assert!(print_stops(&t, 0.0).abs() < 1e-12);
        assert!((print_stops(&t, 3.0) - 3.0).abs() < 0.01, "straight above: {}", print_stops(&t, 3.0));
        // Zone I (four stops down) prints about 0.1 density over fog: well into the toe.
        let zone_one = print_stops(&t, -4.0);
        assert!(zone_one > -4.0 && zone_one - floor(&t) < 0.6, "zone I at {zone_one}, floor {}", floor(&t));
        assert!(toe("panchromatic").is_none());
    }
}
