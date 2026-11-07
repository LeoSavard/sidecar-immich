// SPDX-License-Identifier: Apache-2.0
//! 3D LUTs in the `.cube` format (Adobe's Cube LUT Specification 1.0, which Resolve and most grading tools
//! write), applied after develop as a look (DESIGN.md §15, "3D LUTs").
//!
//! A creative LUT maps display-encoded colour to display-encoded colour in one particular space: the space it
//! was built in. The file doesn't say which, so the edit does (`LutSpace`); develop's display-linear output is
//! taken into that space and encoding, through the table, and back. Colour outside the space's gamut is held to
//! its edge on the way in, as the tool that made the table would have held it.

use serde::{Deserialize, Serialize};
use sidecar_uniform::Uniform;

use crate::mat::{self, M3};
use crate::params::{DisplaySpace, to_mat3};

/// The largest table accepted: 256³ entries is 268 MB of floats on the GPU, and no grading tool writes more
/// than 65³.
pub const MAX_SIZE: u32 = 129;

/// A parsed `.cube` file's 3D table. Entries run red fastest, then green, then blue, as the file lists them.
#[derive(Clone, Debug, PartialEq)]
pub struct CubeLut {
    pub title: Option<String>,
    pub size: u32,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub table: Vec<[f32; 3]>,
}

impl CubeLut {
    /// The table as RGBA floats (alpha 1), for an RGBA32F 3D texture `size` on a side.
    pub fn rgba(&self) -> Vec<f32> {
        self.table
            .iter()
            .flat_map(|c| [c[0], c[1], c[2], 1.0])
            .collect()
    }

    /// The header the pass's uniform needs (`LookLut::uniform`), without the table.
    pub fn header(&self) -> CubeHeader {
        CubeHeader {
            size: self.size,
            domain_min: self.domain_min,
            domain_max: self.domain_max,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CubeHeader {
    pub size: u32,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
}

/// Parse a `.cube` file. Keywords other than the specification's are skipped, as the specification asks; a
/// 1D table, or a file with both, is refused rather than half-applied.
pub fn parse_cube(text: &str) -> Result<CubeLut, String> {
    let (mut title, mut size) = (None, None);
    let (mut domain_min, mut domain_max) = ([0.0f32; 3], [1.0f32; 3]);
    let mut table: Vec<[f32; 3]> = Vec::new();
    let triple = |rest: &str, line: usize| -> Result<[f32; 3], String> {
        let v: Vec<f32> = rest
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| format!("line {line}: expected three numbers"))?;
        <[f32; 3]>::try_from(v).map_err(|_| format!("line {line}: expected three numbers"))
    };
    for (n, raw) in text.lines().enumerate() {
        let line = n + 1;
        let l = raw.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (key, rest) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
        let rest = rest.trim();
        match key {
            "TITLE" => title = Some(rest.trim_matches('"').to_string()),
            "LUT_3D_SIZE" => {
                let s: u32 = rest
                    .parse()
                    .map_err(|_| format!("line {line}: LUT_3D_SIZE is not a number"))?;
                if !(2..=MAX_SIZE).contains(&s) {
                    return Err(format!("LUT_3D_SIZE {s} is outside 2…{MAX_SIZE}"));
                }
                size = Some(s);
            }
            "LUT_1D_SIZE" => {
                return Err("this is a 1D LUT; only 3D LUTs are supported".into());
            }
            "DOMAIN_MIN" => domain_min = triple(rest, line)?,
            "DOMAIN_MAX" => domain_max = triple(rest, line)?,
            // Resolve's older form: one range for all three channels.
            "LUT_3D_INPUT_RANGE" => {
                let v: Vec<f32> = rest
                    .split_whitespace()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                let [lo, hi] = <[f32; 2]>::try_from(v)
                    .map_err(|_| format!("line {line}: expected two numbers"))?;
                (domain_min, domain_max) = ([lo; 3], [hi; 3]);
            }
            k if k
                .starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '.' || c == '+') =>
            {
                table.push(triple(l, line)?);
            }
            _ => {}
        }
    }
    let size = size.ok_or("no LUT_3D_SIZE: not a 3D .cube file")?;
    let expected = (size as usize).pow(3);
    if table.len() != expected {
        return Err(format!(
            "LUT_3D_SIZE {size} needs {expected} entries; the file has {}",
            table.len()
        ));
    }
    if (0..3).any(|i| domain_max[i] <= domain_min[i]) {
        return Err("DOMAIN_MAX must exceed DOMAIN_MIN in every channel".into());
    }
    if table.iter().flatten().any(|v| !v.is_finite()) {
        return Err("the table holds a value that is not a number".into());
    }
    Ok(CubeLut {
        title,
        size,
        domain_min,
        domain_max,
        table,
    })
}

/// The space a LUT was built in: its primaries and its encoding, input and output alike.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub enum LutSpace {
    /// sRGB primaries and the sRGB curve (IEC 61966-2-1): what Photoshop and most photo tools export.
    #[default]
    Srgb,
    /// Rec.709 primaries and BT.1886's γ 2.4 with a zero black: what video grading tools build in.
    Rec709,
    /// Display P3: P3 primaries, D65, the sRGB curve.
    DisplayP3,
}

/// A 3D LUT applied as a look, by the id the server stores it under.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct LookLut {
    /// The stored file's id: the first 16 hex digits of its SHA-256.
    pub id: String,
    /// The name it was uploaded under, so the edit still says what it was where the file is missing.
    pub name: String,
    /// 0…100: how much of the LUT's change is taken, mixed in its own encoding.
    pub amount: f64,
    pub space: LutSpace,
}

impl Default for LookLut {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            amount: 100.0,
            space: LutSpace::Srgb,
        }
    }
}

/// The LUT pass's uniform (`client/look.wgsl`, 128 bytes). Its WGSL declaration, `shaders/uniforms/look_params.wgsl`,
/// is generated from this struct (`sidecar-uniform`).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable, Uniform)]
pub struct LookParams {
    /// Develop's display-linear primaries → the LUT space's, linear; and back.
    #[uniform(wgsl = "mat3x3f")]
    pub to_lut: [[f32; 4]; 3],
    #[uniform(wgsl = "mat3x3f")]
    pub from_lut: [[f32; 4]; 3],
    /// The table's input domain, from and to; `domain_min.w` is the amount (0…1), `domain_max.w` the encoding
    /// (0 the sRGB curve, 1 γ 2.4).
    pub domain_min: [f32; 4],
    pub domain_max: [f32; 4],
    /// x: the table's size on a side; y: 1 to paint the colours the table's alpha marks (a soft proof's gamut
    /// warning) mid grey; z, w unused.
    pub size: [f32; 4],
}

/// Linear Rec.2020 → linear `space`'s primaries.
fn from_rec2020(space: LutSpace) -> M3 {
    match space {
        LutSpace::Srgb | LutSpace::Rec709 => mat::REC2020_TO_SRGB,
        LutSpace::DisplayP3 => mat::REC2020_TO_P3,
    }
}

fn display_from_rec2020(display: DisplaySpace) -> M3 {
    match display {
        DisplaySpace::Srgb => mat::REC2020_TO_SRGB,
        DisplaySpace::DisplayP3 => mat::REC2020_TO_P3,
    }
}

impl LookLut {
    /// The pass's uniform for develop's output in `display`, or `None` when the amount leaves nothing to do.
    pub fn uniform(&self, display: DisplaySpace, header: &CubeHeader) -> Option<Vec<u8>> {
        if self.amount <= 0.0 {
            return None;
        }
        let to_lut = mat::mul(
            &from_rec2020(self.space),
            &mat::inverse(&display_from_rec2020(display)),
        );
        let encoding = match self.space {
            LutSpace::Rec709 => 1.0,
            LutSpace::Srgb | LutSpace::DisplayP3 => 0.0,
        };
        let [a, b, c] = header.domain_min;
        let [d, e, f] = header.domain_max;
        let u = LookParams {
            to_lut: to_mat3(&to_lut),
            from_lut: to_mat3(&mat::inverse(&to_lut)),
            domain_min: [a, b, c, (self.amount.min(100.0) / 100.0) as f32],
            domain_max: [d, e, f, encoding],
            size: [header.size as f32, 0.0, 0.0, 0.0],
        };
        Some(bytemuck::bytes_of(&u).to_vec())
    }
}

/// The sRGB curve (IEC 61966-2-1) and its inverse, held to 0…1 as `common/transfer.wgsl` holds them.
pub fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

pub fn srgb_decode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// A soft proof's uniform (DESIGN.md §15, "Soft proofing"): the proof's table maps the display's own encoded
/// colour to itself, whole, with the gamut warning painted where its alpha says when `warn`.
pub fn proof_uniform(display: DisplaySpace, header: &CubeHeader, warn: bool) -> Vec<u8> {
    let look = LookLut {
        space: match display {
            DisplaySpace::Srgb => LutSpace::Srgb,
            DisplaySpace::DisplayP3 => LutSpace::DisplayP3,
        },
        amount: 100.0,
        ..Default::default()
    };
    let bytes = look.uniform(display, header).expect("a whole amount");
    let mut u: LookParams = bytemuck::pod_read_unaligned(&bytes);
    u.size[1] = if warn { 1.0 } else { 0.0 };
    bytemuck::bytes_of(&u).to_vec()
}

/// The same LUT the pass applies, on the CPU: tetrahedral interpolation (Kasson, Nin, Plouffe and Hafner,
/// "Performing color space conversions with three-dimensional linear interpolation", 1995), which keeps the
/// grey axis on the table's own greys, as trilinear does not. The tests check the shader against it.
pub fn sample(lut: &CubeLut, rgb: [f32; 3]) -> [f32; 3] {
    let n = lut.size as usize;
    let at = |i: [usize; 3]| lut.table[i[0] + n * (i[1] + n * i[2])];
    let p: [f32; 3] = std::array::from_fn(|k| {
        let t = (rgb[k] - lut.domain_min[k]) / (lut.domain_max[k] - lut.domain_min[k]);
        t.clamp(0.0, 1.0) * (n - 1) as f32
    });
    let base: [usize; 3] = std::array::from_fn(|k| (p[k].floor() as usize).min(n - 2));
    let f: [f32; 3] = std::array::from_fn(|k| p[k] - base[k] as f32);
    let corner = |step: [usize; 3]| at([0, 1, 2].map(|k| base[k] + step[k]));
    // The tetrahedron holding the point, by the order of its fractions: with fₐ ≥ f_b ≥ f_c it is the walk
    // (0,0,0) → along a → then b → (1,1,1), weighted 1 − fₐ, fₐ − f_b, f_b − f_c and f_c.
    let mut order = [0usize, 1, 2];
    order.sort_by(|&x, &y| f[y].total_cmp(&f[x]));
    let [a, b, c] = order;
    let mut ab = [0; 3];
    ab[a] = 1;
    let c_a = corner(ab);
    ab[b] = 1;
    let c_ab = corner(ab);
    let (c0, c1) = (corner([0; 3]), corner([1; 3]));
    let w = [1.0 - f[a], f[a] - f[b], f[b] - f[c], f[c]];
    std::array::from_fn(|k| w[0] * c0[k] + w[1] * c_a[k] + w[2] * c_ab[k] + w[3] * c1[k])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(n: u32) -> String {
        let mut s = format!("TITLE \"identity\"\n# a comment\nLUT_3D_SIZE {n}\n");
        let m = (n - 1) as f32;
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    s += &format!("{} {} {}\n", r as f32 / m, g as f32 / m, b as f32 / m);
                }
            }
        }
        s
    }

    #[test]
    fn parses_and_reproduces_an_identity() {
        let lut = parse_cube(&identity(5)).unwrap();
        assert_eq!(lut.title.as_deref(), Some("identity"));
        assert_eq!(lut.size, 5);
        for c in [[0.1, 0.7, 0.33], [0.0, 1.0, 0.5], [0.95, 0.05, 0.6]] {
            let o = sample(&lut, c);
            for k in 0..3 {
                assert!((o[k] - c[k]).abs() < 1e-6, "{c:?} → {o:?}");
            }
        }
    }

    /// Red runs fastest: the entry after the first is (1/(n−1), 0, 0).
    #[test]
    fn reads_red_fastest() {
        let mut text = "LUT_3D_SIZE 2\n".to_string();
        for i in 0..8 {
            text += &format!("{} {} {}\n", i, 10 + i, 20 + i);
        }
        let lut = parse_cube(&text).unwrap();
        assert_eq!(sample(&lut, [1.0, 0.0, 0.0]), [1.0, 11.0, 21.0]);
        assert_eq!(sample(&lut, [0.0, 1.0, 0.0]), [2.0, 12.0, 22.0]);
        assert_eq!(sample(&lut, [0.0, 0.0, 1.0]), [4.0, 14.0, 24.0]);
    }

    #[test]
    fn honours_the_domain() {
        let text = identity(3).replace(
            "LUT_3D_SIZE 3",
            "LUT_3D_SIZE 3\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 2 2 2",
        );
        let lut = parse_cube(&text).unwrap();
        // Input 1.0 is the middle of a [0, 2] domain, which the identity table maps to 0.5.
        assert!((sample(&lut, [1.0, 1.0, 1.0])[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn refuses_what_it_cannot_apply() {
        assert!(
            parse_cube("LUT_1D_SIZE 4\n0 0 0\n")
                .unwrap_err()
                .contains("1D")
        );
        assert!(
            parse_cube("LUT_3D_SIZE 2\n0 0 0\n")
                .unwrap_err()
                .contains("8 entries")
        );
        assert!(parse_cube("0 0 0\n").unwrap_err().contains("LUT_3D_SIZE"));
        assert!(parse_cube("LUT_3D_SIZE 2\n0 0\n").is_err());
    }

    /// Greys stay on the table's greys: a table that only darkens its grey axis leaves greys grey.
    #[test]
    fn tetrahedral_keeps_greys_grey() {
        let n = 9u32;
        let m = (n - 1) as f32;
        let mut s = format!("LUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let c = [r, g, b].map(|v| v as f32 / m);
                    // A saturation boost: away from the mean, which a grey has none of.
                    let mean = (c[0] + c[1] + c[2]) / 3.0;
                    let o = c.map(|v| (mean + 1.5 * (v - mean)).clamp(0.0, 1.0));
                    s += &format!("{} {} {}\n", o[0], o[1], o[2]);
                }
            }
        }
        let lut = parse_cube(&s).unwrap();
        for v in [0.03f32, 0.27, 0.5, 0.81] {
            let o = sample(&lut, [v; 3]);
            assert!(
                (o[0] - o[1]).abs() < 1e-6 && (o[1] - o[2]).abs() < 1e-6,
                "{v} → {o:?}"
            );
        }
    }

    #[test]
    fn the_uniform_round_trips_its_space() {
        let header = CubeHeader {
            size: 33,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
        };
        let look = LookLut {
            space: LutSpace::Srgb,
            ..Default::default()
        };
        let u: LookParams =
            bytemuck::pod_read_unaligned(&look.uniform(DisplaySpace::Srgb, &header).unwrap());
        // sRGB output into an sRGB LUT: no primaries change.
        for i in 0..3 {
            for j in 0..3 {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((u.to_lut[i][j] - want).abs() < 1e-5);
            }
        }
        assert_eq!(u.domain_min[3], 1.0);
        assert_eq!(u.size[0], 33.0);
        let off = LookLut {
            amount: 0.0,
            ..look
        };
        assert!(off.uniform(DisplaySpace::Srgb, &header).is_none());
    }
}
