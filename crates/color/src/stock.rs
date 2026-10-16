// SPDX-License-Identifier: Apache-2.0
//! Named black-and-white stocks' characteristic curves, from their makers' datasheets (DESIGN.md §7, "Stocks").
//!
//! A datasheet's curve is density over log exposure: fog, a toe, a straight line and, for some stocks, a
//! shoulder. Each is fitted (by `crates/color/film-data/fit.py`) as
//! `D = fog + G·(w·sp((E − e₀)/w) − c·v·sp((E − e₁)/v))`, `sp(u) = ln(1 + eᵘ)`: a toe that becomes the straight
//! line, then a shoulder taking `c` of its gradient away. Middle grey sits 1.2 log E (Zone V four stops over
//! Zone I) above the speed point at fog + 0.1 — where Kodak's Tri-X 400 curve puts it, its speed point falling
//! on ISO 400's 0.8 / 400 lux·s. Sidecar keeps the curve's shape, not its gradient, which is the print's to set
//! (Contrast): the curve in stops from grey, scaled to slope 1 at grey.

/// A stock's curve in stops from middle grey: its toe, and its shoulder taking `shoulder` of the gradient away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Curve {
    pub toe_at: f64,
    pub toe_width: f64,
    pub shoulder_at: f64,
    pub shoulder_width: f64,
    pub shoulder: f64,
}

/// One stop in log₁₀ exposure.
const STOP: f64 = std::f64::consts::LOG10_2;

/// Middle grey above the speed point (fog + 0.1), in log exposure: Zone V four stops over Zone I.
const GREY_ABOVE_SPEED_POINT: f64 = 1.2;

fn sp(u: f64) -> f64 {
    if u > 0.0 { u + (-u).exp().ln_1p() } else { u.exp().ln_1p() }
}

fn sigmoid(u: f64) -> f64 {
    1.0 / (1.0 + (-u).exp())
}

/// A curve fitted to a datasheet.
struct Fit {
    fog: f64,
    gradient: f64,
    e0: f64,
    w: f64,
    e1: f64,
    v: f64,
    c: f64,
}

impl Fit {
    fn density(&self, e: f64) -> f64 {
        self.fog + self.gradient * (self.w * sp((e - self.e0) / self.w) - self.c * self.v * sp((e - self.e1) / self.v))
    }

    /// The log exposure where density is fog + 0.1.
    fn speed_point(&self) -> f64 {
        let (mut lo, mut hi) = (self.e0 - 5.0, self.e1 + 5.0);
        for _ in 0..100 {
            let mid = (lo + hi) / 2.0;
            if self.density(mid) < self.fog + 0.1 { lo = mid } else { hi = mid }
        }
        lo
    }

    fn curve(&self) -> Curve {
        let grey = self.speed_point() + GREY_ABOVE_SPEED_POINT;
        Curve { toe_at: (self.e0 - grey) / STOP, toe_width: self.w / STOP, shoulder_at: (self.e1 - grey) / STOP, shoulder_width: self.v / STOP, shoulder: self.c }
    }
}

/// ILFORD HP5 Plus, ILFOTEC HC 1+31, 6½ min (`film_data::HP5_PLUS_CURVE`; RMS 0.0025 density).
const HP5_PLUS: Fit = Fit { fog: 0.171, gradient: 0.750, e0: 1.169, w: 0.258, e1: 2.082, v: 0.212, c: 0.157 };
/// KODAK TRI-X 400, D-76, 8 min (`film_data::TRI_X_400_CURVE`; RMS 0.0034).
const TRI_X_400: Fit = Fit { fog: 0.278, gradient: 0.637, e0: -2.658, w: 0.256, e1: -0.192, v: 0.143, c: 0.295 };
/// KODAK TRI-X 320, D-76, 8 min (`film_data::TRI_X_320_CURVE`; RMS 0.0066): its long toe.
const TRI_X_320: Fit = Fit { fog: 0.009, gradient: 0.900, e0: -2.056, w: 0.531, e1: -0.254, v: 0.104, c: 0.204 };
/// ILFORD SFX 200, ID-11, 10 min (`film_data::SFX_200_CURVE`; RMS 0.0040): its shoulder.
const SFX_200: Fit = Fit { fog: 0.276, gradient: 0.735, e0: 1.001, w: 0.230, e1: 3.931, v: 0.452, c: 1.000 };

fn fit(film: &str) -> Option<&'static Fit> {
    Some(match film {
        "hp5-plus" => &HP5_PLUS,
        "tri-x-400" => &TRI_X_400,
        "tri-x-320" => &TRI_X_320,
        "sfx-200" => &SFX_200,
        _ => return None,
    })
}

/// The curve of a named stock, by film id; `None` for the film classes, which have no one curve.
pub fn curve(film: &str) -> Option<Curve> {
    fit(film).map(Fit::curve)
}

impl Curve {
    /// The curve's shape in stops: the toe's softplus less the shoulder's.
    fn f(&self, x: f64) -> f64 {
        self.toe_width * sp((x - self.toe_at) / self.toe_width) - self.shoulder * self.shoulder_width * sp((x - self.shoulder_at) / self.shoulder_width)
    }

    fn slope(&self, x: f64) -> f64 {
        sigmoid((x - self.toe_at) / self.toe_width) - self.shoulder * sigmoid((x - self.shoulder_at) / self.shoulder_width)
    }

    /// The stops from middle grey a print gives exposure `x` (stops from grey): through grey with slope 1, bent
    /// into fog below the toe and over above the shoulder. `develop/stock.wgsl` is the same.
    pub fn print_stops(&self, x: f64) -> f64 {
        (self.f(x) - self.f(0.0)) / self.slope(0.0)
    }

    /// Where the deepest shadows end up, in stops from grey: the fog they flatten into.
    pub fn floor(&self) -> f64 {
        -self.f(0.0) / self.slope(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::film_data;

    /// Each fit follows its traced curve within 0.02 density.
    #[test]
    fn fits_follow_their_datasheets() {
        let sets: [(&str, &[(f64, f64)]); 4] = [
            ("hp5-plus", &film_data::HP5_PLUS_CURVE),
            ("tri-x-400", &film_data::TRI_X_400_CURVE),
            ("tri-x-320", &film_data::TRI_X_320_CURVE),
            ("sfx-200", &film_data::SFX_200_CURVE),
        ];
        for (film, data) in sets {
            let f = fit(film).unwrap();
            let worst = data.iter().map(|&(e, d)| (f.density(e) - d).abs()).fold(0.0, f64::max);
            assert!(worst < 0.02, "{film}: worst {worst}");
        }
    }

    /// Kodak's Tri-X 400 curve is in lux·seconds: its speed point falls where ISO 400 puts it, 0.8 / 400 lux·s,
    /// which is what places middle grey.
    #[test]
    fn tri_x_speed_point_is_iso_400() {
        let speed = TRI_X_400.speed_point();
        assert!((speed - (0.8f64 / 400.0).log10()).abs() < 0.05, "speed point {speed}");
    }

    /// Every stock's print keeps grey and its slope there; HP5's toe sits about four stops under grey, Tri-X
    /// 320's long toe nearer, and SFX 200's shoulder bends the highlights over.
    #[test]
    fn curves_keep_grey_and_bend_at_their_ends() {
        for film in ["hp5-plus", "tri-x-400", "tri-x-320", "sfx-200"] {
            let c = curve(film).unwrap();
            assert!(c.print_stops(0.0).abs() < 1e-12);
            let slope = (c.print_stops(0.01) - c.print_stops(-0.01)) / 0.02;
            assert!((slope - 1.0).abs() < 1e-3, "{film}: slope {slope}");
            assert!(c.floor() < -2.5, "{film}: floor {}", c.floor());
        }
        let hp5 = curve("hp5-plus").unwrap();
        assert!((-4.5..-3.0).contains(&hp5.toe_at), "HP5 toe at {}", hp5.toe_at);
        assert!(curve("tri-x-320").unwrap().toe_at > hp5.toe_at + 1.0, "Tri-X 320's toe is longer");
        let sfx = curve("sfx-200").unwrap();
        let top = (sfx.print_stops(6.01) - sfx.print_stops(5.99)) / 0.02;
        assert!(top < 0.6, "SFX 200's shoulder: slope {top} six stops over grey");
        assert!(curve("panchromatic").is_none());
    }
}
