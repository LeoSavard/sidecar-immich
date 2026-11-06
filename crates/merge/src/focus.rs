// SPDX-License-Identifier: Apache-2.0
//! Focus stacking: frames focused at different distances, joined so everything is sharp
//! (docs/research/multi-frame-and-denoise.md, "Focus stacking").
//!
//! Defocus is a low-pass filter, so at each point the frame focused there has the most high-frequency energy.
//! Rather than picking the sharpest sample pixel by pixel — which follows noise wherever there is no detail and
//! leaves halos where it switches — this builds a **depth map**: the index of the sharpest frame (the sum-
//! modified Laplacian, Nayar and Nakagawa, "Shape from focus", PAMI 1994, on log luminance so the frames'
//! exposures don't matter), trusted in proportion to how far that frame stands out from the mean, and spread
//! by normalised push–pull from where it is trusted into the flat regions that carry no focus information. A
//! point between two focus planes takes from both, linearly; the frames are then joined by multi-band blending
//! (`blend`), so the joins vanish at every scale.
//!
//! Focusing changes the field of view (focus breathing): frames are aligned to the reference by homography,
//! which includes the scale, and the result keeps only what every frame saw.

use crate::align::{self, H};
use crate::blend::Plane;
use rayon::prelude::*;

/// A frame resampled into the reference's geometry: three channels, and where it has data.
pub fn warp(rgb: &[f32], w: usize, h: usize, from_ref: &H, ow: usize, oh: usize) -> (Plane, Plane) {
    let mut val = Plane::new(ow, oh, 3);
    let mut alpha = Plane::new(ow, oh, 1);
    val.data
        .par_chunks_mut(ow * 3)
        .zip(alpha.data.par_chunks_mut(ow))
        .enumerate()
        .for_each(|(y, (v, a))| {
            for x in 0..ow {
                let p = align::apply(from_ref, [x as f64, y as f64]);
                if p[0] < 0.0 || p[1] < 0.0 || p[0] > (w - 1) as f64 || p[1] > (h - 1) as f64 {
                    continue;
                }
                let (x0, y0) = ((p[0] as usize).min(w - 2), (p[1] as usize).min(h - 2));
                let (fx, fy) = ((p[0] - x0 as f64) as f32, (p[1] - y0 as f64) as f32);
                for c in 0..3 {
                    let s = |xx: usize, yy: usize| rgb[(yy * w + xx) * 3 + c];
                    let top = s(x0, y0) * (1.0 - fx) + s(x0 + 1, y0) * fx;
                    let bot = s(x0, y0 + 1) * (1.0 - fx) + s(x0 + 1, y0 + 1) * fx;
                    v[x * 3 + c] = top * (1.0 - fy) + bot * fy;
                }
                a[x] = 1.0;
            }
        });
    (val, alpha)
}

/// A box mean of radius `r`, separable, edges clamped.
fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; w * h];
    tmp.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let s = &src[y * w..(y + 1) * w];
        let mut acc = 0f64;
        let at = |i: isize| s[i.clamp(0, w as isize - 1) as usize] as f64;
        for i in -(r as isize)..=r as isize {
            acc += at(i);
        }
        for (x, o) in row.iter_mut().enumerate() {
            *o = (acc / (2 * r + 1) as f64) as f32;
            acc += at(x as isize + r as isize + 1) - at(x as isize - r as isize);
        }
    });
    let mut out = vec![0f32; w * h];
    let cols: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let at = |i: isize| tmp[i.clamp(0, h as isize - 1) as usize * w + x] as f64;
            let mut acc = 0f64;
            for i in -(r as isize)..=r as isize {
                acc += at(i);
            }
            (0..h)
                .map(|y| {
                    let v = (acc / (2 * r + 1) as f64) as f32;
                    acc += at(y as isize + r as isize + 1) - at(y as isize - r as isize);
                    v
                })
                .collect()
        })
        .collect();
    for (x, col) in cols.iter().enumerate() {
        for (y, v) in col.iter().enumerate() {
            out[y * w + x] = *v;
        }
    }
    out
}

/// The focus measure of a frame: the modified Laplacian of log luminance at step `step` (|2I − I₋ − I₊| across
/// and down), squared and averaged over a window of radius `4·step`.
pub fn measure(val: &Plane, step: usize) -> Vec<f32> {
    let (w, h) = (val.w, val.h);
    let lum: Vec<f32> = val
        .data
        .par_chunks(3)
        .map(|p| ((p[0] + p[1] + p[2]) / 3.0).max(1e-4).log2())
        .collect();
    let s = step as isize;
    let ml: Vec<f32> = (0..w * h)
        .into_par_iter()
        .map(|k| {
            let (x, y) = ((k % w) as isize, (k / w) as isize);
            let at = |dx: isize, dy: isize| {
                lum[(y + dy).clamp(0, h as isize - 1) as usize * w
                    + (x + dx).clamp(0, w as isize - 1) as usize]
            };
            let c = 2.0 * at(0, 0);
            let m = (c - at(-s, 0) - at(s, 0)).abs() + (c - at(0, -s) - at(0, s)).abs();
            m * m
        })
        .collect();
    box_mean(&ml, w, h, 4 * step)
}

/// The sharpest frame so far at each pixel, and the mean measure, accumulated one frame at a time.
pub struct Focus {
    pub w: usize,
    pub h: usize,
    best: Vec<f32>,
    sum: Vec<f32>,
    index: Vec<u16>,
    /// Every frame saw the pixel.
    pub covered: Vec<bool>,
    frames: usize,
}

impl Focus {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            best: vec![f32::MIN; w * h],
            sum: vec![0.0; w * h],
            index: vec![0; w * h],
            covered: vec![true; w * h],
            frames: 0,
        }
    }

    pub fn add(&mut self, i: usize, measure: &[f32], alpha: &Plane) {
        self.best
            .par_iter_mut()
            .zip(&mut self.index)
            .zip(&mut self.sum)
            .zip(&mut self.covered)
            .zip(measure)
            .zip(&alpha.data)
            .for_each(|(((((b, idx), s), cov), m), a)| {
                if *a < 1.0 {
                    *cov = false;
                    return;
                }
                *s += m;
                if *m > *b {
                    *b = *m;
                    *idx = i as u16;
                }
            });
        self.frames += 1;
    }

    /// The depth map, as a fractional frame index: each pixel's sharpest frame, weighted by how far it stands
    /// out from the mean measure, spread by push–pull into what carries no focus information.
    pub fn depth(&self, levels: usize) -> Vec<f32> {
        let n = self.frames.max(1) as f32;
        let (w, h) = (self.w, self.h);
        // Push–pull works on sizes divisible by 2^levels; pad by repeating the edge.
        let step = 1 << levels;
        let (pw, ph) = (w.div_ceil(step) * step, h.div_ceil(step) * step);
        let mut val = Plane::new(pw, ph, 3);
        let mut alpha = Plane::new(pw, ph, 1);
        for y in 0..ph {
            for x in 0..pw {
                let k = y.min(h - 1) * w + x.min(w - 1);
                let mean = self.sum[k] / n;
                let c = if self.covered[k] && self.best[k] > 0.0 {
                    ((self.best[k] - mean) / self.best[k])
                        .clamp(0.0, 1.0)
                        .powi(2)
                } else {
                    0.0
                };
                let d = self.index[k] as f32;
                let o = y * pw + x;
                val.data[o * 3] = c * d;
                alpha.data[o] = c;
            }
        }
        let filled = crate::blend::fill(val, alpha, levels, false);
        (0..w * h)
            .map(|k| {
                let (x, y) = (k % w, k / w);
                filled.data[(y * pw + x) * 3].clamp(0.0, n - 1.0)
            })
            .collect()
    }
}

/// Above this coarse difference, in stops, neighbouring frames show a scene that moved.
pub const MOVED: f32 = 0.15;

/// How different two aligned frames are once defocus can't matter: both log-luminance proxies box-blurred over
/// a fiftieth of their width — coarser than any defocus a stack's neighbours differ by — and the median
/// absolute difference, in stops, where `b` (seen through `h`, `a` → `b`) covers `a`. A focus stack's
/// neighbours differ in fine detail only; a scene that moved differs here.
pub fn coarse_difference(a: &align::Grey, b: &align::Grey, h: &H) -> f32 {
    let r = (a.w / 50).max(2);
    let (ba, bb) = (
        box_mean(&a.data, a.w, a.h, r),
        box_mean(&b.data, b.w, b.h, r),
    );
    let mut d: Vec<f32> = (0..a.h)
        .step_by(2)
        .flat_map(|y| (0..a.w).step_by(2).map(move |x| (x, y)))
        .filter_map(|(x, y)| {
            let p = align::apply(h, [x as f64, y as f64]);
            let (bx, by) = (p[0].round() as isize, p[1].round() as isize);
            let m = r as isize;
            (bx >= m
                && by >= m
                && bx < b.w as isize - m
                && by < b.h as isize - m
                && x >= r
                && y >= r
                && x + r < a.w
                && y + r < a.h)
                .then(|| (ba[y * a.w + x] - bb[by as usize * b.w + bx as usize]).abs())
        })
        .collect();
    if d.is_empty() {
        return f32::MAX;
    }
    let k = d.len() / 2;
    *d.select_nth_unstable_by(k, f32::total_cmp).1
}

/// Frame `i`'s share at each pixel of a depth map: 1 at its own index, falling linearly to 0 one frame away.
pub fn weights(depth: &[f32], i: usize, alpha: &Plane) -> Plane {
    let mut m = Plane::new(alpha.w, alpha.h, 1);
    m.data
        .par_iter_mut()
        .zip(depth)
        .zip(&alpha.data)
        .for_each(|((o, d), a)| *o = (1.0 - (d - i as f32).abs()).max(0.0) * a);
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blend::Bands;

    fn hash(mut s: u64) -> u64 {
        s = s.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        for _ in 0..3 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
        }
        s
    }

    /// A sharp test card: random discs of random greys.
    fn card(w: usize, h: usize) -> Vec<f32> {
        let mut img = vec![0.2f32; w * h];
        for k in 0..900u64 {
            let r = |j: u64| (hash(k * 7 + j) % 10_000) as f64 / 10_000.0;
            let (cx, cy, rad, v) = (
                r(1) * w as f64,
                r(2) * h as f64,
                2.0 + 10.0 * r(3),
                0.02 + 0.9 * r(4),
            );
            for y in (cy - rad).max(0.0) as usize..((cy + rad) as usize).min(h) {
                for x in (cx - rad).max(0.0) as usize..((cx + rad) as usize).min(w) {
                    if (x as f64 - cx).powi(2) + (y as f64 - cy).powi(2) < rad * rad {
                        img[y * w + x] = v as f32;
                    }
                }
            }
        }
        img
    }

    fn gauss(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
        if sigma < 0.3 {
            return src.to_vec();
        }
        let r = (3.0 * sigma).ceil() as isize;
        let k: Vec<f32> = (-r..=r)
            .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
            .collect();
        let s: f32 = k.iter().sum();
        let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
            (0..w * h)
                .map(|p| {
                    let (x, y) = ((p % w) as isize, (p / w) as isize);
                    k.iter()
                        .enumerate()
                        .map(|(j, kv)| {
                            let d = j as isize - r;
                            let (xx, yy) = if horizontal {
                                ((x + d).clamp(0, w as isize - 1), y)
                            } else {
                                (x, (y + d).clamp(0, h as isize - 1))
                            };
                            kv * src[yy as usize * w + xx as usize]
                        })
                        .sum::<f32>()
                        / s
                })
                .collect()
        };
        pass(&pass(src, true), false)
    }

    /// A scene whose depth runs left to right, with a near square in it, shot at five focus distances (blur
    /// growing with the distance from each): the stack is far nearer the sharp scene than any frame.
    #[test]
    fn a_stack_is_sharper_than_any_frame() {
        let (w, h) = (256, 160);
        let sharp = card(w, h);
        let depth = |x: usize, y: usize| {
            if (100..150).contains(&x) && (50..110).contains(&y) {
                0.1
            } else {
                x as f32 / (w - 1) as f32
            }
        };
        let sigmas = [0.0f32, 1.0, 2.0, 3.0, 4.0, 5.0];
        let blurred: Vec<Vec<f32>> = sigmas.iter().map(|&s| gauss(&sharp, w, h, s)).collect();
        let n = 5;
        let frames: Vec<Vec<f32>> = (0..n)
            .map(|i| {
                let focus = i as f32 / (n - 1) as f32;
                (0..w * h)
                    .map(|k| {
                        let sigma = (8.0 * (depth(k % w, k / w) - focus).abs()).min(4.99);
                        let lo = sigma.floor() as usize;
                        let t = sigma - lo as f32;
                        blurred[lo][k] * (1.0 - t) + blurred[lo + 1][k] * t
                    })
                    .collect()
            })
            .collect();
        let id: H = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let levels = 4;
        let mut focus = Focus::new(w, h);
        let planes: Vec<(Plane, Plane)> = frames
            .iter()
            .map(|f| {
                let rgb: Vec<f32> = f.iter().flat_map(|&v| [v, v, v]).collect();
                warp(&rgb, w, h, &id, w, h)
            })
            .collect();
        for (i, (v, a)) in planes.iter().enumerate() {
            focus.add(i, &measure(v, 1), a);
        }
        let d = focus.depth(levels);
        // The depth map finds the ramp and the square.
        let at = |x: usize, y: usize| d[y * w + x];
        assert!((at(125, 80) - 0.4).abs() < 0.6, "square {}", at(125, 80));
        assert!(
            at(20, 20) < 1.0 && at(235, 20) > 3.0,
            "ramp {} … {}",
            at(20, 20),
            at(235, 20)
        );
        let mut bands = Bands::new(w, h, levels, false);
        for (i, (v, a)) in planes.into_iter().enumerate() {
            let m = weights(&d, i, &a);
            bands.add(0, 0, crate::blend::fill(v, a, levels, false), m, false);
        }
        let out = bands.finish();
        let err = |img: &dyn Fn(usize) -> f32| -> f64 {
            let mut e = 0.0;
            let mut c = 0;
            for y in 8..h - 8 {
                for x in 8..w - 8 {
                    e += (img(y * w + x) - sharp[y * w + x]).abs() as f64;
                    c += 1;
                }
            }
            e / c as f64
        };
        let stacked = err(&|k| out.data[k * 3 + 1]);
        let best_frame = frames
            .iter()
            .map(|f| err(&|k| f[k]))
            .fold(f64::MAX, f64::min);
        // What a stack can reach: at each pixel, the frame focused nearest it.
        let oracle = err(&|k| {
            let i = (depth(k % w, k / w) * (n - 1) as f32).round() as usize;
            frames[i][k]
        });
        eprintln!(
            "mean error: stack {stacked:.4}, nearest-focus oracle {oracle:.4}, best single frame {best_frame:.4}"
        );
        assert!(stacked < 1.3 * oracle, "stack {stacked}, oracle {oracle}");
        // Neighbouring frames differ in focus only: their coarse pictures agree. A moved scene doesn't.
        let grey = |f: &[f32]| align::Grey::log_luminance(w, h, f.iter().copied());
        let still = coarse_difference(&grey(&frames[1]), &grey(&frames[2]), &id);
        let shifted: Vec<f32> = (0..w * h).map(|k| frames[0][(k + 60) % (w * h)]).collect();
        let moved = coarse_difference(&grey(&frames[0]), &grey(&shifted), &id);
        eprintln!("coarse difference: refocused {still:.3}, moved {moved:.3} stops");
        assert!(still < MOVED && moved > MOVED, "{still} {moved}");
        assert!(
            stacked < 0.35 * best_frame,
            "stack {stacked}, best frame {best_frame}"
        );
    }
}
