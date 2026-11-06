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

/// ACES AP1's primaries (ACEScg, SMPTE ST 2065-1 / S-2014-004) and the ACES white point.
pub const AP1_PRIMARIES: [[f64; 2]; 3] = [[0.713, 0.293], [0.165, 0.830], [0.128, 0.044]];
pub const ACES_WHITE: [f64; 2] = [0.32168, 0.33767];

/// Linear Rec.2020 (D65) → ACEScg, adapting D65 to the ACES white by Bradford, so a neutral stays neutral:
/// (1, 1, 1) in, (1, 1, 1) out. `develop/gamut.wgsl` holds it as a constant; a test checks the two agree.
pub fn rec2020_to_ap1() -> M3 {
    mul(
        &inverse(&rgb_to_xyz(AP1_PRIMARIES, ACES_WHITE)),
        &mul(&bradford(D65, ACES_WHITE), &inverse(&XYZ_TO_REC2020)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The A/B's measurable half: monochromatic lights from 400 to 700 nm — the most saturated colours there are,
    /// what LEDs and neon approach — taken into linear Rec.2020, compressed as `develop/gamut.wgsl` does in
    /// Rec.2020 (before `PROCESS_VERSION` 9) and in ACEScg (since), and each result's OKLab hue compared with
    /// the light's own. Compressing where the thresholds were derived must not shift hue more.
    #[test]
    fn compressing_in_ap1_keeps_spectral_hues_at_least_as_well() {
        let (thr, lim, pwr) = ([0.815, 0.803, 0.880], [1.147, 1.264, 1.312], 1.2f64);
        let curve = |d: f64, l: f64, t: f64| {
            if d < t {
                return d;
            }
            let scl = (l - t) / (((1.0 - t) / (l - t)).powf(-pwr) - 1.0).powf(1.0 / pwr);
            let nd = (d - t) / scl;
            t + scl * nd / (1.0 + nd.powf(pwr)).powf(1.0 / pwr)
        };
        let compress = |c: V3| -> V3 {
            let ach = c[0].max(c[1]).max(c[2]);
            if ach <= 0.0 {
                return [0.0; 3];
            }
            std::array::from_fn(|k| ach - curve((ach - c[k]) / ach, lim[k], thr[k]) * ach)
        };
        let to_ap1 = rec2020_to_ap1();
        let from_ap1 = inverse(&to_ap1);
        let rec_to_xyz = inverse(&XYZ_TO_REC2020);
        let hue = |xyz: V3| {
            let m1 = [
                [0.8189330101, 0.3618667424, -0.1288597137],
                [0.0329845436, 0.9293118715, 0.0361456387],
                [0.0482003018, 0.2643662691, 0.6338517070],
            ];
            let lms = apply(&m1, &xyz).map(|v| v.max(0.0).cbrt());
            let a = 1.9779984951 * lms[0] - 2.4285922050 * lms[1] + 0.4505937099 * lms[2];
            let b = 0.0259040371 * lms[0] + 0.7827717662 * lms[1] - 0.8086757660 * lms[2];
            b.atan2(a).to_degrees()
        };
        let diff = |a: f64, b: f64| ((a - b + 540.0) % 360.0 - 180.0).abs();
        let (mut old, mut new, mut n) = (0.0, 0.0, 0.0);
        for (i, cmf) in crate::spectral_data::CMF.iter().enumerate() {
            let nm = 380 + 10 * i;
            if !(400..=700).contains(&nm) {
                continue;
            }
            let xyz = cmf.map(|v| v * 0.5 / cmf[1].max(1e-3));
            let rec = apply(&XYZ_TO_REC2020, &xyz);
            if rec.iter().all(|v| *v >= 0.0) {
                continue;
            }
            let h = hue(xyz);
            let a = compress(rec);
            let b = apply(&from_ap1, &compress(apply(&to_ap1, &rec)));
            let (da, db) = (
                diff(hue(apply(&rec_to_xyz, &a)), h),
                diff(hue(apply(&rec_to_xyz, &b)), h),
            );
            eprintln!("{nm} nm: hue shift {da:5.1}° in Rec.2020, {db:5.1}° in ACEScg");
            old += da;
            new += db;
            n += 1.0;
        }
        eprintln!(
            "mean over {n} out-of-gamut lights: {:.2}° in Rec.2020, {:.2}° in ACEScg",
            old / n,
            new / n
        );
        assert!(
            n > 5.0 && new <= old + 1e-9,
            "ACEScg {new} against Rec.2020 {old}"
        );
    }

    /// `develop/gamut.wgsl` holds Rec.2020 → ACEScg and its inverse as constants: they must be this module's.
    #[test]
    fn the_gamut_shader_holds_rec2020_to_ap1() {
        let wgsl = include_str!("../../../shaders/develop/gamut.wgsl");
        let read = |name: &str| -> M3 {
            let at = wgsl.find(&format!("const {name} = mat3x3f(")).expect(name);
            let body = &wgsl[at..wgsl[at..].find(");").unwrap() + at];
            let v: Vec<f64> = body
                .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
                .filter(|t| t.contains('.'))
                .filter_map(|t| t.parse().ok())
                .collect();
            assert_eq!(v.len(), 9, "{name}");
            std::array::from_fn(|r| std::array::from_fn(|c| v[c * 3 + r]))
        };
        let (f, i) = (rec2020_to_ap1(), inverse(&rec2020_to_ap1()));
        for (name, want) in [("REC2020_TO_AP1", f), ("AP1_TO_REC2020", i)] {
            let got = read(name);
            for r in 0..3 {
                for c in 0..3 {
                    assert!((got[r][c] - want[r][c]).abs() < 1e-9, "{name}[{r}][{c}]");
                }
            }
        }
        let n = apply(&f, &[1.0, 1.0, 1.0]);
        assert!(n.iter().all(|v| (v - 1.0).abs() < 1e-8), "neutral {n:?}");
    }

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
