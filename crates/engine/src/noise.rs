// SPDX-License-Identifier: Apache-2.0
//! The noise of one photo as a function of its level: σ²(y) = a·y + b, Poisson (shot noise, which grows with
//! the light) plus Gaussian (read noise, which doesn't). That is the physics of a raw sensor, and the reason a
//! single noise figure is wrong for most of a frame: at high ISO the shadows are read-noise dominated and the
//! highlights shot-noise dominated, and a transform that assumes one fixed balance between the two denoises one
//! end too hard and the other too little.
//!
//! Estimated from the photo itself, after Foi, Trimeche, Katkovnik and Egiazarian, "Practical
//! Poissonian-Gaussian noise modeling and fitting for single-image raw-data" (IEEE TIP 2008): the luminance's
//! local mean is the level, a high-pass with a known noise gain is the noise, flat pixels only (texture is not
//! noise), a robust spread per level bin, and a weighted line through σ² against the level. The generalized
//! Anscombe transform built from (a, b) makes the noise unit-variance everywhere (`detail/to_opponent.wgsl`),
//! and Mäkitalo and Foi's exact unbiased inverse ("Optimal inversion of the generalized Anscombe
//! transformation for Poisson-Gaussian noise", IEEE TIP 2013) brings the denoised values back without the
//! dark-tone bias a plain algebraic inverse leaves.

/// σ²(y) = gain·y + read, in the units of the luminance it was measured on (the mean of R, G and B of the
/// camera-native linear source, white ≈ 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoiseModel {
    pub gain: f32,
    pub read: f32,
}

impl NoiseModel {
    pub fn sigma(&self, level: f32) -> f32 {
        (self.gain * level.max(0.0) + self.read).max(0.0).sqrt()
    }
}

/// One level bin of the estimate: its mean level, its noise, and how many flat pixels it had.
#[derive(Clone, Copy, Debug)]
pub struct Bin {
    pub level: f32,
    pub sigma: f32,
    pub count: usize,
}

/// The high-pass `y − mean(4-neighbours)`: for white noise of variance σ² its variance is 1.25 σ².
const HIGHPASS_GAIN: f32 = 1.118_034;
/// Level bins: log-spaced from LOW to 1.
const BINS: usize = 24;
const LOW: f32 = 1.0 / 4096.0;
/// The share of each bin kept, flattest first.
const FLAT_SHARE: f32 = 0.4;
const MIN_PER_BIN: usize = 400;

/// The noise level function of a luminance image, `w × h`, row-major. `None` when there are not enough flat,
/// unclipped levels to fit a line — a frame that is all texture, or all one level.
pub fn estimate(lum: &[f32], w: usize, h: usize) -> Option<(NoiseModel, Vec<Bin>)> {
    if w < 16 || h < 16 {
        return None;
    }
    let at = |x: usize, y: usize| lum[y * w + x];
    // 5×5 box mean, by separable running sums.
    let r = 2usize;
    let mut tmp = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = (x0..=x1).map(|xx| at(xx, y)).sum::<f32>() / (x1 - x0 + 1) as f32;
        }
    }
    let mut mean = vec![0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            mean[y * w + x] =
                (y0..=y1).map(|yy| tmp[yy * w + x]).sum::<f32>() / (y1 - y0 + 1) as f32;
        }
    }
    // Per interior pixel: its level, its high-pass, and how much structure the smoothed image has there.
    let mut bins: Vec<Vec<(f32, f32, f32)>> = vec![Vec::new(); BINS];
    let span = (1.0f32 / LOW).ln();
    for y in 3..h - 3 {
        for x in 3..w - 3 {
            let level = mean[y * w + x];
            if !(LOW..0.9).contains(&level) {
                continue;
            }
            let hp = at(x, y) - 0.25 * (at(x - 1, y) + at(x + 1, y) + at(x, y - 1) + at(x, y + 1));
            let gx = mean[y * w + x + 2] - mean[y * w + x - 2];
            let gy = mean[(y + 2) * w + x] - mean[(y - 2) * w + x];
            let structure = (gx * gx + gy * gy).sqrt();
            let b = (((level / LOW).ln() / span) * BINS as f32) as usize;
            bins[b.min(BINS - 1)].push((level, hp, structure));
        }
    }
    let mut out = Vec::new();
    for mut bin in bins {
        if bin.len() < MIN_PER_BIN {
            continue;
        }
        bin.sort_by(|a, b| a.2.total_cmp(&b.2));
        bin.truncate(((bin.len() as f32 * FLAT_SHARE) as usize).max(MIN_PER_BIN / 2));
        let level = bin.iter().map(|p| p.0).sum::<f32>() / bin.len() as f32;
        let mut dev: Vec<f32> = bin.iter().map(|p| p.1.abs()).collect();
        let mid = dev.len() / 2;
        let (_, med, _) = dev.select_nth_unstable_by(mid, f32::total_cmp);
        out.push(Bin {
            level,
            sigma: 1.4826 * *med / HIGHPASS_GAIN,
            count: bin.len(),
        });
    }
    let model = fit(&out)?;
    Some((model, out))
}

/// Weighted least squares of σ² = a·level + b over the bins, each weighted by its count over its variance
/// squared (a variance estimate's own spread grows with it), with a and b held non-negative.
pub fn fit(bins: &[Bin]) -> Option<NoiseModel> {
    if bins.len() < 3 {
        return None;
    }
    let pts: Vec<(f64, f64, f64)> = bins
        .iter()
        .map(|b| {
            let v = (b.sigma as f64).powi(2);
            (b.level as f64, v, b.count as f64 / (v * v).max(1e-30))
        })
        .collect();
    let (sw, sx, sy, sxx, sxy) = pts.iter().fold((0.0, 0.0, 0.0, 0.0, 0.0), |a, &(x, y, w)| {
        (
            a.0 + w,
            a.1 + w * x,
            a.2 + w * y,
            a.3 + w * x * x,
            a.4 + w * x * y,
        )
    });
    let det = sw * sxx - sx * sx;
    if det.abs() < 1e-30 {
        return None;
    }
    let (mut a, mut b) = ((sw * sxy - sx * sy) / det, (sxx * sy - sx * sxy) / det);
    // A negative term means the data can't separate the two: refit with it at zero.
    if b < 0.0 {
        b = 0.0;
        a = sxy / sxx;
    }
    if a < 0.0 {
        a = 0.0;
        b = sy / sw;
    }
    (a.is_finite() && b.is_finite() && (a > 0.0 || b > 0.0)).then_some(NoiseModel {
        gain: a as f32,
        read: b as f32,
    })
}

/// The generalized Anscombe transform for `model`: unit-variance noise at every level.
pub fn forward(model: &NoiseModel, y: f32) -> f32 {
    let a = model.gain.max(1e-12);
    let s2 = model.read / (a * a);
    2.0 * (y / a + 0.375 + s2).max(0.0).sqrt()
}

/// Mäkitalo and Foi's closed-form approximation of the exact unbiased inverse, for `forward`'s output after
/// denoising. Below D = 1 the expansion's negative powers run away, and the algebraic inverse (which is
/// already near zero there) takes over; both are clamped at zero light.
pub fn inverse(model: &NoiseModel, d: f32) -> f32 {
    let a = model.gain.max(1e-12);
    let s2 = model.read / (a * a);
    let x = if d >= 1.0 {
        let r = 1.224_744_9; // √(3/2)
        0.25 * d * d + 0.25 * r / d - 1.375 / (d * d) + 0.625 * r / (d * d * d) - 0.125 - s2
    } else {
        0.25 * d * d - 0.375 - s2
    };
    a * x.max(0.0)
}

/// Poisson-Gaussian noise with `model`'s statistics, for tests and `sidecar-dev denoise-truth`: shot noise
/// drawn as a Poisson count of `y / gain` photo-electrons, read noise as a Gaussian of variance `read`. A
/// small deterministic generator, so a measurement can be repeated exactly.
pub struct Synth {
    state: u64,
}

impl Synth {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
        }
    }

    fn uniform(&mut self) -> f64 {
        // xorshift64*
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        ((self.state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn normal(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }

    fn poisson(&mut self, lambda: f64) -> f64 {
        if lambda < 30.0 {
            // Knuth's product method: exact, and cheap at small means.
            let (l, mut k, mut p) = ((-lambda).exp(), 0.0, 1.0);
            loop {
                p *= self.uniform();
                if p <= l {
                    return k;
                }
                k += 1.0;
            }
        }
        // At large means the normal approximation is accurate to well under the noise itself.
        (lambda + lambda.sqrt() * self.normal()).max(0.0).round()
    }

    /// One noisy observation of the clean level `y` under `model`.
    pub fn observe(&mut self, model: &NoiseModel, y: f32) -> f32 {
        let shot = if model.gain > 0.0 {
            self.poisson((y.max(0.0) / model.gain) as f64) * model.gain as f64
        } else {
            y.max(0.0) as f64
        };
        (shot + model.read.max(0.0).sqrt() as f64 * self.normal()) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of flat patches over three decades of light, with Poisson-Gaussian noise of a known model: the
    /// estimate recovers the model.
    #[test]
    fn the_estimate_recovers_a_known_model() {
        let (w, h) = (768usize, 512usize);
        let truth = NoiseModel {
            gain: 4e-4,
            read: 2.5e-6,
        };
        let mut synth = Synth::new(7);
        // 12 × 8 patches, each a flat level from 1/1000 to 0.8, plus a few textured ones the estimate must skip.
        let level = |x: usize, y: usize| {
            let k = (y / 64) * 12 + x / 64;
            let base = 0.001 * (800f32).powf(k as f32 / 95.0);
            if k % 13 == 5 {
                base * (1.0 + 0.5 * ((x as f32 * 0.9).sin() * (y as f32 * 0.7).cos()))
            } else {
                base
            }
        };
        let lum: Vec<f32> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| synth.observe(&truth, level(x, y)))
            .collect();
        let (model, bins) = estimate(&lum, w, h).expect("enough flat levels");
        eprintln!(
            "noise model: {model:?} from {} bins (truth {truth:?})",
            bins.len()
        );
        assert!(
            (model.gain / truth.gain - 1.0).abs() < 0.15,
            "gain {}",
            model.gain
        );
        assert!(
            (model.read / truth.read - 1.0).abs() < 0.35,
            "read {}",
            model.read
        );
    }

    /// The transform makes the noise unit-variance at every level, and the inverse brings the mean back
    /// without bias down to a few photo-electrons.
    #[test]
    fn the_transform_stabilizes_and_the_inverse_is_unbiased() {
        let m = NoiseModel {
            gain: 1e-3,
            read: 4e-6,
        };
        let mut synth = Synth::new(3);
        for &y in &[0.004f32, 0.02, 0.1, 0.5] {
            let n = 20000;
            let t: Vec<f32> = (0..n).map(|_| forward(&m, synth.observe(&m, y))).collect();
            let mean = t.iter().sum::<f32>() / n as f32;
            let var = t.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n as f32;
            assert!(
                (var - 1.0).abs() < 0.12,
                "level {y}: transformed variance {var}"
            );
            let back = inverse(&m, mean);
            assert!(
                (back - y).abs() < 0.03 * y + 2e-4,
                "level {y}: the mean comes back as {back}"
            );
        }
    }
}
