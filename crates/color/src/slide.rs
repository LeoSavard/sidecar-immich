// SPDX-License-Identifier: Apache-2.0
//! Colour reversal film, spectrally (DESIGN.md §7, "Colour film"): a named stock rendered only from its maker's
//! data sheet and published standards.
//!
//! 1. The scene: each colour's reflectance (`crate::spectral`, Jakob and Hanika) lit by the scene's light, at the
//!    exposure a reflected-light meter gives the stock's ISO speed (ISO 2720: H = q·K / S, K = 12.5, q = 0.65) for
//!    a surface at Sidecar's middle grey (0.18).
//! 2. Each layer's exposure: the spectral exposure through the sheet's spectral sensitivity. FUJIFILM draws it as
//!    log sensitivity on a relative scale — the three layers against each other, not in absolute units — so each
//!    layer is placed on its characteristic curve through the light the curves were exposed with ("Daylight":
//!    ISO 7589's sensitometric daylight): a stimulus giving a layer the response that `H` lux·s of that light gives
//!    it sits at log `H` on the layer's curve.
//! 3. The characteristic curves give each layer's Status A density, as the sheet measured them.
//! 4. The dye amounts whose spectra (the sheet's, each at peak 1) read those Status A densities through the
//!    densitometer's spectral products (ISO 5-3).
//! 5. Viewing, as the sheet says to: on an ISO 3664 light table (CIE D50), the eye adapted to it (Bradford), seen
//!    on the display (D65); the light table itself is white.
//!
//! ISO 5-3's spectral products and ISO 7589's sensitometric daylight are licensed and not part of Sidecar: the
//! owner's copies are read at run time (`Densitometer::parse`, `parse_spectrum`).

use crate::film_data as data;
use crate::spectral::{SAMPLES, grid, reflectance};
use crate::spectral_data::{CMF, D65};

/// The spectra's sampling interval, nm.
const STEP_NM: f64 = 10.0;

/// A reflected-light meter's calibration constant (ISO 2720) and the lens factor that turns scene luminance into
/// exposure at the film: a metered middle grey gets `Q_LENS · K_METER / S` lux·s at speed S.
const K_METER: f64 = 12.5;
const Q_LENS: f64 = 0.65;
/// Sidecar's middle grey, the reflectance the meter is taken to read.
const GREY: f64 = 0.18;
/// Luminous efficacy, lm/W.
const KM: f64 = 683.0;

/// Rows of numbers from a table: `#` starts a comment, values are separated by commas or whitespace; a line
/// `# log10` says the values after the first column are base-10 logarithms (as ISO's tables give them).
fn table(text: &str, columns: usize) -> Result<Vec<Vec<f64>>, String> {
    let log = text.lines().any(|l| l.trim().trim_start_matches('#').trim().eq_ignore_ascii_case("log10"));
    let mut rows = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut v: Vec<f64> = line.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).map(str::parse).collect::<Result<_, _>>().map_err(|e| format!("line {}: {e}", n + 1))?;
        if v.len() != columns {
            return Err(format!("line {}: expected {columns} numbers", n + 1));
        }
        if log {
            for x in &mut v[1..] {
                *x = 10f64.powf(*x);
            }
        }
        rows.push(v);
    }
    if rows.len() < 2 {
        return Err("a table needs at least two rows".into());
    }
    rows.sort_by(|a, b| a[0].total_cmp(&b[0]));
    Ok(rows)
}

/// Column `c` of `rows` on Sidecar's 380–730 nm grid: linear between rows, 0 outside the table.
fn resample(rows: &[Vec<f64>], c: usize) -> [f64; SAMPLES] {
    std::array::from_fn(|i| {
        let l = crate::spectral::wavelength(i);
        rows.windows(2).find(|w| w[0][0] <= l && l <= w[1][0]).map_or(0.0, |w| w[0][c] + (w[1][c] - w[0][c]) * (l - w[0][0]) / (w[1][0] - w[0][0]))
    })
}

/// A spectrum from a table of `nm, value` rows: ISO 7589's sensitometric daylight, from the owner's copy.
pub fn parse_spectrum(text: &str) -> Result<[f64; SAMPLES], String> {
    let s = resample(&table(text, 2)?, 1);
    if s.iter().sum::<f64>() <= 0.0 {
        return Err("the spectrum is zero within 380–730 nm".into());
    }
    Ok(s)
}

/// A densitometer's spectral products (the product of its light source, filters and detector), red, green, blue,
/// linear, on Sidecar's 380–730 nm grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Densitometer {
    response: [[f64; SAMPLES]; 3],
}

impl Densitometer {
    /// From a table of `nm, red, green, blue` rows: ISO 5-3's Status A, from the owner's copy.
    pub fn parse(text: &str) -> Result<Self, String> {
        let rows = table(text, 4)?;
        let response = [1, 2, 3].map(|c| resample(&rows, c));
        if response.iter().any(|r| r.iter().sum::<f64>() <= 0.0) {
            return Err("every channel needs a response within 380–730 nm".into());
        }
        Ok(Self { response })
    }

    /// The red, green and blue densities this densitometer reads from a spectral transmittance.
    pub fn density(&self, t: &[f64; SAMPLES]) -> [f64; 3] {
        self.response.map(|p| {
            let (num, den): (f64, f64) = (0..SAMPLES).fold((0.0, 0.0), |(n, d), i| (n + p[i] * t[i], d + p[i]));
            -(num / den).log10()
        })
    }
}

/// A colour reversal stock: its three layers' characteristic curves (red, green, blue Status A density over log H
/// in lux·s, exposed to daylight), spectral sensitivities (red, green, blue, relative to one another; 0 where the
/// sheet draws none) and dyes (cyan, magenta, yellow spectral density, peak 1), and its ISO speed.
pub struct Slide {
    curves: [&'static [(f64, f64)]; 3],
    sensitivity: [[f64; SAMPLES]; 3],
    dyes: [[f64; SAMPLES]; 3],
    iso: f64,
}

/// Every colour reversal stock Sidecar renders spectrally, by id. A deployment can only offer these if it has
/// the two standards they are measured through (`sidecar_worker::stocks`).
pub const STOCKS: &[&str] = &["provia-100f"];

/// A colour reversal stock by id: `provia-100f`.
pub fn slide(id: &str) -> Option<Slide> {
    let linear = |log: [f64; SAMPLES]| log.map(|l| if l.is_nan() { 0.0 } else { 10f64.powf(l) });
    match id {
        "provia-100f" => Some(Slide {
            curves: [&data::PROVIA_100F_RED_CURVE, &data::PROVIA_100F_GREEN_CURVE, &data::PROVIA_100F_BLUE_CURVE],
            sensitivity: [linear(data::PROVIA_100F_RED_SENSITIVITY), linear(data::PROVIA_100F_GREEN_SENSITIVITY), linear(data::PROVIA_100F_BLUE_SENSITIVITY)],
            dyes: [data::PROVIA_100F_CYAN_DYE, data::PROVIA_100F_MAGENTA_DYE, data::PROVIA_100F_YELLOW_DYE],
            iso: 100.0,
        }),
        _ => None,
    }
}

/// CIE D50, the ISO 3664 viewing illuminant, relative, and its luminance: the light table, computed once. The
/// response table views a quarter of a million slides through it, and CIE daylight's basis functions are not
/// cheap.
fn light_table() -> &'static ([f64; SAMPLES], f64) {
    static LIGHT_TABLE: std::sync::OnceLock<([f64; SAMPLES], f64)> = std::sync::OnceLock::new();
    LIGHT_TABLE.get_or_init(|| {
        let spd = crate::spectral::daylight(5000.0 * 1.4388 / 1.4380);
        let white = xyz_of(&spd)[1];
        (spd, white)
    })
}

/// CIE D50, the ISO 3664 viewing illuminant, relative.
fn d50() -> [f64; SAMPLES] {
    light_table().0
}

fn xyz_of(spd: &[f64; SAMPLES]) -> [f64; 3] {
    std::array::from_fn(|k| (0..SAMPLES).map(|i| spd[i] * CMF[i][k]).sum())
}

/// Bradford's cone response matrix.
const BRADFORD: [[f64; 3]; 3] = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];

/// XYZ seen adapted to the light table (D50) → linear Rec.2020 seen adapted to D65, by Bradford's transform.
fn viewing_matrix() -> [[f64; 3]; 3] {
    use crate::mat::{apply, diag, inverse, mul};
    let (src, dst) = (xyz_of(&d50()), xyz_of(&D65));
    let (s, d) = (apply(&BRADFORD, &src), apply(&BRADFORD, &dst));
    let scale = diag([d[0] / s[0], d[1] / s[1], d[2] / s[2]]);
    let adapt = mul(&inverse(&BRADFORD), &mul(&scale, &BRADFORD));
    mul(&crate::mat::XYZ_TO_REC2020, &adapt)
}

impl Slide {
    /// Layer `k`'s Status A density at log H, the curve held flat beyond its ends.
    fn density(&self, k: usize, log_h: f64) -> f64 {
        let c = self.curves[k];
        if log_h <= c[0].0 {
            return c[0].1;
        }
        if log_h >= c[c.len() - 1].0 {
            return c[c.len() - 1].1;
        }
        let w = c.windows(2).find(|w| w[0].0 <= log_h && log_h <= w[1].0).unwrap();
        w[0].1 + (w[1].1 - w[0].1) * (log_h - w[0].0) / (w[1].0 - w[0].0)
    }

    /// The factor turning reflectance × the light's relative spectrum into spectral exposure (J/m² per nm) for a
    /// metered exposure: a surface at middle grey gets `Q_LENS · K_METER / iso` lux·s.
    fn exposure_scale(&self, light: &[f64; SAMPLES]) -> f64 {
        let lux_per_unit: f64 = KM * (0..SAMPLES).map(|i| GREY * light[i] * CMF[i][1] * STEP_NM).sum::<f64>();
        Q_LENS * K_METER / self.iso / lux_per_unit
    }

    /// Each layer's response to a spectral exposure (J/m² per nm), in the sensitivities' relative units.
    fn response(&self, exposure: &[f64; SAMPLES]) -> [f64; 3] {
        self.sensitivity.map(|s| (0..SAMPLES).map(|i| exposure[i] * s[i] * STEP_NM).sum())
    }

    /// Each layer's offset from log response to log H on its curve: minus the log of the response 1 lux·s of the
    /// reference light (the daylight the curves were exposed with) gives it.
    pub fn offsets(&self, reference: &[f64; SAMPLES]) -> [f64; 3] {
        let lux: f64 = KM * (0..SAMPLES).map(|i| reference[i] * CMF[i][1] * STEP_NM).sum::<f64>();
        let one_lux_s: [f64; SAMPLES] = std::array::from_fn(|i| reference[i] / lux);
        self.response(&one_lux_s).map(|q| -q.log10())
    }

    /// Each layer's log H on its own curve for responses `q`.
    fn log_h(&self, q: [f64; 3], offsets: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|k| offsets[k] + q[k].max(1e-30).log10())
    }

    /// The dye amounts (cyan, magenta, yellow) whose spectra read `target` Status A densities (red, green, blue)
    /// through `meter`, by Newton's method.
    pub fn dyes_for(&self, target: [f64; 3], meter: &Densitometer) -> [f64; 3] {
        self.dyes_from(target, target, meter)
    }

    /// `dyes_for` started from `start` — the dye amounts of a neighbouring entry, which are close, so the table
    /// costs two or three iterations an entry instead of a cold solve's ten or more. The fixed point is the
    /// same: both run to the same 1e-9 in density.
    fn dyes_from(&self, start: [f64; 3], target: [f64; 3], meter: &Densitometer) -> [f64; 3] {
        let mut a = start;
        for _ in 0..40 {
            let t = self.transmittance(a);
            let d = meter.density(&t);
            let r = [d[0] - target[0], d[1] - target[1], d[2] - target[2]];
            if r.iter().all(|v| v.abs() < 1e-9) {
                break;
            }
            // ∂D_j/∂a_k: the dye's density weighted by what the channel sees through the film.
            let jac: [[f64; 3]; 3] = std::array::from_fn(|j| {
                let p = &meter.response[j];
                let den: f64 = (0..SAMPLES).map(|i| p[i] * t[i]).sum();
                std::array::from_fn(|k| (0..SAMPLES).map(|i| p[i] * t[i] * self.dyes[k][i]).sum::<f64>() / den)
            });
            let step = crate::mat::apply(&crate::mat::inverse(&jac), &r);
            let next: [f64; 3] = std::array::from_fn(|k| (a[k] - step[k]).max(0.0));
            // A target no dye amount can read — a density under the film's own base, where the solve is pinned
            // at zero — leaves the residual standing however long it iterates. Stop when the amounts stop
            // moving: the answer is as close as this film gets.
            let moved: f64 = (0..3).map(|k| (next[k] - a[k]).abs()).sum();
            a = next;
            if moved < 1e-12 {
                break;
            }
        }
        a
    }

    /// The film's spectral transmittance for dye amounts (cyan, magenta, yellow).
    pub fn transmittance(&self, a: [f64; 3]) -> [f64; SAMPLES] {
        std::array::from_fn(|i| 10f64.powf(-(a[0] * self.dyes[0][i] + a[1] * self.dyes[1][i] + a[2] * self.dyes[2][i])))
    }

    /// The slide on the light table, seen on the display: linear Rec.2020, the light table white at 1.
    fn view(&self, t: &[f64; SAMPLES], to_display: &[[f64; 3]; 3]) -> [f64; 3] {
        let (light, white) = light_table();
        let lit: [f64; SAMPLES] = std::array::from_fn(|i| light[i] * t[i]);
        let xyz = xyz_of(&lit).map(|v| v / white);
        crate::mat::apply(to_display, &xyz)
    }

    /// The slide's colour (linear Rec.2020 on the display) for layer log H values.
    pub fn colour_at(&self, log_h: [f64; 3], meter: &Densitometer) -> [f64; 3] {
        let d = std::array::from_fn(|k| self.density(k, log_h[k]));
        self.view(&self.transmittance(self.dyes_for(d, meter)), &viewing_matrix())
    }

    /// A scene colour (linear Rec.2020, white-balanced, middle grey 0.18) photographed under `light` and viewed:
    /// the whole chain, for checks; the develop pass reads it from `exposure_lut` and `response_lut`.
    pub fn render(&self, rgb: [f64; 3], light: &[f64; SAMPLES], meter: &Densitometer, reference: &[f64; SAMPLES]) -> [f64; 3] {
        let m = rgb.iter().cloned().fold(0.0, f64::max);
        if m <= 0.0 {
            return self.colour_at([-9.0; 3], meter);
        }
        let k = crate::spectral::fit_scale(rgb.map(|v| v / m));
        let c = crate::spectral::coefficients(rgb.map(|v| v / m * k));
        let scale = self.exposure_scale(light) * m / k;
        let exposure: [f64; SAMPLES] = std::array::from_fn(|i| reflectance(c, i) * light[i] * scale);
        self.colour_at(self.log_h(self.response(&exposure), self.offsets(reference)), meter)
    }

    /// Over the colour cube (`spectral::GRID`³, red fastest), each colour's layer responses per unit of its
    /// largest channel, under `light`: a pixel's responses are its largest channel times the table's at its
    /// chromaticity, as light is linear; log H on each layer's curve adds `offsets`. Four floats per entry (red,
    /// green, blue, 0).
    pub fn exposure_lut(&self, light: &[f64; SAMPLES]) -> Vec<f32> {
        let scale = self.exposure_scale(light);
        grid()
            .iter()
            .flat_map(|&(c, k)| {
                let exposure: [f64; SAMPLES] = std::array::from_fn(|i| reflectance(c, i) * light[i] * scale / k);
                let q = self.response(&exposure);
                [q[0] as f32, q[1] as f32, q[2] as f32, 0.0]
            })
            .collect()
    }
}

/// The response table's domain in log H (lux·s), each axis: past either end every curve is flat.
pub const RESPONSE_LOG_H: [f64; 2] = [-3.6, 1.2];
/// The response table's side.
pub const RESPONSE_SIDE: usize = 65;

impl Slide {
    /// Over the cube of layer log H values (`RESPONSE_LOG_H`, `RESPONSE_SIDE`³, red fastest), the slide's colour on
    /// the display as log₂ of linear Rec.2020 (interpolated in log, which follows densities far better than linear
    /// values do). Four floats per entry (red, green, blue, 0).
    pub fn response_lut(&self, meter: &Densitometer) -> Vec<f32> {
        self.response_lut_sized(meter, RESPONSE_SIDE)
    }

    fn response_lut_sized(&self, meter: &Densitometer, n: usize) -> Vec<f32> {
        let to_display = viewing_matrix();
        let at = |i: usize| RESPONSE_LOG_H[0] + (RESPONSE_LOG_H[1] - RESPONSE_LOG_H[0]) * i as f64 / (n - 1) as f64;
        let mut out = Vec::with_capacity(n * n * n * 4);
        // Red runs fastest, so each entry's solve starts from the entry before it along red, and the first of a
        // row from the row's own start — amounts a neighbour away, which Newton reaches in two or three steps.
        let mut along_red = [0.0; 3];
        let mut row_start = [0.0; 3];
        for idx in 0..n * n * n {
            let h = [at(idx % n), at(idx / n % n), at(idx / (n * n))];
            let d: [f64; 3] = std::array::from_fn(|k| self.density(k, h[k]));
            let start = if idx == 0 {
                d
            } else if idx % n == 0 {
                row_start
            } else {
                along_red
            };
            let a = self.dyes_from(start, d, meter);
            if idx % n == 0 {
                row_start = a;
            }
            along_red = a;
            let rgb = self.view(&self.transmittance(a), &to_display);
            out.extend_from_slice(&[rgb[0].max(1e-6).log2() as f32, rgb[1].max(1e-6).log2() as f32, rgb[2].max(1e-6).log2() as f32, 0.0]);
        }
        out
    }
}

/// What the develop pass needs from a stock that only the licensed standards can give: the slide's colour over
/// layer log H (`response_lut`, from ISO 5-3's Status A) and each layer's offset from log response to log H
/// (`offsets`, from ISO 7589's sensitometric daylight), ready for the shader's lookup. The whole of a stock's
/// chain that touches a standard is these numbers, so this is what the server hands the browser, which carries
/// no standard of its own: the table holds the slide's colours and the offsets are three numbers — neither ISO
/// table can be recovered from them. Everything else (the sheet's sensitivities, the scene's light) is public
/// and `Slide::exposure_lut` builds it anywhere, per edit.
#[derive(Clone, Debug, PartialEq)]
pub struct SlideProfile {
    /// Where a layer response of 1 lands in the response table, as a fraction of its domain:
    /// `(offset − RESPONSE_LOG_H[0]) / (RESPONSE_LOG_H[1] − RESPONSE_LOG_H[0])`.
    pub offsets: [f32; 3],
    /// A decade of log H as a fraction of the domain: `1 / (RESPONSE_LOG_H[1] − RESPONSE_LOG_H[0])`.
    pub scale: f32,
    /// The response table's side.
    pub side: u32,
    /// The response table, `side`³ entries of four floats (`response_lut`).
    pub response: Vec<f32>,
}

/// A densitometer and a reference light for testing the plumbing, anywhere, without a standard: narrow bands at
/// 610, 540 and 440 nm, and CIE D55. They are **not** ISO 5-3's Status A or ISO 7589's sensitometric daylight,
/// which Sidecar does not carry and cannot stand in for, so nothing they render is a PROVIA 100F slide — they
/// exist so the tables, the shader and the pass can be checked against the same chain on the CPU.
pub fn stand_in_standards() -> (Densitometer, [f64; SAMPLES]) {
    let rows: String = (38..=73)
        .map(|i| {
            let l = i as f64 * 10.0;
            let g = |c: f64| (-((l - c) / 20.0).powi(2)).exp();
            format!("{l}, {}, {}, {}\n", g(610.0), g(540.0), g(440.0))
        })
        .collect();
    (Densitometer::parse(&rows).expect("the stand-in densitometer parses"), crate::spectral::daylight(5500.0 * 1.4388 / 1.4380))
}

/// `SlideProfile`'s header: `SCSL`, the format's version, the table's side, then the offsets and the scale.
const PROFILE_MAGIC: u32 = u32::from_le_bytes(*b"SCSL");
const PROFILE_VERSION: u32 = 1;

impl Slide {
    /// This stock's profile, measured through the owner's standards.
    pub fn profile(&self, meter: &Densitometer, reference: &[f64; SAMPLES]) -> SlideProfile {
        self.profile_sized(meter, reference, RESPONSE_SIDE)
    }

    /// A profile with a `side`³ response table instead of `RESPONSE_SIDE`³: the full one costs a quarter of a
    /// million dye solves, which is worth caching but not worth a test's time.
    pub fn profile_sized(&self, meter: &Densitometer, reference: &[f64; SAMPLES], side: usize) -> SlideProfile {
        SlideProfile { side: side as u32, response: self.response_lut_sized(meter, side), ..self.profile_header(reference) }
    }

    /// Where the reference light puts each layer on its curve, as the shader reads the response table; `profile`
    /// adds the table itself.
    fn profile_header(&self, reference: &[f64; SAMPLES]) -> SlideProfile {
        let (lo, hi) = (RESPONSE_LOG_H[0], RESPONSE_LOG_H[1]);
        SlideProfile {
            offsets: self.offsets(reference).map(|o| ((o - lo) / (hi - lo)) as f32),
            scale: (1.0 / (hi - lo)) as f32,
            side: RESPONSE_SIDE as u32,
            response: Vec::new(),
        }
    }
}

impl SlideProfile {
    /// Just the numbers the uniform needs, for a caller that holds the response table itself: the browser
    /// uploads the table it downloaded straight to the GPU, and only needs `pack` to describe it.
    pub fn header(offsets: [f32; 3], scale: f32, side: u32) -> Self {
        Self { offsets, scale, side, response: Vec::new() }
    }

    /// The profile as bytes, for the cache and the wire.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + self.response.len() * 4);
        for v in [PROFILE_MAGIC, PROFILE_VERSION, self.side, 0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in [self.offsets[0], self.offsets[1], self.offsets[2], self.scale] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(bytemuck::cast_slice(&self.response));
        out
    }

    /// A profile from `to_bytes`; `None` for anything else, so a stale cache file or a truncated download is
    /// simply no profile rather than a wrong rendering.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 32 || bytes.len() % 4 != 0 {
            return None;
        }
        let word = |i: usize| u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        let float = |i: usize| f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        if word(0) != PROFILE_MAGIC || word(1) != PROFILE_VERSION {
            return None;
        }
        let side = word(2) as usize;
        let entries = side.checked_pow(3)?.checked_mul(4)?;
        if side == 0 || bytes.len() != 32 + entries * 4 {
            return None;
        }
        Some(Self {
            offsets: [float(4), float(5), float(6)],
            scale: float(7),
            side: side as u32,
            response: (8..8 + entries).map(float).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_meter() -> Densitometer {
        stand_in_standards().0
    }

    fn test_daylight() -> [f64; SAMPLES] {
        stand_in_standards().1
    }

    /// The owner's ISO 5-3 Status A table and ISO 7589 sensitometric daylight, if `SIDECAR_STATUS_A` and
    /// `SIDECAR_ISO_DAYLIGHT` point to them.
    fn standards() -> Option<(Densitometer, [f64; SAMPLES])> {
        let read = |var: &str| std::fs::read_to_string(std::env::var(var).ok()?).ok();
        let meter = Densitometer::parse(&read("SIDECAR_STATUS_A")?).expect("the Status A table parses");
        let daylight = parse_spectrum(&read("SIDECAR_ISO_DAYLIGHT")?).expect("the daylight table parses");
        Some((meter, daylight))
    }

    #[test]
    fn tables_parse_linear_or_log() {
        let lin = Densitometer::parse("# test\n400, 1, 2, 3\n500 1 2 3\n").unwrap();
        let log = Densitometer::parse("# log10\n400, 0, 0.30103, 0.47712\n500, 0, 0.30103, 0.47712\n").unwrap();
        for c in 0..3 {
            assert!((lin.response[c][3] - log.response[c][3]).abs() < 1e-4, "channel {c}");
        }
        assert_eq!(lin.response[0][0], 0.0, "zero outside the table");
        assert!(Densitometer::parse("400, 1, 2\n").is_err());
        assert!(parse_spectrum("380, 1\n730, 1\n").is_ok() && parse_spectrum("380, 1\n").is_err());
    }

    /// The dye solve finds the amounts that made a set of densities: dyes → densities → dyes.
    #[test]
    fn dyes_round_trip_through_their_densities() {
        let s = slide("provia-100f").unwrap();
        let meter = test_meter();
        for a in [[0.1, 0.1, 0.1], [1.0, 1.5, 2.0], [3.0, 3.1, 3.2], [0.2, 2.5, 0.8], [2.4, 0.3, 1.1]] {
            let target = meter.density(&s.transmittance(a));
            let back = s.dyes_for(target, &meter);
            for k in 0..3 {
                assert!((back[k] - a[k]).abs() < 1e-6, "{a:?} → {target:?} → {back:?}");
            }
        }
    }

    /// `H` lux·s of the reference light lands each layer at log `H` on its curve.
    #[test]
    fn the_reference_light_lands_on_the_curves() {
        let s = slide("provia-100f").unwrap();
        let reference = test_daylight();
        let lux: f64 = KM * (0..SAMPLES).map(|i| reference[i] * CMF[i][1] * STEP_NM).sum::<f64>();
        let exposure: [f64; SAMPLES] = std::array::from_fn(|i| reference[i] * 0.05 / lux);
        let h = s.log_h(s.response(&exposure), s.offsets(&reference));
        for (k, h) in h.iter().enumerate() {
            assert!((h - 0.05f64.log10()).abs() < 1e-9, "layer {k}: {h}");
        }
    }

    /// A metered middle grey is exposed at ISO 2720's q·K/S lux·s: 0.081 lux·s at ISO 100.
    #[test]
    fn middle_grey_gets_the_metered_exposure() {
        let s = slide("provia-100f").unwrap();
        let light = test_daylight();
        let scale = s.exposure_scale(&light);
        let lux_s: f64 = KM * (0..SAMPLES).map(|i| GREY * light[i] * scale * CMF[i][1] * STEP_NM).sum::<f64>();
        assert!((lux_s - 0.08125).abs() < 1e-9, "{lux_s}");
    }

    /// More light makes a slide lighter.
    #[test]
    fn lighter_scenes_make_lighter_slides() {
        let s = slide("provia-100f").unwrap();
        let (meter, light) = (test_meter(), test_daylight());
        let y = |v: f64| s.render([v; 3], &light, &meter, &light)[1];
        assert!(y(0.05) < y(0.18) && y(0.18) < y(0.7), "{} {} {}", y(0.05), y(0.18), y(0.7));
    }

    /// The response table holds the chain's colour at its lattice points, in log₂, red fastest; and at
    /// `RESPONSE_SIDE` points per axis, halfway between two the chain is within 0.03 (2 %) of their mean.
    #[test]
    fn the_response_table_follows_the_chain() {
        let s = slide("provia-100f").unwrap();
        let meter = test_meter();
        let n = 5;
        let lut = s.response_lut_sized(&meter, n);
        let at = |i: usize| RESPONSE_LOG_H[0] + (RESPONSE_LOG_H[1] - RESPONSE_LOG_H[0]) * i as f64 / (n - 1) as f64;
        for (i, j, k) in [(0, 0, 0), (1, 2, 3), (4, 0, 2)] {
            let direct = s.colour_at([at(i), at(j), at(k)], &meter).map(|v| v.max(1e-6).log2());
            let e = (i + j * n + k * n * n) * 4;
            for c in 0..3 {
                assert!((lut[e + c] as f64 - direct[c]).abs() < 1e-4);
            }
        }
        let step = (RESPONSE_LOG_H[1] - RESPONSE_LOG_H[0]) / (RESPONSE_SIDE - 1) as f64;
        let log = |h: f64| s.colour_at([h; 3], &meter).map(|v| v.max(1e-6).log2());
        let mut worst: f64 = 0.0;
        for i in 0..RESPONSE_SIDE - 1 {
            let h = RESPONSE_LOG_H[0] + i as f64 * step;
            let (a, b, mid) = (log(h), log(h + step), log(h + step / 2.0));
            for c in 0..3 {
                worst = worst.max((mid[c] - (a[c] + b[c]) / 2.0).abs());
            }
        }
        assert!(worst < 0.03, "worst log₂ error between lattice points: {worst}");
    }

    /// A profile survives the trip to bytes and back, and nothing else parses as one.
    #[test]
    fn profiles_round_trip_through_bytes() {
        let s = slide("provia-100f").unwrap();
        let (meter, light) = (test_meter(), test_daylight());
        let p = s.profile_sized(&meter, &light, 2);
        let bytes = p.to_bytes();
        assert_eq!(bytes.len(), 32 + 8 * 4 * 4);
        assert_eq!(SlideProfile::parse(&bytes).as_ref(), Some(&p));
        assert!(SlideProfile::parse(&bytes[..bytes.len() - 4]).is_none(), "a truncated profile is no profile");
        assert!(SlideProfile::parse(&[0; 64]).is_none(), "zeroes are no profile");
        let mut wrong = bytes.clone();
        wrong[4] = 9;
        assert!(SlideProfile::parse(&wrong).is_none(), "another version is no profile");
    }

    /// The whole response table, as the server builds it once per stock: the slowest part of the chain, so it is
    /// cached rather than packed per edit. Run with `--ignored --nocapture` to time it.
    #[test]
    #[ignore = "builds the full 65³ table"]
    fn the_response_table_is_worth_caching() {
        let s = slide("provia-100f").unwrap();
        let t = std::time::Instant::now();
        let lut = s.response_lut(&test_meter());
        eprintln!("{}³ response table in {:?}", RESPONSE_SIDE, t.elapsed());
        assert_eq!(lut.len(), RESPONSE_SIDE.pow(3) * 4);
    }

    /// With the owner's standards: a metered middle grey in the film's own daylight comes out near neutral. Skipped
    /// without them.
    #[test]
    fn with_the_standards_grey_is_neutral() {
        let Some((meter, daylight)) = standards() else { return eprintln!("skipping: SIDECAR_STATUS_A or SIDECAR_ISO_DAYLIGHT not set") };
        let s = slide("provia-100f").unwrap();
        let rgb = s.render([0.18; 3], &daylight, &meter, &daylight);
        eprintln!("PROVIA 100F, middle grey in ISO sensitometric daylight: {rgb:?}");
        let y = 0.2627 * rgb[0] + 0.6780 * rgb[1] + 0.0593 * rgb[2];
        assert!(rgb.iter().all(|v| (v / y - 1.0).abs() < 0.15), "{rgb:?}");
    }
}
