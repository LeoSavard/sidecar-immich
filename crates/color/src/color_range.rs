// SPDX-License-Identifier: Apache-2.0
//! Colour ranges (DESIGN.md §15, "Colour ranges"): a mask layer that selects by colour — a hue sector with a
//! saturation band and a soft edge, optionally only within a model's mask (skin, for the skin preset) — so every
//! layer adjustment can then work on it. Capture One's colour editor and Skin Tone tool are the reference.
//!
//! Colour is judged as the edit sees it: camera RGB through the edit's own white balance and camera matrix, so a
//! range set on a face stays on that face when the light is corrected, and in OKLab, where hue is perceptual. The
//! saturation measure is OKLab chroma over lightness, which a change of exposure leaves alone. The worker and the
//! browser (through wasm) run this same code on the proxy, downsampled to `SIZE` first, so both select alike.

use serde::{Deserialize, Serialize};

use crate::mat::{self, M3};
use crate::params::DevelopParams;

/// The long edge the selection is computed at: a mask, sampled through a linear filter.
pub const SIZE: u32 = 1024;

/// The mask name a colour-range layer carries.
pub const COLOR: &str = "color";

/// Below this OKLab lightness (about 0.8 % linear: OKLab L is a cube root) a pixel's hue is mostly noise: it is
/// not selected.
const DARK: f32 = 0.2;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct ColorRange {
    /// The sector's centre, OKLab hue in degrees.
    pub hue: f64,
    /// The sector's full width, degrees.
    pub width: f64,
    /// Saturation (OKLab chroma over lightness, 0…100 for 0…0.5) from and to.
    pub saturation: [f64; 2],
    /// The soft edge beyond each limit: degrees of hue, and as many saturation points.
    pub feather: f64,
    /// Only within this model mask (`skin` for the skin preset); none for anywhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub within: Option<String>,
}

impl Default for ColorRange {
    /// Skin: the warm hues people's skin falls in across its range of tones, at moderate saturation.
    fn default() -> Self {
        Self {
            hue: 55.0,
            width: 40.0,
            saturation: [8.0, 60.0],
            feather: 10.0,
            within: None,
        }
    }
}

/// OKLab lightness, chroma and hue (degrees) of a linear sRGB colour (Ottosson's matrices).
pub fn oklch(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    let l = 0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let [l, m, s] = [l, m, s].map(f32::cbrt);
    let lab_l = 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s;
    let a = 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s;
    let bb = 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s;
    let h = bb.atan2(a).to_degrees().rem_euclid(360.0);
    [lab_l, (a * a + bb * bb).sqrt(), h]
}

/// Camera RGB → linear sRGB under the edit's white balance and matrix (`DevelopParams`), exposure left out.
fn to_srgb(develop: &DevelopParams) -> impl Fn(&[f32]) -> [f32; 3] {
    let cam: M3 =
        std::array::from_fn(|r| std::array::from_fn(|c| develop.cam_to_work[c][r] as f64));
    let m = mat::mul(&mat::REC2020_TO_SRGB, &cam);
    let wb = develop.wb_mul;
    move |p: &[f32]| {
        let c = [0, 1, 2].map(|k| (p[k] * wb[k]) as f64);
        mat::apply(&m, &c).map(|v| v.max(0.0) as f32)
    }
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How much one colour (OKLCh) is in the range, 0…1.
pub fn weight(range: &ColorRange, lch: [f32; 3]) -> f32 {
    let [l, c, h] = lch;
    if l < DARK {
        return 0.0;
    }
    let f = range.feather.max(0.5) as f32;
    let half = (range.width as f32 / 2.0).clamp(0.0, 180.0);
    let d = ((h - range.hue as f32 + 540.0).rem_euclid(360.0) - 180.0).abs();
    let wh = 1.0 - smooth(half, half + f, d);
    let sat = (c / l * 200.0).min(200.0);
    let [lo, hi] = [range.saturation[0] as f32, range.saturation[1] as f32];
    let ws = smooth(lo - f, lo, sat) * (1.0 - smooth(hi, hi + f, sat));
    wh * ws
}

/// The selection over a camera-RGB picture (RGBA, any size, as the proxy holds it), one byte a pixel at the
/// picture's own size. `within`, when given, is a model mask (one byte a pixel, any size) it is multiplied by,
/// read at the nearest of its pixels.
pub fn select(
    range: &ColorRange,
    develop: &DevelopParams,
    rgba: &[f32],
    width: u32,
    height: u32,
    within: Option<(&[u8], u32, u32)>,
) -> Vec<u8> {
    let srgb = to_srgb(develop);
    let mut out = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let i = ((y * width + x) * 4) as usize;
            let p = &rgba[i..i + 4];
            // Clipped pixels have lost their colour: not selected.
            let mut w = if p[3] > 0.5 {
                0.0
            } else {
                weight(range, oklch(srgb(p)))
            };
            if let Some((m, mw, mh)) = within {
                let mx = ((x as u64 * mw as u64) / width as u64).min(mw as u64 - 1);
                let my = ((y as u64 * mh as u64) / height as u64).min(mh as u64 - 1);
                w *= m[(my * mw as u64 + mx) as usize] as f32 / 255.0;
            }
            out.push((w * 255.0).round() as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> DevelopParams {
        let mut d: DevelopParams = bytemuck::Zeroable::zeroed();
        d.wb_mul = [1.0; 3];
        // Columns of the Rec.2020 → … identity: cam_to_work is camera → Rec.2020, so sRGB primaries need its
        // inverse for the test to speak sRGB.
        let to2020 = mat::inverse(&mat::REC2020_TO_SRGB);
        d.cam_to_work = [0, 1, 2].map(|j| {
            [
                to2020[0][j] as f32,
                to2020[1][j] as f32,
                to2020[2][j] as f32,
                0.0,
            ]
        });
        d
    }

    /// OKLab hues of plain colours land where Ottosson's tables put them.
    #[test]
    fn oklch_hues() {
        let h = |c: [f32; 3]| oklch(c)[2];
        assert!(
            (h([1.0, 0.0, 0.0]) - 29.2).abs() < 1.0,
            "red {}",
            h([1.0, 0.0, 0.0])
        );
        assert!((h([0.0, 1.0, 0.0]) - 142.5).abs() < 1.0);
        assert!((h([0.0, 0.0, 1.0]) - 264.1).abs() < 1.0);
        assert!(oklch([0.5, 0.5, 0.5])[1] < 1e-4, "grey has no chroma");
    }

    /// Skin-coloured patches are selected, a blue sky and a grey wall are not, a dark pixel and a clipped one
    /// neither — and the same skin twice as bright is selected as much (saturation ignores exposure).
    #[test]
    fn selects_by_hue_and_saturation() {
        let skin = [0.62f32, 0.38, 0.27];
        let px = [
            skin,
            skin.map(|v| v * 0.5),
            [0.2, 0.35, 0.6],
            [0.4, 0.4, 0.4],
            [0.003, 0.002, 0.0015],
            skin,
        ];
        let mut rgba: Vec<f32> = px.iter().flat_map(|c| [c[0], c[1], c[2], 0.0]).collect();
        rgba[5 * 4 + 3] = 1.0; // the last one clipped
        let range = ColorRange::default();
        let sel = select(&range, &identity(), &rgba, px.len() as u32, 1, None);
        println!("{sel:?} skin {:?}", oklch(skin));
        assert!(
            sel[0] > 200 && sel[1] > 200,
            "skin, at two exposures: {sel:?}"
        );
        assert_eq!(&sel[2..], &[0, 0, 0, 0], "sky, grey, black, clipped");
        // Within a mask: half of it.
        let half = select(&range, &identity(), &rgba[..4], 1, 1, Some((&[128], 1, 1)));
        assert!((half[0] as i32 - (sel[0] as i32) / 2).abs() <= 1);
    }

    /// A hue sector across 0°, and the soft edge.
    #[test]
    fn hue_wraps_and_feathers() {
        let r = ColorRange {
            hue: 350.0,
            width: 40.0,
            saturation: [0.0, 200.0],
            feather: 10.0,
            within: None,
        };
        assert_eq!(weight(&r, [0.5, 0.1, 5.0]), 1.0, "inside, across 0°");
        assert_eq!(weight(&r, [0.5, 0.1, 340.0]), 1.0);
        let edge = weight(&r, [0.5, 0.1, 15.0]);
        assert!(edge > 0.0 && edge < 1.0, "in the feather: {edge}");
        assert_eq!(weight(&r, [0.5, 0.1, 40.0]), 0.0, "beyond it");
    }
}
