// SPDX-License-Identifier: Apache-2.0
//! Parametric auto-edit (DESIGN.md §7, "Auto"): the light's colour estimated from the picture's statistics, and
//! the exposure and the ends of the tone curve fitted to its luminance — the textbook methods, implemented from
//! the papers, mapped into Sidecar's own controls so the result is an ordinary edit to adjust.
//!
//! Illuminant estimators, all in camera RGB before white balance, on pixels that neither clipped nor sit in the
//! noise: grey-world (Buchsbaum 1980), white-patch (Land's retinex; the 99th percentile rather than the maximum),
//! shades-of-grey (Finlayson and Trezzi 2004: the Minkowski p-norm, grey-world at p = 1 and white-patch as p → ∞)
//! and grey-edge (van de Weijer, Gevers and Gijsenij 2007: the same norm over the image's derivatives, since the
//! average reflectance *difference* is more nearly achromatic than the average reflectance).

/// A way to estimate the light's colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Estimator {
    GreyWorld,
    WhitePatch,
    ShadesOfGrey(f64),
    GreyEdge(f64),
}

impl Estimator {
    pub fn name(self) -> String {
        match self {
            Self::GreyWorld => "grey-world".into(),
            Self::WhitePatch => "white-patch".into(),
            Self::ShadesOfGrey(p) => format!("shades-of-grey p={p}"),
            Self::GreyEdge(p) => format!("grey-edge p={p}"),
        }
    }
}

/// A camera-RGB picture as the proxy holds it: RGBA, alpha the share of the pixel that clipped on the sensor.
pub struct Picture<'a> {
    pub width: usize,
    pub height: usize,
    pub rgba: &'a [f32],
}

/// Pixels that say something about the light: none of it clipped, no channel at the top of the range, and
/// bright enough to be above the noise.
fn usable(p: &[f32]) -> bool {
    p[3] <= 0.0 && p[..3].iter().all(|&v| v < 0.95) && p[0] + p[1] + p[2] > 0.01
}

/// The light's colour in camera RGB, green 1, or `None` when the picture has too little to go on.
pub fn illuminant(pic: &Picture, est: Estimator) -> Option<[f64; 3]> {
    let px = |x: usize, y: usize| &pic.rgba[(y * pic.width + x) * 4..(y * pic.width + x) * 4 + 4];
    let mut sum = [0f64; 3];
    let mut n = 0usize;
    match est {
        Estimator::GreyWorld | Estimator::ShadesOfGrey(_) => {
            let p = if let Estimator::ShadesOfGrey(p) = est {
                p
            } else {
                1.0
            };
            for c in pic
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|c| usable(&c[..]))
            {
                for k in 0..3 {
                    sum[k] += (c[k].max(0.0) as f64).powf(p);
                }
                n += 1;
            }
            if n < 100 {
                return None;
            }
            sum = sum.map(|s| (s / n as f64).powf(1.0 / p));
        }
        Estimator::WhitePatch => {
            let mut ch: [Vec<f32>; 3] = Default::default();
            for c in pic
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|c| usable(&c[..]))
            {
                for k in 0..3 {
                    ch[k].push(c[k]);
                }
            }
            if ch[0].len() < 100 {
                return None;
            }
            for k in 0..3 {
                let i = (ch[k].len() as f64 * 0.99) as usize;
                let (_, v, _) = ch[k].select_nth_unstable_by(i, f32::total_cmp);
                sum[k] = *v as f64;
            }
        }
        Estimator::GreyEdge(p) => {
            // Central differences of a 3×3 box-smoothed picture (a light Gaussian's stand-in), where both
            // neighbours are usable.
            let (w, h) = (pic.width, pic.height);
            let smooth = |x: usize, y: usize, k: usize| -> f64 {
                let mut s = 0.0;
                for dy in 0..3 {
                    for dx in 0..3 {
                        s += px(x + dx - 1, y + dy - 1)[k] as f64;
                    }
                }
                s / 9.0
            };
            for y in 2..h.saturating_sub(2) {
                for x in 2..w.saturating_sub(2) {
                    if ![px(x - 1, y), px(x + 1, y), px(x, y - 1), px(x, y + 1)]
                        .iter()
                        .all(|c| usable(c))
                    {
                        continue;
                    }
                    for (k, s) in sum.iter_mut().enumerate() {
                        let gx = smooth(x + 1, y, k) - smooth(x - 1, y, k);
                        let gy = smooth(x, y + 1, k) - smooth(x, y - 1, k);
                        *s += (gx * gx + gy * gy).sqrt().powf(p);
                    }
                    n += 1;
                }
            }
            if n < 100 {
                return None;
            }
            sum = sum.map(|s| (s / n as f64).powf(1.0 / p));
        }
    }
    (sum[1] > 0.0 && sum.iter().all(|v| v.is_finite() && *v > 0.0))
        .then(|| [sum[0] / sum[1], 1.0, sum[2] / sum[1]])
}

/// Middle grey: what the tone curve renders as display grey, and where an average scene's log-average belongs.
pub const KEY: f64 = 0.18;

/// Scene luminance as develop sees it, before the tone curve: camera RGB through the edit's white balance,
/// camera matrix (to Rec.2020) and exposure, from the develop uniform. `None` for a pixel that clipped.
pub fn luminance(develop: &crate::params::DevelopParams, rgba: &[f32]) -> Vec<Option<f32>> {
    const Y: [f32; 3] = [0.2627, 0.6780, 0.0593];
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            if p[3] > 0.0 {
                return None;
            }
            let c = [0, 1, 2].map(|k| p[k] * develop.wb_mul[k]);
            // `cam_to_work` holds columns.
            let w: [f32; 3] =
                std::array::from_fn(|r| (0..3).map(|k| develop.cam_to_work[k][r] * c[k]).sum());
            Some((0..3).map(|k| Y[k] * w[k]).sum::<f32>().max(0.0) * develop.exposure)
        })
        .collect()
}

/// How many stops to add to bring the picture's log-average luminance (Reinhard et al. 2002's key) to middle
/// grey: the exposure an average scene wants. `weights` (say, faces counted more) is per pixel, or `None` for
/// uniform. Clipped pixels are left out, as is the noise floor.
pub fn key_ev(lum: &[Option<f32>], weights: Option<&[f32]>) -> Option<f64> {
    let (mut sum, mut total) = (0f64, 0f64);
    for (i, l) in lum.iter().enumerate() {
        let Some(l) = l else { continue };
        let w = weights.map_or(1.0, |w| w[i]) as f64;
        if w <= 0.0 {
            continue;
        }
        sum += w * (*l as f64).max(1e-4).ln();
        total += w;
    }
    (total > 100.0).then(|| (KEY / (sum / total).exp()).log2())
}

/// The angle between two lights' colours, in degrees: the field's usual measure of an estimate's error.
pub fn angular_error(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dot: f64 = (0..3).map(|k| a[k] * b[k]).sum();
    let n = |v: [f64; 3]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
    (dot / (n(a) * n(b))).clamp(-1.0, 1.0).acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scene of random greys and colours lit by a known light: every estimator finds a light near it, and the
    /// error measure is zero for the light itself.
    #[test]
    fn estimators_find_a_known_light() {
        let light = [0.55, 1.0, 0.7];
        let (w, h) = (96usize, 64usize);
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let mut rgba = Vec::new();
        // Patches 8 px square of reflectances around grey, so edges and means are both achromatic on average.
        let patches: Vec<[f32; 3]> = (0..(w / 8) * (h / 8))
            .map(|_| {
                let g = 0.1 + 0.6 * rnd();
                [g * (0.7 + 0.6 * rnd()), g, g * (0.7 + 0.6 * rnd())]
            })
            .collect();
        let mean_tint = {
            let s = patches.iter().fold([0f32; 3], |a, p| {
                [a[0] + p[0] / p[1], 0.0, a[2] + p[2] / p[1]]
            });
            [
                s[0] / patches.len() as f32,
                1.0,
                s[2] / patches.len() as f32,
            ]
        };
        for y in 0..h {
            for x in 0..w {
                let r = patches[(y / 8) * (w / 8) + x / 8];
                // Reflectances normalised so their average is grey, then lit.
                rgba.extend([
                    r[0] / mean_tint[0] * light[0] as f32,
                    r[1],
                    r[2] / mean_tint[2] * light[2] as f32,
                    0.0,
                ]);
            }
        }
        let pic = Picture {
            width: w,
            height: h,
            rgba: &rgba,
        };
        for est in [
            Estimator::GreyWorld,
            Estimator::ShadesOfGrey(6.0),
            Estimator::GreyEdge(1.0),
            Estimator::WhitePatch,
        ] {
            let e = illuminant(&pic, est).unwrap();
            let err = angular_error(e, light);
            assert!(err < 6.0, "{}: {e:?}, {err:.2}°", est.name());
        }
        assert!(angular_error(light, light) < 1e-6);
        assert!((angular_error([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]) - 90.0).abs() < 1e-9);
    }

    #[test]
    fn a_clipped_or_black_picture_says_nothing() {
        let rgba = [1.0f32, 1.0, 1.0, 1.0].repeat(500);
        assert!(
            illuminant(
                &Picture {
                    width: 50,
                    height: 10,
                    rgba: &rgba
                },
                Estimator::GreyWorld
            )
            .is_none()
        );
        let black = vec![0.0f32; 4 * 500];
        assert!(
            illuminant(
                &Picture {
                    width: 50,
                    height: 10,
                    rgba: &black
                },
                Estimator::GreyEdge(1.0)
            )
            .is_none()
        );
    }
}
