// SPDX-License-Identifier: Apache-2.0
//! Multi-band blending (Burt and Adelson, "A multiresolution spline with application to image mosaics", ACM ToG
//! 1983), shared by the panorama and the focus stack: each frame's Laplacian bands are blended with the Gaussian
//! bands of its weight mask, so every frequency changes over a width proportional to its wavelength — seams
//! vanish without blurring detail. Three channels, linear light, on the CPU.

use rayon::prelude::*;

/// An interleaved multi-channel image.
#[derive(Clone, Debug)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub c: usize,
    pub data: Vec<f32>,
}

impl Plane {
    pub fn new(w: usize, h: usize, c: usize) -> Self {
        Self {
            w,
            h,
            c,
            data: vec![0.0; w * h * c],
        }
    }
}

/// Burt and Adelson's generating kernel, a = 0.375.
const K5: [f32; 5] = [0.0625, 0.25, 0.375, 0.25, 0.0625];

fn index(i: isize, n: usize, wrap: bool) -> usize {
    if wrap {
        i.rem_euclid(n as isize) as usize
    } else {
        i.clamp(0, n as isize - 1) as usize
    }
}

/// Blur and halve (both sizes even). `wrap` joins the left and right edges, for a full turn.
pub fn reduce(p: &Plane, wrap: bool) -> Plane {
    let (w2, h2, c) = (p.w / 2, p.h / 2, p.c);
    let mut t = vec![0f32; w2 * p.h * c];
    t.par_chunks_mut(w2 * c).enumerate().for_each(|(y, row)| {
        let src = &p.data[y * p.w * c..(y + 1) * p.w * c];
        for x in 0..w2 {
            for (k, wt) in K5.iter().enumerate() {
                let sx = index(2 * x as isize + k as isize - 2, p.w, wrap);
                for ch in 0..c {
                    row[x * c + ch] += wt * src[sx * c + ch];
                }
            }
        }
    });
    let mut out = Plane::new(w2, h2, c);
    out.data
        .par_chunks_mut(w2 * c)
        .enumerate()
        .for_each(|(y, row)| {
            for (k, wt) in K5.iter().enumerate() {
                let sy = index(2 * y as isize + k as isize - 2, p.h, false);
                for (o, s) in row.iter_mut().zip(&t[sy * w2 * c..(sy + 1) * w2 * c]) {
                    *o += wt * s;
                }
            }
        });
    out
}

/// Double and interpolate with the same kernel: Burt and Adelson's EXPAND.
pub fn expand(p: &Plane, wrap: bool) -> Plane {
    let (w, h, c) = (p.w * 2, p.h * 2, p.c);
    let mut t = vec![0f32; w * p.h * c];
    t.par_chunks_mut(w * c).enumerate().for_each(|(y, row)| {
        let src = &p.data[y * p.w * c..(y + 1) * p.w * c];
        let at = |i: isize, ch: usize| src[index(i, p.w, wrap) * c + ch];
        for x in 0..w {
            let m = (x / 2) as isize;
            for ch in 0..c {
                row[x * c + ch] = if x % 2 == 0 {
                    (at(m - 1, ch) + 6.0 * at(m, ch) + at(m + 1, ch)) * 0.125
                } else {
                    (at(m, ch) + at(m + 1, ch)) * 0.5
                };
            }
        }
    });
    let mut out = Plane::new(w, h, c);
    let rows = |i: isize| {
        let y = index(i, p.h, false);
        &t[y * w * c..(y + 1) * w * c]
    };
    out.data
        .par_chunks_mut(w * c)
        .enumerate()
        .for_each(|(y, row)| {
            let m = (y / 2) as isize;
            if y % 2 == 0 {
                let (a, b, d) = (rows(m - 1), rows(m), rows(m + 1));
                for (k, o) in row.iter_mut().enumerate() {
                    *o = (a[k] + 6.0 * b[k] + d[k]) * 0.125;
                }
            } else {
                let (a, b) = (rows(m), rows(m + 1));
                for (k, o) in row.iter_mut().enumerate() {
                    *o = (a[k] + b[k]) * 0.5;
                }
            }
        });
    out
}

/// Fill what a frame doesn't see by push–pull (each level's unseen pixels from the level below, premultiplied by
/// coverage), so its pyramid has no false edge at its border. `val` holds three channels where `alpha` is 1.
pub fn fill(val: Plane, alpha: Plane, levels: usize, wrap: bool) -> Plane {
    let mut ps = vec![val];
    let mut as_ = vec![alpha];
    for k in 0..levels {
        let (p, a) = (reduce(&ps[k], wrap), reduce(&as_[k], wrap));
        ps.push(p);
        as_.push(a);
    }
    let mut filled = {
        let (p, a) = (&ps[levels], &as_[levels]);
        let mut sum = [0f64; 3];
        let mut n = 0f64;
        for (pp, aa) in p.data.chunks(3).zip(&a.data) {
            if *aa > 1e-6 {
                for k in 0..3 {
                    sum[k] += (pp[k] / aa) as f64;
                }
                n += 1.0;
            }
        }
        let mean = sum.map(|v| (v / n.max(1.0)) as f32);
        let mut out = p.clone();
        for (o, aa) in out.data.chunks_mut(3).zip(&a.data) {
            for k in 0..3 {
                o[k] = if *aa > 1e-6 { o[k] / aa } else { mean[k] };
            }
        }
        out
    };
    for k in (0..levels).rev() {
        let e = expand(&filled, wrap);
        let mut v = ps[k].clone();
        for ((o, aa), ee) in v.data.chunks_mut(3).zip(&as_[k].data).zip(e.data.chunks(3)) {
            for c in 0..3 {
                o[c] += (1.0 - aa.min(1.0)) * ee[c];
            }
        }
        filled = v;
    }
    filled
}

/// The blend's running sums: per band, Σ weight · Laplacian and Σ weight, over the whole output.
pub struct Bands {
    pub levels: usize,
    /// The output's left and right edges meet (a full turn).
    pub wrap: bool,
    num: Vec<Plane>,
    den: Vec<Plane>,
}

impl Bands {
    /// Sums for a `w × h` output (multiples of 2^levels).
    pub fn new(w: usize, h: usize, levels: usize, wrap: bool) -> Self {
        Self {
            levels,
            wrap,
            num: (0..=levels)
                .map(|k| Plane::new(w >> k, h >> k, 3))
                .collect(),
            den: (0..=levels)
                .map(|k| Plane::new(w >> k, h >> k, 1))
                .collect(),
        }
    }

    /// Blend in one frame, `filled` (three channels, no holes) with its weight `mask`, placed at `(x0, y0)` in
    /// the output — multiples of 2^levels; `x0` may run past the edges of a wrapping output. `wrap` says the
    /// frame itself is a whole turn.
    pub fn add(&mut self, x0: isize, y0: usize, filled: Plane, mask: Plane, wrap: bool) {
        let levels = self.levels;
        let mut g = filled;
        let mut m = mask;
        for k in 0..=levels {
            let (band, next) = if k < levels {
                let next = reduce(&g, wrap);
                let up = expand(&next, wrap);
                let mut band = g;
                for (b, u) in band.data.iter_mut().zip(&up.data) {
                    *b -= u;
                }
                (band, Some(next))
            } else {
                (g.clone(), None)
            };
            let (num, den) = (&mut self.num[k], &mut self.den[k]);
            let (cw, ox, oy) = (num.w, x0 >> k, y0 >> k);
            num.data
                .par_chunks_mut(cw * 3)
                .zip(den.data.par_chunks_mut(cw))
                .enumerate()
                .filter(|(y, _)| *y >= oy && *y < oy + band.h)
                .for_each(|(y, (nr, dr))| {
                    let by = y - oy;
                    for bx in 0..band.w {
                        let wgt = m.data[by * m.w + bx];
                        if wgt <= 0.0 {
                            continue;
                        }
                        let xx = (ox + bx as isize).rem_euclid(cw as isize) as usize;
                        for c in 0..3 {
                            nr[xx * 3 + c] += wgt * band.data[(by * band.w + bx) * 3 + c];
                        }
                        dr[xx] += wgt;
                    }
                });
            if let Some(next) = next {
                g = next;
                m = reduce(&m, wrap);
            } else {
                break;
            }
        }
    }

    /// Collapse the bands into the blended image, three channels.
    pub fn finish(self) -> Plane {
        let levels = self.levels;
        let norm = |num: &Plane, den: &Plane| {
            let mut out = num.clone();
            for (o, d) in out.data.chunks_mut(3).zip(&den.data) {
                for v in o.iter_mut() {
                    *v = if *d > 1e-12 { *v / d } else { 0.0 };
                }
            }
            out
        };
        let mut r = norm(&self.num[levels], &self.den[levels]);
        for k in (0..levels).rev() {
            let e = expand(&r, self.wrap);
            let mut band = norm(&self.num[k], &self.den[k]);
            for (b, v) in band.data.iter_mut().zip(&e.data) {
                *b += v;
            }
            r = band;
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame with a full mask comes back exactly: the bands sum to what they were cut from.
    #[test]
    fn a_single_frame_reconstructs_exactly() {
        let (w, h) = (64, 32);
        let mut img = Plane::new(w, h, 3);
        for (k, v) in img.data.iter_mut().enumerate() {
            *v = ((k * 7919) % 101) as f32 / 101.0;
        }
        let mut mask = Plane::new(w, h, 1);
        mask.data.fill(1.0);
        let mut b = Bands::new(w, h, 3, false);
        b.add(0, 0, img.clone(), mask, false);
        let out = b.finish();
        for (a, o) in img.data.iter().zip(&out.data) {
            assert!((a - o).abs() < 1e-5);
        }
    }
}
