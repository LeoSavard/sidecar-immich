// SPDX-License-Identifier: Apache-2.0
//! Merging bracketed raws into one linear radiance image (docs/research/multi-frame-and-denoise.md, "HDR merge").
//!
//! Raw makes this simpler than most accounts of HDR: a sensor's response is linear, so there is no camera
//! response curve to estimate (Debevec–Malik is for already-rendered pictures). Each frame is a measurement of
//! the same radiance, scaled by its exposure and corrupted by Poisson–Gaussian noise; the best estimate of the
//! radiance at a pixel is the inverse-variance weighted mean of the frames that measured it there (Granados,
//! Ajdin, Wand, Theobalt, Seidel and Lensch, "Optimal HDR reconstruction with linear digital cameras", CVPR
//! 2010). A sample at or near saturation measured nothing and weighs zero.
//!
//! Ghosts — something that moved between frames — are samples that disagree with the estimate by more than
//! their noise allows; a second pass drops them (after Granados et al. 2013, "Automatic noise modeling for
//! ghost-free HDR reconstruction"), so a moving thing is taken from fewer frames rather than smeared.
//!
//! This module is the arithmetic, on frames already demosaiced to camera-native linear RGB; it streams — one
//! frame at a time into running sums — so a merge of nine 45 MP frames never holds nine frames.

use crate::align::{H, apply};
use rayon::prelude::*;

/// One frame of a bracket, demosaiced: camera-native linear RGB, the sensor's white at 1.
pub struct Frame<'a> {
    pub w: usize,
    pub h: usize,
    /// Three floats a pixel.
    pub rgb: &'a [f32],
    /// Whether any of the pixel's samples reached saturation (the demosaic's clip coverage).
    pub clipped: &'a [bool],
    /// What multiplies this frame's values into the reference frame's radiance.
    pub scale: f64,
    /// Per-channel noise in this frame's own units: σ² = gain·v + read.
    pub gain: f64,
    pub read: f64,
    /// Reference pixel → this frame's pixel. The identity for a tripod bracket.
    pub from_ref: H,
}

/// A sample at or above this share of white is treated as saturated even where the clip flag missed it: the
/// top of the range is where a sensor stops being linear.
pub const NEAR_WHITE: f32 = 0.95;
/// How many standard deviations a sample may stray from the estimate before the second pass drops it.
pub const GHOST_SIGMA: f64 = 4.0;

/// Running sums for the estimate, per reference pixel and channel: Σ w·x and Σ w, plus the darkest frame's
/// scaled value — the one with the largest scale to reference radiance, which saturates last — which is what a
/// pixel no frame measured falls back to.
pub struct Accum {
    pub w: usize,
    pub h: usize,
    sum_wx: Vec<f64>,
    sum_w: Vec<f64>,
    fallback: Vec<f32>,
    fallback_scale: f64,
}

impl Accum {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            sum_wx: vec![0.0; w * h * 3],
            sum_w: vec![0.0; w * h * 3],
            fallback: vec![0.0; w * h * 3],
            fallback_scale: 0.0,
        }
    }

    /// Add `f`. With `estimate` (a first pass's result), samples further than `GHOST_SIGMA` from it are dropped.
    pub fn add(&mut self, f: &Frame, estimate: Option<&[f32]>) {
        let w = self.w;
        let darkest = f.scale > self.fallback_scale;
        if darkest {
            self.fallback_scale = f.scale;
        }
        self.sum_wx
            .par_chunks_mut(w * 3)
            .zip(self.sum_w.par_chunks_mut(w * 3))
            .zip(self.fallback.par_chunks_mut(w * 3))
            .enumerate()
            .for_each(|(y, ((swx, sw), fb))| {
                for x in 0..w {
                    let q = apply(&f.from_ref, [x as f64 + 0.5, y as f64 + 0.5]);
                    let Some((v, clipped)) = sample(f, q[0] - 0.5, q[1] - 0.5) else {
                        continue;
                    };
                    if darkest {
                        for c in 0..3 {
                            fb[x * 3 + c] = (v[c] as f64 * f.scale) as f32;
                        }
                    }
                    if clipped || v.iter().any(|&c| c >= NEAR_WHITE) {
                        continue;
                    }
                    for c in 0..3 {
                        let vc = v[c].max(0.0) as f64;
                        let xc = vc * f.scale;
                        let var = f.scale * f.scale * (f.gain * vc + f.read).max(1e-12);
                        if let Some(est) = estimate {
                            let e = est[(y * w + x) * 3 + c] as f64;
                            // The estimate's own noise is small beside a single frame's; the sample's decides.
                            let allowed = GHOST_SIGMA
                                * (f.scale * f.scale * (f.gain * e.max(0.0) / f.scale + f.read))
                                    .max(1e-12)
                                    .sqrt();
                            if (xc - e).abs() > allowed {
                                continue;
                            }
                        }
                        swx[x * 3 + c] += xc / var;
                        sw[x * 3 + c] += 1.0 / var;
                    }
                }
            });
    }

    /// The estimate: the weighted mean where any frame measured the pixel, the darkest frame's scaled value
    /// (saturated there too) where none did. Also how many pixels fell back.
    pub fn finish(&self) -> (Vec<f32>, usize) {
        let mut fell_back = 0;
        let out = (0..self.sum_w.len())
            .map(|i| {
                if self.sum_w[i] > 0.0 {
                    (self.sum_wx[i] / self.sum_w[i]) as f32
                } else {
                    fell_back += 1;
                    self.fallback[i]
                }
            })
            .collect();
        (out, fell_back / 3)
    }
}

/// Bilinear sample of `f` at `(x, y)` (pixel-centre coordinates), and whether any of the four taps was clipped;
/// `None` outside the frame.
fn sample(f: &Frame, x: f64, y: f64) -> Option<([f32; 3], bool)> {
    if x < 0.0 || y < 0.0 || x > (f.w - 1) as f64 || y > (f.h - 1) as f64 {
        return None;
    }
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(f.w - 1), (y0 + 1).min(f.h - 1));
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let px = |xx: usize, yy: usize| {
        let i = yy * f.w + xx;
        (
            [f.rgb[i * 3], f.rgb[i * 3 + 1], f.rgb[i * 3 + 2]],
            f.clipped[i],
        )
    };
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    let v: [f32; 3] = std::array::from_fn(|k| {
        (a.0[k] * (1.0 - fx) + b.0[k] * fx) * (1.0 - fy) + (c.0[k] * (1.0 - fx) + d.0[k] * fx) * fy
    });
    Some((v, a.1 || b.1 || c.1 || d.1))
}

/// How many times more light frame `a` recorded than frame `b`, measured: the median of `a / b` over pixels
/// both exposed well (3 % to 85 % of white, unclipped), on a sparse grid of reference positions — each frame
/// sampled through its own `from_ref`. EXIF says what the camera was asked for; this is what it delivered —
/// shutter speeds are nominal, and a bracket's real steps can be off by a tenth of a stop. Meant for
/// neighbouring frames of a bracket: far apart, too few pixels are well exposed in both. `w × h` is the
/// reference frame's size.
pub fn measured_ratio(a: &Frame, b: &Frame, w: usize, h: usize) -> Option<f64> {
    let mut r = Vec::new();
    let step = (w.max(h) / 400).max(1);
    let ok = |v: [f32; 3], clipped: bool| !clipped && (0.03..0.85).contains(&v[1]);
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let p = [x as f64 + 0.5, y as f64 + 0.5];
            let (qa, qb) = (apply(&a.from_ref, p), apply(&b.from_ref, p));
            if let (Some((va, ca)), Some((vb, cb))) = (
                sample(a, qa[0] - 0.5, qa[1] - 0.5),
                sample(b, qb[0] - 0.5, qb[1] - 0.5),
            ) && ok(va, ca)
                && ok(vb, cb)
            {
                r.push(va[1] as f64 / vb[1] as f64);
            }
        }
    }
    if r.len() < 200 {
        return None;
    }
    let mid = r.len() / 2;
    let (_, m, _) = r.select_nth_unstable_by(mid, f64::total_cmp);
    Some(*m)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: H = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    struct Rng(u64);
    impl Rng {
        fn u(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % 1_000_000) as f64 / 1_000_000.0 + 5e-7
        }
        fn n(&mut self) -> f64 {
            (-2.0 * self.u().ln()).sqrt() * (std::f64::consts::TAU * self.u()).cos()
        }
    }

    /// A scene spanning 11.5 stops, three exposures two stops apart, each with Poisson–Gaussian noise (the normal
    /// approximation is enough here) and clipping at white. The merge recovers radiance the bright frame clipped
    /// from the dark one, beats the best single frame's noise in the shadows, and stays unbiased.
    #[test]
    fn a_bracket_merges_to_less_noise_and_no_clipping() {
        let (w, h) = (512usize, 64usize);
        // Radiance in the middle frame's units: 2^-10 at the left to 2^1.5 at the right — within what the darkest
        // frame (two stops down) still measures.
        let radiance = |x: usize| 2f64.powf(-10.0 + 11.5 * x as f64 / (w - 1) as f64);
        let (gain, read) = (2e-4, 4e-7);
        let mut rng = Rng(42);
        let evs = [-2.0, 0.0, 2.0];
        let frames: Vec<(Vec<f32>, Vec<bool>, f64)> = evs
            .iter()
            .map(|&ev| {
                let k = 2f64.powf(ev); // what this frame records of the radiance
                let mut rgb = vec![0f32; w * h * 3];
                let mut clipped = vec![false; w * h];
                for y in 0..h {
                    for x in 0..w {
                        let v = radiance(x) * k;
                        for c in 0..3 {
                            let noisy = v + (gain * v + read).sqrt() * rng.n();
                            rgb[(y * w + x) * 3 + c] = noisy.min(1.0) as f32;
                        }
                        clipped[y * w + x] = v >= 1.0;
                    }
                }
                (rgb, clipped, 1.0 / k)
            })
            .collect();
        fn frame<'a>(
            t: &'a (Vec<f32>, Vec<bool>, f64),
            w: usize,
            h: usize,
            gain: f64,
            read: f64,
        ) -> Frame<'a> {
            Frame {
                w,
                h,
                rgb: &t.0,
                clipped: &t.1,
                scale: t.2,
                gain,
                read,
                from_ref: ID,
            }
        }
        let mut acc = Accum::new(w, h);
        for f in &frames {
            acc.add(&frame(f, w, h, gain, read), None);
        }
        let (first, fell) = acc.finish();
        let mut acc = Accum::new(w, h);
        for f in &frames {
            acc.add(&frame(f, w, h, gain, read), Some(&first));
        }
        let (merged, _) = acc.finish();
        assert_eq!(fell, 0, "every pixel was measured by some frame");
        // Columns: mean and spread of green over the rows, against the truth.
        let col = |img: &[f32], x: usize| {
            let v: Vec<f64> = (0..h).map(|y| img[(y * w + x) * 3 + 1] as f64).collect();
            let m = v.iter().sum::<f64>() / h as f64;
            (
                m,
                (v.iter().map(|a| (a - m).powi(2)).sum::<f64>() / h as f64).sqrt(),
            )
        };
        // Highlights: 1.2 stops over the middle frame's white, clipped in two frames, measured by the darkest.
        let x_hi = ((w - 1) as f64 * 11.2 / 11.5) as usize;
        let (m, _) = col(&merged, x_hi);
        assert!(
            (m / radiance(x_hi) - 1.0).abs() < 0.03,
            "highlight {m} against {}",
            radiance(x_hi)
        );
        // Shadows: 8 stops under, where the bright frame is the cleanest single measurement.
        let x_lo = ((w - 1) as f64 * 2.0 / 11.5) as usize;
        let (m, s) = col(&merged, x_lo);
        let bright = col(&frames[2].0, x_lo);
        assert!(
            (m / radiance(x_lo) - 1.0).abs() < 0.05,
            "shadow mean {m} against {}",
            radiance(x_lo)
        );
        assert!(
            s < bright.1 / 4.0 * 1.15,
            "merged noise {s} against the bright frame's {} (scaled ×¼)",
            bright.1
        );
    }

    /// Something that moved in one frame is taken from the others.
    #[test]
    fn a_ghost_is_dropped() {
        let (w, h) = (64usize, 64usize);
        let (gain, read) = (1e-4, 1e-7);
        let mk = |k: f64, ghost: bool| {
            let mut rgb = vec![(0.2 * k) as f32; w * h * 3];
            if ghost {
                for y in 20..40 {
                    for x in 20..40 {
                        for c in 0..3 {
                            rgb[(y * w + x) * 3 + c] = (0.6 * k) as f32;
                        }
                    }
                }
            }
            rgb
        };
        let frames = [
            (mk(0.5, false), 2.0),
            (mk(1.0, true), 1.0),
            (mk(2.0, false), 0.5),
        ];
        let clipped = vec![false; w * h];
        fn f<'a>(
            t: &'a (Vec<f32>, f64),
            clipped: &'a [bool],
            n: usize,
            gain: f64,
            read: f64,
        ) -> Frame<'a> {
            Frame {
                w: n,
                h: n,
                rgb: &t.0,
                clipped,
                scale: t.1,
                gain,
                read,
                from_ref: ID,
            }
        }
        let mut a = Accum::new(w, h);
        frames
            .iter()
            .for_each(|x| a.add(&f(x, &clipped, w, gain, read), None));
        let (first, _) = a.finish();
        let mut b = Accum::new(w, h);
        frames
            .iter()
            .for_each(|x| b.add(&f(x, &clipped, w, gain, read), Some(&first)));
        let (merged, _) = b.finish();
        let at = |img: &[f32], x: usize, y: usize| img[(y * w + x) * 3 + 1];
        assert!(
            at(&first, 30, 30) > 0.22,
            "the first pass is pulled by the ghost: {}",
            at(&first, 30, 30)
        );
        assert!(
            (at(&merged, 30, 30) - 0.2).abs() < 0.01,
            "the second drops it: {}",
            at(&merged, 30, 30)
        );
        assert!((at(&merged, 5, 5) - 0.2).abs() < 1e-3);
    }
}
