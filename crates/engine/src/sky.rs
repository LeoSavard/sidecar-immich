// SPDX-License-Identifier: Apache-2.0
//! The sky mask (DESIGN.md §15, "Sky"): where the depth model puts the far plane, refined to the photo's own
//! edges by the guided filter.
//!
//! The depth model's map is coarse (518 px across) and soft at a skyline; the photo is sharp there. So the mask
//! is found in the depth map and *drawn* by the photo:
//!
//! 1. **Far:** depth within `FAR` of the map's range from its farthest value (Depth Anything's relative inverse
//!    depth puts the sky at its minimum), and from there out to `FAR_REACH` across the map's blurred skyline.
//! 2. **Above:** only the far region connected to the top edge — sky is above, a distant wall seen through a
//!    window is not sky.
//! 3. **Like sky**, pixel by pixel: smooth — no texture at the depth map's scale, which a brick wall or foliage
//!    has (`ROUGH`) — and not green once white-balanced (`GREEN`), which out-of-focus foliage is however smooth;
//!    the flood's seeds brighter than most of the frame, and the rest within `SKY_DROP` of the seeds' level.
//! 4. **Not the subject**, where a subject mask is given.
//! 5. **Light:** in daylight the sky is the light source, so a region darker than the frame on average is not
//!    taken for one; the mask is then empty rather than wrong.
//! 6. **Edges from the photo:** a pixel at a skyline is a mixture, and in linear light the compositing equation
//!    holds, I = α·S + (1 − α)·G, with S the sky's colour there and G the ground's. The depth map says where the
//!    sky certainly is and where it certainly isn't (a trimap: its edge, widened by `BAND` either way); S and G
//!    are the nearby certain colours' means (normalised convolution over `NEIGHBOURHOOD`); and α, the mask,
//!    is solved at each pixel of the band by least squares in RGB — the known-colour case of alpha matting. A
//!    skyline and a branch against the sky follow the picture rather than the depth map's blur.

use crate::RgbaF32;

/// How far from the farthest depth, as a share of the map's range, counts as the far plane for certain (the
/// flood's seeds), and how far it may still reach from them: the model's map is blurred across a skyline, so
/// the far plane's edge in it lies partway up the step to the nearer thing — hysteresis, as an edge detector
/// thresholds, puts the coarse edge back near the middle of that step.
pub const FAR: f32 = 0.06;
pub const FAR_REACH: f32 = 0.25;
/// The most texture sky has: the standard deviation of log₂ luminance over 5 × 5 texels of the depth map's size.
/// A clear or overcast sky, a blown one, the soft edge of a cloud, sit well under it; brick, foliage and
/// windows above.
pub const ROUGH: f32 = 0.2;
/// Green's largest share of a white-balanced colour (r + g + b) that sky has; foliage's is above it.
pub const GREEN: f32 = 0.38;
/// How far under the sky's own level (the median of the flood's seeds) the sky may reach, in stops. A diffuse
/// surface lit by an overcast sky returns at most its albedo times half the sky's radiance — a pale wall a couple
/// of stops under it — so a far, smooth, light facade below that is left out.
pub const SKY_DROP: f32 = 1.5;
/// The band of doubt either side of the depth map's edge, and the neighbourhood the sky's and the ground's
/// colours are taken from, as shares of the output's long edge.
pub const BAND: f32 = 0.02;
pub const NEIGHBOURHOOD: f32 = 0.04;

/// The coarse sky in the depth map's frame, 0 or 1: far, connected to the top edge, like sky, not the subject,
/// and no darker than the frame. `rgb` is the photo, white-balanced linear RGB, at the depth map's size;
/// `subject`, when given, the subject mask at that size too (0…255). `None` when no sky is found.
pub fn coarse(
    depth: &[u8],
    w: usize,
    h: usize,
    rgb: &[[f32; 3]],
    subject: Option<&[u8]>,
) -> Option<Vec<f32>> {
    let log_lum: Vec<f32> = rgb
        .iter()
        .map(|c| ((c[0] + c[1] + c[2]) / 3.0).max(1e-5).log2())
        .collect();
    let rough: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as isize, (i / w) as isize);
            let (mut s, mut s2, mut n) = (0f32, 0f32, 0f32);
            for j in -2..=2 {
                for k in -2..=2 {
                    let (xx, yy) = (
                        (x + k).clamp(0, w as isize - 1),
                        (y + j).clamp(0, h as isize - 1),
                    );
                    let v = log_lum[yy as usize * w + xx as usize];
                    s += v;
                    s2 += v * v;
                    n += 1.0;
                }
            }
            (s2 / n - (s / n).powi(2)).max(0.0).sqrt()
        })
        .collect();
    let green = |i: usize| {
        let c = rgb[i];
        c[1] / (c[0] + c[1] + c[2]).max(1e-9)
    };
    let mut sorted = log_lum.clone();
    sorted.sort_by(f32::total_cmp);
    let bright = sorted[sorted.len() / 2];
    let like_sky = |i: usize| rough[i] < ROUGH && green(i) < GREEN;
    let log_lum = &log_lum;
    let (lo, hi) = depth
        .iter()
        .fold((u8::MAX, u8::MIN), |(a, b), &d| (a.min(d), b.max(d)));
    if hi <= lo {
        return None;
    }
    let range = (hi - lo) as f32;
    let (seed, reach) = (lo as f32 + FAR * range, lo as f32 + FAR_REACH * range);
    let not_subject = |i: usize| subject.is_none_or(|s| s[i] < 128);
    let certain =
        |i: usize| depth[i] as f32 <= seed && not_subject(i) && like_sky(i) && log_lum[i] >= bright;
    let seeds: Vec<f32> = (0..w * h)
        .filter(|&i| certain(i))
        .map(|i| log_lum[i])
        .collect();
    if seeds.is_empty() {
        return None;
    }
    let mut level = seeds.clone();
    level.sort_by(f32::total_cmp);
    let floor = level[level.len() / 2] - SKY_DROP;
    let candidate =
        |i: usize| depth[i] as f32 <= reach && not_subject(i) && like_sky(i) && log_lum[i] >= floor;
    // Flood from the certain far plane along the top edge, through what is near enough to it and like sky.
    let mut sky = vec![false; w * h];
    let mut stack: Vec<usize> = (0..w).filter(|&x| certain(x)).collect();
    for &i in &stack {
        sky[i] = true;
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let mut push = |j: usize| {
            if !sky[j] && candidate(j) {
                sky[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            push(i - 1);
        }
        if x + 1 < w {
            push(i + 1);
        }
        if y > 0 {
            push(i - w);
        }
        if y + 1 < h {
            push(i + w);
        }
    }
    let n = sky.iter().filter(|&&s| s).count();
    if n == 0 {
        return None;
    }
    let mean = |pick: &dyn Fn(usize) -> bool| {
        let (s, c) = (0..w * h)
            .filter(|&i| pick(i))
            .fold((0f64, 0usize), |(s, c), i| (s + log_lum[i] as f64, c + 1));
        s / c.max(1) as f64
    };
    if mean(&|i| sky[i]) < mean(&|_| true) {
        return None;
    }
    Some(sky.iter().map(|&s| if s { 1.0 } else { 0.0 }).collect())
}

/// Box sums over a `w × h` field by summed-area table: for each pixel, the sum of `v` over the square of radius
/// `r` around it, clipped to the field.
fn box_sums(v: &[f64], w: usize, h: usize, r: usize) -> Vec<f64> {
    let mut sat = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0.0;
        for x in 0..w {
            row += v[y * w + x];
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let (x0, y0) = (x.saturating_sub(r), y.saturating_sub(r));
            let (x1, y1) = ((x + r + 1).min(w), (y + r + 1).min(h));
            sat[y1 * (w + 1) + x1] - sat[y0 * (w + 1) + x1] - sat[y1 * (w + 1) + x0]
                + sat[y0 * (w + 1) + x0]
        })
        .collect()
}

/// The coarse mask (`cw × ch`, 0…1) refined against `photo` — linear RGB at the output's size — into the mask
/// (0…255): certain sky and certain ground from the coarse mask, and α by the compositing equation between them.
pub fn refine(photo: &RgbaF32, coarse: &[f32], cw: usize, ch: usize) -> Vec<u8> {
    let (w, h) = (photo.width as usize, photo.height as usize);
    let long = w.max(h) as f32;
    let (band, near) = (
        ((BAND * long).round() as usize).max(1),
        ((NEIGHBOURHOOD * long).round() as usize).max(2),
    );
    // The coarse mask at the output's size, nearest texel.
    let sky: Vec<f64> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w * cw / w, i / w * ch / h);
            if coarse[y.min(ch - 1) * cw + x.min(cw - 1)] >= 0.5 {
                1.0
            } else {
                0.0
            }
        })
        .collect();
    // Certain: every pixel within the band is on the same side.
    let area = |i: usize| {
        let (x, y) = (i % w, i / w);
        ((x + band + 1).min(w) - x.saturating_sub(band)) as f64
            * ((y + band + 1).min(h) - y.saturating_sub(band)) as f64
    };
    let around = box_sums(&sky, w, h, band);
    let certain_sky: Vec<f64> = (0..w * h)
        .map(|i| if around[i] >= area(i) - 0.5 { 1.0 } else { 0.0 })
        .collect();
    let certain_ground: Vec<f64> = (0..w * h)
        .map(|i| if around[i] <= 0.5 { 1.0 } else { 0.0 })
        .collect();
    // The certain colours' local means, channel by channel.
    let mean = |which: &[f64]| -> Vec<[f64; 3]> {
        let n = box_sums(which, w, h, near);
        let sums: Vec<Vec<f64>> = (0..3)
            .map(|c| {
                let v: Vec<f64> = (0..w * h)
                    .map(|i| which[i] * photo.data[i * 4 + c] as f64)
                    .collect();
                box_sums(&v, w, h, near)
            })
            .collect();
        (0..w * h)
            .map(|i| {
                if n[i] < 0.5 {
                    [f64::NAN; 3]
                } else {
                    [sums[0][i] / n[i], sums[1][i] / n[i], sums[2][i] / n[i]]
                }
            })
            .collect()
    };
    let (s_mean, g_mean) = (mean(&certain_sky), mean(&certain_ground));
    (0..w * h)
        .map(|i| {
            let alpha = if certain_sky[i] > 0.5 {
                1.0
            } else if certain_ground[i] > 0.5 {
                0.0
            } else {
                let (s, g) = (s_mean[i], g_mean[i]);
                let p = [0, 1, 2].map(|c| photo.data[i * 4 + c] as f64);
                let d = [s[0] - g[0], s[1] - g[1], s[2] - g[2]];
                let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                if s[0].is_nan() || g[0].is_nan() || dd < 1e-12 {
                    sky[i]
                } else {
                    (((p[0] - g[0]) * d[0] + (p[1] - g[1]) * d[1] + (p[2] - g[2]) * d[2]) / dd)
                        .clamp(0.0, 1.0)
                }
            };
            (alpha * 255.0).round() as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A landscape: a bright sky over dark ground, a tree whose crown is a ragged disc against the sky, and a
    /// depth map at a quarter of the size, blurred, as a model's is. The refined mask is far nearer the true
    /// skyline than the depth map's own edge: measured, a fifth of its error at the skyline.
    #[test]
    fn the_sky_follows_the_photo_not_the_depth_map() {
        let (w, h) = (512usize, 384usize);
        let tree = |x: f32, y: f32| {
            let (dx, dy) = (x - 300.0, y - 140.0);
            let r = 70.0 + 12.0 * (dy.atan2(dx) * 9.0).sin();
            dx.hypot(dy) < r || ((290.0..310.0).contains(&x) && y > 140.0)
        };
        let is_sky = |x: usize, y: usize| y < 200 && !tree(x as f32, y as f32);
        let photo = RgbaF32 {
            width: w as u32,
            height: h as u32,
            data: (0..w * h)
                .flat_map(|i| {
                    let (x, y) = (i % w, i / w);
                    let v = if is_sky(x, y) {
                        0.5 + 0.2 * (1.0 - y as f32 / 200.0)
                    } else if tree(x as f32, y as f32) {
                        0.03 + 0.02 * ((x * 7 + y * 13) % 5) as f32 / 5.0
                    } else {
                        0.08 + 0.04 * ((x * 3 + y * 5) % 7) as f32 / 7.0
                    };
                    [v * 0.9, v, v * 1.1, 0.0]
                })
                .collect(),
        };
        // Depth at a quarter size: sky 0, the tree near, the ground nearer toward the bottom; then blurred.
        let (dw, dh) = (w / 4, h / 4);
        let sharp: Vec<f32> = (0..dw * dh)
            .map(|i| {
                let (x, y) = (i % dw * 4 + 2, i / dw * 4 + 2);
                if is_sky(x, y) {
                    0.0
                } else if tree(x as f32, y as f32) {
                    150.0
                } else {
                    100.0 + 155.0 * (y as f32 - 200.0).max(0.0) / 184.0
                }
            })
            .collect();
        let blurred: Vec<u8> = (0..dw * dh)
            .map(|i| {
                let (x, y) = ((i % dw) as isize, (i / dw) as isize);
                let mut s = 0.0;
                for j in -2..=2isize {
                    for k in -2..=2isize {
                        let (xx, yy) = (
                            (x + k).clamp(0, dw as isize - 1),
                            (y + j).clamp(0, dh as isize - 1),
                        );
                        s += sharp[yy as usize * dw + xx as usize];
                    }
                }
                (s / 25.0) as u8
            })
            .collect();
        let rgb: Vec<[f32; 3]> = (0..dw * dh)
            .map(|i| {
                let p = ((i / dw * 4 + 2) * w + i % dw * 4 + 2) * 4;
                [photo.data[p], photo.data[p + 1], photo.data[p + 2]]
            })
            .collect();
        let c = coarse(&blurred, dw, dh, &rgb, None).expect("a sky");
        let fine = refine(&photo, &c, dw, dh);
        // Errors against the truth, over the band within 12 px of the skyline, where they all are.
        let near_edge = |x: usize, y: usize| {
            let t = is_sky(x, y);
            (-12..=12isize).any(|d| {
                let (xx, yy) = (
                    (x as isize + d).clamp(0, w as isize - 1) as usize,
                    (y as isize + d).clamp(0, h as isize - 1) as usize,
                );
                is_sky(xx, y) != t || is_sky(x, yy) != t
            })
        };
        let (mut e_coarse, mut e_fine, mut n) = (0.0, 0.0, 0.0);
        for y in 0..h {
            for x in 0..w {
                if !near_edge(x, y) {
                    continue;
                }
                let truth = if is_sky(x, y) { 1.0 } else { 0.0 };
                let cv = c[(y / 4).min(dh - 1) * dw + (x / 4).min(dw - 1)];
                e_coarse += (cv - truth).abs() as f64;
                e_fine += (fine[y * w + x] as f32 / 255.0 - truth).abs() as f64;
                n += 1.0;
            }
        }
        let wrong = |f: &dyn Fn(usize, usize) -> bool| {
            (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .filter(|&(x, y)| f(x, y) != is_sky(x, y))
                .count()
        };
        eprintln!(
            "mean error at the skyline: depth map {:.3}, refined {:.3}; pixels on the wrong side: depth map {}, refined {}",
            e_coarse / n,
            e_fine / n,
            wrong(&|x, y| c[(y / 4).min(dh - 1) * dw + (x / 4).min(dw - 1)] > 0.5),
            wrong(&|x, y| fine[y * w + x] > 127)
        );
        assert!(
            e_fine < 0.3 * e_coarse,
            "refined {e_fine} against coarse {e_coarse}"
        );
        // Well inside: sky is sky and ground is ground.
        assert!(fine[20 * w + 50] > 240 && fine[300 * w + 50] < 15);
    }

    /// What is far but not like sky isn't taken for it: a far region darker than the room (an indoor wall), one
    /// that is green (foliage, however out of focus), one with texture (brick). The same region light, neutral
    /// and smooth is.
    #[test]
    fn far_is_not_enough() {
        let (w, h) = (64usize, 48usize);
        let depth: Vec<u8> = (0..w * h)
            .map(|i| if i / w < 10 { 0 } else { 200 })
            .collect();
        let scene = |top: &dyn Fn(usize) -> [f32; 3]| -> Vec<[f32; 3]> {
            (0..w * h)
                .map(|i| if i / w < 10 { top(i) } else { [0.2, 0.2, 0.2] })
                .collect()
        };
        let sky = scene(&|_| [0.6, 0.65, 0.8]);
        assert!(
            coarse(&depth, w, h, &sky, None).is_some(),
            "light, neutral, smooth: sky"
        );
        let dark = scene(&|_| [0.02, 0.02, 0.02]);
        assert!(coarse(&depth, w, h, &dark, None).is_none(), "dark");
        let green = scene(&|_| [0.3, 0.6, 0.2]);
        assert!(coarse(&depth, w, h, &green, None).is_none(), "green");
        let brick = scene(&|i| {
            if (i % w / 2 + i / w / 2) % 2 == 0 {
                [0.7, 0.5, 0.4]
            } else {
                [0.25, 0.15, 0.12]
            }
        });
        assert!(coarse(&depth, w, h, &brick, None).is_none(), "textured");
    }
}
