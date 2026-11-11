// SPDX-License-Identifier: Apache-2.0
//! Camera profiles as the DNG specification defines them (Adobe, "Digital Negative Specification" 1.6, chapter 6,
//! "Mapping Camera Color Space to CIE XYZ Space", and the `ProfileHueSatMap` / `ProfileLookTable` tags) — from the
//! specification, which is public; no profile is shipped. A profile is read from the photo's own file (a DNG
//! carries the one it was made with, "Adobe Standard" from Adobe's converter) and rendered only when the edit
//! asks for it (`EditParams::profile`).
//!
//! What a profile adds to the colour matrices:
//!
//! - **Forward matrices**: white-balanced camera RGB → XYZ under D50, measured rather than inverted from
//!   `ColorMatrix`, interpolated by the white balance's temperature as the matrices are.
//! - **HueSatMap**: a table over hue, saturation and value in linear ProPhoto RGB of a hue shift (degrees), a
//!   saturation scale and a value scale — the camera's characterisation beyond what a matrix can say. Applied
//!   before exposure.
//! - **LookTable**: the same kind of table, applied after exposure: the profile's look.
//!
//! The profile's tone curve is the maker's look of contrast, and Sidecar's own tone curve stays in charge.

use serde::{Deserialize, Serialize};

use crate::mat::{self, M3};

/// One hue/saturation/value table: `dims` hue × saturation × value divisions, three floats an entry (hue shift
/// in degrees, saturation scale, value scale), value slowest, then hue, then saturation fastest (the DNG order).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HsvTable {
    pub dims: [u32; 3],
    pub data: Vec<f32>,
    /// Value divisions spaced in sRGB's encoding rather than linearly (`ProfileHueSatMapEncoding` 1).
    #[serde(default)]
    pub srgb: bool,
}

impl HsvTable {
    pub fn is_valid(&self) -> bool {
        let [h, s, v] = self.dims;
        h >= 1 && s >= 2 && v >= 1 && self.data.len() == (h * s * v * 3) as usize
    }
}

/// A DCP's data, as a photo's file carries it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    /// `ProfileName`.
    pub name: String,
    /// `ForwardMatrix1/2` (white-balanced camera → XYZ D50, rows), each with its calibration's temperature.
    #[serde(default)]
    pub forward: Vec<(f64, M3)>,
    /// `ProfileHueSatMapData1/2`, each with its calibration's temperature.
    #[serde(default)]
    pub hue_sat: Vec<(f64, HsvTable)>,
    #[serde(default)]
    pub look: Option<HsvTable>,
}

/// Interpolate between two calibrations by inverse temperature, as `CameraColor` does its matrices: the weight of
/// the lower-temperature one.
fn weight(lo: f64, hi: f64, t: f64) -> f64 {
    let inv = |t: f64| 1.0 / t;
    ((inv(t) - inv(hi)) / (inv(lo) - inv(hi))).clamp(0.0, 1.0)
}

fn pick<T: Clone>(items: &[(f64, T)], t: f64, mix: impl Fn(&T, &T, f64) -> T) -> Option<T> {
    match items {
        [] => None,
        [(_, only)] => Some(only.clone()),
        [a, b, ..] => {
            let (lo, hi) = if a.0 <= b.0 { (a, b) } else { (b, a) };
            Some(mix(&lo.1, &hi.1, weight(lo.0, hi.0, t)))
        }
    }
}

impl Profile {
    pub fn forward_at(&self, temperature_k: f64) -> Option<M3> {
        pick(&self.forward, temperature_k, mat::lerp)
    }

    pub fn hue_sat_at(&self, temperature_k: f64) -> Option<HsvTable> {
        pick(&self.hue_sat, temperature_k, |a, b, w| {
            if a.dims != b.dims {
                return a.clone();
            }
            HsvTable {
                dims: a.dims,
                data: a
                    .data
                    .iter()
                    .zip(&b.data)
                    .map(|(x, y)| (*x as f64 * w + *y as f64 * (1.0 - w)) as f32)
                    .collect(),
                srgb: a.srgb,
            }
        })
        .filter(HsvTable::is_valid)
    }
}

/// ProFoto RGB (linear) primaries and its D50 white: the space the DNG specification applies its tables in.
pub const PROPHOTO_PRIMARIES: [[f64; 2]; 3] =
    [[0.7347, 0.2653], [0.1596, 0.8404], [0.0366, 0.0001]];
pub const D50: [f64; 2] = [0.3457, 0.3585];

/// Linear Rec.2020 (D65) → linear ProPhoto RGB (D50), Bradford-adapted, so a neutral stays neutral.
pub fn rec2020_to_prophoto() -> M3 {
    mat::mul(
        &mat::inverse(&mat::rgb_to_xyz(PROPHOTO_PRIMARIES, D50)),
        &mat::mul(
            &mat::bradford(mat::D65, D50),
            &mat::inverse(&mat::XYZ_TO_REC2020),
        ),
    )
}

fn srgb_encode(v: f64) -> f64 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// One table applied to a linear ProPhoto colour: its hue (0…6), saturation and value looked up with
/// interpolation (hue wrapping round), then shifted and scaled. What `client/profile.wgsl` does, for tests.
pub fn apply(t: &HsvTable, rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb;
    let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
    if mx <= 0.0 {
        return rgb;
    }
    let delta = mx - mn;
    let s = delta / mx;
    let mut h = if delta <= 0.0 {
        0.0
    } else if r == mx {
        (g - b) / delta
    } else if g == mx {
        2.0 + (b - r) / delta
    } else {
        4.0 + (r - g) / delta
    };
    if h < 0.0 {
        h += 6.0;
    }
    let [hd, sd, vd] = t.dims.map(|d| d as usize);
    let entry = |hi: usize, si: usize, vi: usize| {
        let o = ((vi * hd + hi % hd) * sd + si) * 3;
        [t.data[o] as f64, t.data[o + 1] as f64, t.data[o + 2] as f64]
    };
    let hf = h * hd as f64 / 6.0;
    let sf = s * (sd - 1) as f64;
    let vv = if t.srgb {
        srgb_encode(mx.min(1.0))
    } else {
        mx.min(1.0)
    };
    let vf = vv * (vd.max(1) - 1) as f64;
    let (h0, s0, v0) = (
        hf.floor() as usize,
        (sf.floor() as usize).min(sd - 2),
        (vf.floor() as usize).min(vd.saturating_sub(2)),
    );
    let (th, ts, tv) = (
        hf - h0 as f64,
        sf - s0 as f64,
        if vd > 1 { vf - v0 as f64 } else { 0.0 },
    );
    let mut acc = [0.0; 3];
    for (dv, wv) in [(0usize, 1.0 - tv), (1, tv)] {
        if wv == 0.0 || (vd == 1 && dv == 1) {
            continue;
        }
        for (dh, wh) in [(0usize, 1.0 - th), (1, th)] {
            for (ds, ws) in [(0usize, 1.0 - ts), (1, ts)] {
                let e = entry(h0 + dh, s0 + ds, (v0 + dv).min(vd - 1));
                let w = wv * wh * ws;
                for k in 0..3 {
                    acc[k] += w * e[k];
                }
            }
        }
    }
    let [shift, sat_scale, val_scale] = acc;
    let mut h2 = h + shift * 6.0 / 360.0;
    h2 = h2.rem_euclid(6.0);
    let s2 = (s * sat_scale).clamp(0.0, 1.0);
    let v2 = mx * val_scale;
    // Back from HSV.
    let i = h2.floor();
    let f = h2 - i;
    let (p, q, u) = (
        v2 * (1.0 - s2),
        v2 * (1.0 - s2 * f),
        v2 * (1.0 - s2 * (1.0 - f)),
    );
    match i as i32 {
        0 => [v2, u, p],
        1 => [q, v2, p],
        2 => [p, v2, u],
        3 => [p, q, v2],
        4 => [u, p, v2],
        _ => [v2, p, q],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(dims: [u32; 3]) -> HsvTable {
        let n = (dims[0] * dims[1] * dims[2]) as usize;
        HsvTable {
            dims,
            data: (0..n).flat_map(|_| [0.0, 1.0, 1.0]).collect(),
            srgb: false,
        }
    }

    #[test]
    fn an_identity_table_changes_nothing_and_neutrals_stay_neutral() {
        let t = identity([90, 30, 1]);
        for c in [
            [0.5, 0.2, 0.1],
            [0.1, 0.4, 0.3],
            [0.2, 0.2, 0.2],
            [0.9, 0.05, 0.6],
        ] {
            let o = apply(&t, c);
            for k in 0..3 {
                assert!((o[k] - c[k]).abs() < 1e-9, "{c:?} → {o:?}");
            }
        }
        let n = mat::apply(&rec2020_to_prophoto(), &[1.0, 1.0, 1.0]);
        assert!(n.iter().all(|v| (v - 1.0).abs() < 1e-6), "{n:?}");
    }

    /// A table that turns every hue 30° and halves saturation does that, wherever the colour falls between
    /// its divisions.
    #[test]
    fn a_table_shifts_hue_and_scales_saturation() {
        let mut t = identity([6, 5, 1]);
        for e in t.data.chunks_mut(3) {
            e[0] = 30.0;
            e[1] = 0.5;
        }
        let c = [0.6, 0.3, 0.2];
        let o = apply(&t, c);
        let hsv = |c: [f64; 3]| {
            let (mx, mn) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
            let h = (c[1] - c[2]) / (mx - mn);
            (h * 60.0, (mx - mn) / mx, mx)
        };
        let (h0, s0, v0) = hsv(c);
        let (h1, s1, v1) = hsv(o);
        assert!((h1 - h0 - 30.0).abs() < 1e-6, "{h0} → {h1}");
        assert!((s1 - s0 * 0.5).abs() < 1e-9 && (v1 - v0).abs() < 1e-9);
    }
}
