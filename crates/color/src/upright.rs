// SPDX-License-Identifier: Apache-2.0
//! Upright (DESIGN.md §15, "Upright"): straighten and keystone from the photo's own lines.
//!
//! Lines are found as the line-segment detector finds them (Grompone von Gioi, Jakubowicz, Morel and Randall,
//! "LSD: a fast line segment detector with a false detection control", PAMI 2010 — from the paper; its reference
//! code is AGPL and nothing of it is used): pixels whose gradient points the same way, grown into regions, each
//! region's line its principal axis. Segments within `NEAR_VERTICAL` of vertical are the picture's verticals,
//! within `NEAR_HORIZONTAL` of horizontal its horizontals. A photo's near-vertical edges are not all verticals —
//! fur, hair, a car's flank — but the true ones share a vanishing point, which the camera's tilt put there: each
//! family's vanishing point is found by RANSAC (Fischler and Bolles, 1981) over pairs of its segments, scored by the
//! length of the segments that point at it within `CONSENSUS`, and only those are solved for.
//!
//! The correction is the editor's own geometry — a virtual camera turned about its centre (`geometry`, K·R·K⁻¹),
//! then straightened — so the result is ordinary sliders the person can carry on adjusting. The sliders are
//! solved for numerically, through `geometry`'s own map, so the answer is whatever makes the mapped lines
//! vertical (and horizontal) in the picture that is actually shown, with no sign convention to get wrong:
//!
//! - **Level**: Straighten alone, so the verticals stand up on average.
//! - **Vertical**: Straighten and Vertical, so every vertical is vertical — converging verticals made parallel.
//! - **Full**: and Horizontal, so the horizontals are level too — a facade seen square on.
//!
//! The virtual camera's focal length is fixed (`geometry::FOCAL_DIAGONALS`); the real lens's, when the photo says
//! it, sets Aspect, so the corrected building is the shape the real camera would have shown turned square on.

use serde::{Deserialize, Serialize};

use crate::geometry::{self, Geometry};
use crate::mat::{self, M3};

/// How far from vertical, and from horizontal, a segment may lean and still be taken for one, degrees.
pub const NEAR_VERTICAL: f64 = 20.0;
pub const NEAR_HORIZONTAL: f64 = 15.0;
/// The shortest segment counted, as a share of the picture's diagonal.
pub const SHORTEST: f64 = 0.03;
/// How far a segment may point from a vanishing point and still be one of its lines, degrees.
pub const CONSENSUS: f64 = 1.0;
/// The least a vanishing point needs: this many segments, and this share of the family's length.
pub const LEAST_LINES: usize = 3;
pub const LEAST_SHARE: f64 = 0.25;

/// A segment in picture coordinates (0…1 of width and height).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub a: [f64; 2],
    pub b: [f64; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    Level,
    Vertical,
    Full,
}

/// Line segments in a luminance image (`w × h`, linear or log, any scale), by gradient-orientation region growing.
pub fn segments(lum: &[f32], w: usize, h: usize) -> Vec<Segment> {
    // Work at most 800 across: segments are long, and noise at full size only fragments them.
    let k = (w.max(h)).div_ceil(800).max(1);
    let (sw, sh) = (w / k, h / k);
    if sw < 8 || sh < 8 {
        return vec![];
    }
    let img: Vec<f32> = (0..sw * sh)
        .map(|i| {
            let (x, y) = (i % sw * k, i / sw * k);
            let mut s = 0.0;
            for yy in y..y + k {
                for xx in x..x + k {
                    s += lum[yy * w + xx].max(0.0);
                }
            }
            (s / (k * k) as f32 + 1e-4).log2()
        })
        .collect();
    // Gradients by 2 × 2 differences (LSD's choice: the least spatial spread).
    let mut mag = vec![0f32; sw * sh];
    let mut ang = vec![0f32; sw * sh];
    for y in 0..sh - 1 {
        for x in 0..sw - 1 {
            let (a, b, c, d) = (
                img[y * sw + x],
                img[y * sw + x + 1],
                img[(y + 1) * sw + x],
                img[(y + 1) * sw + x + 1],
            );
            let gx = (b + d - a - c) / 2.0;
            let gy = (c + d - a - b) / 2.0;
            mag[y * sw + x] = gx.hypot(gy);
            // The level line's direction: across the gradient.
            ang[y * sw + x] = gx.atan2(-gy);
        }
    }
    // Only gradients that stand out: LSD's ρ, a quantisation-noise floor, here a fixed share of a stop.
    let rho = 0.08f32;
    let tau = 22.5f32.to_radians();
    let mut order: Vec<usize> = (0..sw * sh).filter(|&i| mag[i] > rho).collect();
    order.sort_by(|&p, &q| mag[q].total_cmp(&mag[p]));
    let mut used = vec![false; sw * sh];
    let shortest = SHORTEST * ((sw * sw + sh * sh) as f64).sqrt();
    let mut out = vec![];
    let diff = |a: f32, b: f32| {
        // Angles of lines, so modulo π.
        let d = (a - b).rem_euclid(std::f32::consts::PI);
        d.min(std::f32::consts::PI - d)
    };
    for &seed in &order {
        if used[seed] {
            continue;
        }
        let mut region = vec![seed];
        used[seed] = true;
        let (mut sx, mut sy) = (ang[seed].cos() as f64, ang[seed].sin() as f64);
        // The region's angle as the mean of doubled angles, so opposite directions agree.
        let (mut s2x, mut s2y) = (
            (2.0 * ang[seed]).cos() as f64,
            (2.0 * ang[seed]).sin() as f64,
        );
        let mut head = 0;
        while head < region.len() {
            let p = region[head];
            head += 1;
            let (px, py) = (p % sw, p / sw);
            let theta = (s2y.atan2(s2x) / 2.0) as f32;
            for (dx, dy) in [
                (-1i32, -1i32),
                (0, -1),
                (1, -1),
                (-1, 0),
                (1, 0),
                (-1, 1),
                (0, 1),
                (1, 1),
            ] {
                let (nx, ny) = (px as i32 + dx, py as i32 + dy);
                if nx < 0 || ny < 0 || nx >= sw as i32 || ny >= sh as i32 {
                    continue;
                }
                let q = ny as usize * sw + nx as usize;
                if used[q] || mag[q] <= rho || diff(ang[q], theta) > tau {
                    continue;
                }
                used[q] = true;
                region.push(q);
                s2x += (2.0 * ang[q]).cos() as f64;
                s2y += (2.0 * ang[q]).sin() as f64;
                sx += ang[q].cos() as f64;
                sy += ang[q].sin() as f64;
            }
        }
        let _ = (sx, sy);
        if region.len() < 10 {
            continue;
        }
        // Principal axis, weighted by gradient.
        let (mut wsum, mut mx, mut my) = (0f64, 0f64, 0f64);
        for &p in &region {
            let m = mag[p] as f64;
            wsum += m;
            mx += m * (p % sw) as f64;
            my += m * (p / sw) as f64;
        }
        mx /= wsum;
        my /= wsum;
        let (mut cxx, mut cxy, mut cyy) = (0f64, 0f64, 0f64);
        for &p in &region {
            let m = mag[p] as f64;
            let (dx, dy) = ((p % sw) as f64 - mx, (p / sw) as f64 - my);
            cxx += m * dx * dx;
            cxy += m * dx * dy;
            cyy += m * dy * dy;
        }
        let t = 0.5 * (2.0 * cxy).atan2(cxx - cyy);
        let (ux, uy) = (t.cos(), t.sin());
        let (mut lo, mut hi, mut spread) = (f64::MAX, f64::MIN, 0f64);
        for &p in &region {
            let (dx, dy) = ((p % sw) as f64 - mx, (p / sw) as f64 - my);
            let along = dx * ux + dy * uy;
            lo = lo.min(along);
            hi = hi.max(along);
            spread += mag[p] as f64 * (dx * uy - dy * ux).powi(2);
        }
        let width = (spread / wsum).sqrt();
        if hi - lo < shortest || width > 1.5 {
            continue;
        }
        let at = |s: f64| {
            [
                (mx + s * ux + 0.5) * k as f64 / w as f64,
                (my + s * uy + 0.5) * k as f64 / h as f64,
            ]
        };
        out.push(Segment {
            a: at(lo),
            b: at(hi),
        });
    }
    out
}

/// A segment's direction in the isotropic frame (picture heights), and its length there.
fn direction(s: &Segment, aspect: f64) -> ([f64; 2], f64) {
    let d = [(s.b[0] - s.a[0]) * aspect, s.b[1] - s.a[1]];
    let len = d[0].hypot(d[1]);
    ([d[0] / len, d[1] / len], len)
}

/// The segments near vertical and near horizontal in the picture, as they are.
pub fn families(segs: &[Segment], aspect: f64) -> (Vec<Segment>, Vec<Segment>) {
    let (mut v, mut h) = (vec![], vec![]);
    for s in segs {
        let (d, _) = direction(s, aspect);
        let lean = d[0].abs().asin().to_degrees();
        if lean <= NEAR_VERTICAL {
            v.push(*s);
        } else if (90.0 - lean) <= NEAR_HORIZONTAL {
            h.push(*s);
        }
    }
    (v, h)
}

/// The segments of one family that share its strongest vanishing point: RANSAC over pairs, each pair's lines
/// meeting at a candidate (in the isotropic frame, homogeneous, so parallel lines meet at infinity), a segment its
/// line if it points at the candidate within `CONSENSUS`, the candidate scored by its lines' total length. Empty
/// when no candidate gathers `LEAST_LINES` and `LEAST_SHARE` of the family's length.
pub fn consensus(family: &[Segment], aspect: f64) -> Vec<Segment> {
    let iso = |p: [f64; 2]| [(p[0] - 0.5) * aspect, p[1] - 0.5, 1.0];
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let lines: Vec<[f64; 3]> = family.iter().map(|s| cross(iso(s.a), iso(s.b))).collect();
    let length = |s: &Segment| direction(s, aspect).1;
    let total: f64 = family.iter().map(length).sum();
    let tolerance = CONSENSUS.to_radians().sin();
    // Does segment `s` point at the homogeneous point `v`?
    let agrees = |s: &Segment, v: [f64; 3]| {
        let (a, b) = (iso(s.a), iso(s.b));
        let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let d = [b[0] - a[0], b[1] - a[1]];
        // The direction from the midpoint to v (a direction itself when v is at infinity).
        let t = if v[2].abs() < 1e-12 {
            [v[0], v[1]]
        } else {
            [v[0] / v[2] - m[0], v[1] / v[2] - m[1]]
        };
        let (ld, lt) = (d[0].hypot(d[1]), t[0].hypot(t[1]));
        lt > 1e-12 && ((d[0] * t[1] - d[1] * t[0]) / (ld * lt)).abs() < tolerance
    };
    let n = family.len();
    let mut best: (f64, Vec<usize>) = (0.0, vec![]);
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut pick = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n.max(1) as u64) as usize
    };
    if n >= 2 {
        for _ in 0..(n * n).min(2000) {
            let (i, j) = (pick(), pick());
            if i == j {
                continue;
            }
            let v = cross(lines[i], lines[j]);
            if v.iter().all(|c| c.abs() < 1e-15) {
                continue;
            }
            let inliers: Vec<usize> = (0..n).filter(|&k| agrees(&family[k], v)).collect();
            let score: f64 = inliers.iter().map(|&k| length(&family[k])).sum();
            if score > best.0 {
                best = (score, inliers);
            }
        }
    }
    if best.1.len() < LEAST_LINES || best.0 < LEAST_SHARE * total {
        return vec![];
    }
    best.1.into_iter().map(|k| family[k]).collect()
}

/// Each segment's angle from where it should be — vertical or horizontal — once mapped through `g`, weighted by
/// its length in the picture: the cost the solve minimises (sum of squared sines, so the long ones count).
fn cost(g: &Geometry, aspect: f64, verticals: &[Segment], horizontals: &[Segment]) -> f64 {
    // Picture → plane, isotropic.
    let to_plane: M3 = mat::inverse(&geometry::plane_to_picture(g, aspect));
    let iso = geometry::to_iso(aspect);
    let through = |p: [f64; 2]| geometry::map(&to_plane, geometry::map(&iso, p));
    // Weighted by the segment's length in the picture, which the sliders don't change: weighting by the mapped
    // length would let the solve lower the cost by shrinking lines, which keystone does.
    let off = |s: &Segment, vertical: bool| {
        let (a, b) = (through(s.a), through(s.b));
        let d = [b[0] - a[0], b[1] - a[1]];
        let len = d[0].hypot(d[1]).max(1e-12);
        let sine = if vertical { d[0] / len } else { d[1] / len };
        direction(s, aspect).1 * sine * sine
    };
    verticals.iter().map(|s| off(s, true)).sum::<f64>()
        + horizontals.iter().map(|s| off(s, false)).sum::<f64>()
}

/// Nelder–Mead over a few of the geometry's sliders, from where they are.
fn minimise(f: impl Fn(&[f64]) -> f64, start: Vec<f64>, step: f64) -> Vec<f64> {
    let n = start.len();
    let mut pts: Vec<Vec<f64>> = vec![start.clone()];
    for i in 0..n {
        let mut p = start.clone();
        p[i] += step;
        pts.push(p);
    }
    let mut vals: Vec<f64> = pts.iter().map(|p| f(p)).collect();
    for _ in 0..400 * n {
        let mut idx: Vec<usize> = (0..=n).collect();
        idx.sort_by(|&a, &b| vals[a].total_cmp(&vals[b]));
        pts = idx.iter().map(|&i| pts[i].clone()).collect();
        vals = idx.iter().map(|&i| vals[i]).collect();
        if (vals[n] - vals[0]).abs() < 1e-14 {
            break;
        }
        let centroid: Vec<f64> = (0..n)
            .map(|j| pts[..n].iter().map(|p| p[j]).sum::<f64>() / n as f64)
            .collect();
        let along = |t: f64| -> Vec<f64> {
            (0..n)
                .map(|j| centroid[j] + t * (pts[n][j] - centroid[j]))
                .collect()
        };
        let r = along(-1.0);
        let fr = f(&r);
        if fr < vals[0] {
            let e = along(-2.0);
            let fe = f(&e);
            (pts[n], vals[n]) = if fe < fr { (e, fe) } else { (r, fr) };
        } else if fr < vals[n - 1] {
            (pts[n], vals[n]) = (r, fr);
        } else {
            let c = along(0.5);
            let fc = f(&c);
            if fc < vals[n] {
                (pts[n], vals[n]) = (c, fc);
            } else {
                for i in 1..=n {
                    pts[i] = (0..n)
                        .map(|j| pts[0][j] + 0.5 * (pts[i][j] - pts[0][j]))
                        .collect();
                    vals[i] = f(&pts[i]);
                }
            }
        }
    }
    let best = (0..=n)
        .min_by(|&a, &b| vals[a].total_cmp(&vals[b]))
        .unwrap();
    pts[best].clone()
}

/// Aspect that makes the virtual camera's correction the shape the real lens's would be, for the vertical tilt
/// `vertical` the solve chose: with the real focal length `focal` (picture diagonals), find the tilt that sends
/// the same vanishing point to infinity, and compare the two corrections' vertical-to-horizontal scale at the
/// centre. `None` when the photo doesn't give its focal length.
fn aspect_for(g: &Geometry, aspect: f64, focal: Option<f64>) -> Option<f64> {
    let focal = focal?;
    if g.vertical == 0.0 {
        return Some(0.0);
    }
    let virt = geometry::FOCAL_DIAGONALS;
    let tilt = (g.vertical.clamp(-100.0, 100.0) / 100.0 * geometry::MAX_TILT_DEG).to_radians();
    // A vertical's vanishing point lies at y = f·cot(tilt) from the centre (in diagonals) for the virtual camera;
    // the real camera, seeing the same point, was tilted by atan(f_real / y).
    let y = virt / tilt.tan();
    let real = (focal / y).atan();
    // A rotation homography about the horizontal axis scales the centre by 1 / cos²(tilt) vertically and
    // 1 / cos(tilt) across, so its vertical-over-horizontal stretch there is 1 / cos(tilt). The real camera's
    // correction stretches by 1 / cos(real), the virtual one's by 1 / cos(tilt): Aspect (the output's height
    // scaled by 2^(aspect·ASPECT_STOPS)) makes up the difference.
    let ratio = tilt.cos() / real.cos();
    let _ = aspect;
    Some((ratio.log2() / geometry::ASPECT_STOPS * 100.0).clamp(-100.0, 100.0))
}

/// Upright `g` from the segments found in the picture (`aspect` is its width ÷ height): the sliders `mode` asks
/// for solved so the lines stand true, the rest of `g` kept, the crop refitted. `focal` is the lens's focal length
/// in picture diagonals, when known, for Aspect. `None` when the picture hasn't the lines the mode needs.
pub fn solve(
    g: &Geometry,
    aspect: f64,
    segs: &[Segment],
    mode: Mode,
    focal: Option<f64>,
) -> Option<Geometry> {
    let (verticals, horizontals) = families(segs, aspect);
    let need_h = mode == Mode::Full;
    let verticals = consensus(&verticals, aspect);
    let horizontals = if need_h {
        consensus(&horizontals, aspect)
    } else {
        vec![]
    };
    if verticals.len() < if mode == Mode::Level { 1 } else { 2 }
        || (need_h && horizontals.len() < 2)
    {
        return None;
    }
    let horizontals: &[Segment] = if need_h { &horizontals } else { &[] };
    let base = Geometry {
        angle: 0.0,
        vertical: 0.0,
        horizontal: 0.0,
        aspect: 0.0,
        ..g.clone()
    };
    let with = |p: &[f64]| {
        let mut t = base.clone();
        t.angle = p[0];
        if mode != Mode::Level {
            t.vertical = p[1];
        }
        if need_h {
            t.horizontal = p[2];
        }
        t
    };
    // Out of range is out of the slider: a steep penalty.
    let f = |p: &[f64]| {
        let over = (p[0].abs() - geometry::MAX_ANGLE_DEG).max(0.0)
            + p.iter()
                .skip(1)
                .map(|v| (v.abs() - 100.0).max(0.0))
                .sum::<f64>();
        cost(&with(p), aspect, &verticals, horizontals) + over * over
    };
    let n = match mode {
        Mode::Level => 1,
        Mode::Vertical => 2,
        Mode::Full => 3,
    };
    let mut p = minimise(f, vec![0.0; n], 5.0);
    // Again from there, smaller: Nelder–Mead settles faster than it converges.
    p = minimise(f, p, 0.5);
    let mut out = with(&p);
    if mode != Mode::Level
        && let Some(a) = aspect_for(&out, aspect, focal)
    {
        out.aspect = a;
    }
    out.angle = (out.angle * 100.0).round() / 100.0;
    out.vertical = (out.vertical * 10.0).round() / 10.0;
    out.horizontal = (out.horizontal * 10.0).round() / 10.0;
    out.aspect = out.aspect.round();
    Some(geometry::fit(&out, g, aspect))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture of a known geometry's inverse: a grid of vertical and horizontal edges on a facade, seen through
    /// `g`'s own map backwards, so the true answer is `g`.
    fn facade(g: &Geometry, w: usize, h: usize) -> Vec<f32> {
        let a = w as f64 / h as f64;
        let to_plane: M3 = mat::inverse(&geometry::plane_to_picture(g, a));
        let iso = geometry::to_iso(a);
        let iso_inv = mat::inverse(&iso);
        // Supersampled 4 × 4, as a lens and a sensor's pixels average: unaliased edges, whose angles a detector
        // can measure without the staircase pulling them toward the pixel grid.
        let shade = |p: [f64; 2]| {
            let q = geometry::map(&iso_inv, geometry::map(&to_plane, geometry::map(&iso, p)));
            // Windows: bright panels on a darker wall, in plane coordinates.
            let (u, v) = ((q[0] * 9.0).rem_euclid(1.0), (q[1] * 7.0).rem_euclid(1.0));
            if (0.2..0.8).contains(&u) && (0.25..0.75).contains(&v) {
                0.6
            } else {
                0.08
            }
        };
        (0..w * h)
            .map(|i| {
                let mut s = 0.0;
                for j in 0..4 {
                    for k in 0..4 {
                        s += shade([
                            ((i % w) as f64 + (k as f64 + 0.5) / 4.0) / w as f64,
                            ((i / w) as f64 + (j as f64 + 0.5) / 4.0) / h as f64,
                        ]);
                    }
                }
                (s / 16.0) as f32
            })
            .collect()
    }

    #[test]
    fn a_tilted_facade_is_stood_up() {
        let (w, h) = (900usize, 600usize);
        let a = w as f64 / h as f64;
        let truth = Geometry {
            angle: 3.0,
            vertical: 35.0,
            horizontal: -15.0,
            ..Default::default()
        };
        let lum = facade(&truth, w, h);
        let segs = segments(&lum, w, h);
        let (v, hz) = families(&segs, a);
        eprintln!(
            "{} segments: {} vertical, {} horizontal",
            segs.len(),
            v.len(),
            hz.len()
        );
        assert!(v.len() > 10 && hz.len() > 10);
        let level = solve(&Geometry::default(), a, &segs, Mode::Level, None).unwrap();
        let vert = solve(&Geometry::default(), a, &segs, Mode::Vertical, None).unwrap();
        let full = solve(&Geometry::default(), a, &segs, Mode::Full, None).unwrap();
        eprintln!(
            "level {:.2}°; vertical {:.2}° {:.1}; full {:.2}° {:.1} {:.1} (truth {:.2}° {} {})",
            level.angle,
            vert.angle,
            vert.vertical,
            full.angle,
            full.vertical,
            full.horizontal,
            truth.angle,
            truth.vertical,
            truth.horizontal
        );
        eprintln!(
            "cost at the truth {:.6}, at the solve {:.6}",
            cost(&truth, a, &v, &hz),
            cost(&full, a, &v, &hz)
        );
        assert!((full.angle - truth.angle).abs() < 0.3, "{full:?}");
        assert!((full.vertical - truth.vertical).abs() < 2.0, "{full:?}");
        assert!((full.horizontal - truth.horizontal).abs() < 2.0, "{full:?}");
        // Vertical alone makes the verticals true: its cost on them is near nothing.
        assert!(cost(&vert, a, &v, &[]) < 1e-3 * v.len() as f64);
        // Every facade line agrees with its family's vanishing point.
        assert!(consensus(&v, a).len() as f64 > 0.9 * v.len() as f64);
        // And the crop was refitted inside the corrected picture.
        assert!(full.crop != Geometry::default().crop);
    }

    /// Near-vertical edges that share no vanishing point — strands of fur, at random leans — are not verticals:
    /// a facade's lines among them are found, and alone they give no answer.
    #[test]
    fn stray_lines_are_left_out() {
        let a = 1.5;
        let truth = Geometry {
            vertical: 30.0,
            ..Default::default()
        };
        let (w, h) = (900usize, 600usize);
        let facade_lines: Vec<Segment> = {
            let lum = facade(&truth, w, h);
            let (v, _) = families(&segments(&lum, w, h), a);
            v
        };
        let mut s = 99u64;
        let mut r = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s % 10_000) as f64 / 10_000.0
        };
        let strays: Vec<Segment> = (0..60)
            .map(|_| {
                let (x, y, lean, len) = (r(), r(), (r() - 0.5) * 0.6, 0.05 + 0.1 * r());
                Segment {
                    a: [x, y],
                    b: [x + lean * len / a, y + len],
                }
            })
            .collect();
        assert!(
            consensus(&strays, a).is_empty(),
            "strays alone share no vanishing point"
        );
        let mixed: Vec<Segment> = facade_lines.iter().chain(&strays).copied().collect();
        let kept = consensus(&mixed, a);
        let from_facade = kept.iter().filter(|k| facade_lines.contains(k)).count();
        eprintln!(
            "kept {} of {} ({} from the facade)",
            kept.len(),
            mixed.len(),
            from_facade
        );
        assert!(from_facade as f64 > 0.9 * facade_lines.len() as f64);
        assert!(kept.len() - from_facade < 6, "few strays agree by chance");
        let solved = solve(&Geometry::default(), a, &mixed, Mode::Vertical, None).unwrap();
        assert!((solved.vertical - truth.vertical).abs() < 2.0, "{solved:?}");
    }

    /// A photo with no lines to speak of gives no answer rather than a wrong one.
    #[test]
    fn no_lines_no_answer() {
        let (w, h) = (300usize, 200usize);
        let lum: Vec<f32> = (0..w * h)
            .map(|i| 0.2 + 0.05 * ((i * 7919) % 13) as f32 / 13.0)
            .collect();
        let segs = segments(&lum, w, h);
        assert!(solve(&Geometry::default(), 1.5, &segs, Mode::Vertical, None).is_none());
    }

    /// With the real lens longer than the virtual camera's, the same vertical correction needs the picture made
    /// taller (the real tilt was smaller, so less foreshortening to undo) — Aspect positive — and shorter for a
    /// wider lens.
    #[test]
    fn aspect_follows_the_real_lens() {
        let g = Geometry {
            vertical: 40.0,
            ..Default::default()
        };
        let long = aspect_for(&g, 1.5, Some(2.0)).unwrap();
        let wide = aspect_for(&g, 1.5, Some(0.5)).unwrap();
        let same = aspect_for(&g, 1.5, Some(geometry::FOCAL_DIAGONALS)).unwrap();
        eprintln!("aspect: long lens {long}, wide {wide}, virtual's own {same}");
        assert!(same.abs() < 1e-9);
        assert!(long > 0.0 && wide < 0.0, "long {long}, wide {wide}");
    }
}
