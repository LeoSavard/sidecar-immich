// SPDX-License-Identifier: Apache-2.0
//! Aligning two frames: which point of one is which point of the other, as a homography. The shared first step
//! of HDR merge (handheld brackets), panoramas, HDRIs and focus stacks (docs/research/multi-frame-and-denoise.md).
//!
//! Written from the papers, nothing ported: FAST corners (Rosten and Drummond, ECCV 2006) scored and spread over
//! a grid, oriented by intensity centroid and described by steered BRIEF (Calonder et al., ECCV 2010; Rublee et
//! al., "ORB", ICCV 2011 — with BRIEF's own Gaussian sampling pattern, drawn from a fixed seed), matched by
//! Hamming distance with Lowe's ratio test and a cross-check, and a homography by RANSAC over the normalised
//! four-point DLT (Hartley and Zisserman), refined by least squares on its inliers.
//!
//! Everything runs on **log luminance**: a bracket's exposure difference is then a constant offset, which
//! FAST's differences and BRIEF's comparisons ignore by construction, so one aligner serves brackets and
//! panoramas alike.

use rayon::prelude::*;

/// A single-channel image, row-major.
#[derive(Clone, Debug)]
pub struct Grey {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Grey {
    pub fn at(&self, x: i32, y: i32) -> f32 {
        let x = x.clamp(0, self.w as i32 - 1) as usize;
        let y = y.clamp(0, self.h as i32 - 1) as usize;
        self.data[y * self.w + x]
    }

    /// Log₂ of linear luminance, floored 12 stops under 1: what the features are found on.
    pub fn log_luminance(w: usize, h: usize, linear: impl Iterator<Item = f32>) -> Self {
        Self {
            w,
            h,
            data: linear.map(|y| y.max(1.0 / 4096.0).log2()).collect(),
        }
    }

    fn blurred(&self, r: usize) -> Grey {
        let (w, h) = (self.w, self.h);
        let mut tmp = vec![0f32; w * h];
        let mut out = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
                tmp[y * w + x] =
                    (a..=b).map(|i| self.data[y * w + i]).sum::<f32>() / (b - a + 1) as f32;
            }
        }
        for y in 0..h {
            let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
            for x in 0..w {
                out[y * w + x] = (a..=b).map(|j| tmp[j * w + x]).sum::<f32>() / (b - a + 1) as f32;
            }
        }
        Grey { w, h, data: out }
    }
}

/// A feature: where it is, its orientation, its descriptor.
#[derive(Clone, Copy, Debug)]
pub struct Feature {
    pub x: f32,
    pub y: f32,
    pub angle: f32,
    pub desc: [u64; 4],
}

/// The Bresenham circle of radius 3 FAST tests, clockwise from the top.
const CIRCLE: [(i32, i32); 16] = [
    (0, -3),
    (1, -3),
    (2, -2),
    (3, -1),
    (3, 0),
    (3, 1),
    (2, 2),
    (1, 3),
    (0, 3),
    (-1, 3),
    (-2, 2),
    (-3, 1),
    (-3, 0),
    (-3, -1),
    (-2, -2),
    (-1, -3),
];

const PATCH: i32 = 15;

/// FAST-9 at `(x, y)` with threshold `t`: the corner's score (sum of the arc's excess over `t`), or 0.
fn fast(img: &Grey, x: i32, y: i32, t: f32) -> f32 {
    let p = img.at(x, y);
    let ring: [f32; 16] = CIRCLE.map(|(dx, dy)| img.at(x + dx, y + dy));
    let mut best = 0.0f32;
    for sign in [1.0f32, -1.0] {
        let mut run = 0;
        let mut score = 0.0;
        // Twice round, so a run can wrap.
        for k in 0..32 {
            let d = sign * (ring[k % 16] - p) - t;
            if d > 0.0 {
                run += 1;
                score += d;
                if run >= 9 {
                    best = best.max(score);
                }
            } else {
                run = 0;
                score = 0.0;
            }
        }
    }
    best
}

/// BRIEF's sampling pattern: 256 pairs from an isotropic Gaussian of σ = patch/5 (Calonder et al., G II),
/// clipped to the patch, from a fixed seed so every run describes alike.
fn pattern() -> &'static [[(f32, f32); 2]; 256] {
    static P: std::sync::OnceLock<[[(f32, f32); 2]; 256]> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let mut s = 0x2545_F491_4F6C_DD1Du64;
        let mut u = || {
            s ^= s >> 12;
            s ^= s << 25;
            s ^= s >> 27;
            ((s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        let sigma = (2 * PATCH + 1) as f64 / 5.0;
        let mut g = || {
            let (a, b) = (u(), u());
            let v = (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos() * sigma;
            v.clamp(-(PATCH as f64 - 1.0), PATCH as f64 - 1.0) as f32
        };
        std::array::from_fn(|_| [(g(), g()), (g(), g())])
    })
}

/// Up to `max` features of `img` (log luminance), spread over a grid so one textured corner can't take them all.
pub fn features(img: &Grey, max: usize) -> Vec<Feature> {
    let smooth = img.blurred(2);
    let t = 0.15; // a 10 % step in light
    let margin = PATCH + 4;
    let (w, h) = (img.w as i32, img.h as i32);
    if w <= 2 * margin || h <= 2 * margin {
        return Vec::new();
    }
    let sm = &smooth;
    let scores: Vec<f32> = (0..h)
        .into_par_iter()
        .flat_map_iter(move |y| {
            (0..w).map(move |x| {
                if x < margin || y < margin || x >= w - margin || y >= h - margin {
                    0.0
                } else {
                    fast(sm, x, y, t)
                }
            })
        })
        .collect();
    let score = |x: i32, y: i32| scores[(y * w + x) as usize];
    // Local maxima over 5 × 5, by grid cell.
    let cell = ((w.max(h) as usize) / 24).max(16) as i32;
    let (cw, ch) = ((w + cell - 1) / cell, (h + cell - 1) / cell);
    let mut cells: Vec<Vec<(f32, i32, i32)>> = vec![Vec::new(); (cw * ch) as usize];
    for y in margin..h - margin {
        for x in margin..w - margin {
            let s = score(x, y);
            if s <= 0.0 {
                continue;
            }
            let peak = (-2..=2).all(|dy| {
                (-2..=2).all(|dx| {
                    (dx == 0 && dy == 0)
                        || score(x + dx, y + dy) < s
                        || (score(x + dx, y + dy) == s && (dy, dx) > (0, 0))
                })
            });
            if peak {
                cells[((y / cell) * cw + x / cell) as usize].push((s, x, y));
            }
        }
    }
    let per_cell = (max / cells.len()).max(2);
    let mut picked: Vec<(f32, i32, i32)> = cells
        .into_iter()
        .flat_map(|mut c| {
            c.sort_by(|a, b| b.0.total_cmp(&a.0));
            c.truncate(per_cell);
            c
        })
        .collect();
    picked.sort_by(|a, b| b.0.total_cmp(&a.0));
    picked.truncate(max);
    let pat = pattern();
    picked
        .par_iter()
        .map(|&(_, x, y)| {
            // Orientation: the intensity centroid of the patch, on the smoothed image.
            let (mut m10, mut m01) = (0.0f32, 0.0f32);
            for dy in -PATCH..=PATCH {
                for dx in -PATCH..=PATCH {
                    if dx * dx + dy * dy <= PATCH * PATCH {
                        let v = smooth.at(x + dx, y + dy);
                        m10 += dx as f32 * v;
                        m01 += dy as f32 * v;
                    }
                }
            }
            let angle = m01.atan2(m10);
            let (s, c) = angle.sin_cos();
            let sample = |(px, py): (f32, f32)| {
                let (rx, ry) = (c * px - s * py, s * px + c * py);
                smooth.at(x + rx.round() as i32, y + ry.round() as i32)
            };
            let mut desc = [0u64; 4];
            for (i, [a, b]) in pat.iter().enumerate() {
                if sample(*a) < sample(*b) {
                    desc[i / 64] |= 1 << (i % 64);
                }
            }
            Feature {
                x: x as f32,
                y: y as f32,
                angle,
                desc,
            }
        })
        .collect()
}

fn hamming(a: &[u64; 4], b: &[u64; 4]) -> u32 {
    (0..4).map(|i| (a[i] ^ b[i]).count_ones()).sum()
}

/// Matches from `a` to `b`: index pairs that pass Lowe's ratio test (best under `ratio` of the second best)
/// and are each other's best.
pub fn matches(a: &[Feature], b: &[Feature], ratio: f32) -> Vec<(usize, usize)> {
    let best = |from: &[Feature], to: &[Feature]| -> Vec<Option<(usize, u32, u32)>> {
        from.par_iter()
            .map(|f| {
                let (mut i0, mut d0, mut d1) = (usize::MAX, u32::MAX, u32::MAX);
                for (j, g) in to.iter().enumerate() {
                    let d = hamming(&f.desc, &g.desc);
                    if d < d0 {
                        d1 = d0;
                        d0 = d;
                        i0 = j;
                    } else if d < d1 {
                        d1 = d;
                    }
                }
                (i0 != usize::MAX).then_some((i0, d0, d1))
            })
            .collect()
    };
    let (ab, ba) = (best(a, b), best(b, a));
    ab.iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let (j, d0, d1) = (*m)?;
            let ok = (d0 as f32) < ratio * d1 as f32 && d0 <= 80;
            (ok && ba[j].map(|x| x.0) == Some(i)).then_some((i, j))
        })
        .collect()
}

/// A 3 × 3 homography, row-major, mapping a point of one frame to the other.
pub type H = [[f64; 3]; 3];

pub fn apply(h: &H, p: [f64; 2]) -> [f64; 2] {
    let w = h[2][0] * p[0] + h[2][1] * p[1] + h[2][2];
    [
        (h[0][0] * p[0] + h[0][1] * p[1] + h[0][2]) / w,
        (h[1][0] * p[0] + h[1][1] * p[1] + h[1][2]) / w,
    ]
}

/// Solve `m · x = v` (8 unknowns) by Gaussian elimination with partial pivoting.
fn solve8(mut m: [[f64; 9]; 8]) -> Option<[f64; 8]> {
    for c in 0..8 {
        let p = (c..8).max_by(|&a, &b| m[a][c].abs().total_cmp(&m[b][c].abs()))?;
        if m[p][c].abs() < 1e-12 {
            return None;
        }
        m.swap(c, p);
        let pivot = m[c];
        for (r, row) in m.iter_mut().enumerate() {
            if r != c {
                let f = row[c] / pivot[c];
                for (v, p) in row[c..].iter_mut().zip(&pivot[c..]) {
                    *v -= f * p;
                }
            }
        }
    }
    Some(std::array::from_fn(|i| m[i][8] / m[i][i]))
}

/// Hartley's normalisation: the points centred and scaled to mean distance √2, and the transform that does it.
fn normalise(pts: &[[f64; 2]]) -> (Vec<[f64; 2]>, H) {
    let n = pts.len() as f64;
    let (cx, cy) = pts
        .iter()
        .fold((0.0, 0.0), |a, p| (a.0 + p[0] / n, a.1 + p[1] / n));
    let d = pts
        .iter()
        .map(|p| ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt())
        .sum::<f64>()
        / n;
    let s = if d > 0.0 {
        std::f64::consts::SQRT_2 / d
    } else {
        1.0
    };
    let t = [[s, 0.0, -s * cx], [0.0, s, -s * cy], [0.0, 0.0, 1.0]];
    (
        pts.iter()
            .map(|p| [s * (p[0] - cx), s * (p[1] - cy)])
            .collect(),
        t,
    )
}

fn mul(a: &H, b: &H) -> H {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn inverse(m: &H) -> Option<H> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-15 {
        return None;
    }
    let c =
        |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    Some([
        [
            c(1, 1, 2, 2) / det,
            -c(0, 1, 2, 2) / det,
            c(0, 1, 1, 2) / det,
        ],
        [
            -c(1, 0, 2, 2) / det,
            c(0, 0, 2, 2) / det,
            -c(0, 0, 1, 2) / det,
        ],
        [
            c(1, 0, 2, 1) / det,
            -c(0, 0, 2, 1) / det,
            c(0, 0, 1, 1) / det,
        ],
    ])
}

/// The homography from `src` to `dst` by DLT on normalised points (h₃₃ = 1), least squares when more than four.
pub fn dlt(src: &[[f64; 2]], dst: &[[f64; 2]]) -> Option<H> {
    if src.len() < 4 {
        return None;
    }
    let (s, ts) = normalise(src);
    let (d, td) = normalise(dst);
    // Normal equations AᵀA x = Aᵀb over every correspondence's two rows.
    let mut ata = [[0f64; 9]; 8];
    for (p, q) in s.iter().zip(&d) {
        let rows = [
            (
                [p[0], p[1], 1.0, 0.0, 0.0, 0.0, -p[0] * q[0], -p[1] * q[0]],
                q[0],
            ),
            (
                [0.0, 0.0, 0.0, p[0], p[1], 1.0, -p[0] * q[1], -p[1] * q[1]],
                q[1],
            ),
        ];
        for (a, b) in rows {
            for i in 0..8 {
                for j in 0..8 {
                    ata[i][j] += a[i] * a[j];
                }
                ata[i][8] += a[i] * b;
            }
        }
    }
    let x = solve8(ata)?;
    let hn = [[x[0], x[1], x[2]], [x[3], x[4], x[5]], [x[6], x[7], 1.0]];
    let h = mul(&inverse(&td)?, &mul(&hn, &ts));
    let k = h[2][2];
    (k.abs() > 1e-12).then(|| h.map(|r| r.map(|v| v / k)))
}

/// What an alignment found.
#[derive(Clone, Debug)]
pub struct Alignment {
    /// From the first frame to the second.
    pub h: H,
    pub matches: usize,
    pub inliers: usize,
    /// The inliers' root-mean-square reprojection error, in pixels of the second frame.
    pub rms: f64,
}

/// RANSAC over four-point DLTs, then a least-squares refit on the inliers. `threshold` is the reprojection error,
/// in pixels, under which a match counts as an inlier.
pub fn homography(
    a: &[Feature],
    b: &[Feature],
    m: &[(usize, usize)],
    threshold: f64,
) -> Option<Alignment> {
    if m.len() < 8 {
        return None;
    }
    let src: Vec<[f64; 2]> = m
        .iter()
        .map(|&(i, _)| [a[i].x as f64, a[i].y as f64])
        .collect();
    let dst: Vec<[f64; 2]> = m
        .iter()
        .map(|&(_, j)| [b[j].x as f64, b[j].y as f64])
        .collect();
    let err = |h: &H, k: usize| {
        let p = apply(h, src[k]);
        ((p[0] - dst[k][0]).powi(2) + (p[1] - dst[k][1]).powi(2)).sqrt()
    };
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut pick = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % m.len() as u64) as usize
    };
    let mut best: (usize, Option<H>) = (0, None);
    for _ in 0..2000 {
        let idx = [pick(), pick(), pick(), pick()];
        if (0..4).any(|i| (i + 1..4).any(|j| idx[i] == idx[j])) {
            continue;
        }
        let Some(h) = dlt(&idx.map(|k| src[k]), &idx.map(|k| dst[k])) else {
            continue;
        };
        let n = (0..m.len()).filter(|&k| err(&h, k) < threshold).count();
        if n > best.0 {
            best = (n, Some(h));
        }
    }
    let h = best.1?;
    // Refit on the inliers twice: the refit can admit a few more.
    let mut h = h;
    for _ in 0..2 {
        let inl: Vec<usize> = (0..m.len()).filter(|&k| err(&h, k) < threshold).collect();
        if inl.len() < 8 {
            return None;
        }
        h = dlt(
            &inl.iter().map(|&k| src[k]).collect::<Vec<_>>(),
            &inl.iter().map(|&k| dst[k]).collect::<Vec<_>>(),
        )?;
    }
    let inl: Vec<usize> = (0..m.len()).filter(|&k| err(&h, k) < threshold).collect();
    let rms =
        (inl.iter().map(|&k| err(&h, k).powi(2)).sum::<f64>() / inl.len().max(1) as f64).sqrt();
    Some(Alignment {
        h,
        matches: m.len(),
        inliers: inl.len(),
        rms,
    })
}

/// The share of frame `a` (`aw × ah`) that lands inside frame `b` under `h`.
pub fn overlap(h: &H, aw: usize, ah: usize, bw: usize, bh: usize) -> f64 {
    let n = 40;
    let mut inside = 0;
    for j in 0..n {
        for i in 0..n {
            let p = apply(
                h,
                [
                    (i as f64 + 0.5) / n as f64 * aw as f64,
                    (j as f64 + 0.5) / n as f64 * ah as f64,
                ],
            );
            if p[0] >= 0.0 && p[1] >= 0.0 && p[0] < bw as f64 && p[1] < bh as f64 {
                inside += 1;
            }
        }
    }
    inside as f64 / (n * n) as f64
}

/// Align `a` to `b` (both log luminance): features, matches, homography.
pub fn align(a: &Grey, b: &Grey) -> Option<Alignment> {
    let (fa, fb) = (features(a, 2000), features(b, 2000));
    let m = matches(&fa, &fb, 0.8);
    let scale = (a.w.max(a.h) as f64 / 1600.0).max(0.5);
    homography(&fa, &fb, &m, 3.0 * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A textured test card: random discs and bars of different light, so corners are many and distinct.
    fn card(w: usize, h: usize, seed: u64) -> Grey {
        let mut s = seed;
        let mut r = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s % 10_000) as f64 / 10_000.0
        };
        let mut lin = vec![0.05f32; w * h];
        for _ in 0..400 {
            let (cx, cy, rad, v) = (
                r() * w as f64,
                r() * h as f64,
                3.0 + r() * 25.0,
                0.02 + r() * 0.9,
            );
            let square = r() < 0.5;
            for y in (cy - rad).max(0.0) as usize..((cy + rad) as usize).min(h) {
                for x in (cx - rad).max(0.0) as usize..((cx + rad) as usize).min(w) {
                    let (dx, dy) = (x as f64 - cx, y as f64 - cy);
                    if square || dx * dx + dy * dy < rad * rad {
                        lin[y * w + x] = v as f32;
                    }
                }
            }
        }
        Grey { w, h, data: lin }
    }

    /// `img` seen through `h` (output point p shows input point h⁻¹·p), bilinear, times `gain`.
    fn warp(img: &Grey, h: &H, gain: f32) -> Grey {
        let inv = inverse(h).unwrap();
        let mut out = vec![0.05 * gain; img.w * img.h];
        for y in 0..img.h {
            for x in 0..img.w {
                let p = apply(&inv, [x as f64, y as f64]);
                if p[0] < 0.0
                    || p[1] < 0.0
                    || p[0] > (img.w - 2) as f64
                    || p[1] > (img.h - 2) as f64
                {
                    continue;
                }
                let (x0, y0) = (p[0].floor() as usize, p[1].floor() as usize);
                let (fx, fy) = ((p[0] - x0 as f64) as f32, (p[1] - y0 as f64) as f32);
                let v = |xx: usize, yy: usize| img.data[yy * img.w + xx];
                let top = v(x0, y0) * (1.0 - fx) + v(x0 + 1, y0) * fx;
                let bot = v(x0, y0 + 1) * (1.0 - fx) + v(x0 + 1, y0 + 1) * fx;
                out[y * img.w + x] = gain * (top * (1.0 - fy) + bot * fy);
            }
        }
        Grey { data: out, ..*img }
    }

    fn log(g: &Grey) -> Grey {
        Grey::log_luminance(g.w, g.h, g.data.iter().copied())
    }

    /// A known homography — a pan, a small turn, a little perspective — is recovered from a frame and its
    /// warped copy two stops darker, to well under a pixel at the corners.
    #[test]
    fn a_known_homography_is_recovered_across_two_stops() {
        let (w, h) = (900, 600);
        let a = card(w, h, 7);
        let truth: H = [[0.98, -0.05, 60.0], [0.04, 0.99, -25.0], [2e-5, -1e-5, 1.0]];
        let b = warp(&a, &truth, 0.25);
        let found = align(&log(&a), &log(&b)).expect("aligned");
        eprintln!("{found:?}");
        assert!(found.inliers > 100, "{} inliers", found.inliers);
        for p in [
            [100.0, 100.0],
            [800.0, 120.0],
            [450.0, 300.0],
            [150.0, 520.0],
        ] {
            let (want, got) = (apply(&truth, p), apply(&found.h, p));
            let e = ((want[0] - got[0]).powi(2) + (want[1] - got[1]).powi(2)).sqrt();
            assert!(e < 0.75, "at {p:?}: {got:?} against {want:?}");
        }
        assert!((overlap(&found.h, w, h, w, h) - overlap(&truth, w, h, w, h)).abs() < 0.02);
    }

    /// Two unrelated frames don't align. (The same card turned half round does, which is right.)
    #[test]
    fn unrelated_frames_do_not_align() {
        let a = card(600, 400, 7);
        let b = card(600, 400, 1234567);
        let r = align(&log(&a), &log(&b));
        assert!(r.as_ref().is_none_or(|r| r.inliers < 20), "{r:?}");
    }

    #[test]
    fn dlt_is_exact_on_four_points() {
        let h: H = [[1.1, 0.1, 5.0], [-0.05, 0.9, 3.0], [1e-4, 2e-4, 1.0]];
        let src = [[0.0, 0.0], [100.0, 0.0], [100.0, 80.0], [0.0, 80.0]];
        let dst = src.map(|p| apply(&h, p));
        let got = dlt(&src, &dst).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                assert!((got[i][j] - h[i][j]).abs() < 1e-6, "{got:?}");
            }
        }
    }
}
