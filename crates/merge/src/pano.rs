// SPDX-License-Identifier: Apache-2.0
//! Panoramas: frames taken by turning the camera about one point, joined on a sphere into one linear image
//! (docs/research/multi-frame-and-denoise.md, "Panoramas").
//!
//! Written from the papers, nothing ported. The pipeline is Brown and Lowe's ("Automatic panoramic image
//! stitching using invariant features", IJCV 2007):
//!
//! 1. **Matches** between every pair of frames (`align`'s features and RANSAC homographies, on proxies), kept
//!    when the inliers pass Brown and Lowe's test; the reference is the frame with the most.
//! 2. **A camera that only turns:** each frame is a rotation of one pinhole lens. Focal length starts from the
//!    homographies (Shum and Szeliski, "Construction of panoramic mosaics with global and local alignment",
//!    IJCV 2000 — the two conditions a rotation's columns and rows must meet), rotations from a spanning tree
//!    of the strongest pairs (Horn's absolute orientation on the matched rays), then **bundle adjustment** —
//!    Levenberg–Marquardt over every rotation and the focal length, minimising the distance between matched
//!    rays, with a Huber loss. The lens's distortion is the file's or lensfun's profile when there is one;
//!    otherwise the adjustment fits one radial term.
//! 3. **Wave correction:** the horizon is level when the cameras' horizontal axes lie in one plane, whose
//!    normal is up (Brown and Lowe, §5).
//! 4. **Exposure:** raw is linear, so each frame differs from the reference by one gain, exactly — solved by
//!    least squares on log luminance sampled across every overlap, together with the lens's vignetting when no
//!    profile gives it (Goldman and Chen, "Vignette and exposure calibration and compensation", ICCV 2005,
//!    with the falloff as an even polynomial in radius).
//! 5. **Seams and blending:** each direction is taken from the frame whose optical axis is nearest it — the
//!    least lens aberration — and the seams are hidden by multi-band blending (Burt and Adelson, "A
//!    multiresolution spline with application to image mosaics", ACM ToG 1983): every frequency band blended
//!    over a width proportional to its wavelength. Blending is in linear light, which keeps the noise
//!    unbiased; with gains and vignetting removed the frames agree, and where they agree a multi-band blend
//!    returns them unchanged in any domain.
//!
//! Coordinates: camera space has x right, y down, z forward along the optical axis, in the sensor's own
//! orientation; a frame's rotation `R` takes world directions to its camera's. Sensor positions are measured
//! from the optical centre in half-diagonals, so one lens serves the proxies and the full frames alike.

use crate::align::{self, Grey, H};
use crate::blend::{self, Bands, Plane};
use crate::linalg::{self, M3, V3};
use rayon::prelude::*;

/// Curves are sampled evenly over `0 ..= R_MAX` half-diagonals (as `sidecar_color::lens` samples them).
pub const R_MAX: f64 = 1.5;

/// What is known about the lens before stitching: from the file's own correction or lensfun's.
#[derive(Clone, Debug, Default)]
pub struct Optics {
    /// The optical centre, 0…1 of the frame's width and height. `[0.5, 0.5]` when nothing says otherwise.
    pub center: [f64; 2],
    /// Source radius ÷ corrected radius (below 1 corrects barrel), sampled over `0 ..= R_MAX`; empty when
    /// nothing is known, and the bundle adjustment fits one radial term instead.
    pub distortion: Vec<f64>,
    /// The red and the blue channel's source radius ÷ green's, likewise; empty for none.
    pub ca: [Vec<f64>; 2],
    /// The gain that flattens vignetting, by the radius a pixel is read from; empty, and the exposure solve
    /// fits it.
    pub vignetting: Vec<f64>,
}

fn curve(s: &[f64], r: f64) -> f64 {
    if s.is_empty() {
        return 1.0;
    }
    let x = (r / R_MAX * (s.len() - 1) as f64).clamp(0.0, (s.len() - 1) as f64);
    let i = (x.floor() as usize).min(s.len().saturating_sub(2));
    let t = x - i as f64;
    if s.len() == 1 {
        s[0]
    } else {
        s[i] + (s[i + 1] - s[i]) * t
    }
}

/// The lens as solved: the optics plus a focal length (in half-diagonals), the fitted radial term, and the
/// fitted vignetting (log₂ of the flattening gain, as a polynomial in r², r⁴, r⁶).
#[derive(Clone, Debug)]
pub struct Lens {
    pub optics: Optics,
    pub focal: f64,
    pub k1: f64,
    pub vignette: [f64; 3],
}

impl Lens {
    /// Source radius ÷ corrected radius for channel `ch` (0 red, 1 green, 2 blue).
    fn factor(&self, r: f64, ch: usize) -> f64 {
        let mut s = curve(&self.optics.distortion, r) * (1.0 + self.k1 * r * r);
        if ch != 1 {
            s *= curve(&self.optics.ca[ch / 2], r);
        }
        s
    }

    /// A corrected position to where channel `ch` reads it on the sensor.
    pub fn distort(&self, uc: [f64; 2], ch: usize) -> [f64; 2] {
        let s = self.factor(uc[0].hypot(uc[1]), ch);
        [uc[0] * s, uc[1] * s]
    }

    /// A sensor position (green) to its corrected position: the radius by bisection, since `r · factor(r)`
    /// rises with `r` for any usable lens.
    pub fn undistort(&self, us: [f64; 2]) -> [f64; 2] {
        let rs = us[0].hypot(us[1]);
        if rs < 1e-12 {
            return us;
        }
        let (mut lo, mut hi) = (0.0, 2.0 * R_MAX);
        for _ in 0..48 {
            let mid = 0.5 * (lo + hi);
            if mid * self.factor(mid, 1) < rs {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let rc = 0.5 * (lo + hi);
        [us[0] * rc / rs, us[1] * rc / rs]
    }

    /// The unit direction, in camera space, that a sensor position sees.
    pub fn ray(&self, us: [f64; 2]) -> V3 {
        let c = self.undistort(us);
        linalg::normalize([c[0], c[1], self.focal])
    }

    /// Where on the sensor channel `ch` sees the camera-space direction `d`; `None` behind the camera or far
    /// outside any frame.
    pub fn project(&self, d: V3, ch: usize) -> Option<[f64; 2]> {
        if d[2] <= 1e-9 {
            return None;
        }
        let uc = [d[0] / d[2] * self.focal, d[1] / d[2] * self.focal];
        (uc[0].hypot(uc[1]) < 2.0 * R_MAX).then(|| self.distort(uc, ch))
    }

    /// The gain that flattens vignetting at sensor radius `rs`.
    pub fn flatten(&self, rs: f64) -> f64 {
        let r2 = rs * rs;
        let [a, b, c] = self.vignette;
        curve(&self.optics.vignetting, rs) * (a * r2 + b * r2 * r2 + c * r2 * r2 * r2).exp2()
    }
}

/// Pixel geometry of one frame at one resolution: the map between pixel indices and sensor positions.
#[derive(Clone, Copy, Debug)]
pub struct Sensor {
    pub w: usize,
    pub h: usize,
    pub center: [f64; 2],
    pub half_diag: f64,
}

impl Sensor {
    pub fn new(w: usize, h: usize, optics: &Optics) -> Self {
        Self {
            w,
            h,
            center: [optics.center[0] * w as f64, optics.center[1] * h as f64],
            half_diag: 0.5 * (w as f64).hypot(h as f64),
        }
    }

    /// Pixel index (centres at integers) to sensor position.
    pub fn to_unit(&self, p: [f64; 2]) -> [f64; 2] {
        [
            (p[0] + 0.5 - self.center[0]) / self.half_diag,
            (p[1] + 0.5 - self.center[1]) / self.half_diag,
        ]
    }

    pub fn to_pixel(&self, u: [f64; 2]) -> [f64; 2] {
        [
            self.center[0] + u[0] * self.half_diag - 0.5,
            self.center[1] + u[1] * self.half_diag - 0.5,
        ]
    }

    pub fn inside(&self, p: [f64; 2], margin: f64) -> bool {
        p[0] >= margin
            && p[1] >= margin
            && p[0] <= self.w as f64 - 1.0 - margin
            && p[1] <= self.h as f64 - 1.0 - margin
    }
}

/// How the sphere of directions is laid flat. World space after wave correction: y down, z the panorama's
/// centre direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    /// A pinhole's own: straight lines stay straight; only for fields under about 100°.
    Rectilinear,
    /// Longitude across, height on a cylinder down: a single row of any width.
    Cylindrical,
    /// Longitude and latitude (equirectangular): any coverage, to the full sphere — what an HDRI is.
    Spherical,
}

impl Projection {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "rectilinear" => Self::Rectilinear,
            "cylindrical" => Self::Cylindrical,
            "spherical" | "equirectangular" => Self::Spherical,
            _ => return None,
        })
    }

    /// Direction to plane position (radians or tangents).
    pub fn forward(self, d: V3) -> Option<[f64; 2]> {
        let [x, y, z] = d;
        match self {
            Self::Rectilinear => (z > 1e-6).then(|| [x / z, y / z]),
            Self::Cylindrical => {
                let r = x.hypot(z);
                (r > 1e-9).then(|| [x.atan2(z), y / r])
            }
            Self::Spherical => Some([x.atan2(z), y.atan2(x.hypot(z))]),
        }
    }

    pub fn inverse(self, p: [f64; 2]) -> V3 {
        match self {
            Self::Rectilinear => linalg::normalize([p[0], p[1], 1.0]),
            Self::Cylindrical => linalg::normalize([p[0].sin(), p[1], p[0].cos()]),
            Self::Spherical => {
                let c = p[1].cos();
                [c * p[0].sin(), p[1].sin(), c * p[0].cos()]
            }
        }
    }

    fn wraps(self) -> bool {
        self != Self::Rectilinear
    }
}

/// One frame as the solver sees it.
pub struct Input {
    /// Full-resolution size, sensor orientation.
    pub w: usize,
    pub h: usize,
    /// Log₂ luminance at proxy size, sensor orientation, every frame white-balanced alike.
    pub proxy: Grey,
    /// Which way is up in the displayed picture, as a direction on the sensor (from the EXIF orientation).
    pub up: [f64; 2],
    /// What the frame let in by its EXIF (shutter × ISO ÷ f²): the exposure solve's prior.
    pub light: f64,
    /// The linear level above which the proxy is no longer trusted for the exposure solve: near the sensor's
    /// saturation for a single raw (0.6), the merged range's top for an HDR merge.
    pub ceiling: f32,
}

/// Matched sensor positions between two frames.
#[derive(Clone, Debug)]
pub struct Link {
    pub i: usize,
    pub j: usize,
    pub a: Vec<[f64; 2]>,
    pub b: Vec<[f64; 2]>,
    pub inliers: usize,
}

/// The solved panorama.
#[derive(Clone, Debug)]
pub struct Solution {
    pub lens: Lens,
    /// World → camera, per input; `None` for a frame that joined nothing.
    pub rotations: Vec<Option<M3>>,
    /// What multiplies each frame into the reference's radiance.
    pub gains: Vec<f64>,
    pub reference: usize,
    pub links: Vec<Link>,
    /// Root-mean-square distance between matched rays after adjustment, in proxy pixels.
    pub rms: f64,
    /// Focal length from the homographies, before adjustment, in half-diagonals.
    pub focal_initial: f64,
}

/// Brown and Lowe's test that a pair's inliers are not chance: more than 8 + 0.3 of its matches.
fn trusted(inliers: usize, matches: usize) -> bool {
    inliers >= 20 && inliers as f64 > 8.0 + 0.3 * matches as f64
}

/// Focal length in half-diagonals from a homography between sensor positions of two frames of one lens that
/// turned about its centre: `H ∝ K₁ R K₀⁻¹`, so `K₁⁻¹ H K₀` must have orthogonal columns of equal length (giving
/// f₁) and orthogonal rows of equal length (giving f₀). Each pair of conditions gives an estimate; the better
/// conditioned is kept. `None` when neither is positive.
pub fn focal_from_homography(h: &H) -> Option<f64> {
    let pick = |(n1, d1): (f64, f64), (n2, d2): (f64, f64)| -> Option<f64> {
        let (v1, v2) = (n1 / d1, n2 / d2);
        let ok1 = d1.abs() > 1e-12 && v1 > 0.0;
        let ok2 = d2.abs() > 1e-12 && v2 > 0.0;
        match (ok1, ok2) {
            (true, true) => Some(if d1.abs() > d2.abs() { v1 } else { v2 }.sqrt()),
            (true, false) => Some(v1.sqrt()),
            (false, true) => Some(v2.sqrt()),
            _ => None,
        }
    };
    let [[a, b, c], [d, e, f], [g, k, _]] = *h;
    // Columns: orthogonal, and of equal length.
    let f1 = pick(
        (-(a * b + d * e), g * k),
        (a * a + d * d - b * b - e * e, k * k - g * g),
    );
    // Rows: orthogonal, and of equal length.
    let f0 = pick(
        (-c * f, a * d + b * e),
        (f * f - c * c, a * a + b * b - d * d - e * e),
    );
    match (f0, f1) {
        (Some(x), Some(y)) => Some((x * y).sqrt()),
        (x, y) => x.or(y),
    }
}

/// Every pair's matches, as sensor positions.
pub fn link(inputs: &[Input], optics: &Optics) -> (Vec<Link>, Vec<f64>) {
    let feats: Vec<Vec<align::Feature>> = inputs
        .par_iter()
        .map(|f| align::features(&f.proxy, 2000))
        .collect();
    let pairs: Vec<(usize, usize)> = (0..inputs.len())
        .flat_map(|i| (i + 1..inputs.len()).map(move |j| (i, j)))
        .collect();
    let found: Vec<Option<(Link, Option<f64>)>> = pairs
        .par_iter()
        .map(|&(i, j)| {
            let (pa, pb) = (&inputs[i].proxy, &inputs[j].proxy);
            let m = align::matches(&feats[i], &feats[j], 0.8);
            let scale = (pa.w.max(pa.h) as f64 / 1600.0).max(0.5);
            let a = align::homography(&feats[i], &feats[j], &m, 3.0 * scale)?;
            if !trusted(a.inliers, a.matches) {
                return None;
            }
            let (si, sj) = (
                Sensor::new(pa.w, pa.h, optics),
                Sensor::new(pb.w, pb.h, optics),
            );
            let mut link = Link {
                i,
                j,
                a: vec![],
                b: vec![],
                inliers: a.inliers,
            };
            for &(x, y) in &m {
                let (p, q) = (&feats[i][x], &feats[j][y]);
                let pp = align::apply(&a.h, [p.x as f64, p.y as f64]);
                if (pp[0] - q.x as f64).hypot(pp[1] - q.y as f64) >= 3.0 * scale {
                    continue;
                }
                let pa_ = [p.x as f64, p.y as f64];
                if let Some(qb) = align::refine(pa, pb, &a.h, pa_) {
                    link.a.push(si.to_unit(pa_));
                    link.b.push(sj.to_unit(qb));
                }
            }
            // The homography between sensor positions: Tⱼ · H · Tᵢ⁻¹.
            let t = |s: &Sensor| -> H {
                let k = 1.0 / s.half_diag;
                [
                    [k, 0.0, (0.5 - s.center[0]) * k],
                    [0.0, k, (0.5 - s.center[1]) * k],
                    [0.0, 0.0, 1.0],
                ]
            };
            let ti_inv: H = [
                [si.half_diag, 0.0, si.center[0] - 0.5],
                [0.0, si.half_diag, si.center[1] - 0.5],
                [0.0, 0.0, 1.0],
            ];
            let hn = linalg::mul(&t(&sj), &linalg::mul(&a.h, &ti_inv));
            Some((link, focal_from_homography(&hn)))
        })
        .collect();
    let mut links = vec![];
    let mut focals = vec![];
    for (l, f) in found.into_iter().flatten() {
        links.push(l);
        focals.extend(f);
    }
    (links, focals)
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

/// The frames that join the reference through trusted links, and a first rotation for each: a maximum
/// spanning tree by inlier count, grown from the reference, each new frame placed by Horn's absolute
/// orientation on the rays it shares with the frame it joins through.
fn spanning(n: usize, reference: usize, links: &[Link], lens: &Lens) -> Vec<Option<M3>> {
    let mut rot: Vec<Option<M3>> = vec![None; n];
    rot[reference] = Some(linalg::I3);
    loop {
        let best = links
            .iter()
            .filter(|l| rot[l.i].is_some() != rot[l.j].is_some())
            .max_by_key(|l| l.inliers);
        let Some(l) = best else { break };
        let (known, new, pk, pn) = if rot[l.i].is_some() {
            (l.i, l.j, &l.a, &l.b)
        } else {
            (l.j, l.i, &l.b, &l.a)
        };
        let rk = linalg::transpose(&rot[known].unwrap());
        let w: Vec<V3> = pk.iter().map(|&u| linalg::mulv(&rk, lens.ray(u))).collect();
        let v: Vec<V3> = pn.iter().map(|&u| lens.ray(u)).collect();
        rot[new] = Some(linalg::absolute_orientation(&w, &v));
    }
    rot
}

/// The bundle adjustment's free parameters, laid out flat: three per frame other than the reference (a
/// rotation vector applied before the frame's starting rotation), log focal length, and k₁ when fitted.
struct Params<'a> {
    free: Vec<usize>,
    start: &'a [Option<M3>],
    fit_k1: bool,
    base: Lens,
}

impl Params<'_> {
    fn len(&self) -> usize {
        3 * self.free.len() + 1 + self.fit_k1 as usize
    }

    fn unpack(&self, x: &[f64]) -> (Vec<Option<M3>>, Lens) {
        let mut rot = self.start.to_vec();
        for (k, &i) in self.free.iter().enumerate() {
            let w = [x[3 * k], x[3 * k + 1], x[3 * k + 2]];
            rot[i] = Some(linalg::mul(&linalg::rodrigues(w), &self.start[i].unwrap()));
        }
        let mut lens = self.base.clone();
        let o = 3 * self.free.len();
        lens.focal = self.base.focal * x[o].exp();
        if self.fit_k1 {
            lens.k1 = x[o + 1];
        }
        (rot, lens)
    }
}

/// Matched rays' differences in world space, scaled by the focal length into (proxy) pixels: three values a
/// match.
fn residuals(links: &[Link], rot: &[Option<M3>], lens: &Lens, px: f64) -> Vec<f64> {
    links
        .par_iter()
        .flat_map_iter(|l| {
            let (ri, rj) = (
                linalg::transpose(&rot[l.i].unwrap()),
                linalg::transpose(&rot[l.j].unwrap()),
            );
            let s = lens.focal * px;
            l.a.iter().zip(&l.b).flat_map(move |(&a, &b)| {
                let (p, q) = (
                    linalg::mulv(&ri, lens.ray(a)),
                    linalg::mulv(&rj, lens.ray(b)),
                );
                [(p[0] - q[0]) * s, (p[1] - q[1]) * s, (p[2] - q[2]) * s]
            })
        })
        .collect()
}

/// Levenberg–Marquardt with a Huber loss (`delta` pixels) and a numerical Jacobian; returns the solved
/// rotations and lens.
fn adjust(
    links: &[Link],
    start: &[Option<M3>],
    reference: usize,
    lens: &Lens,
    px: f64,
    delta: f64,
) -> (Vec<Option<M3>>, Lens) {
    let p = Params {
        free: (0..start.len())
            .filter(|&i| i != reference && start[i].is_some())
            .collect(),
        start,
        fit_k1: lens.optics.distortion.is_empty(),
        base: lens.clone(),
    };
    let n = p.len();
    let mut x = vec![0.0; n];
    let weights = |r: &[f64]| -> Vec<f64> {
        r.chunks(3)
            .map(|c| {
                let e = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
                if e < delta { 1.0 } else { delta / e }
            })
            .collect()
    };
    let cost = |r: &[f64], w: &[f64]| -> f64 {
        r.chunks(3)
            .zip(w)
            .map(|(c, w)| w * (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]))
            .sum()
    };
    let eval = |x: &[f64]| {
        let (rot, lens) = p.unpack(x);
        residuals(links, &rot, &lens, px)
    };
    let mut lambda = 1e-3;
    let mut r = eval(&x);
    for _ in 0..60 {
        let w = weights(&r);
        let c0 = cost(&r, &w);
        let eps = 1e-6;
        let cols: Vec<Vec<f64>> = (0..n)
            .into_par_iter()
            .map(|k| {
                let mut xp = x.clone();
                xp[k] += eps;
                let rp = eval(&xp);
                rp.iter().zip(&r).map(|(a, b)| (a - b) / eps).collect()
            })
            .collect();
        let mut a = vec![vec![0.0; n]; n];
        let mut g = vec![0.0; n];
        for i in 0..n {
            for j in i..n {
                let s: f64 = cols[i]
                    .iter()
                    .zip(&cols[j])
                    .enumerate()
                    .map(|(m, (u, v))| w[m / 3] * u * v)
                    .sum();
                a[i][j] = s;
                a[j][i] = s;
            }
            g[i] = cols[i]
                .iter()
                .zip(&r)
                .enumerate()
                .map(|(m, (u, v))| w[m / 3] * u * v)
                .sum();
        }
        let mut improved = false;
        while lambda < 1e10 {
            let mut damped = a.clone();
            for i in 0..n {
                damped[i][i] += lambda * a[i][i].max(1e-9);
            }
            let Some(step) = linalg::solve(damped, g.iter().map(|v| -v).collect()) else {
                lambda *= 4.0;
                continue;
            };
            let xn: Vec<f64> = x.iter().zip(&step).map(|(a, b)| a + b).collect();
            let rn = eval(&xn);
            let c1 = cost(&rn, &w);
            if c1 < c0 {
                let gain = (c0 - c1) / c0.max(1e-30);
                x = xn;
                r = rn;
                lambda = (lambda / 3.0).max(1e-9);
                improved = gain > 1e-9;
                break;
            }
            lambda *= 4.0;
        }
        if !improved {
            break;
        }
    }
    p.unpack(&x)
}

/// Level the horizon: world rotated so that y is down — the normal of the plane the cameras' horizontal axes
/// lie in, signed by the cameras' own up — and z is the cameras' mean direction across it.
fn level(inputs: &[Input], rot: &[Option<M3>], reference: usize) -> M3 {
    let mut s = vec![vec![0.0; 3]; 3];
    let (mut up_sum, mut fwd) = ([0.0; 3], [0.0; 3]);
    let mut count = 0;
    for (f, r) in inputs.iter().zip(rot) {
        let Some(r) = r else { continue };
        let rt = linalg::transpose(r);
        let up = [f.up[0], f.up[1], 0.0];
        let across = [-f.up[1], f.up[0], 0.0];
        let x = linalg::mulv(&rt, across);
        for i in 0..3 {
            for j in 0..3 {
                s[i][j] += x[i] * x[j];
            }
        }
        let u = linalg::mulv(&rt, up);
        let z = linalg::mulv(&rt, [0.0, 0.0, 1.0]);
        for k in 0..3 {
            up_sum[k] += u[k];
            fwd[k] += z[k];
        }
        count += 1;
    }
    let (vals, vecs) = linalg::symmetric_eigen(s);
    let mut order = [0, 1, 2];
    order.sort_by(|&a, &b| vals[a].total_cmp(&vals[b]));
    let spread = vals[order[1]] / vals[order[2]].max(1e-30);
    let mut up = if count >= 2 && spread > 0.01 {
        let k = order[0];
        [vecs[0][k], vecs[1][k], vecs[2][k]]
    } else {
        linalg::normalize(up_sum)
    };
    if linalg::dot(up, up_sum) < 0.0 {
        up = up.map(|v| -v);
    }
    let down = up.map(|v| -v);
    let mut z = fwd;
    if linalg::norm(z) < 0.1 * count as f64 {
        z = linalg::mulv(
            &linalg::transpose(&rot[reference].unwrap()),
            [0.0, 0.0, 1.0],
        );
    }
    let along = linalg::dot(z, down);
    let z = linalg::normalize(std::array::from_fn(|k| z[k] - along * down[k]));
    let x = linalg::cross(down, z);
    [x, down, z]
}

/// Sample a log-luminance proxy bilinearly at pixel position `p`.
fn sample_grey(g: &Grey, p: [f64; 2]) -> f32 {
    let (x0, y0) = (p[0].floor(), p[1].floor());
    let (fx, fy) = ((p[0] - x0) as f32, (p[1] - y0) as f32);
    let (x0, y0) = (x0 as i32, y0 as i32);
    let top = g.at(x0, y0) * (1.0 - fx) + g.at(x0 + 1, y0) * fx;
    let bot = g.at(x0, y0 + 1) * (1.0 - fx) + g.at(x0 + 1, y0 + 1) * fx;
    top * (1.0 - fy) + bot * fy
}

/// Gains per frame and, when no profile gives it, the vignetting: log luminance sampled on a grid across every
/// trusted overlap, where both frames are well exposed and smooth, solved by iteratively reweighted least
/// squares. The reference's gain is 1; EXIF's exposure difference is a weak prior, so a frame whose overlaps
/// are all sky still lands near it.
fn exposure(inputs: &[Input], sol: &mut Solution) {
    let optics = &sol.lens.optics;
    let fit_vignette = optics.vignetting.is_empty();
    let free: Vec<usize> = (0..inputs.len())
        .filter(|&i| i != sol.reference && sol.rotations[i].is_some())
        .collect();
    let col = |i: usize| free.iter().position(|&k| k == i);
    let n = free.len() + if fit_vignette { 3 } else { 0 };
    if n == 0 {
        return;
    }
    // Rows: (coefficients, right-hand side).
    let mut rows: Vec<(Vec<f64>, f64)> = vec![];
    let lens = &sol.lens;
    let lo = (1.0f32 / 500.0).log2();
    let smooth = |g: &Grey, p: [f64; 2]| {
        let d = |a: [f64; 2], b: [f64; 2]| (sample_grey(g, a) - sample_grey(g, b)).abs();
        d([p[0] + 1.0, p[1]], [p[0] - 1.0, p[1]]) + d([p[0], p[1] + 1.0], [p[0], p[1] - 1.0]) < 0.15
    };
    for l in &sol.links {
        let (fi, fj) = (&inputs[l.i], &inputs[l.j]);
        let hi = fi.ceiling.min(fj.ceiling).log2();
        let (si, sj) = (
            Sensor::new(fi.proxy.w, fi.proxy.h, optics),
            Sensor::new(fj.proxy.w, fj.proxy.h, optics),
        );
        let (ri, rj) = (sol.rotations[l.i].unwrap(), sol.rotations[l.j].unwrap());
        let rit = linalg::transpose(&ri);
        let step = 6;
        for y in (3..fi.proxy.h - 3).step_by(step) {
            for x in (3..fi.proxy.w - 3).step_by(step) {
                let p = [x as f64, y as f64];
                let ua = si.to_unit(p);
                let d = linalg::mulv(&rit, lens.ray(ua));
                let Some(ub) = lens.project(linalg::mulv(&rj, d), 1) else {
                    continue;
                };
                let q = sj.to_pixel(ub);
                if !sj.inside(q, 3.0) {
                    continue;
                }
                let (ya, yb) = (sample_grey(&fi.proxy, p), sample_grey(&fj.proxy, q));
                if ya > hi || yb > hi || ya < lo || yb < lo {
                    continue;
                }
                if !smooth(&fi.proxy, p) || !smooth(&fj.proxy, q) {
                    continue;
                }
                let (ra, rb) = (ua[0].hypot(ua[1]), ub[0].hypot(ub[1]));
                // ya + gᵢ + v(ra) = yb + gⱼ + v(rb)
                let mut c = vec![0.0; n];
                if let Some(k) = col(l.i) {
                    c[k] += 1.0;
                }
                if let Some(k) = col(l.j) {
                    c[k] -= 1.0;
                }
                let mut rhs = (yb - ya) as f64;
                if fit_vignette {
                    let o = free.len();
                    let (a2, b2) = (ra * ra, rb * rb);
                    c[o] += a2 - b2;
                    c[o + 1] += a2 * a2 - b2 * b2;
                    c[o + 2] += a2 * a2 * a2 - b2 * b2 * b2;
                } else {
                    rhs -= lens.flatten(ra).log2() - lens.flatten(rb).log2();
                }
                rows.push((c, rhs));
            }
        }
    }
    let ref_light = inputs[sol.reference].light;
    let prior = |k: usize, i: usize| {
        let mut c = vec![0.0; n];
        c[k] = 1.0;
        (c, (ref_light / inputs[i].light).log2())
    };
    let priors: Vec<(Vec<f64>, f64)> = free.iter().enumerate().map(|(k, &i)| prior(k, i)).collect();
    let mut x = priors.iter().map(|p| p.1).collect::<Vec<_>>();
    x.resize(n, 0.0);
    for _ in 0..6 {
        let mut a = vec![vec![0.0; n]; n];
        let mut b = vec![0.0; n];
        let mut add = |c: &[f64], rhs: f64, w: f64| {
            for i in 0..n {
                if c[i] == 0.0 {
                    continue;
                }
                for j in 0..n {
                    a[i][j] += w * c[i] * c[j];
                }
                b[i] += w * c[i] * rhs;
            }
        };
        for (c, rhs) in &rows {
            let e = c.iter().zip(&x).map(|(u, v)| u * v).sum::<f64>() - rhs;
            let w = if e.abs() < 0.05 { 1.0 } else { 0.05 / e.abs() };
            add(c, *rhs, w);
        }
        for (c, rhs) in &priors {
            add(c, *rhs, 1.0);
        }
        for (i, row) in a.iter_mut().enumerate() {
            row[i] += 1e-6;
        }
        match linalg::solve(a, b) {
            Some(v) => x = v,
            None => break,
        }
    }
    for (k, &i) in free.iter().enumerate() {
        sol.gains[i] = x[k].exp2();
    }
    if fit_vignette {
        let o = free.len();
        sol.lens.vignette = [x[o], x[o + 1], x[o + 2]];
    }
}

/// Solve a panorama: matches, focal length, rotations, the level horizon, the exposure. `focal` is a starting
/// focal length in half-diagonals for when the homographies give none (from EXIF, say).
pub fn solve(inputs: &[Input], optics: &Optics, focal: Option<f64>) -> Result<Solution, String> {
    if inputs.len() < 2 {
        return Err("a panorama needs two frames".into());
    }
    let (links, focals) = link(inputs, optics);
    if links.is_empty() {
        return Err("no two frames overlap".into());
    }
    let mut score = vec![0usize; inputs.len()];
    for l in &links {
        score[l.i] += l.inliers;
        score[l.j] += l.inliers;
    }
    let reference = (0..inputs.len()).max_by_key(|&i| score[i]).unwrap();
    let focal0 = median(focals).or(focal).unwrap_or(1.5);
    let lens = Lens {
        optics: optics.clone(),
        focal: focal0,
        k1: 0.0,
        vignette: [0.0; 3],
    };
    let start = spanning(inputs.len(), reference, &links, &lens);
    let links: Vec<Link> = links
        .into_iter()
        .filter(|l| start[l.i].is_some() && start[l.j].is_some())
        .collect();
    // Residuals in the reference proxy's pixels.
    let px = Sensor::new(inputs[reference].proxy.w, inputs[reference].proxy.h, optics).half_diag;
    let (rot, lens) = adjust(&links, &start, reference, &lens, px, 2.0);
    // Drop what the first adjustment shows to be wrong, then adjust again on the rest.
    let mut kept = links.clone();
    for l in kept.iter_mut() {
        let r = residuals(std::slice::from_ref(l), &rot, &lens, px);
        let keep: Vec<bool> = r
            .chunks(3)
            .map(|c| (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt() < 3.0)
            .collect();
        let mut k = keep.iter();
        l.a.retain(|_| *k.next().unwrap());
        let mut k = keep.iter();
        l.b.retain(|_| *k.next().unwrap());
    }
    kept.retain(|l| l.a.len() >= 8);
    let (rot, lens) = adjust(&kept, &rot, reference, &lens, px, 1.0);
    let r = residuals(&kept, &rot, &lens, px);
    let rms = (r.iter().map(|v| v * v).sum::<f64>() / (r.len() / 3).max(1) as f64).sqrt();
    let w = level(inputs, &rot, reference);
    let wt = linalg::transpose(&w);
    let rotations = rot
        .iter()
        .map(|r| r.map(|r| linalg::mul(&r, &wt)))
        .collect();
    let mut sol = Solution {
        lens,
        rotations,
        gains: vec![1.0; inputs.len()],
        reference,
        links: kept,
        rms,
        focal_initial: focal0,
    };
    exposure(inputs, &mut sol);
    Ok(sol)
}

/// Where the panorama lies on its projection's plane, and at what resolution.
#[derive(Clone, Debug)]
pub struct Layout {
    pub projection: Projection,
    /// Canvas pixels per plane unit (radian, or tangent).
    pub scale: f64,
    /// The plane position of the canvas's top-left corner.
    pub origin: [f64; 2],
    pub w: usize,
    pub h: usize,
    /// The canvas is a full turn: its left and right edges meet.
    pub wrap: bool,
    /// Bands of the multi-band blend; the canvas's sizes are multiples of 2^levels.
    pub levels: usize,
}

impl Layout {
    fn direction(&self, x: f64, y: f64) -> V3 {
        self.projection.inverse([
            self.origin[0] + (x + 0.5) / self.scale,
            self.origin[1] + (y + 0.5) / self.scale,
        ])
    }
}

/// World directions a frame sees on an `n × n` grid that includes its edges.
fn frame_directions(lens: &Lens, r: &M3, s: &Sensor, n: usize) -> Vec<V3> {
    let rt = linalg::transpose(r);
    (0..n * n)
        .map(|k| {
            let (i, j) = (
                (k % n) as f64 / (n - 1) as f64,
                (k / n) as f64 / (n - 1) as f64,
            );
            let p = [i * (s.w as f64 - 1.0) - 0.0, j * (s.h as f64 - 1.0) - 0.0];
            linalg::mulv(&rt, lens.ray(s.to_unit(p)))
        })
        .collect()
}

/// Whether a frame sees the world direction `d`.
fn sees(lens: &Lens, r: &M3, s: &Sensor, d: V3) -> bool {
    lens.project(linalg::mulv(r, d), 1)
        .is_some_and(|u| s.inside(s.to_pixel(u), 0.0))
}

fn round_up(v: usize, m: usize) -> usize {
    v.div_ceil(m) * m
}

/// Choose the projection (when not given), the plane's extent and the canvas's size. `scale` 1 keeps the
/// reference frame's full resolution at the panorama's centre. `sphere` asks for the whole sphere whatever the
/// frames cover — an HDRI's 2 : 1 equirectangular canvas, uncovered directions black.
pub fn layout(
    sol: &Solution,
    sizes: &[(usize, usize)],
    projection: Option<Projection>,
    scale: f64,
    max_pixels: usize,
    sphere: bool,
) -> Result<Layout, String> {
    let lens = &sol.lens;
    let frames: Vec<(usize, Sensor, M3)> = sol
        .rotations
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let (w, h) = sizes[i];
            r.map(|r| (i, Sensor::new(w, h, &lens.optics), r))
        })
        .collect();
    // Longitude coverage in 5° bins, latitude range, and whether a pole is in view.
    let mut bins = [false; 72];
    let (mut lat0, mut lat1) = (f64::MAX, f64::MIN);
    let mut poles = [false; 2];
    for (_, s, r) in &frames {
        let dirs = frame_directions(lens, r, s, 33);
        for row in dirs.chunks(33) {
            let lons: Vec<f64> = row.iter().map(|d| d[0].atan2(d[2])).collect();
            for w in lons.windows(2) {
                let (mut a, mut b) = (w[0], w[1]);
                if (b - a).abs() > std::f64::consts::PI {
                    // The row crosses ±180°: mark both ends.
                    if a > b {
                        std::mem::swap(&mut a, &mut b);
                    }
                    for (lo, hi) in [(-std::f64::consts::PI, a), (b, std::f64::consts::PI)] {
                        mark(&mut bins, lo, hi);
                    }
                } else {
                    mark(&mut bins, a.min(b), a.max(b));
                }
            }
        }
        for d in &dirs {
            let lat = d[1].atan2(d[0].hypot(d[2]));
            lat0 = lat0.min(lat);
            lat1 = lat1.max(lat);
        }
        for (k, pole) in [[0.0, -1.0, 0.0], [0.0, 1.0, 0.0]].into_iter().enumerate() {
            if sees(lens, r, s, pole) {
                poles[k] = true;
            }
        }
    }
    if poles[0] {
        lat0 = -std::f64::consts::FRAC_PI_2;
    }
    if poles[1] {
        lat1 = std::f64::consts::FRAC_PI_2;
    }
    if sphere {
        poles = [true, true];
        lat0 = -std::f64::consts::FRAC_PI_2;
        lat1 = std::f64::consts::FRAC_PI_2;
    }
    let full_turn = sphere || bins.iter().all(|&b| b) || poles.iter().any(|&p| p);
    // The longitude range: everything but the widest uncovered gap.
    let (lon0, lon1) = if full_turn {
        (-std::f64::consts::PI, std::f64::consts::PI)
    } else {
        let mut best = (0, 0);
        for start in 0..72 {
            if bins[start] || !bins[(start + 71) % 72] {
                continue;
            }
            let len = (0..72).take_while(|k| !bins[(start + k) % 72]).count();
            if len > best.1 {
                best = (start, len);
            }
        }
        let bin = std::f64::consts::TAU / 72.0;
        let first = (best.0 + best.1) % 72;
        let lo = -std::f64::consts::PI + first as f64 * bin;
        (lo, lo + (72 - best.1) as f64 * bin)
    };
    let span = lon1 - lon0;
    let projection = if sphere {
        Some(Projection::Spherical)
    } else {
        projection
    };
    let projection = projection.unwrap_or(
        if span < 100f64.to_radians() && (lat1 - lat0) < 100f64.to_radians() && !full_turn {
            Projection::Rectilinear
        } else if !poles[0] && !poles[1] && lat0 > -65f64.to_radians() && lat1 < 65f64.to_radians()
        {
            Projection::Cylindrical
        } else {
            Projection::Spherical
        },
    );
    let unwrap = |a: f64| {
        let mut a = a;
        while a < lon0 {
            a += std::f64::consts::TAU;
        }
        while a >= lon0 + std::f64::consts::TAU {
            a -= std::f64::consts::TAU;
        }
        a
    };
    let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for (_, s, r) in &frames {
        for d in frame_directions(lens, r, s, 33) {
            let Some(p) = projection.forward(d) else {
                return Err(format!(
                    "the frames cover too wide a field for a {projection:?} projection"
                ));
            };
            let px = if projection.wraps() {
                unwrap(p[0])
            } else {
                p[0]
            };
            x0 = x0.min(px);
            x1 = x1.max(px);
            y0 = y0.min(p[1]);
            y1 = y1.max(p[1]);
        }
    }
    if projection == Projection::Spherical {
        if poles[0] {
            y0 = -std::f64::consts::FRAC_PI_2;
        }
        if poles[1] {
            y1 = std::f64::consts::FRAC_PI_2;
        }
    }
    if projection.wraps() {
        x0 = x0.max(lon0);
        x1 = x1.min(lon0 + std::f64::consts::TAU);
    }
    let reference = sizes[sol.reference];
    let mut s = lens.focal * Sensor::new(reference.0, reference.1, &lens.optics).half_diag * scale;
    let small = frames
        .iter()
        .map(|(_, f, _)| f.w.min(f.h))
        .min()
        .unwrap_or(1) as f64
        * scale;
    let levels = ((small / 8.0).log2().floor() as usize).clamp(1, 7);
    let step = 1usize << levels;
    let wrap = projection.wraps() && full_turn;
    let (w, origin_x) = if wrap {
        // A sphere's canvas is exactly twice as wide as it is high, both multiples of the bands' step.
        let w = round_up(
            (std::f64::consts::TAU * s).round() as usize,
            if sphere { 2 * step } else { step },
        );
        s = w as f64 / std::f64::consts::TAU;
        (w, lon0)
    } else {
        let w = round_up(((x1 - x0) * s).ceil() as usize, step);
        (w, 0.5 * (x0 + x1) - 0.5 * w as f64 / s)
    };
    let h = if sphere {
        w / 2
    } else {
        round_up(((y1 - y0) * s).ceil() as usize, step)
    };
    let origin = [origin_x, 0.5 * (y0 + y1) - 0.5 * h as f64 / s];
    if w * h > max_pixels {
        return Err(format!(
            "the panorama would be {w} × {h} ({:.0} MP); a smaller scale would fit",
            (w * h) as f64 / 1e6
        ));
    }
    Ok(Layout {
        projection,
        scale: s,
        origin,
        w,
        h,
        wrap,
        levels,
    })
}

fn mark(bins: &mut [bool; 72], lo: f64, hi: f64) {
    let bin = std::f64::consts::TAU / 72.0;
    let a = (((lo + std::f64::consts::PI) / bin).floor() as usize).min(71);
    let b = (((hi + std::f64::consts::PI) / bin).floor() as usize).min(71);
    for v in &mut bins[a..=b] {
        *v = true;
    }
}

/// A frame's footprint on the canvas: pixels `x0..x1` (unwrapped: may run past the right edge of a full turn)
/// and `y0..y1`, multiples of 2^levels, padded so the blend's coarsest band sees past the frame's edge.
#[derive(Clone, Copy, Debug)]
struct Footprint {
    x0: isize,
    x1: isize,
    y0: usize,
    y1: usize,
    /// The footprint is the whole turn.
    ring: bool,
}

/// Accumulates frames into a multi-band blend.
pub struct Canvas {
    pub layout: Layout,
    lens: Lens,
    rotations: Vec<Option<M3>>,
    gains: Vec<f64>,
    sizes: Vec<(usize, usize)>,
    feet: Vec<Option<Footprint>>,
    /// Per canvas pixel, the frame whose optical axis is nearest; `u16::MAX` for none.
    best: Vec<u16>,
    bands: Bands,
}

impl Canvas {
    pub fn new(sol: &Solution, sizes: &[(usize, usize)], layout: Layout) -> Self {
        let lens = sol.lens.clone();
        let step = 1isize << layout.levels;
        let pad = 2 * step;
        let feet: Vec<Option<Footprint>> = sol
            .rotations
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let r = r.as_ref()?;
                let s = Sensor::new(sizes[i].0, sizes[i].1, &lens.optics);
                let dirs = frame_directions(&lens, r, &s, 33);
                let centre = layout
                    .projection
                    .forward(linalg::mulv(&linalg::transpose(r), [0.0, 0.0, 1.0]))?;
                let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
                for d in dirs {
                    let p = layout.projection.forward(d)?;
                    let mut x = p[0];
                    if layout.projection.wraps() {
                        // Unwrapped to lie within half a turn of the frame's centre.
                        x += std::f64::consts::TAU
                            * ((centre[0] - x) / std::f64::consts::TAU).round();
                    }
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(p[1]);
                    y1 = y1.max(p[1]);
                }
                let pole = [[0.0, -1.0, 0.0], [0.0, 1.0, 0.0]]
                    .into_iter()
                    .map(|d| sees(&lens, r, &s, d))
                    .collect::<Vec<_>>();
                let to = |v: f64, o: f64| ((v - o) * layout.scale) as isize;
                let mut fx0 = to(x0, layout.origin[0]) - pad;
                let mut fx1 = to(x1, layout.origin[0]) + 1 + pad;
                let mut fy0 = to(y0, layout.origin[1]) - pad;
                let mut fy1 = to(y1, layout.origin[1]) + 1 + pad;
                if pole[0] {
                    fy0 = 0;
                }
                if pole[1] {
                    fy1 = layout.h as isize;
                }
                fx0 = fx0.div_euclid(step) * step;
                fx1 = (fx1 + step - 1).div_euclid(step) * step;
                fy0 = fy0.div_euclid(step) * step;
                fy1 = (fy1 + step - 1).div_euclid(step) * step;
                let (fy0, fy1) = (fy0.max(0) as usize, (fy1.max(0) as usize).min(layout.h));
                let ring =
                    layout.wrap && (pole.iter().any(|&p| p) || fx1 - fx0 >= layout.w as isize);
                let (fx0, fx1) = if ring {
                    (0, layout.w as isize)
                } else if layout.wrap {
                    (fx0, fx1)
                } else {
                    (fx0.max(0), fx1.min(layout.w as isize))
                };
                (fx1 > fx0 && fy1 > fy0).then_some(Footprint {
                    x0: fx0,
                    x1: fx1,
                    y0: fy0,
                    y1: fy1,
                    ring,
                })
            })
            .collect();
        let (w, h) = (layout.w, layout.h);
        let mut best = vec![u16::MAX; w * h];
        let mut score = vec![f32::MIN; w * h];
        for (i, f) in feet.iter().enumerate() {
            let (Some(f), Some(r)) = (f, sol.rotations[i]) else {
                continue;
            };
            let s = Sensor::new(sizes[i].0, sizes[i].1, &lens.optics);
            best.par_chunks_mut(w)
                .zip(score.par_chunks_mut(w))
                .enumerate()
                .filter(|(y, _)| (f.y0..f.y1).contains(y))
                .for_each(|(y, (b, sc))| {
                    for x in f.x0..f.x1 {
                        let d = layout.direction(x as f64, y as f64);
                        let c = linalg::mulv(&r, d);
                        let Some(u) = lens.project(c, 1) else {
                            continue;
                        };
                        if !s.inside(s.to_pixel(u), 0.0) {
                            continue;
                        }
                        let xx = x.rem_euclid(w as isize) as usize;
                        let v = c[2] as f32;
                        if v > sc[xx] {
                            sc[xx] = v;
                            b[xx] = i as u16;
                        }
                    }
                });
        }
        let bands = Bands::new(w, h, layout.levels, layout.wrap);
        Self {
            layout,
            lens,
            rotations: sol.rotations.clone(),
            gains: sol.gains.clone(),
            sizes: sizes.to_vec(),
            feet,
            best,
            bands,
        }
    }

    /// Blend frame `i`: three floats a pixel at its full size, sensor orientation, white-balanced as the
    /// solve's proxies were.
    pub fn add(&mut self, i: usize, rgb: &[f32]) {
        let (Some(f), Some(r)) = (self.feet[i], self.rotations[i]) else {
            return;
        };
        let lay = &self.layout;
        let lens = &self.lens;
        let s = Sensor::new(self.sizes[i].0, self.sizes[i].1, &lens.optics);
        let (bw, bh) = ((f.x1 - f.x0) as usize, f.y1 - f.y0);
        let gain = self.gains[i] as f32;
        let mut val = Plane::new(bw, bh, 3);
        let mut alpha = Plane::new(bw, bh, 1);
        let mut mask = Plane::new(bw, bh, 1);
        let best = &self.best;
        let sample = |p: [f64; 2], ch: usize| -> f32 {
            let x = p[0].clamp(0.0, s.w as f64 - 1.001);
            let y = p[1].clamp(0.0, s.h as f64 - 1.001);
            let (x0, y0) = (x.floor() as usize, y.floor() as usize);
            let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
            let v = |xx: usize, yy: usize| rgb[(yy * s.w + xx) * 3 + ch];
            let top = v(x0, y0) * (1.0 - fx) + v(x0 + 1, y0) * fx;
            let bot = v(x0, y0 + 1) * (1.0 - fx) + v(x0 + 1, y0 + 1) * fx;
            top * (1.0 - fy) + bot * fy
        };
        val.data
            .par_chunks_mut(bw * 3)
            .zip(alpha.data.par_chunks_mut(bw))
            .zip(mask.data.par_chunks_mut(bw))
            .enumerate()
            .for_each(|(y, ((v, a), m))| {
                let cy = f.y0 + y;
                for x in 0..bw {
                    let cx = f.x0 + x as isize;
                    let d = lay.direction(cx as f64, cy as f64);
                    let c = linalg::mulv(&r, d);
                    let Some(ug) = lens.project(c, 1) else {
                        continue;
                    };
                    let pg = s.to_pixel(ug);
                    if !s.inside(pg, 0.0) {
                        continue;
                    }
                    let flat = gain * lens.flatten(ug[0].hypot(ug[1])) as f32;
                    for ch in 0..3 {
                        let p = if ch == 1 {
                            pg
                        } else {
                            s.to_pixel(lens.project(c, ch).unwrap_or(ug))
                        };
                        v[x * 3 + ch] = sample(p, ch) * flat;
                    }
                    a[x] = 1.0;
                    let xx = cx.rem_euclid(lay.w as isize) as usize;
                    if best[cy * lay.w + xx] == i as u16 {
                        m[x] = 1.0;
                    }
                }
            });
        self.bands.add(
            f.x0,
            f.y0,
            blend::fill(val, alpha, lay.levels, f.ring),
            mask,
            f.ring,
        );
    }

    /// Collapse the bands: the panorama, three floats a pixel, and which pixels any frame saw.
    pub fn finish(self) -> (Vec<f32>, Vec<bool>) {
        let mut r = self.bands.finish();
        let seen: Vec<bool> = self.best.iter().map(|&b| b != u16::MAX).collect();
        for (p, &s) in r.data.chunks_mut(3).zip(&seen) {
            if !s {
                p.fill(0.0);
            }
        }
        (r.data, seen)
    }
}
/// The largest axis-aligned rectangle every pixel of which some frame saw: `(x0, y0, x1, y1)`, exclusive at
/// the far end. Found on blocks of `block` pixels (a block counts only when all of it was seen), by the
/// largest rectangle under each row's histogram of covered heights.
pub fn largest_rectangle(
    seen: &[bool],
    w: usize,
    h: usize,
    block: usize,
) -> (usize, usize, usize, usize) {
    let (bw, bh) = (w / block, h / block);
    let full: Vec<bool> = (0..bw * bh)
        .into_par_iter()
        .map(|k| {
            let (bx, by) = (k % bw, k / bw);
            (0..block).all(|y| (0..block).all(|x| seen[(by * block + y) * w + bx * block + x]))
        })
        .collect();
    let mut heights = vec![0usize; bw];
    let mut best = (0usize, (0, 0, 0, 0));
    for by in 0..bh {
        for bx in 0..bw {
            heights[bx] = if full[by * bw + bx] {
                heights[bx] + 1
            } else {
                0
            };
        }
        let mut stack: Vec<usize> = vec![];
        for x in 0..=bw {
            let hx = if x < bw { heights[x] } else { 0 };
            while let Some(&top) = stack.last() {
                if heights[top] <= hx {
                    break;
                }
                stack.pop();
                let left = stack.last().map_or(0, |&l| l + 1);
                let area = heights[top] * (x - left);
                if area > best.0 {
                    best = (area, (left, by + 1 - heights[top], x, by + 1));
                }
            }
            stack.push(x);
        }
    }
    let (x0, y0, x1, y1) = best.1;
    (x0 * block, y0 * block, x1 * block, y1 * block)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(mut s: u64) -> u64 {
        s = s.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        for _ in 0..3 {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
        }
        s
    }

    /// A scene on the sphere: discs of random size and grey scattered over it (a few per 3° cell, found by
    /// hashing the cell), the topmost wins, over a slow gradient — corners many and distinct, as `align`'s
    /// test card.
    fn radiance(d: V3) -> f32 {
        let lon = d[0].atan2(d[2]);
        let lat = d[1].atan2(d[0].hypot(d[2]));
        let cell = 3f64.to_radians();
        let (ci, cj) = ((lon / cell).floor() as i64, (lat / cell).floor() as i64);
        let u = |h: u64| (h % 10_000) as f64 / 10_000.0;
        let mut top = (0u64, 0.04 * (1.0 + 0.3 * lon.cos()) as f32);
        for j in cj - 1..=cj + 1 {
            for i in ci - 1..=ci + 1 {
                for k in 0..3u64 {
                    let h = hash(
                        (i as u64).wrapping_mul(1_000_003)
                            ^ (j as u64).wrapping_mul(7_919)
                            ^ (k << 40),
                    );
                    let (clon, clat) = ((i as f64 + u(h)) * cell, (j as f64 + u(hash(h))) * cell);
                    let c = [clat.cos() * clon.sin(), clat.sin(), clat.cos() * clon.cos()];
                    let r = (0.4 + 1.6 * u(hash(h ^ 1))).to_radians();
                    if linalg::dot(c, d) > r.cos() && h > top.0 {
                        top = (h, (0.03 + 0.8 * u(hash(h ^ 2))) as f32);
                    }
                }
            }
        }
        top.1
    }

    struct Truth {
        lens: Lens,
        rot: Vec<M3>,
        exposure: Vec<f64>,
        falloff: f64,
    }

    fn rot(yaw: f64, pitch: f64, roll: f64) -> M3 {
        // World → camera: undo the camera's yaw (about y), then pitch (about x), then roll (about z).
        let r = linalg::mul(
            &linalg::rodrigues([0.0, 0.0, roll.to_radians()]),
            &linalg::mul(
                &linalg::rodrigues([pitch.to_radians(), 0.0, 0.0]),
                &linalg::rodrigues([0.0, yaw.to_radians(), 0.0]),
            ),
        );
        linalg::transpose(&linalg::transpose(&r))
    }

    /// Render frame `i` of the truth: luminance, supersampled 3 × 3.
    fn render(t: &Truth, i: usize, w: usize, h: usize) -> Vec<f32> {
        let s = Sensor::new(w, h, &t.lens.optics);
        let rt = linalg::transpose(&t.rot[i]);
        (0..w * h)
            .into_par_iter()
            .map(|k| {
                let (x, y) = ((k % w) as f64, (k / w) as f64);
                let mut sum = 0.0;
                for sy in 0..3 {
                    for sx in 0..3 {
                        let p = [x + (sx as f64 - 1.0) / 3.0, y + (sy as f64 - 1.0) / 3.0];
                        let u = s.to_unit(p);
                        let d = linalg::mulv(&rt, t.lens.ray(u));
                        let r2 = u[0] * u[0] + u[1] * u[1];
                        sum += radiance(d) as f64 * t.exposure[i] * (1.0 - t.falloff * r2);
                    }
                }
                (sum / 9.0) as f32
            })
            .collect()
    }

    fn truth() -> Truth {
        Truth {
            lens: Lens {
                optics: Optics {
                    center: [0.5, 0.5],
                    ..Default::default()
                },
                focal: 1.6,
                k1: -0.04,
                vignette: [0.0; 3],
            },
            rot: vec![
                rot(-36.0, 4.0, 0.0),
                rot(-12.0, -3.0, 0.0),
                rot(12.0, 2.0, 0.0),
                rot(36.0, -1.0, 0.0),
            ],
            exposure: vec![1.0, 0.5, 1.0, 2.0],
            falloff: 0.25,
        }
    }

    fn inputs(t: &Truth, w: usize, h: usize) -> (Vec<Input>, Vec<Vec<f32>>) {
        let frames: Vec<Vec<f32>> = (0..t.rot.len()).map(|i| render(t, i, w, h)).collect();
        let inputs = frames
            .iter()
            .enumerate()
            .map(|(i, f)| Input {
                w,
                h,
                proxy: Grey::log_luminance(w, h, f.iter().copied()),
                up: [0.0, -1.0],
                light: t.exposure[i],
                ceiling: 0.6,
            })
            .collect();
        (inputs, frames)
    }

    #[test]
    fn focal_length_from_a_rotation_homography() {
        let f = 1.7;
        let k: M3 = [[f, 0.0, 0.0], [0.0, f, 0.0], [0.0, 0.0, 1.0]];
        let ki: M3 = [[1.0 / f, 0.0, 0.0], [0.0, 1.0 / f, 0.0], [0.0, 0.0, 1.0]];
        let h = linalg::mul(&k, &linalg::mul(&rot(20.0, 5.0, 2.0), &ki));
        let got = focal_from_homography(&h).unwrap();
        assert!((got - f).abs() < 1e-6, "{got}");
    }

    /// Four frames of a known rig — turned 24° apart, pitched a little, with barrel distortion, vignetting and
    /// exposures two stops apart — solve to their focal length, relative rotations, gains and a level horizon.
    #[test]
    fn a_known_rig_is_recovered() {
        let t = truth();
        let (inputs, _) = inputs(&t, 720, 480);
        let sol = solve(&inputs, &t.lens.optics, None).expect("solved");
        eprintln!(
            "focal {:.4} (from {:.4}) k1 {:.4} rms {:.3} px, gains {:?}, vignette {:?}, ref {}",
            sol.lens.focal,
            sol.focal_initial,
            sol.lens.k1,
            sol.rms,
            sol.gains,
            sol.lens.vignette,
            sol.reference
        );
        assert!(sol.rotations.iter().all(|r| r.is_some()));
        assert!(
            (sol.lens.focal / t.lens.focal - 1.0).abs() < 0.01,
            "focal {}",
            sol.lens.focal
        );
        assert!((sol.lens.k1 - t.lens.k1).abs() < 0.01, "k1 {}", sol.lens.k1);
        assert!(sol.rms < 0.5, "rms {}", sol.rms);
        // Relative rotations, against the truth's, to 0.05°.
        let r = |v: &[Option<M3>], i: usize, j: usize| {
            linalg::mul(&v[j].unwrap(), &linalg::transpose(&v[i].unwrap()))
        };
        let tr: Vec<Option<M3>> = t.rot.iter().map(|r| Some(*r)).collect();
        for (i, j) in [(0, 1), (1, 2), (2, 3), (0, 3)] {
            let d = linalg::mul(&r(&sol.rotations, i, j), &linalg::transpose(&r(&tr, i, j)));
            let angle = ((d[0][0] + d[1][1] + d[2][2] - 1.0) / 2.0)
                .clamp(-1.0, 1.0)
                .acos();
            assert!(
                angle.to_degrees() < 0.05,
                "{i}→{j}: {:.4}°",
                angle.to_degrees()
            );
        }
        // Gains into the reference's radiance.
        for i in 0..4 {
            let want = t.exposure[sol.reference] / t.exposure[i];
            assert!(
                (sol.gains[i] / want - 1.0).abs() < 0.02,
                "gain {i}: {} against {want}",
                sol.gains[i]
            );
        }
        // Level: the solved world's down is the truth's down (the rig had no roll).
        let q = linalg::mul(&linalg::transpose(&sol.rotations[0].unwrap()), &t.rot[0]);
        let down = linalg::mulv(&q, [0.0, 1.0, 0.0]);
        assert!(down[1] > 0.9998, "down {down:?}");
        // The fitted falloff flattens the truth's: 1 / (1 − 0.25 r²) at r = 0.8.
        let want = 1.0 / (1.0 - t.falloff * 0.64);
        let got = sol.lens.flatten(0.8);
        assert!(
            (got / want - 1.0).abs() < 0.02,
            "vignetting at 0.8: {got} against {want}"
        );
    }

    /// Stitch the truth's frames and compare the canvas with the scene's radiance, in the reference's
    /// exposure, wherever it is covered and away from disc edges: (median, p95) relative error.
    fn stitch_error(
        t: &Truth,
        w: usize,
        h: usize,
        projection: Projection,
    ) -> (Layout, f64, f64, Vec<bool>) {
        let (inputs, frames) = inputs(t, w, h);
        let sol = solve(&inputs, &t.lens.optics, None).expect("solved");
        let sizes = vec![(w, h); t.rot.len()];
        let lay = layout(&sol, &sizes, Some(projection), 0.5, 50_000_000, false).unwrap();
        eprintln!("{lay:?}");
        let mut canvas = Canvas::new(&sol, &sizes, lay.clone());
        for (i, f) in frames.iter().enumerate() {
            let rgb: Vec<f32> = f.iter().flat_map(|&v| [v, v, v]).collect();
            canvas.add(i, &rgb);
        }
        let (img, seen) = canvas.finish();
        // The solved world differs from the truth's by a turn about the vertical; find it from frame 0.
        let q = linalg::mul(&linalg::transpose(&t.rot[0]), &sol.rotations[0].unwrap());
        let mut err = vec![];
        for y in (0..lay.h).step_by(3) {
            for x in (0..lay.w).step_by(3) {
                if !seen[y * lay.w + x] {
                    continue;
                }
                let at = |dx: f64, dy: f64| {
                    radiance(linalg::mulv(
                        &q,
                        lay.direction(x as f64 + dx, y as f64 + dy),
                    ))
                };
                let v = at(0.0, 0.0);
                if [(-2.0, 0.0), (2.0, 0.0), (0.0, -2.0), (0.0, 2.0)]
                    .iter()
                    .any(|&(a, b)| (at(a, b) - v).abs() > 1e-6)
                {
                    continue;
                }
                let want = v as f64 * t.exposure[sol.reference];
                err.push((img[(y * lay.w + x) * 3 + 1] as f64 / want - 1.0).abs());
            }
        }
        err.sort_by(f64::total_cmp);
        assert!(err.len() > 1000);
        let (median, p95) = (err[err.len() / 2], err[err.len() * 95 / 100]);
        eprintln!("{} samples: median {median:.4}, p95 {p95:.4}", err.len());
        (lay, median, p95, seen)
    }

    /// The stitched canvas shows the scene's radiance, in the reference's exposure, wherever it is covered;
    /// the crop keeps only what was covered.
    #[test]
    fn the_blend_reproduces_the_scene() {
        let (lay, median, p95, seen) = stitch_error(&truth(), 720, 480, Projection::Cylindrical);
        assert!(!lay.wrap);
        assert!(median < 0.01 && p95 < 0.04, "median {median}, p95 {p95}");
        let (x0, y0, x1, y1) = largest_rectangle(&seen, lay.w, lay.h, 4);
        assert!((x1 - x0) * (y1 - y0) > lay.w * lay.h / 2);
        assert!((y0..y1).all(|y| (x0..x1).all(|x| seen[y * lay.w + x])));
    }

    /// Eight frames round a full turn close into a ring: the canvas wraps, and the seam where its edges meet
    /// is as good as the rest.
    #[test]
    fn a_full_turn_wraps() {
        let mut t = truth();
        t.lens.focal = 1.2;
        t.rot = (0..8)
            .map(|k| rot(k as f64 * 45.0, if k % 2 == 0 { 3.0 } else { -3.0 }, 0.0))
            .collect();
        t.exposure = (0..8).map(|k| [1.0, 0.7, 1.4][k % 3]).collect();
        let (lay, median, p95, seen) = stitch_error(&t, 480, 320, Projection::Spherical);
        assert!(lay.wrap, "{lay:?}");
        assert!(median < 0.01 && p95 < 0.05, "median {median}, p95 {p95}");
        // Every column is covered somewhere: no gap at the join.
        assert!((0..lay.w).all(|x| (0..lay.h).any(|y| seen[y * lay.w + x])));
        // As an HDRI: the whole sphere, 2 : 1, the ring in its middle band.
        let (inputs, _) = inputs(&t, 480, 320);
        let sol = solve(&inputs, &t.lens.optics, None).unwrap();
        let sphere = layout(&sol, &[(480, 320); 8], None, 0.5, 50_000_000, true).unwrap();
        assert_eq!(sphere.projection, Projection::Spherical);
        assert!(sphere.wrap && sphere.w == 2 * sphere.h, "{sphere:?}");
        assert!((sphere.origin[1] + std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    }
}
