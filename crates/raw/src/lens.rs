// SPDX-License-Identifier: Apache-2.0
//! Lens corrections a RAW file carries itself, read through ExifTool and converted to the one radial model in
//! `sidecar_color::lens`. What the file says wins over any database: it was measured for this lens, often for
//! this focal length and aperture, by the people who made it.
//!
//! Sources, and how each reading was checked rather than assumed (the numbers are in the tests below):
//!
//! - **DNG opcodes** — `WarpRectilinear` (distortion, per-plane lateral CA) and `FixVignetteRadial`, as the
//!   DNG specification (1.7, chapter 7) defines them. Lightroom writes them into the DNGs it converts from
//!   cameras whose own tables it trusts; iPhone ProRAW carries them too.
//! - **Sony ARW** — `DistortionCorrParams` and `ChromaticAberrationCorrParams` in the RAW's SubIFD. Checked
//!   against Lightroom's `WarpRectilinear` for the same exposures (a ZV-1 at 9.4 mm and at 15.9 mm, kept as
//!   ARW and as a Lightroom-made DNG): distortion is `1 + v / 2^14` on knots evenly spaced from the centre to
//!   the corner, agreeing to within Lightroom's own polynomial fit (within a pixel on a 20 MP frame); red
//!   and blue CA are `1 + v / 2^21` relative to green on the same knots, agreeing in shape and size. The
//!   camera's own JPEG confirms the distortion reading a second way (`sidecar-dev camera-jpeg`): aligned to
//!   the RAW through the table, 36 of 36 ZV-1 frames correlate better than without it (median 0.979 against
//!   0.848), at a zoom of 0.995 — the table maps straight onto the frame the camera renders.
//!
//!   `VignettingCorrParams` is *not* used. The same comparison shows the ZV-1's JPEG brightening its corners
//!   by nothing relative to the RAW (median +0.006 EV at the corner over 36 frames) where every plausible
//!   reading of the table asks for 0.3–0.8 EV, and the RAW's own falloff averaged over 376 frames is nearly
//!   flat: this body writes shading-corrected sensor data, and the table would brighten the corners twice.
//!   The ILCE-7RM4's JPEG does lift its corners, by 0–0.3 EV where its table reads 0.8–1.6 EV — partial, and
//!   no reading of the table fits both bodies.
//! - **Fujifilm RAF** — `VignettingParams` and `ChromaticAberrationParams` in the RAF's own directory.
//!   Vignetting (percent of centre brightness on knots from 0 to the corner of the recorded frame) matches
//!   Lightroom's `FixVignetteRadial` for the same X100F exposure to 0.3 %. CA has the sign and shape of
//!   Lightroom's, at about 1.6× its size (0.037 % against 0.022 % at the corner, ~1.3 px against ~0.8 px on a
//!   24 MP frame): the camera's figure is used as it stands. `GeometricDistortionParams` is used only when
//!   zero — every X100F in this library, a fixed-lens camera with no distortion to speak of — because no pair
//!   with a non-zero table exists to fix its units.
//!
//! Anything else — Nikon, Canon, Olympus, Panasonic — carries only on/off flags or unverified tables, and
//! falls back to the lensfun database, matched on what ExifTool identifies as the lens (`identity`).

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sidecar_color::lens::{Curve, LensProfile};

/// ExifTool arguments for everything this module reads, for a caller running ExifTool anyway. `-G1` names
/// the IFD each tag came from, which picks the RAW's own opcodes over a preview's; `-b` hands binary opcode
/// lists over as base64 — as long as they are asked for numerically (`#`), or ExifTool prints their names.
pub const TAGS: &[&str] = &[
    "-G1",
    "-b",
    "-SubIFD:DistortionCorrParams",
    "-SubIFD:ChromaticAberrationCorrParams",
    "-SubIFD:DistortionCorrection",
    "-FujiIFD:GeometricDistortionParams",
    "-FujiIFD:ChromaticAberrationParams",
    "-FujiIFD:VignettingParams",
    "-RAF:RawImageCroppedSize",
    "-OpcodeList2#",
    "-OpcodeList3#",
    "-SubfileType",
    "-ImageWidth",
    "-ImageHeight",
    "-Composite:LensID",
    "-LensModel",
    "-LensMake",
    "-Composite:ScaleFactor35efl",
    "-FocusDistance",
    "-ExifIFD:SubjectDistance",
];

/// What ExifTool knows about the lens, for matching against lensfun and for the editor to show.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LensIdentity {
    /// ExifTool's `LensID`: the maker's lens-number tables resolved to a name, which is more specific than
    /// the plain `LensModel` string many bodies write (and the only name Nikon DSLRs give at all).
    pub lens_id: Option<String>,
    pub lens_model: Option<String>,
    pub lens_make: Option<String>,
    /// Sensor crop factor relative to 35 mm.
    pub crop_factor: Option<f64>,
    /// Metres; absent or infinite when the camera doesn't say.
    pub focus_distance_m: Option<f64>,
}

fn text<'a>(tags: &'a Value, key: &str) -> Option<&'a str> {
    tags.get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "-" && *s != "n/a")
}

fn number(tags: &Value, key: &str) -> Option<f64> {
    let v = tags.get(key)?;
    v.as_f64().or_else(|| {
        v.as_str()?
            .split_whitespace()
            .next()?
            .trim_end_matches('m')
            .parse()
            .ok()
    })
}

fn numbers(tags: &Value, key: &str) -> Option<Vec<f64>> {
    let v = tags.get(key)?;
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    let out: Vec<f64> = s
        .split_whitespace()
        .filter_map(|t| t.parse().ok())
        .collect();
    (!out.is_empty()).then_some(out)
}

/// The first key ending in `:name` — `-G1` prefixes every tag with the group it came from.
fn find<'a>(tags: &'a Value, name: &str) -> Option<(&'a str, &'a Value)> {
    tags.as_object()?
        .iter()
        .find(|(k, _)| k.rsplit(':').next() == Some(name))
        .map(|(k, v)| (k.as_str(), v))
}

fn find_text<'a>(tags: &'a Value, name: &str) -> Option<&'a str> {
    let (k, _) = find(tags, name)?;
    text(tags, k)
}

/// The same tags without their `-G1` group prefixes (the first of each name wins), for readers written
/// against plain names — `af::from_tags` — when one ExifTool run served both.
pub fn ungrouped(tags: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for (k, v) in tags.as_object().into_iter().flatten() {
        out.entry(k.rsplit(':').next().unwrap_or(k).to_string())
            .or_insert_with(|| v.clone());
    }
    Value::Object(out)
}

pub fn identity(tags: &Value) -> LensIdentity {
    LensIdentity {
        lens_id: find_text(tags, "LensID").map(String::from),
        lens_model: find_text(tags, "LensModel").map(String::from),
        lens_make: find_text(tags, "LensMake").map(String::from),
        crop_factor: find(tags, "ScaleFactor35efl")
            .and_then(|(k, _)| number(tags, k))
            .filter(|v| *v > 0.1),
        focus_distance_m: find(tags, "FocusDistance")
            .or_else(|| find(tags, "SubjectDistance"))
            .and_then(|(k, _)| number(tags, k))
            .filter(|v| v.is_finite() && *v > 0.0),
    }
}

/// The correction the file carries, if any. `size` is the decoded image's width and height before
/// orientation — the frame every radius here is normalized to.
pub fn from_tags(tags: &Value, size: [u32; 2]) -> Option<LensProfile> {
    dng(tags, size)
        .or_else(|| sony(tags))
        .or_else(|| fujifilm(tags, size))
}

fn half_diag(w: f64, h: f64) -> f64 {
    (w * w + h * h).sqrt() / 2.0
}

// ---- DNG

/// An opcode from a DNG opcode list (big-endian; DNG 1.7 §7).
#[derive(Clone, Debug, PartialEq)]
pub enum Opcode {
    /// Per plane: `kr0..kr3` (radial) and `kt0, kt1` (tangential); optical centre, normalized.
    WarpRectilinear {
        planes: Vec<[f64; 6]>,
        center: [f64; 2],
    },
    /// Gain `1 + k0 r² + k1 r⁴ + … + k4 r¹⁰`; optical centre, normalized.
    FixVignetteRadial {
        k: [f64; 5],
        center: [f64; 2],
    },
    Other(u32),
}

pub fn parse_opcodes(d: &[u8]) -> Option<Vec<Opcode>> {
    let u32_at = |o: usize| Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?));
    let f64_at = |o: usize| Some(f64::from_be_bytes(d.get(o..o + 8)?.try_into().ok()?));
    let count = u32_at(0)?;
    let mut off = 4;
    let mut out = Vec::new();
    for _ in 0..count.min(256) {
        let (id, size) = (u32_at(off)?, u32_at(off + 12)? as usize);
        let p = off + 16;
        out.push(match id {
            1 => {
                let n = u32_at(p)? as usize;
                if n == 0 || n > 4 {
                    return None;
                }
                let planes = (0..n)
                    .map(|i| {
                        Some(
                            [0, 1, 2, 3, 4, 5]
                                .map(|j| f64_at(p + 4 + 48 * i + 8 * j).unwrap_or(0.0)),
                        )
                    })
                    .collect::<Option<Vec<_>>>()?;
                let c = p + 4 + 48 * n;
                Opcode::WarpRectilinear {
                    planes,
                    center: [f64_at(c)?, f64_at(c + 8)?],
                }
            }
            3 => Opcode::FixVignetteRadial {
                k: [0, 1, 2, 3, 4].map(|j| f64_at(p + 8 * j).unwrap_or(0.0)),
                center: [f64_at(p + 40)?, f64_at(p + 48)?],
            },
            other => Opcode::Other(other),
        });
        off = p + size;
    }
    Some(out)
}

/// Radius in the model (1 = the image's half-diagonal) → radius in an opcode's normalization (1 = the
/// farthest corner from its centre).
fn opcode_radius(center: [f64; 2], size: [u32; 2]) -> f64 {
    let (w, h) = (size[0] as f64, size[1] as f64);
    let (cx, cy) = (center[0] * w, center[1] * h);
    let m = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)]
        .iter()
        .map(|(x, y)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt())
        .fold(0.0, f64::max);
    half_diag(w, h) / m
}

pub fn from_opcodes(list2: &[Opcode], list3: &[Opcode], size: [u32; 2]) -> Option<LensProfile> {
    let warp = list3.iter().find_map(|o| match o {
        Opcode::WarpRectilinear { planes, center } => Some((planes, *center)),
        _ => None,
    });
    let vignette = list2.iter().chain(list3).find_map(|o| match o {
        Opcode::FixVignetteRadial { k, center } => Some((*k, *center)),
        _ => None,
    });
    if warp.is_none() && vignette.is_none() {
        return None;
    }
    const N: usize = 97;
    let r_max = sidecar_color::lens::R_MAX;
    let radial =
        |c: &[f64; 6], r: f64| c[0] + c[1] * r.powi(2) + c[2] * r.powi(4) + c[3] * r.powi(6);
    let mut p = LensProfile {
        id: "dng".into(),
        label: "From the file (DNG)".into(),
        center: warp
            .map(|w| w.1)
            .or(vignette.map(|v| v.1))
            .unwrap_or([0.5, 0.5]),
        distortion: None,
        ca_red: None,
        ca_blue: None,
        vignetting: None,
        required: false,
    };
    if let Some((planes, center)) = warp {
        let s = opcode_radius(center, size);
        // One plane covers all three channels; three are red, green, blue. Tangential terms are dropped: the
        // model is radial, and every file surveyed carries them as zero.
        let green = &planes[if planes.len() >= 3 { 1 } else { 0 }];
        p.distortion = Some(Curve::sample(N, r_max, |r| radial(green, r * s))).filter(departs);
        if planes.len() >= 3 {
            p.ca_red = Some(Curve::sample(N, r_max, |r| {
                radial(&planes[0], r * s) / radial(green, r * s)
            }))
            .filter(departs);
            p.ca_blue = Some(Curve::sample(N, r_max, |r| {
                radial(&planes[2], r * s) / radial(green, r * s)
            }))
            .filter(departs);
        }
    }
    if let Some((k, center)) = vignette {
        let s = opcode_radius(center, size);
        p.vignetting = Some(Curve::sample(N, r_max, |r| {
            let r2 = (r * s).powi(2);
            1.0 + k
                .iter()
                .enumerate()
                .map(|(i, k)| k * r2.powi(i as i32 + 1))
                .sum::<f64>()
        }))
        .filter(departs);
    }
    (p.distortion.is_some() || p.ca_red.is_some() || p.ca_blue.is_some() || p.vignetting.is_some())
        .then_some(p)
}

/// A curve that does something: an identity plane (an X100F's green, next to its red CA) would cost a
/// resample and change nothing.
fn departs(c: &Curve) -> bool {
    c.0.iter().any(|p| (p[1] - 1.0).abs() > 1e-6)
}

/// The RAW image's opcode lists. A DNG may carry opcodes for its previews too, so the lists come from the
/// IFD that holds the full-resolution image (SubfileType 0), or else from any IFD that has a warp.
fn dng(tags: &Value, size: [u32; 2]) -> Option<LensProfile> {
    let obj = tags.as_object()?;
    let groups: Vec<&str> = obj
        .keys()
        .filter_map(|k| {
            k.strip_suffix(":OpcodeList3")
                .or_else(|| k.strip_suffix(":OpcodeList2"))
        })
        .collect();
    let full = |g: &str| {
        obj.get(&format!("{g}:SubfileType")).is_some_and(|v| {
            v.as_str().is_some_and(|s| s.starts_with("Full-resolution")) || v.as_u64() == Some(0)
        })
    };
    let decode = |key: String| -> Vec<Opcode> {
        obj.get(&key)
            .and_then(Value::as_str)
            .and_then(|s| {
                base64::engine::general_purpose::STANDARD
                    .decode(s.strip_prefix("base64:").unwrap_or(s))
                    .ok()
            })
            .and_then(|b| parse_opcodes(&b))
            .unwrap_or_default()
    };
    let mut candidates: Vec<&str> = groups.iter().copied().filter(|g| full(g)).collect();
    candidates.extend(groups.iter().copied().filter(|g| !full(g)));
    candidates.into_iter().find_map(|g| {
        from_opcodes(
            &decode(format!("{g}:OpcodeList2")),
            &decode(format!("{g}:OpcodeList3")),
            size,
        )
    })
}

// ---- Sony

/// A table with its length first, as the RAW's SubIFD writes it: `11 1265 1218 …` → the 11 values.
fn counted(v: &[f64]) -> Option<&[f64]> {
    let n = *v.first()? as usize;
    v.get(1..1 + n).filter(|t| t.len() >= 2)
}

fn knots(values: &[f64], f: impl Fn(f64) -> f64) -> Curve {
    let n = values.len();
    Curve(
        values
            .iter()
            .enumerate()
            .map(|(i, &v)| [i as f64 / (n - 1) as f64, f(v)])
            .collect(),
    )
}

pub fn from_sony(
    distortion: Option<&[f64]>,
    ca: Option<&[f64]>,
    fixed_by_lens: bool,
) -> Option<LensProfile> {
    let distortion = distortion
        .and_then(counted)
        .filter(|t| t.iter().any(|&v| v != 0.0));
    let ca = ca
        .and_then(counted)
        .filter(|t| t.len() % 2 == 0 && t.len() >= 4 && t.iter().any(|&v| v != 0.0));
    if distortion.is_none() && ca.is_none() {
        return None;
    }
    let (red, blue) = match ca {
        Some(t) => {
            let (r, b) = t.split_at(t.len() / 2);
            (
                Some(knots(r, |v| 1.0 + v / 2f64.powi(21))),
                Some(knots(b, |v| 1.0 + v / 2f64.powi(21))),
            )
        }
        None => (None, None),
    };
    Some(LensProfile {
        id: "sony".into(),
        label: "From the file (Sony)".into(),
        center: [0.5, 0.5],
        distortion: distortion.map(|t| knots(t, |v| 1.0 + v / 2f64.powi(14))),
        ca_red: red,
        ca_blue: blue,
        vignetting: None,
        required: fixed_by_lens,
    })
}

fn sony(tags: &Value) -> Option<LensProfile> {
    let fixed =
        find_text(tags, "DistortionCorrection").is_some_and(|s| s.contains("fixed by lens"));
    from_sony(
        numbers(tags, "SubIFD:DistortionCorrParams").as_deref(),
        numbers(tags, "SubIFD:ChromaticAberrationCorrParams").as_deref(),
        fixed,
    )
}

// ---- Fujifilm

/// `scale` turns the tables' radius (1 = corner of the recorded frame) into the model's (1 = corner of the
/// decoded image, which can include a margin the camera crops away).
pub fn from_fujifilm(
    geometric: Option<&[f64]>,
    ca: Option<&[f64]>,
    vignetting: Option<&[f64]>,
    scale: f64,
) -> Option<LensProfile> {
    // [spacing, 11 radii, 11 values]; [spacing, 10 radii, 10 red, 10 blue]; [spacing, 11 radii, 11 values].
    let pairs = |v: &[f64], n: usize, from: usize| -> Option<Vec<[f64; 2]>> {
        (v.len() >= from + n).then(|| (0..n).map(|i| [v[1 + i] * scale, v[from + i]]).collect())
    };
    if geometric.is_some_and(|g| g.iter().skip(12).any(|&v| v != 0.0)) {
        // Units unverified (see the module notes): no correction beats a wrong one.
        return None;
    }
    let vignetting = vignetting
        .and_then(|v| pairs(v, 11, 12))
        .filter(|k| k.iter().all(|p| p[1] > 1.0))
        .map(|k| {
            let mut k: Vec<[f64; 2]> = k.into_iter().map(|[r, pct]| [r, 100.0 / pct]).collect();
            if k[0][0] > 0.0 {
                k.insert(0, [0.0, 1.0]);
            }
            Curve(k)
        });
    let ca = ca.filter(|v| v.len() >= 31).map(|v| {
        let curve = |from: usize| {
            let mut k: Vec<[f64; 2]> = (0..10)
                .map(|i| [v[1 + i] * scale, 1.0 + v[from + i]])
                .collect();
            k.insert(0, [0.0, k[0][1]]);
            Curve(k)
        };
        (curve(11), curve(21))
    });
    let ca = ca.filter(|(r, b)| r.0.iter().chain(&b.0).any(|p| p[1] != 1.0));
    if vignetting.is_none() && ca.is_none() {
        return None;
    }
    Some(LensProfile {
        id: "fujifilm".into(),
        label: "From the file (Fujifilm)".into(),
        center: [0.5, 0.5],
        distortion: None,
        ca_red: ca.as_ref().map(|c| c.0.clone()),
        ca_blue: ca.map(|c| c.1),
        vignetting,
        required: false,
    })
}

fn fujifilm(tags: &Value, size: [u32; 2]) -> Option<LensProfile> {
    let recorded = find_text(tags, "RawImageCroppedSize").and_then(|s| {
        let (w, h) = s.split_once('x')?;
        Some((w.trim().parse::<f64>().ok()?, h.trim().parse::<f64>().ok()?))
    });
    let (w, h) = (size[0].max(size[1]) as f64, size[0].min(size[1]) as f64);
    let scale = recorded.map_or(1.0, |(rw, rh)| {
        half_diag(rw.max(rh), rw.min(rh)) / half_diag(w, h)
    });
    from_fujifilm(
        numbers(tags, "FujiIFD:GeometricDistortionParams").as_deref(),
        numbers(tags, "FujiIFD:ChromaticAberrationParams").as_deref(),
        numbers(tags, "FujiIFD:VignettingParams").as_deref(),
        scale,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> Vec<Opcode> {
        parse_opcodes(&base64::engine::general_purpose::STANDARD.decode(s).unwrap()).unwrap()
    }

    // Lightroom's opcodes for DSC03425 (ZV-1 at 9.4 mm) and for DSCF5152 (X100F), from the DNGs Lightroom made
    // of those exposures; the Sony and Fujifilm tables below are from the original RAWs.
    const ZV1_WARP: &str = "AAAAAQAAAAEBAwAAAAAAAAAAAKQAAAADP/FBzIXHiqK/0zoX0qdjkD/E9+UzhrCAv6fT0TVsCYAAAAAAAAAAAAAAAAAAAAAAP/FBpInH4xO/0z13A5Sv5j/E5OgeJVi0v6ep5aZ5IC4AAAAAAAAAAAAAAAAAAAAAP/FESOseKP6/02DvPHdGWD/FM45RC1+Av6gbKC53AAAAAAAAAAAAAAAAAAAAAAAAP+AAAAAAAAA/4AAAAAAAAA==";
    const X100F_VIGNETTE: &str = "AAAAAQAAAAMBAwAAAAAAAQAAADg/0gUP+Rm74r/RZ5aVGuzQP/rCozY3YiO//GOhS+VjWj/k6HRKTf+jP+AAAAAAAAA/4AAAAAAAAA==";
    const X100F_WARP: &str = "AAAAAQAAAAEBAwAAAAAAAQAAAKQAAAADP/AAmCY/7kg/PlrafmNS4b9KnMpLvLURPzwa6c+ZhzcAAAAAAAAAAAAAAAAAAAAAP/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAP/AAAAAAAAA84FdsPdSoc7z5CRE7ePvOPPEL5+9qqwEAAAAAAAAAAAAAAAAAAAAAP9/qU/qU/qU/4AQOtVFGoA==";
    const ZV1_DISTORTION: [f64; 17] = [
        11.0, 1265.0, 1218.0, 1083.0, 860.0, 562.0, 209.0, -179.0, -580.0, -972.0, -1353.0,
        -1725.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ];
    const ZV1_CA: [f64; 33] = [
        22.0, 0.0, 0.0, 128.0, 128.0, 128.0, 256.0, 384.0, 512.0, 896.0, 1152.0, 1536.0, 0.0,
        896.0, 1024.0, 1024.0, 768.0, 512.0, 256.0, 0.0, 128.0, 0.0, 256.0, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ];
    const X100F_GEOMETRIC: [f64; 23] = [
        327.7272727,
        0.0,
        0.1,
        0.2,
        0.3,
        0.4,
        0.5,
        0.6,
        0.7,
        0.8,
        0.9,
        1.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
    ];
    const X100F_CA: [f64; 31] = [
        360.5, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 0.000138, 0.000259, 0.000342,
        0.000372, 0.000366, 0.000365, 0.000366, 0.000366, 0.000366, 0.000366, 0.0, 0.0, 0.0, 0.0,
        0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    ];
    const X100F_VIGNETTING: [f64; 23] = [
        327.7272727,
        0.0,
        0.1,
        0.2,
        0.3,
        0.4,
        0.5,
        0.6,
        0.7,
        0.8,
        0.9,
        1.0,
        100.0,
        99.51,
        98.81,
        97.7,
        95.96,
        93.33,
        89.57,
        84.7,
        78.61,
        71.83,
        64.7,
    ];

    #[test]
    fn parses_the_dng_opcodes_lightroom_writes() {
        let ops = b64(ZV1_WARP);
        let Opcode::WarpRectilinear { planes, center } = &ops[0] else {
            panic!("{ops:?}")
        };
        assert_eq!(planes.len(), 3);
        assert_eq!(*center, [0.5, 0.5]);
        assert!((planes[1][0] - 1.0785260564864003).abs() < 1e-12);
        let ops = b64(X100F_VIGNETTE);
        let Opcode::FixVignetteRadial { k, .. } = &ops[0] else {
            panic!("{ops:?}")
        };
        assert!((k[0] - 0.28155898405287705).abs() < 1e-12);
    }

    /// Sony's own distortion table and Lightroom's warp for the same ZV-1 exposure describe the same lens:
    /// the pixel each reads for a corrected point agrees to within a pixel (0.85 px at worst, 70 % of the way out) across the 5,472-pixel
    /// frame (Lightroom fits a polynomial through the table, which bends near the centre).
    #[test]
    fn sony_distortion_agrees_with_lightroom_for_the_same_exposure() {
        let size = [5496, 3672];
        let adobe = from_opcodes(&[], &b64(ZV1_WARP), size).unwrap();
        let sony = from_sony(Some(&ZV1_DISTORTION), Some(&ZV1_CA), true).unwrap();
        let hd = half_diag(5496.0, 3672.0);
        for i in 0..=50 {
            let r = i as f64 / 50.0;
            let (a, s) = (
                adobe.distortion.as_ref().unwrap().eval(r),
                sony.distortion.as_ref().unwrap().eval(r),
            );
            let px = (a - s).abs() * r * hd;
            assert!(
                px < 1.0,
                "r = {r}: Lightroom reads {:.2} px, Sony {:.2} px — {px:.2} px apart",
                a * r * hd,
                s * r * hd
            );
        }
        // Lateral CA: same direction and size. Red reads outward, increasingly so towards the corner.
        let corner = |p: &LensProfile| p.ca_red.as_ref().unwrap().eval(1.0) - 1.0;
        let (a, s) = (corner(&adobe), corner(&sony));
        assert!(
            a > 0.0 && s > 0.0 && (s / a) > 0.7 && (s / a) < 1.5,
            "red CA at the corner: Lightroom {a:.2e}, Sony {s:.2e}"
        );
        assert!(sony.required);
    }

    /// Fujifilm's vignetting table and Lightroom's `FixVignetteRadial` for the same X100F exposure agree to
    /// 0.3 %. The DNG's centre sits a few pixels off the middle of the active area (its default crop's
    /// centre), and its radius is normalized to the farthest corner — both of which `from_opcodes` undoes.
    #[test]
    fn fujifilm_vignetting_agrees_with_lightroom_for_the_same_exposure() {
        let size = [6048, 4038];
        let adobe = from_opcodes(&b64(X100F_VIGNETTE), &b64(X100F_WARP), size).unwrap();
        let scale = half_diag(6000.0, 4000.0) / half_diag(6048.0, 4038.0);
        let fuji = from_fujifilm(
            Some(&X100F_GEOMETRIC),
            Some(&X100F_CA),
            Some(&X100F_VIGNETTING),
            scale,
        )
        .unwrap();
        for i in 0..=20 {
            let r = i as f64 / 20.0 * scale;
            let (a, f) = (
                adobe.vignetting.as_ref().unwrap().eval(r),
                fuji.vignetting.as_ref().unwrap().eval(r),
            );
            assert!(
                (a / f - 1.0).abs() < 0.004,
                "r = {r:.2}: Lightroom gain {a:.4}, Fujifilm {f:.4}"
            );
        }
        assert!(
            fuji.distortion.is_none(),
            "an all-zero distortion table is no correction"
        );
        // CA: same sign, the camera's figure larger than Lightroom's (see the module notes).
        let (a, f) = (
            adobe.ca_red.as_ref().unwrap().eval(scale) - 1.0,
            fuji.ca_red.as_ref().unwrap().eval(scale) - 1.0,
        );
        assert!(
            a > 0.0 && f > a && f < 2.0 * a,
            "red CA at the corner: Lightroom {a:.2e}, Fujifilm {f:.2e}"
        );
        assert_eq!(fuji.ca_blue.as_ref().unwrap().eval(0.5), 1.0);
    }

    #[test]
    fn a_non_zero_fujifilm_distortion_table_is_not_guessed_at() {
        let mut g = X100F_GEOMETRIC;
        g[20] = -0.5;
        assert!(from_fujifilm(Some(&g), Some(&X100F_CA), Some(&X100F_VIGNETTING), 1.0).is_none());
    }

    #[test]
    fn reads_the_profile_and_identity_from_exiftool_json() {
        let tags: Value = serde_json::json!({
            "SourceFile": "DSC03425.ARW",
            "SubIFD:DistortionCorrParams": "11 1265 1218 1083 860 562 209 -179 -580 -972 -1353 -1725 0 0 0 0 0",
            "SubIFD:ChromaticAberrationCorrParams": "22 0 0 128 128 128 256 384 512 896 1152 1536 0 896 1024 1024 768 512 256 0 128 0 256 0 0 0 0 0 0 0 0 0 0",
            "SubIFD:DistortionCorrection": "Auto fixed by lens",
            "Composite:LensID": "E 24-70mm F1.8-2.8",
            "Composite:ScaleFactor35efl": 2.7,
            "Sony:FocusDistance2": "1.2 m",
        });
        let p = from_tags(&tags, [5504, 3672]).unwrap();
        assert_eq!(p.id, "sony");
        assert!(
            p.required && p.distortion.is_some() && p.ca_red.is_some() && p.vignetting.is_none()
        );
        let id = identity(&tags);
        assert_eq!(id.lens_id.as_deref(), Some("E 24-70mm F1.8-2.8"));
        assert_eq!(id.crop_factor, Some(2.7));

        let dng: Value = serde_json::json!({
            "IFD0:SubfileType": "Reduced-resolution image",
            "SubIFD:SubfileType": "Full-resolution image",
            "SubIFD:OpcodeList3": format!("base64:{ZV1_WARP}"),
            "SubIFD3:OpcodeList2": "base64:AAAAAA==",
        });
        assert_eq!(from_tags(&dng, [5496, 3672]).unwrap().id, "dng");
        assert!(
            from_tags(
                &serde_json::json!({ "Nikon:VignetteControl": "Normal" }),
                [6000, 4000]
            )
            .is_none()
        );
    }
}
