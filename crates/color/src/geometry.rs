// SPDX-License-Identifier: Apache-2.0
//! Crop, straighten, keystone and quarter turns (DESIGN.md §15), as one projective map that every renderer
//! shares: the editor's view pass, its histogram, and the worker's export, preview and loupe.
//!
//! Three frames, in the order the pixels go through them:
//!
//! - the *picture*: oriented and lens-corrected, what the develop pass renders, `(u, v)` in 0…1;
//! - the *plane*: the picture after keystone, aspect and straighten. Crop coordinates live here, scaled so
//!   that with nothing else set the picture spans 0…1 on both axes — `Crop::default()` keeps it all;
//! - the *output*: the crop, then turned and mirrored, `(s, t)` in 0…1.
//!
//! The maps are written output → picture, which is the direction a renderer samples in. Distances are
//! isotropic in between: one unit is the picture's height, so a rotation is a rotation however wide it is.

use serde::{Deserialize, Serialize};

use crate::mat::{self, M3};

/// Virtual tilt at ±100 on Vertical or Horizontal, degrees.
pub const MAX_TILT_DEG: f64 = 30.0;
/// The virtual camera's focal length for keystone, in picture diagonals — about a 35 mm lens. Aspect corrects
/// the stretch a different lens would have given.
pub const FOCAL_DIAGONALS: f64 = 0.8;
/// Aspect at ±100: the output's height scaled by 2^±this.
pub const ASPECT_STOPS: f64 = 0.5;
pub const MAX_ANGLE_DEG: f64 = 45.0;

/// A crop in the plane: fractions of the picture's width and height before any turn.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct Crop {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Default for Crop {
    fn default() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        }
    }
}

impl Crop {
    fn width(&self) -> f64 {
        self.right - self.left
    }
    fn height(&self) -> f64 {
        self.bottom - self.top
    }
    fn center(&self) -> [f64; 2] {
        [
            (self.left + self.right) / 2.0,
            (self.top + self.bottom) / 2.0,
        ]
    }
    fn around(c: [f64; 2], w: f64, h: f64) -> Self {
        Self {
            left: c[0] - w / 2.0,
            top: c[1] - h / 2.0,
            right: c[0] + w / 2.0,
            bottom: c[1] + h / 2.0,
        }
    }
    fn corners(&self) -> [[f64; 2]; 4] {
        [
            [self.left, self.top],
            [self.right, self.top],
            [self.right, self.bottom],
            [self.left, self.bottom],
        ]
    }
    fn near(&self, o: &Crop) -> bool {
        [
            self.left - o.left,
            self.top - o.top,
            self.right - o.right,
            self.bottom - o.bottom,
        ]
        .iter()
        .all(|d| d.abs() < 1e-6)
    }
}

/// Stored as `sc:Geometry`. Absent means none: every photo edited before it existed renders unchanged.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct Geometry {
    /// Quarter turns clockwise, applied after the crop.
    pub rotate: u32,
    /// Mirrored left to right, after the turn.
    pub flip: bool,
    /// Straighten, degrees clockwise, −45…45.
    pub angle: f64,
    /// Keystone, −100…100: positive corrects a camera tilted up (stretches the top), negative one tilted down.
    pub vertical: f64,
    /// Positive stretches the right side, for a wall receding to the right.
    pub horizontal: f64,
    /// −100…100: positive makes the picture taller, for the stretch keystone leaves.
    pub aspect: f64,
    pub crop: Crop,
    /// Width ÷ height the crop is held to, in the output's orientation; 0 leaves it free. Kept so the lock
    /// is still on when the photo is opened again.
    pub ratio: f64,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            rotate: 0,
            flip: false,
            angle: 0.0,
            vertical: 0.0,
            horizontal: 0.0,
            aspect: 0.0,
            crop: Crop::default(),
            ratio: 0.0,
        }
    }
}

impl Geometry {
    /// Nothing to do: the output is the picture.
    pub fn is_identity(&self) -> bool {
        let g = self;
        g.rotate.is_multiple_of(4)
            && !g.flip
            && g.angle == 0.0
            && g.vertical == 0.0
            && g.horizontal == 0.0
            && g.aspect == 0.0
            && g.crop.near(&Crop::default())
    }

    /// Whether the plane's frame keeps its shape: no straighten or keystone, so any crop of 0…1 is inside.
    fn is_square_on(&self) -> bool {
        self.angle == 0.0 && self.vertical == 0.0 && self.horizontal == 0.0 && self.aspect == 0.0
    }
}

/// Plane coordinates (0…1 of the picture) → isotropic, centred, in picture heights. `a` is width ÷ height.
fn to_iso(a: f64) -> M3 {
    [[a, 0.0, -a / 2.0], [0.0, 1.0, -0.5], [0.0, 0.0, 1.0]]
}

fn rx(t: f64) -> M3 {
    let (s, c) = t.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

fn ry(t: f64) -> M3 {
    let (s, c) = t.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

/// Straightened plane → picture, isotropic: undo the straighten, the aspect, then the keystone.
fn plane_to_picture(g: &Geometry, a: f64) -> M3 {
    let f = FOCAL_DIAGONALS * (a * a + 1.0).sqrt();
    let k = mat::diag([f, f, 1.0]);
    let k_inv = mat::diag([1.0 / f, 1.0 / f, 1.0]);
    let tilt = |v: f64| -(v.clamp(-100.0, 100.0) / 100.0 * MAX_TILT_DEG).to_radians();
    // A virtual camera turned by the tilt sees the picture's rays from the output's: K · R · K⁻¹.
    let keystone = mat::mul(
        &k,
        &mat::mul(
            &mat::mul(&ry(tilt(g.horizontal)), &rx(tilt(g.vertical))),
            &k_inv,
        ),
    );
    let s = 2f64.powf(g.aspect.clamp(-100.0, 100.0) / 100.0 * ASPECT_STOPS);
    let stretch = mat::diag([1.0, 1.0 / s, 1.0]);
    // y points down, so the usual rotation matrix turns clockwise on screen; this undoes it.
    let (sn, cs) = g
        .angle
        .clamp(-MAX_ANGLE_DEG, MAX_ANGLE_DEG)
        .to_radians()
        .sin_cos();
    let unrotate = [[cs, sn, 0.0], [-sn, cs, 0.0], [0.0, 0.0, 1.0]];
    mat::mul(&keystone, &mat::mul(&stretch, &unrotate))
}

/// Output → crop coordinates before the turn and the mirror.
fn untwist(g: &Geometry) -> M3 {
    let unflip = if g.flip {
        [[-1.0, 0.0, 1.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    } else {
        mat::diag([1.0; 3])
    };
    let unturn = match g.rotate % 4 {
        1 => [[0.0, 1.0, 0.0], [-1.0, 0.0, 1.0], [0.0, 0.0, 1.0]],
        2 => [[-1.0, 0.0, 1.0], [0.0, -1.0, 1.0], [0.0, 0.0, 1.0]],
        3 => [[0.0, -1.0, 1.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        _ => mat::diag([1.0; 3]),
    };
    mat::mul(&unturn, &unflip)
}

/// Output (0…1) → picture (0…1), projective. `a` is the picture's width ÷ height.
pub fn to_picture(g: &Geometry, a: f64) -> M3 {
    let c = &g.crop;
    let crop = [
        [c.width(), 0.0, c.left],
        [0.0, c.height(), c.top],
        [0.0, 0.0, 1.0],
    ];
    let plane = mat::mul(
        &mat::inverse(&to_iso(a)),
        &mat::mul(&plane_to_picture(g, a), &to_iso(a)),
    );
    mat::mul(&plane, &mat::mul(&crop, &untwist(g)))
}

/// A point through a projective map.
pub fn map(m: &M3, p: [f64; 2]) -> [f64; 2] {
    let v = mat::apply(m, &[p[0], p[1], 1.0]);
    [v[0] / v[2], v[1] / v[2]]
}

/// The picture's outline in crop coordinates: its corners, clockwise from top-left.
pub fn quad(g: &Geometry, a: f64) -> [[f64; 2]; 4] {
    let to_plane = mat::mul(
        &mat::inverse(&to_iso(a)),
        &mat::mul(&mat::inverse(&plane_to_picture(g, a)), &to_iso(a)),
    );
    [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].map(|p| map(&to_plane, p))
}

pub fn bounds(quad: &[[f64; 2]; 4]) -> Crop {
    let (mut c, f) = (
        Crop {
            left: f64::MAX,
            top: f64::MAX,
            right: f64::MIN,
            bottom: f64::MIN,
        },
        quad,
    );
    for p in f {
        (c.left, c.top, c.right, c.bottom) = (
            c.left.min(p[0]),
            c.top.min(p[1]),
            c.right.max(p[0]),
            c.bottom.max(p[1]),
        );
    }
    c
}

/// Inside a convex quad, with a hair of slack so a crop fitted to an edge counts as inside it.
fn inside(quad: &[[f64; 2]; 4], p: [f64; 2]) -> bool {
    let mut sign = 0.0;
    for i in 0..4 {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        let cross = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        if cross.abs() < 1e-9 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            if (cross / len).abs() > 1e-7 {
                return false;
            }
        }
    }
    true
}

fn crop_inside(quad: &[[f64; 2]; 4], c: &Crop) -> bool {
    c.corners().iter().all(|&p| inside(quad, p))
}

/// The picture's output size in pixels, for a picture of `size`.
pub fn output_size(g: &Geometry, size: [u32; 2]) -> [u32; 2] {
    let w = ((g.crop.width() * size[0] as f64).round() as u32).max(1);
    let h = ((g.crop.height() * size[1] as f64).round() as u32).max(1);
    if g.rotate % 2 == 1 { [h, w] } else { [w, h] }
}

/// Width ÷ height in crop coordinates of a crop whose output is `ratio` wide, for a picture `a` wide.
fn crop_ratio(g: &Geometry, ratio: f64, a: f64) -> f64 {
    let before_turn = if g.rotate % 2 == 1 {
        1.0 / ratio
    } else {
        ratio
    };
    before_turn / a
}

/// The largest crop of shape `r` (crop units) centred at `c` that fits, and its height.
fn largest_at(quad: &[[f64; 2]; 4], c: [f64; 2], r: f64) -> f64 {
    if !inside(quad, c) {
        return 0.0;
    }
    let (mut lo, mut hi) = (0.0, 4.0);
    for _ in 0..48 {
        let mid = (lo + hi) / 2.0;
        if crop_inside(quad, &Crop::around(c, mid * r, mid)) {
            lo = mid
        } else {
            hi = mid
        }
    }
    lo
}

/// The largest crop of shape `r` anywhere inside: a pattern search for the centre, from the quad's.
fn largest(quad: &[[f64; 2]; 4], r: f64) -> Crop {
    let mut c = [
        quad.iter().map(|p| p[0]).sum::<f64>() / 4.0,
        quad.iter().map(|p| p[1]).sum::<f64>() / 4.0,
    ];
    let mut best = largest_at(quad, c, r);
    let mut step = 0.05;
    while step > 1e-5 {
        let mut moved = false;
        for d in [[step, 0.0], [-step, 0.0], [0.0, step], [0.0, -step]] {
            let t = [c[0] + d[0], c[1] + d[1]];
            let k = largest_at(quad, t, r);
            if k > best + 1e-9 {
                (c, best, moved) = (t, k, true);
            }
        }
        if !moved {
            step /= 2.0;
        }
    }
    Crop::around(c, best * r, best)
}

/// What the crop is when nobody has drawn one: the largest of the photo's shape (or the locked ratio) the
/// straightened and keystoned picture fills.
pub fn auto_crop(g: &Geometry, a: f64) -> Crop {
    let r = if g.ratio > 0.0 {
        crop_ratio(g, g.ratio, a)
    } else {
        1.0
    };
    if g.is_square_on() && r == 1.0 {
        return Crop::default();
    }
    largest(&quad(g, a), r)
}

/// `g` with its crop made to fit after a change from `before`: a crop nobody drew follows the geometry
/// (straightening further shrinks it, straightening back grows it again); a drawn one keeps its place and
/// shape and only shrinks about its centre as far as it has to.
pub fn fit(g: &Geometry, before: &Geometry, a: f64) -> Geometry {
    let mut out = g.clone();
    let q = quad(g, a);
    if before.crop.near(&auto_crop(before, a)) && g.ratio == before.ratio {
        out.crop = auto_crop(g, a);
        return out;
    }
    let mut c = g.crop;
    if g.ratio > 0.0 {
        // A newly locked ratio: the same area, in the new shape.
        let r = crop_ratio(g, g.ratio, a);
        let h = (c.width() * c.height() / r).sqrt();
        c = Crop::around(c.center(), h * r, h);
    }
    if crop_inside(&q, &c) {
        out.crop = c;
        return out;
    }
    let r = c.width() / c.height();
    let mut center = c.center();
    if !inside(&q, center) {
        center = [
            q.iter().map(|p| p[0]).sum::<f64>() / 4.0,
            q.iter().map(|p| p[1]).sum::<f64>() / 4.0,
        ];
    }
    let h = largest_at(&q, center, r).min(c.height());
    out.crop = Crop::around(center, h * r, h);
    out
}

/// Everything a renderer or the editor needs about one geometry over a picture of `size` pixels.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct Frame {
    /// Output (0…1) → picture (0…1), row-major, projective.
    pub to_picture: M3,
    pub from_picture: M3,
    /// The output's size in pixels.
    pub size: [u32; 2],
    /// The picture's outline in crop coordinates, clockwise from its top-left corner.
    pub quad: [[f64; 2]; 4],
    /// The smallest crop holding the whole picture: what the crop tool shows around the crop.
    pub bounds: Crop,
    /// The crop nobody drew, for "Reset crop".
    pub auto: Crop,
}

pub fn frame(g: &Geometry, size: [u32; 2]) -> Frame {
    let a = size[0] as f64 / size[1].max(1) as f64;
    let m = to_picture(g, a);
    let quad = quad(g, a);
    Frame {
        from_picture: mat::inverse(&m),
        to_picture: m,
        size: output_size(g, size),
        bounds: bounds(&quad),
        quad,
        auto: auto_crop(g, a),
    }
}

/// Oriented (0…1) → the sensor's own orientation (0…1), for a texture the worker hasn't oriented yet. `flip` is
/// LibRaw's bit set, as `RgbaF32::oriented` and `lens::oriented_center` apply it.
pub fn sensor_from_oriented(flip: i32) -> M3 {
    let swap = if flip & 4 != 0 {
        [[0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]
    } else {
        mat::diag([1.0; 3])
    };
    let fx = if flip & 1 != 0 {
        [[-1.0, 0.0, 1.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    } else {
        mat::diag([1.0; 3])
    };
    let fy = if flip & 2 != 0 {
        [[1.0, 0.0, 0.0], [0.0, -1.0, 1.0], [0.0, 0.0, 1.0]]
    } else {
        mat::diag([1.0; 3])
    };
    mat::mul(&mat::mul(&fx, &fy), &swap)
}

/// Byte-for-byte mirror of `struct WarpParams` in shaders/server/warp.wgsl (64 bytes); checked by naga.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WarpParams {
    /// Output pixel (its centre) → input texel coordinates, as WGSL `mat3x3f` columns.
    pub m: [[f32; 4]; 3],
    pub out_size: [u32; 2],
    pub _pad: [u32; 2],
}

/// The warp pass's uniforms. The output is the rectangle at `out_offset` of `out_size` within the geometry's
/// output (`frame_out` pixels); the input texture holds the picture (`picture` pixels, oriented) in the sensor's
/// orientation `flip`, starting at `in_offset` in that orientation's pixels.
pub fn warp_params(
    g: &Geometry,
    picture: [u32; 2],
    flip: i32,
    frame_out: [u32; 2],
    out_offset: [f64; 2],
    out_size: [u32; 2],
    in_offset: [f64; 2],
) -> WarpParams {
    let a = picture[0] as f64 / picture[1] as f64;
    let sensor = if flip & 4 != 0 {
        [picture[1], picture[0]]
    } else {
        picture
    };
    let out_px = [
        [
            1.0 / frame_out[0] as f64,
            0.0,
            out_offset[0] / frame_out[0] as f64,
        ],
        [
            0.0,
            1.0 / frame_out[1] as f64,
            out_offset[1] / frame_out[1] as f64,
        ],
        [0.0, 0.0, 1.0],
    ];
    let in_px = [
        [sensor[0] as f64, 0.0, -in_offset[0]],
        [0.0, sensor[1] as f64, -in_offset[1]],
        [0.0, 0.0, 1.0],
    ];
    let m = mat::mul(
        &in_px,
        &mat::mul(
            &sensor_from_oriented(flip),
            &mat::mul(&to_picture(g, a), &out_px),
        ),
    );
    WarpParams {
        m: [0, 1, 2].map(|j| [m[0][j] as f32, m[1][j] as f32, m[2][j] as f32, 0.0]),
        out_size,
        _pad: [0; 2],
    }
}

/// The input pixels an output rectangle reads, `(x0, y0, x1, y1)` in the input's pixels (before `in_offset`),
/// from its edges — a projective map takes a rectangle's edge to its image's edge.
pub fn warp_bounds(
    g: &Geometry,
    picture: [u32; 2],
    flip: i32,
    frame_out: [u32; 2],
    rect: (u32, u32, u32, u32),
) -> (f64, f64, f64, f64) {
    let p = warp_params(
        g,
        picture,
        flip,
        frame_out,
        [rect.0 as f64, rect.1 as f64],
        [rect.2, rect.3],
        [0.0; 2],
    );
    let m: M3 = [0, 1, 2].map(|r| [0, 1, 2].map(|c| p.m[c][r] as f64));
    let (w, h) = (rect.2 as f64, rect.3 as f64);
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for i in 0..=32 {
        let t = i as f64 / 32.0;
        for q in [[w * t, 0.0], [w * t, h], [0.0, h * t], [w, h * t]] {
            let s = map(&m, q);
            (x0, y0, x1, y1) = (x0.min(s[0]), y0.min(s[1]), x1.max(s[0]), y1.max(s[1]));
        }
    }
    (x0, y0, x1, y1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: f64 = 1.5;

    fn close(p: [f64; 2], q: [f64; 2]) -> bool {
        (p[0] - q[0]).abs() < 1e-9 && (p[1] - q[1]).abs() < 1e-9
    }

    #[test]
    fn nothing_set_is_the_identity() {
        let g = Geometry::default();
        assert!(g.is_identity());
        let m = to_picture(&g, A);
        for p in [[0.0, 0.0], [0.3, 0.7], [1.0, 1.0]] {
            assert!(close(map(&m, p), p));
        }
        assert_eq!(output_size(&g, [3000, 2000]), [3000, 2000]);
        assert_eq!(auto_crop(&g, A), Crop::default());
        assert!(close(quad(&g, A)[2], [1.0, 1.0]));
    }

    #[test]
    fn the_crop_is_where_the_output_reads() {
        let g = Geometry {
            crop: Crop {
                left: 0.1,
                top: 0.2,
                right: 0.6,
                bottom: 0.9,
            },
            ..Default::default()
        };
        let m = to_picture(&g, A);
        assert!(close(map(&m, [0.0, 0.0]), [0.1, 0.2]));
        assert!(close(map(&m, [1.0, 1.0]), [0.6, 0.9]));
        assert_eq!(output_size(&g, [3000, 2000]), [1500, 1400]);
    }

    #[test]
    fn a_quarter_turn_puts_the_top_left_top_right() {
        let g = Geometry {
            rotate: 1,
            ..Default::default()
        };
        let m = to_picture(&g, A);
        assert!(close(map(&m, [1.0, 0.0]), [0.0, 0.0]));
        assert!(close(map(&m, [1.0, 1.0]), [1.0, 0.0]));
        assert_eq!(output_size(&g, [3000, 2000]), [2000, 3000]);
        for k in 0..4 {
            let g = Geometry {
                rotate: k,
                flip: k % 2 == 1,
                ..Default::default()
            };
            let m = to_picture(&g, A);
            let back = mat::mul(&mat::inverse(&m), &m);
            assert!(close(map(&back, [0.3, 0.8]), [0.3, 0.8]));
        }
        let mirrored = Geometry {
            flip: true,
            ..Default::default()
        };
        assert!(close(
            map(&to_picture(&mirrored, A), [0.0, 0.0]),
            [1.0, 0.0]
        ));
    }

    /// Clockwise: what is east of the centre in the output came from north-east of it in the picture.
    #[test]
    fn a_positive_angle_turns_the_picture_clockwise() {
        let g = Geometry {
            angle: 10.0,
            ..Default::default()
        };
        let p = map(&to_picture(&g, A), [0.75, 0.5]);
        assert!(p[0] > 0.5 && p[1] < 0.5, "{p:?}");
        assert!(close(map(&to_picture(&g, A), [0.5, 0.5]), [0.5, 0.5]));
    }

    fn span_x(m: &M3, t: f64) -> f64 {
        map(m, [0.9, t])[0] - map(m, [0.1, t])[0]
    }

    #[test]
    fn keystone_stretches_the_side_it_names() {
        let m = to_picture(
            &Geometry {
                vertical: 50.0,
                ..Default::default()
            },
            A,
        );
        assert!(
            span_x(&m, 0.1) < span_x(&m, 0.9),
            "top {} bottom {}",
            span_x(&m, 0.1),
            span_x(&m, 0.9)
        );
        let m = to_picture(
            &Geometry {
                vertical: -50.0,
                ..Default::default()
            },
            A,
        );
        assert!(span_x(&m, 0.1) > span_x(&m, 0.9));
        let m = to_picture(
            &Geometry {
                horizontal: 50.0,
                ..Default::default()
            },
            A,
        );
        let span_y = |s: f64| map(&m, [s, 0.9])[1] - map(&m, [s, 0.1])[1];
        assert!(
            span_y(0.9) < span_y(0.1),
            "right {} left {}",
            span_y(0.9),
            span_y(0.1)
        );
        let m = to_picture(
            &Geometry {
                aspect: 50.0,
                ..Default::default()
            },
            A,
        );
        assert!(
            map(&m, [0.5, 0.0])[1] > 0.0,
            "taller output reads less of the height"
        );
    }

    #[test]
    fn straightening_crops_to_the_largest_frame_of_the_same_shape() {
        let g = Geometry {
            angle: 8.0,
            ..Default::default()
        };
        let c = auto_crop(&g, A);
        let q = quad(&g, A);
        assert!(crop_inside(&q, &c));
        assert!(!crop_inside(
            &q,
            &Crop::around(c.center(), c.width() * 1.01, c.height() * 1.01)
        ));
        assert!(
            (c.width() / c.height() - 1.0).abs() < 1e-6,
            "same shape as the photo"
        );
        assert!((c.center()[0] - 0.5).abs() < 1e-3 && (c.center()[1] - 0.5).abs() < 1e-3);
        // The crop nobody drew follows the angle, there and back.
        let tilted = fit(&g, &Geometry::default(), A);
        assert!(tilted.crop.near(&c));
        let back = fit(
            &Geometry {
                angle: 0.0,
                ..tilted.clone()
            },
            &tilted,
            A,
        );
        assert_eq!(back.crop, Crop::default());
    }

    #[test]
    fn a_drawn_crop_keeps_its_place_until_it_must_shrink() {
        let drawn = Crop {
            left: 0.4,
            top: 0.4,
            right: 0.6,
            bottom: 0.6,
        };
        let before = Geometry {
            crop: drawn,
            ..Default::default()
        };
        let after = fit(
            &Geometry {
                angle: 5.0,
                ..before.clone()
            },
            &before,
            A,
        );
        assert_eq!(after.crop, drawn);
        let corner = Crop {
            left: 0.0,
            top: 0.0,
            right: 0.3,
            bottom: 0.3,
        };
        let before = Geometry {
            crop: corner,
            ..Default::default()
        };
        let after = fit(
            &Geometry {
                angle: 10.0,
                ..before.clone()
            },
            &before,
            A,
        );
        assert!(crop_inside(&quad(&after, A), &after.crop));
        assert!((after.crop.width() / after.crop.height() - 1.0).abs() < 1e-9);
        assert!(after.crop.width() < 0.3);
    }

    #[test]
    fn a_locked_ratio_is_in_the_output_orientation() {
        let g = Geometry {
            ratio: 1.0,
            ..Default::default()
        };
        let c = auto_crop(&g, A);
        let size = output_size(
            &Geometry {
                crop: c,
                ..g.clone()
            },
            [3000, 2000],
        );
        assert!(
            (size[0] as f64 / size[1] as f64 - 1.0).abs() < 1e-3,
            "{size:?}"
        );
        let g = Geometry {
            ratio: 4.0 / 5.0,
            rotate: 1,
            ..Default::default()
        };
        let size = output_size(
            &Geometry {
                crop: auto_crop(&g, A),
                ..g.clone()
            },
            [3000, 2000],
        );
        assert!(
            (size[0] as f64 / size[1] as f64 - 0.8).abs() < 1e-3,
            "{size:?}"
        );
    }

    #[test]
    fn sensor_from_oriented_undoes_the_orientation() {
        for flip in [0, 1, 2, 3, 4, 5, 6, 7] {
            let c = [0.2, 0.7];
            let o = crate::lens::oriented_center(c, flip);
            assert!(close(map(&sensor_from_oriented(flip), o), c), "flip {flip}");
        }
    }

    /// The warp reads the texel the frame says, in the sensor's orientation and from a cropped input.
    #[test]
    fn warp_params_place_the_input() {
        let g = Geometry {
            crop: Crop {
                left: 0.5,
                top: 0.0,
                right: 1.0,
                bottom: 0.5,
            },
            ..Default::default()
        };
        let pic = [400, 200];
        let out = output_size(&g, pic);
        let p = warp_params(&g, pic, 0, out, [0.0; 2], out, [0.0; 2]);
        let m: M3 = [0, 1, 2].map(|r| [0, 1, 2].map(|c| p.m[c][r] as f64));
        assert!(close(map(&m, [0.0, 0.0]), [200.0, 0.0]));
        // A picture shown turned (flip 6: the sensor is 200 × 400), read from a crop of it starting at (10, 20).
        let p = warp_params(
            &Geometry::default(),
            pic,
            6,
            pic,
            [0.0; 2],
            pic,
            [10.0, 20.0],
        );
        let m: M3 = [0, 1, 2].map(|r| [0, 1, 2].map(|c| p.m[c][r] as f64));
        let o = map(&m, [0.0, 0.0]);
        let s = map(&sensor_from_oriented(6), [0.0, 0.0]);
        assert!(
            close(o, [s[0] * 200.0 - 10.0, s[1] * 400.0 - 20.0]),
            "{o:?}"
        );
    }

    #[test]
    fn warp_layout_matches_wgsl() {
        let src = sidecar_shaders::compose("server/warp.wgsl").unwrap();
        let module = naga::front::wgsl::parse_str(&src).unwrap();
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("WarpParams"))
            .unwrap();
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("not a struct")
        };
        assert_eq!(*span as usize, std::mem::size_of::<WarpParams>());
        let off = |n: &str| {
            members
                .iter()
                .find(|m| m.name.as_deref() == Some(n))
                .unwrap()
                .offset as usize
        };
        assert_eq!(off("m"), std::mem::offset_of!(WarpParams, m));
        assert_eq!(off("out_size"), std::mem::offset_of!(WarpParams, out_size));
    }
}
