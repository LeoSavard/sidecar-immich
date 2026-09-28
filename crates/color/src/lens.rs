// SPDX-License-Identifier: Apache-2.0
//! Lens correction (DESIGN.md §15): one radial model that every source converts into — DNG opcodes, a
//! camera maker's own tables, the lensfun database — and its packing into the lens pass's uniform buffer.
//!
//! Radii are measured from the optical centre and normalized so that 1 is the image's half-diagonal. Every
//! curve is a function of the radius in the *corrected* image, except vignetting, which happens on the sensor
//! and is a function of the radius a pixel is read from.
//!
//! - distortion: source radius ÷ corrected radius. Below 1 pulls the corners in (corrects barrel).
//! - lateral CA: the red (blue) channel's source radius ÷ green's, at the same corrected radius.
//! - vignetting: the gain that flattens the falloff; 1 at the centre, above 1 towards the corners.

use serde::{Deserialize, Serialize};

use crate::params::Lens;

/// Samples of each curve handed to the shader, evenly spaced over `0 ..= R_MAX`.
pub const KNOTS: usize = 64;
/// Corners sit at 1 when the optical centre is the image centre; an off-centre optical axis, a zoom that
/// shows more than the frame, and pincushion's outward reach all read a little past it.
pub const R_MAX: f64 = 1.5;

/// A function of radius through `(r, value)` points, `r` ascending. Evaluated with the same monotone cubic as
/// the tone curve (`spline::Monotone`), so it never overshoots between a table's knots; held flat past either
/// end.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Curve(pub Vec<[f64; 2]>);

impl Curve {
    /// A curve sampled from a function at `n` evenly spaced radii over `0 ..= r_max`.
    pub fn sample(n: usize, r_max: f64, f: impl Fn(f64) -> f64) -> Self {
        Curve((0..n).map(|i| i as f64 / (n - 1) as f64 * r_max).map(|r| [r, f(r)]).collect())
    }

    pub fn eval(&self, r: f64) -> f64 {
        match self.0.as_slice() {
            [] => 1.0,
            [only] => only[1],
            points => crate::spline::Monotone::new(points).map_or(points[0][1], |c| c.eval(r)),
        }
    }
}

/// Where a correction came from. The editor names it; `sc:Lens/Profile` stores `id`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LensProfile {
    /// Stable identifier: `dng` (the file's own opcodes), `sony` or `fujifilm` (the camera's tables),
    /// `lensfun:<maker>/<model>`.
    pub id: String,
    /// For people: "From the file (Sony)", "lensfun: Nikon AF-S DX 18-55mm…".
    pub label: String,
    /// Optical centre in the unoriented image, 0…1, top-left origin.
    pub center: [f64; 2],
    #[serde(default)]
    pub distortion: Option<Curve>,
    #[serde(default)]
    pub ca_red: Option<Curve>,
    #[serde(default)]
    pub ca_blue: Option<Curve>,
    #[serde(default)]
    pub vignetting: Option<Curve>,
    /// The lens is designed to be corrected in software (Sony's "fixed by lens"): uncorrected, the frame
    /// shows distortion the camera never let anyone see. The editor says so rather than hiding the control.
    #[serde(default)]
    pub required: bool,
}

/// The four curves after the edit's strengths are applied, sampled for the shader, and the zoom that keeps
/// blank edges out of the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Effective {
    /// Per knot: distortion, red CA, blue CA, vignetting gain.
    pub knots: [[f64; 4]; KNOTS],
    pub flags: u32,
    pub zoom: f64,
}

pub const DISTORTION: u32 = 1;
pub const CHROMATIC: u32 = 2;
pub const VIGNETTING: u32 = 4;

fn strength(v: f64) -> f64 {
    (v / 100.0).clamp(0.0, 2.0)
}

impl Effective {
    pub fn radius(i: usize) -> f64 {
        i as f64 / (KNOTS - 1) as f64 * R_MAX
    }

    fn at(&self, r: f64) -> [f64; 4] {
        let x = (r / R_MAX * (KNOTS - 1) as f64).clamp(0.0, (KNOTS - 1) as f64);
        let i = (x.floor() as usize).min(KNOTS - 2);
        let t = x - i as f64;
        let (a, b) = (self.knots[i], self.knots[i + 1]);
        [0, 1, 2, 3].map(|c| a[c] + (b[c] - a[c]) * t)
    }

    /// Where the green channel of a corrected-image point `u` is read from, both relative to the optical
    /// centre in half-diagonals, before the zoom. What the shader does, for tests and the zoom search.
    pub fn source(&self, u: [f64; 2]) -> [f64; 2] {
        let s = self.at((u[0] * u[0] + u[1] * u[1]).sqrt())[0];
        [u[0] * s, u[1] * s]
    }
}

/// Apply the edit's strengths to a profile. `aspect` is width ÷ height of the unoriented image, which sets
/// where its edges are for the zoom search.
pub fn effective(profile: &LensProfile, lens: &Lens, aspect: f64) -> Effective {
    let (sd, sc, sv) = (strength(lens.distortion), strength(lens.lateral_ca), strength(lens.vignetting));
    let mut flags = 0;
    let mut knots = [[1.0; 4]; KNOTS];
    for (i, k) in knots.iter_mut().enumerate() {
        let r = Effective::radius(i);
        if let Some(c) = profile.distortion.as_ref().filter(|_| sd > 0.0) {
            k[0] = 1.0 + sd * (c.eval(r) - 1.0);
            flags |= DISTORTION;
        }
        if sc > 0.0 && (profile.ca_red.is_some() || profile.ca_blue.is_some()) {
            k[1] = profile.ca_red.as_ref().map_or(1.0, |c| 1.0 + sc * (c.eval(r) - 1.0));
            k[2] = profile.ca_blue.as_ref().map_or(1.0, |c| 1.0 + sc * (c.eval(r) - 1.0));
            flags |= CHROMATIC;
        }
        if let Some(c) = profile.vignetting.as_ref().filter(|_| sv > 0.0) {
            // A power, not a blend: half strength is half the correction in stops, which is how falloff is seen.
            k[3] = c.eval(r).max(1e-3).powf(sv);
            flags |= VIGNETTING;
        }
    }
    let mut e = Effective { knots, flags, zoom: 1.0 };
    if flags & (DISTORTION | CHROMATIC) != 0 {
        e.zoom = fill_zoom(&e, profile.center, aspect);
    }
    e
}

/// The widest view whose every edge pixel still reads from inside the photo: the zoom `k` for which the
/// corrected frame, scaled by `k` about the optical centre, maps entirely into the source. Below 1 when the
/// correction pushes outward (pincushion, or a table normalized with its centre stretched); above 1 when
/// barrel correction pulls the corners in and there is more of the photo left to show. Found by bisection,
/// which holds because `r · distortion(r)` rises with `r` for any usable profile.
fn fill_zoom(e: &Effective, center: [f64; 2], aspect: f64) -> f64 {
    // The frame in half-diagonal units, centred on the image centre; the optical centre sits `c` away from it.
    let hd = (aspect * aspect + 1.0).sqrt() / 2.0;
    let (w, h) = (aspect / hd, 1.0 / hd);
    let c = [(center[0] - 0.5) * w, (center[1] - 0.5) * h];
    let (x0, x1, y0, y1) = (-w / 2.0 - c[0], w / 2.0 - c[0], -h / 2.0 - c[1], h / 2.0 - c[1]);
    let boundary: Vec<[f64; 2]> = (0..=64)
        .flat_map(|i| {
            let t = i as f64 / 64.0;
            let (x, y) = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
            [[x, y0], [x, y1], [x0, y], [x1, y]]
        })
        .collect();
    let inside = |k: f64| {
        boundary.iter().all(|&u| {
            let u = [u[0] * k, u[1] * k];
            let v = e.at((u[0] * u[0] + u[1] * u[1]).sqrt());
            let s = v[0] * v[1].max(v[2]).max(1.0);
            let p = [u[0] * s, u[1] * s];
            // Half a texel's worth of slack keeps the clamp at the edge from ever reading as a blank band.
            p[0] >= x0 - 1e-4 && p[0] <= x1 + 1e-4 && p[1] >= y0 - 1e-4 && p[1] <= y1 + 1e-4
        })
    };
    let (mut lo, mut hi) = (0.5, 1.5);
    if !inside(lo) {
        return lo;
    }
    if inside(hi) {
        return hi;
    }
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if inside(mid) { lo = mid } else { hi = mid }
    }
    lo
}

/// Byte-for-byte mirror of `struct LensParams` in shaders/lens/params.wgsl (1,072 bytes); checked by naga.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LensParams {
    /// Optical centre, in frame pixels.
    pub center: [f32; 2],
    /// Frame pixel of the output texture's top-left texel, and of the input texture's.
    pub out_offset: [f32; 2],
    pub in_offset: [f32; 2],
    pub half_diag: f32,
    pub zoom: f32,
    /// Knots per unit radius.
    pub r_scale: f32,
    pub flags: u32,
    pub _pad: [f32; 2],
    pub knots: [[f32; 4]; KNOTS],
}

/// The optical centre in the frame a texture holds: the unoriented image flipped by `flip` (LibRaw's bit
/// set, as `RgbaF32::oriented` applies it). Normalized in, normalized out.
pub fn oriented_center(c: [f64; 2], flip: i32) -> [f64; 2] {
    let u = if flip & 1 != 0 { 1.0 - c[0] } else { c[0] };
    let v = if flip & 2 != 0 { 1.0 - c[1] } else { c[1] };
    if flip & 4 != 0 { [v, u] } else { [u, v] }
}

/// Pack the lens pass's uniforms for one texture. `frame` is the whole image's size in this texture's pixels
/// and orientation, `flip` the orientation applied to reach it from the sensor's; `out_offset` and
/// `in_offset` place the output and input textures within that frame (zero for a whole image; a loupe tile
/// and the source crop it reads set them).
pub fn pack(e: &Effective, center: [f64; 2], frame: [u32; 2], flip: i32, out_offset: [f64; 2], in_offset: [f64; 2]) -> LensParams {
    let c = oriented_center(center, flip);
    let (w, h) = (frame[0] as f64, frame[1] as f64);
    LensParams {
        center: [(c[0] * w) as f32, (c[1] * h) as f32],
        out_offset: out_offset.map(|v| v as f32),
        in_offset: in_offset.map(|v| v as f32),
        half_diag: ((w * w + h * h).sqrt() / 2.0) as f32,
        zoom: e.zoom as f32,
        r_scale: ((KNOTS - 1) as f64 / R_MAX) as f32,
        flags: e.flags,
        _pad: [0.0; 2],
        knots: e.knots.map(|k| k.map(|v| v as f32)),
    }
}

/// The source pixels a corrected rectangle reads: `(x0, y0, x1, y1)` in frame pixels, exclusive at the far
/// end, for the rectangle `(x, y, w, h)` of the corrected image and a frame packed as `pack` would. The image
/// of a rectangle's edge under a continuous one-to-one map is the edge of its image, so walking the edge,
/// with every channel's reach, finds the box. The caller adds whatever its filters need around it.
pub fn source_bounds(p: &LensParams, rect: (u32, u32, u32, u32)) -> (f64, f64, f64, f64) {
    let (x, y, w, h) = (rect.0 as f64, rect.1 as f64, rect.2 as f64, rect.3 as f64);
    let c = [p.center[0] as f64, p.center[1] as f64];
    let (hd, zoom) = (p.half_diag as f64, p.zoom as f64);
    let at = |r: f64| {
        let k = (r * p.r_scale as f64).clamp(0.0, (KNOTS - 1) as f64);
        let i = (k.floor() as usize).min(KNOTS - 2);
        let t = k - i as f64;
        let (a, b) = (p.knots[i], p.knots[i + 1]);
        [0, 1, 2].map(|j| a[j] as f64 + (b[j] as f64 - a[j] as f64) * t)
    };
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let steps = 64;
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        for q in [[x + w * t, y], [x + w * t, y + h], [x, y + h * t], [x + w, y + h * t]] {
            let u = [(q[0] - c[0]) / hd * zoom, (q[1] - c[1]) / hd * zoom];
            let k = at((u[0] * u[0] + u[1] * u[1]).sqrt());
            for s in [k[0], k[0] * k[1], k[0] * k[2]] {
                let (sx, sy) = (c[0] + u[0] * s * hd, c[1] + u[1] * s * hd);
                (x0, y0, x1, y1) = (x0.min(sx), y0.min(sy), x1.max(sx), y1.max(sy));
            }
        }
    }
    (x0, y0, x1, y1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn barrel() -> LensProfile {
        LensProfile {
            id: "test".into(),
            label: "test".into(),
            center: [0.5, 0.5],
            // A ZV-1 at 9.4 mm, from its own table (see sidecar_raw::lens).
            distortion: Some(Curve(
                [1265, 1218, 1083, 860, 562, 209, -179, -580, -972, -1353, -1725].iter().enumerate().map(|(i, &v)| [i as f64 / 10.0, 1.0 + v as f64 / 16384.0]).collect(),
            )),
            ca_red: None,
            ca_blue: None,
            vignetting: None,
            required: true,
        }
    }

    #[test]
    fn curves_pass_through_their_points_and_hold_flat_past_the_ends() {
        let c = Curve(vec![[0.0, 1.0], [0.5, 2.0], [1.0, 1.5]]);
        assert!((c.eval(0.5) - 2.0).abs() < 1e-12);
        assert!((c.eval(1.0) - 1.5).abs() < 1e-12);
        assert_eq!(c.eval(3.0), 1.5);
        // No overshoot above the peak between knots.
        assert!((0..=100).all(|i| c.eval(i as f64 / 100.0) <= 2.0 + 1e-12));
    }

    #[test]
    fn no_profile_or_zero_strength_is_the_identity() {
        let mut p = barrel();
        let off = Lens { distortion: 0.0, lateral_ca: 0.0, vignetting: 0.0, ..Default::default() };
        let e = effective(&p, &off, 1.5);
        assert_eq!((e.flags, e.zoom), (0, 1.0));
        assert!(e.knots.iter().all(|k| *k == [1.0; 4]));
        p.distortion = None;
        let e = effective(&p, &Lens::default(), 1.5);
        assert_eq!((e.flags, e.zoom), (0, 1.0));
    }

    /// Barrel correction pulls the corners in, so the corrected frame can show a little more than the photo
    /// did along the long edge — and never reads past it anywhere.
    #[test]
    fn the_zoom_fills_the_frame_without_blank_edges() {
        let aspect = 1.5;
        let e = effective(&barrel(), &Lens::default(), aspect);
        assert!(e.zoom > 0.9 && e.zoom < 1.2, "zoom {}", e.zoom);
        let hd = (aspect * aspect + 1.0f64).sqrt() / 2.0;
        let (w, h) = (aspect / hd, 1.0 / hd);
        let mut touches = false;
        for i in 0..=200 {
            let t = i as f64 / 200.0;
            for u in [[w * (t - 0.5), -h / 2.0], [w * (t - 0.5), h / 2.0], [-w / 2.0, h * (t - 0.5)], [w / 2.0, h * (t - 0.5)]] {
                let p = e.source([u[0] * e.zoom, u[1] * e.zoom]);
                assert!(p[0].abs() <= w / 2.0 + 1e-3 && p[1].abs() <= h / 2.0 + 1e-3, "{u:?} reads {p:?}");
                touches |= (p[0].abs() - w / 2.0).abs() < 2e-3 || (p[1].abs() - h / 2.0).abs() < 2e-3;
            }
        }
        assert!(touches, "the widest view should reach the photo's edge somewhere");
    }

    #[test]
    fn half_strength_halves_the_displacement_and_the_stops() {
        let mut p = barrel();
        p.vignetting = Some(Curve(vec![[0.0, 1.0], [1.0, 4.0]]));
        let full = effective(&p, &Lens::default(), 1.5);
        let half = effective(&p, &Lens { distortion: 50.0, vignetting: 50.0, ..Default::default() }, 1.5);
        let i = KNOTS * 2 / 3;
        assert!(((half.knots[i][0] - 1.0) - (full.knots[i][0] - 1.0) / 2.0).abs() < 1e-12);
        assert!((half.knots[i][3].log2() - full.knots[i][3].log2() / 2.0).abs() < 1e-12);
    }

    #[test]
    fn the_optical_centre_follows_the_orientation() {
        let c = [0.2, 0.7];
        assert_eq!(oriented_center(c, 0), [0.2, 0.7]);
        let r = oriented_center(c, 3);
        assert!((r[0] - 0.8).abs() < 1e-12 && (r[1] - 0.3).abs() < 1e-12);
        let r = oriented_center(c, 6); // 90° clockwise: x' = 1 − y, y' = x
        assert!((r[0] - 0.3).abs() < 1e-12 && (r[1] - 0.2).abs() < 1e-12);
    }

    #[test]
    fn uniform_layout_matches_wgsl() {
        let src = sidecar_shaders::compose("lens/params.wgsl").unwrap();
        let module = naga::front::wgsl::parse_str(&src).unwrap();
        let (_, ty) = module.types.iter().find(|(_, t)| t.name.as_deref() == Some("LensParams")).unwrap();
        let naga::TypeInner::Struct { members, span } = &ty.inner else { panic!("not a struct") };
        assert_eq!(*span as usize, std::mem::size_of::<LensParams>());
        assert_eq!(std::mem::size_of::<LensParams>(), 1072);
        let off = |n: &str| members.iter().find(|m| m.name.as_deref() == Some(n)).unwrap().offset as usize;
        assert_eq!(off("center"), std::mem::offset_of!(LensParams, center));
        assert_eq!(off("out_offset"), std::mem::offset_of!(LensParams, out_offset));
        assert_eq!(off("in_offset"), std::mem::offset_of!(LensParams, in_offset));
        assert_eq!(off("half_diag"), std::mem::offset_of!(LensParams, half_diag));
        assert_eq!(off("zoom"), std::mem::offset_of!(LensParams, zoom));
        assert_eq!(off("r_scale"), std::mem::offset_of!(LensParams, r_scale));
        assert_eq!(off("flags"), std::mem::offset_of!(LensParams, flags));
        assert_eq!(off("knots"), std::mem::offset_of!(LensParams, knots));
    }
}
