// SPDX-License-Identifier: Apache-2.0
//! Small row-major 3×3 helpers in f64. Rows are written the way colour-science papers print matrices.

pub type M3 = [[f64; 3]; 3];
pub type V3 = [f64; 3];

pub fn mul(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

pub fn apply(m: &M3, v: &V3) -> V3 {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

pub fn inverse(m: &M3) -> M3 {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let (ca, cb, cc) = (e * i - f * h, -(d * i - f * g), d * h - e * g);
    let det = a * ca + b * cb + c * cc;
    [
        [ca / det, -(b * i - c * h) / det, (b * f - c * e) / det],
        [cb / det, (a * i - c * g) / det, -(a * f - c * d) / det],
        [cc / det, -(a * h - b * g) / det, (a * e - b * d) / det],
    ]
}

pub fn diag(v: V3) -> M3 {
    [[v[0], 0.0, 0.0], [0.0, v[1], 0.0], [0.0, 0.0, v[2]]]
}

pub fn lerp(a: &M3, b: &M3, t: f64) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = a[i][j] * t + b[i][j] * (1.0 - t);
        }
    }
    r
}

pub fn xy_to_xyz(xy: [f64; 2]) -> V3 {
    [xy[0] / xy[1], 1.0, (1.0 - xy[0] - xy[1]) / xy[1]]
}

pub fn xyz_to_xy(v: &V3) -> [f64; 2] {
    let s = v[0] + v[1] + v[2];
    [v[0] / s, v[1] / s]
}

pub const D65: [f64; 2] = [0.3127, 0.3290];

/// RGB → XYZ for an RGB space given by its primaries' and white's chromaticities: each primary's XYZ, scaled so
/// that (1, 1, 1) lands on the white at Y = 1 (SMPTE RP 177).
pub fn rgb_to_xyz(primaries: [[f64; 2]; 3], white: [f64; 2]) -> M3 {
    let p = primaries.map(xy_to_xyz);
    let cols = [
        [p[0][0], p[1][0], p[2][0]],
        [p[0][1], p[1][1], p[2][1]],
        [p[0][2], p[1][2], p[2][2]],
    ];
    let s = apply(&inverse(&cols), &xy_to_xyz(white));
    mul(&cols, &diag(s))
}

/// Adobe RGB (1998) primaries, as Adobe's specification of the space gives them; its white is D65.
pub const ADOBE_RGB_PRIMARIES: [[f64; 2]; 3] = [[0.64, 0.33], [0.21, 0.71], [0.15, 0.06]];

pub const XYZ_TO_REC2020: M3 = [
    [1.716651188, -0.3556707838, -0.2533662814],
    [-0.6666843518, 1.6164812366, 0.0157685458],
    [0.0176398574, -0.0427706133, 0.9421031212],
];

pub const REC2020_TO_SRGB: M3 = [
    [1.6604910021, -0.5876411388, -0.0728498633],
    [-0.1245504745, 1.1328998971, -0.0083494226],
    [-0.0181507634, -0.100578898, 1.1187296614],
];

pub const REC2020_TO_P3: M3 = [
    [1.3435782526, -0.2821796705, -0.0613985821],
    [-0.0652974528, 1.0757879158, -0.0104904631],
    [0.0028217873, -0.0195984945, 1.0167767073],
];

const BRADFORD: M3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// Bradford chromatic adaptation from `src` white to `dst` white (both chromaticities).
pub fn bradford(src: [f64; 2], dst: [f64; 2]) -> M3 {
    let s = apply(&BRADFORD, &xy_to_xyz(src));
    let d = apply(&BRADFORD, &xy_to_xyz(dst));
    mul(
        &inverse(&BRADFORD),
        &mul(&diag([d[0] / s[0], d[1] / s[1], d[2] / s[2]]), &BRADFORD),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The primaries route reproduces the sRGB matrix written out above, so it can be trusted for Adobe RGB.
    #[test]
    fn primaries_give_the_srgb_matrix() {
        let srgb = rgb_to_xyz([[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]], D65);
        let want = inverse(&mul(&REC2020_TO_SRGB, &XYZ_TO_REC2020));
        for (r, row) in srgb.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                assert!(
                    (v - want[r][c]).abs() < 1e-4,
                    "{r},{c}: {v} vs {}",
                    want[r][c]
                );
            }
        }
    }

    #[test]
    fn inverse_round_trips() {
        let i = mul(&XYZ_TO_REC2020, &inverse(&XYZ_TO_REC2020));
        for (r, row) in i.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                assert!((v - if r == c { 1.0 } else { 0.0 }).abs() < 1e-9);
            }
        }
    }
}
