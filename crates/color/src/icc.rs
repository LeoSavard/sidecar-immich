// SPDX-License-Identifier: Apache-2.0
//! ICC v4 display profiles synthesised from primaries, a white point and the sRGB curve (ICC.1:2022): what a
//! colour space *is*, written out, instead of a third-party binary. Export embeds one for Rec.2020, the space
//! Sidecar already works in (ROADMAP, "Wide gamut and HDR").
//!
//! The profile states the colorants adapted to the PCS white (D50) with the linear Bradford transform, and the
//! adaptation itself in `chad`, as version 4 requires; the curve is the sRGB function as a parametric type-3
//! curve, the same one every export already encodes with. Checked against the bundled `sRGB-v4.icc`
//! (Compact-ICC-Profiles, CC0): building sRGB from its primaries here gives the same colorants and adaptation.

use crate::mat::{self, M3};

/// CIE 1931 chromaticities.
pub type Xy = [f64; 2];

pub const D65: Xy = [0.3127, 0.3290];
pub const SRGB_PRIMARIES: [Xy; 3] = [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06]];
pub const REC2020_PRIMARIES: [Xy; 3] = [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]];
/// The ICC PCS illuminant, D50, as the specification gives it in s15Fixed16.
pub const PCS_D50: [f64; 3] = [0.9642, 1.0, 0.8249];

fn xyz(c: Xy) -> [f64; 3] {
    [c[0] / c[1], 1.0, (1.0 - c[0] - c[1]) / c[1]]
}

/// RGB → XYZ for these primaries and white, white mapping to Y = 1.
pub fn rgb_to_xyz(primaries: &[Xy; 3], white: Xy) -> M3 {
    let p = primaries.map(xyz);
    let m: M3 = [
        [p[0][0], p[1][0], p[2][0]],
        [p[0][1], p[1][1], p[2][1]],
        [p[0][2], p[1][2], p[2][2]],
    ];
    let s = mat::apply(&mat::inverse(&m), &xyz(white));
    mat::mul(&m, &mat::diag(s))
}

/// The linear Bradford adaptation from `from` to `to` (XYZ, Y = 1).
pub fn bradford(from: [f64; 3], to: [f64; 3]) -> M3 {
    const B: M3 = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let (s, d) = (mat::apply(&B, &from), mat::apply(&B, &to));
    mat::mul(
        &mat::inverse(&B),
        &mat::mul(&mat::diag([d[0] / s[0], d[1] / s[1], d[2] / s[2]]), &B),
    )
}

fn s15(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz_tag(v: [f64; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    for c in v {
        t.extend(s15(c));
    }
    t
}

fn mluc(text: &str) -> Vec<u8> {
    let s: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
    let mut t = b"mluc\0\0\0\0".to_vec();
    t.extend(1u32.to_be_bytes());
    t.extend(12u32.to_be_bytes());
    t.extend(b"enUS");
    t.extend((s.len() as u32).to_be_bytes());
    t.extend(28u32.to_be_bytes());
    t.extend(s);
    t
}

/// The sRGB function as ICC parametric curve type 3: Y = (aX + b)^g above d, cX below.
fn srgb_curve() -> Vec<u8> {
    let mut t = b"para\0\0\0\0".to_vec();
    t.extend(3u16.to_be_bytes());
    t.extend(0u16.to_be_bytes());
    for v in [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045] {
        t.extend(s15(v));
    }
    t
}

/// A version 4 RGB display profile with these primaries and white, and the sRGB curve.
pub fn rgb_profile(description: &str, primaries: &[Xy; 3], white: Xy) -> Vec<u8> {
    let adapt = bradford(xyz(white), PCS_D50);
    let m = mat::mul(&adapt, &rgb_to_xyz(primaries, white));
    let col = |i: usize| [m[0][i], m[1][i], m[2][i]];
    let chad: Vec<u8> = {
        let mut t = b"sf32\0\0\0\0".to_vec();
        for row in adapt {
            for v in row {
                t.extend(s15(v));
            }
        }
        t
    };
    let curve = srgb_curve();
    // Tag signature and data; the three curves share one copy, as the specification allows.
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", mluc(description)),
        (b"cprt", mluc("No copyright, use freely (CC0)")),
        (b"wtpt", xyz_tag(PCS_D50)),
        (b"chad", chad),
        (b"rXYZ", xyz_tag(col(0))),
        (b"gXYZ", xyz_tag(col(1))),
        (b"bXYZ", xyz_tag(col(2))),
        (b"rTRC", curve.clone()),
        (b"gTRC", curve.clone()),
        (b"bTRC", curve),
    ];
    let table = 4 + 12 * tags.len();
    let mut data = Vec::new();
    let mut entries = Vec::new();
    let mut shared_curve: Option<(u32, u32)> = None;
    for (sig, bytes) in &tags {
        let is_curve = sig.ends_with(b"TRC");
        let (off, len) = match (is_curve, shared_curve) {
            (true, Some(at)) => at,
            _ => {
                let off = (128 + table + data.len()) as u32;
                data.extend(bytes);
                while data.len() % 4 != 0 {
                    data.push(0);
                }
                let at = (off, bytes.len() as u32);
                if is_curve {
                    shared_curve = Some(at);
                }
                at
            }
        };
        entries.push((**sig, off, len));
    }
    let size = 128 + table + data.len();
    let mut out = Vec::with_capacity(size);
    out.extend((size as u32).to_be_bytes());
    out.extend([0u8; 4]); // preferred CMM: none
    out.extend([4, 0x30, 0, 0]); // version 4.3
    out.extend(b"mntrRGB XYZ ");
    for v in [2026u16, 11, 6, 0, 0, 0] {
        out.extend(v.to_be_bytes());
    }
    out.extend(b"acsp");
    out.extend([0u8; 4 + 4 + 4 + 4 + 8]); // platform, flags, manufacturer, model, attributes
    out.extend(0u32.to_be_bytes()); // rendering intent: perceptual
    for v in PCS_D50 {
        out.extend(s15(v));
    }
    out.extend([0u8; 4]); // creator
    out.extend([0u8; 16]); // profile ID: not computed, which version 4 allows
    out.extend([0u8; 28]);
    out.extend((entries.len() as u32).to_be_bytes());
    for (sig, off, len) in entries {
        out.extend(sig);
        out.extend(off.to_be_bytes());
        out.extend(len.to_be_bytes());
    }
    out.extend(data);
    out
}

/// Rec.2020: ITU-R BT.2020 primaries, D65, with the sRGB curve every export encodes with.
pub fn rec2020() -> Vec<u8> {
    rgb_profile("Rec.2020 (sRGB curve)", &REC2020_PRIMARIES, D65)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tags of an ICC profile: signature → bytes.
    fn tags(p: &[u8]) -> std::collections::HashMap<[u8; 4], &[u8]> {
        let be = |o: usize| u32::from_be_bytes(p[o..o + 4].try_into().unwrap()) as usize;
        (0..be(128))
            .map(|i| {
                let e = 132 + i * 12;
                let (off, len) = (be(e + 4), be(e + 8));
                (p[e..e + 4].try_into().unwrap(), &p[off..off + len])
            })
            .collect()
    }

    fn s15s(t: &[u8]) -> Vec<f64> {
        t[8..]
            .chunks(4)
            .map(|c| i32::from_be_bytes(c.try_into().unwrap()) as f64 / 65536.0)
            .collect()
    }

    /// Built from sRGB's primaries and D65, the profile states what the bundled sRGB profile states: the same
    /// colorants and adaptation, to a few units of s15Fixed16's last place (rounding of the published values).
    #[test]
    fn synthesised_srgb_matches_the_bundled_profile() {
        let bundled = include_bytes!("../../../assets/icc/sRGB-v4.icc");
        let ours = rgb_profile("sRGB", &SRGB_PRIMARIES, D65);
        let (b, o) = (tags(bundled), tags(&ours));
        assert_eq!(
            u32::from_be_bytes(ours[0..4].try_into().unwrap()) as usize,
            ours.len()
        );
        for sig in [b"rXYZ", b"gXYZ", b"bXYZ", b"wtpt", b"chad"] {
            let (bv, ov) = (s15s(b[sig]), s15s(o[sig]));
            let worst = bv
                .iter()
                .zip(&ov)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0, f64::max);
            assert!(
                worst < 6e-4,
                "{}: {bv:?} against ours {ov:?}",
                String::from_utf8_lossy(sig)
            );
        }
        assert_eq!(
            &o[b"rTRC"][..12],
            b"para\0\0\0\0\0\x03\0\0",
            "a type-3 parametric curve"
        );
    }

    /// Rec.2020's colorants sum to the PCS white, and its matrix has BT.2020's luminance coefficients.
    #[test]
    fn rec2020_profile_is_consistent() {
        let p = rec2020();
        let t = tags(&p);
        let sum: Vec<f64> = (0..3)
            .map(|i| {
                [b"rXYZ", b"gXYZ", b"bXYZ"]
                    .iter()
                    .map(|s| s15s(t[*s])[i])
                    .sum()
            })
            .collect();
        for (a, b) in sum.iter().zip(PCS_D50) {
            assert!((a - b).abs() < 2e-4, "colorants sum to {sum:?}");
        }
        // Before adaptation, the luminance row is BT.2020's own coefficients: 0.2627, 0.6780, 0.0593.
        let m = rgb_to_xyz(&REC2020_PRIMARIES, D65);
        for (got, want) in m[1].iter().zip([0.2627, 0.6780, 0.0593]) {
            assert!((got - want).abs() < 1e-4, "luminance row {:?}", m[1]);
        }
    }
}
