// SPDX-License-Identifier: Apache-2.0
//! Spectra for colours, and black-and-white film through them (DESIGN.md §7, "Black and white").
//!
//! Three numbers can't say which spectrum a colour came from: many spectra give the same colour. This picks the
//! one of Jakob and Hanika's family — a sigmoid of a quadratic in wavelength, `s(λ) = ½ + x / (2√(1 + x²))`,
//! `x = c₀t² + c₁t + c₂` — whose colour under D65 is the target's: always a valid reflectance (0–1), smooth as
//! real surfaces' reflectances are, and exactly the colour it came from. Implemented from the paper (W. Jakob,
//! J. Hanika, "A Low-Dimensional Function Space for Efficient Spectral Upsampling", Computer Graphics Forum
//! 38(2), 2019): Newton's method on the CIELAB difference, from grey out to the colour in steps, each step
//! starting from the last. `t` is wavelength normalised to 0–1 over 380–730 nm.
//!
//! Black-and-white film then sees that spectrum as a reflectance, lit by the scene's light (`light`), through a
//! filter and its own spectral sensitivity: one exposure per colour. `mono_lut` tabulates it over the colour
//! cube for the develop pass.

use crate::spectral_data::{CMF, D65, DAYLIGHT_S0, DAYLIGHT_S1, DAYLIGHT_S2};

/// Samples: 380–730 nm, every 10 nm.
pub const SAMPLES: usize = 36;

fn wavelength(i: usize) -> f64 {
    380.0 + 10.0 * i as f64
}

/// Linear Rec.2020 → XYZ (D65), as colour-science gives it.
const REC2020_TO_XYZ: [[f64; 3]; 3] = [[0.6369580483, 0.1446169036, 0.1688809752], [0.262700212, 0.6779980715, 0.0593017165], [0.0, 0.028072693, 1.0609850577]];

/// The reflectance of coefficients `c` at sample `i`.
pub fn reflectance(c: [f64; 3], i: usize) -> f64 {
    let t = i as f64 / (SAMPLES - 1) as f64;
    let x = c[0] * t * t + c[1] * t + c[2];
    0.5 + x / (2.0 * (1.0 + x * x).sqrt())
}

/// XYZ of a reflectance under D65, white at Y = 1.
fn xyz(c: [f64; 3]) -> [f64; 3] {
    let mut out = [0.0; 3];
    let mut norm = 0.0;
    for i in 0..SAMPLES {
        let s = reflectance(c, i) * D65[i];
        for (k, o) in out.iter_mut().enumerate() {
            *o += s * CMF[i][k];
        }
        norm += D65[i] * CMF[i][1];
    }
    out.map(|v| v / norm)
}

/// D65's white in this tabulation, so a reflectance of 1 is exactly white.
fn white() -> [f64; 3] {
    let mut out = [0.0; 3];
    let mut norm = 0.0;
    for i in 0..SAMPLES {
        for (k, o) in out.iter_mut().enumerate() {
            *o += D65[i] * CMF[i][k];
        }
        norm += D65[i] * CMF[i][1];
    }
    out.map(|v| v / norm)
}

fn lab(xyz: [f64; 3], white: [f64; 3]) -> [f64; 3] {
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let (fx, fy, fz) = (f(xyz[0] / white[0]), f(xyz[1] / white[1]), f(xyz[2] / white[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn rec2020_to_xyz(rgb: [f64; 3]) -> [f64; 3] {
    REC2020_TO_XYZ.map(|row| row[0] * rgb[0] + row[1] * rgb[1] + row[2] * rgb[2])
}

/// CIELAB of coefficients `c`, and its derivative by each coefficient (exact: the family is smooth).
fn lab_and_jacobian(c: [f64; 3], white: [f64; 3]) -> ([f64; 3], [[f64; 3]; 3]) {
    let mut xyz = [0.0; 3];
    let mut dxyz = [[0.0; 3]; 3];
    let mut norm = 0.0;
    for i in 0..SAMPLES {
        let t = i as f64 / (SAMPLES - 1) as f64;
        let x = c[0] * t * t + c[1] * t + c[2];
        let root = (1.0 + x * x).sqrt();
        let s = 0.5 + x / (2.0 * root);
        let ds = 1.0 / (2.0 * root * root * root);
        let dx = [t * t, t, 1.0];
        for k in 0..3 {
            let w = D65[i] * CMF[i][k];
            xyz[k] += s * w;
            for (j, d) in dx.iter().enumerate() {
                dxyz[k][j] += ds * d * w;
            }
        }
        norm += D65[i] * CMF[i][1];
    }
    let xyz = xyz.map(|v| v / norm);
    let f = |t: f64| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let df = |t: f64| if t > 216.0 / 24389.0 { 1.0 / (3.0 * t.cbrt().powi(2)) } else { 24389.0 / 27.0 / 116.0 };
    let r: [f64; 3] = std::array::from_fn(|k| xyz[k] / white[k]);
    let (fx, fy, fz) = (f(r[0]), f(r[1]), f(r[2]));
    // d f(xyz_k / white_k) / dc_j
    let dfk = |k: usize, j: usize| df(r[k]) * dxyz[k][j] / norm / white[k];
    let jac = [
        std::array::from_fn(|j| 116.0 * dfk(1, j)),
        std::array::from_fn(|j| 500.0 * (dfk(0, j) - dfk(1, j))),
        std::array::from_fn(|j| 200.0 * (dfk(1, j) - dfk(2, j))),
    ];
    ([116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)], jac)
}

/// Newton's method from `c` toward the coefficients whose colour is `target` (CIELAB), a step that would make
/// it worse halved. A colour no reflectance has (more saturated than any surface) ends at the nearest it
/// reaches in the iterations it gets.
fn refine(mut c: [f64; 3], target: [f64; 3], white: [f64; 3]) -> [f64; 3] {
    let err = |l: [f64; 3]| (l[0] - target[0]).powi(2) + (l[1] - target[1]).powi(2) + (l[2] - target[2]).powi(2);
    for _ in 0..15 {
        let (l, j) = lab_and_jacobian(c, white);
        let before = err(l);
        if before < 1e-8 {
            break;
        }
        let Some(delta) = solve(j, [target[0] - l[0], target[1] - l[1], target[2] - l[2]]) else { break };
        let mut step = 1.0;
        let mut moved = false;
        for _ in 0..6 {
            let trial = [c[0] + step * delta[0], c[1] + step * delta[1], c[2] + step * delta[2]];
            if err(lab(xyz(trial), white)) < before {
                c = trial;
                moved = true;
                break;
            }
            step *= 0.5;
        }
        if !moved {
            break;
        }
    }
    c
}

/// `a·x = b` for 3×3, by Cramer's rule; `None` when singular.
fn solve(a: [[f64; 3]; 3], b: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: [[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = det(a);
    if d.abs() < 1e-14 {
        return None;
    }
    Some(std::array::from_fn(|k| {
        let mut m = a;
        for row in 0..3 {
            m[row][k] = b[row];
        }
        det(m) / d
    }))
}

/// The coefficients for a linear Rec.2020 colour inside the unit cube: from the grey of its luminance, in steps.
pub fn coefficients(rgb: [f64; 3]) -> [f64; 3] {
    let white = white();
    let rgb = rgb.map(|v| v.clamp(0.0, 1.0));
    let y = (REC2020_TO_XYZ[1][0] * rgb[0] + REC2020_TO_XYZ[1][1] * rgb[1] + REC2020_TO_XYZ[1][2] * rgb[2]).clamp(1e-3, 0.999);
    // The flat spectrum at that grey: s = y everywhere.
    let mut c = [0.0, 0.0, (2.0 * y - 1.0) / (2.0 * (y * (1.0 - y)).sqrt())];
    const STEPS: usize = 4;
    for k in 1..=STEPS {
        let f = k as f64 / STEPS as f64;
        let target = std::array::from_fn(|i| y + (rgb[i] - y) * f);
        c = refine(c, lab(rec2020_to_xyz(target), white), white);
    }
    c
}

/// A spectral curve as knots `(nm, value)`, linear between them and 0 beyond the ends.
fn sampled(knots: &[(f64, f64)]) -> [f64; SAMPLES] {
    std::array::from_fn(|i| {
        let l = wavelength(i);
        match knots.windows(2).find(|w| w[0].0 <= l && l <= w[1].0) {
            Some(w) => w[0].1 + (w[1].1 - w[0].1) * (l - w[0].0) / (w[1].0 - w[0].0),
            None => 0.0,
        }
    })
}

/// A black-and-white film type: its spectral sensitivity, relative, per wavelength. The classes' shapes —
/// where panchromatic, orthochromatic and blue-sensitive emulsions begin and stop seeing, as their
/// manufacturers' datasheets show them — and named stocks from their own datasheets.
pub fn film_sensitivity(id: &str) -> Option<[f64; SAMPLES]> {
    use crate::film_data as data;
    match id {
        "hp5-plus" => return Some(normalised(data::HP5_PLUS_SPECTROGRAM)),
        "sfx-200" => return Some(normalised(data::SFX_200_SPECTROGRAM)),
        "tri-x-400" | "tri-x-320" => return Some(normalised(data::TRI_X_LOG_SENSITIVITY.map(|l| 10f64.powf(l)))),
        "double-x" => return Some(normalised(data::DOUBLE_X_LOG_SENSITIVITY.map(|l| 10f64.powf(l)))),
        _ => {}
    }
    Some(sampled(match id {
        // Classic panchromatic (HP5, Tri-X and their kind): blue-heavy, a dip in the green, sensitising out to
        // the orange-red and gone by about 680 nm, which is why reds print darker than the eye sees them.
        "panchromatic" => &[(380.0, 0.55), (400.0, 0.8), (430.0, 0.95), (460.0, 1.0), (490.0, 0.85), (510.0, 0.7), (530.0, 0.75), (560.0, 0.85), (590.0, 0.9), (620.0, 0.9), (640.0, 0.7), (660.0, 0.3), (680.0, 0.05), (700.0, 0.0)],
        // Modern panchromatic with an extended red (T-grain films): flatter, seeing further into the red.
        "panchromatic-extended" => &[(380.0, 0.5), (400.0, 0.75), (440.0, 0.9), (480.0, 0.9), (510.0, 0.8), (540.0, 0.85), (580.0, 0.95), (620.0, 1.0), (650.0, 0.9), (670.0, 0.65), (690.0, 0.3), (710.0, 0.05), (720.0, 0.0)],
        // Orthochromatic: blind to red past about 600 nm — reds go black, skies stay bright.
        "orthochromatic" => &[(380.0, 0.6), (400.0, 0.85), (440.0, 1.0), (480.0, 0.95), (520.0, 0.85), (550.0, 0.8), (570.0, 0.6), (590.0, 0.2), (600.0, 0.05), (610.0, 0.0)],
        // Blue-sensitive, as wet collodion and the earliest plates: ultraviolet to blue only (the ultraviolet
        // itself no camera records).
        "blue-sensitive" => &[(380.0, 1.0), (400.0, 1.0), (430.0, 0.9), (460.0, 0.6), (480.0, 0.35), (500.0, 0.12), (520.0, 0.02), (530.0, 0.0)],
        _ => return None,
    }))
}

/// A datasheet's sensitivity, largest 1. ILFORD's "wedge spectrograms to tungsten light" are read as drawn —
/// the film's relative sensitivity — which SFX 200's filter-factor table confirms; Kodak's curves are log
/// sensitivity per unit energy, the film's own by definition, and taken out of the log. (TRI-X's sheet gives one
/// spectral curve for its 320 and 400.)
fn normalised(s: [f64; SAMPLES]) -> [f64; SAMPLES] {
    let max = s.iter().cloned().fold(0.0, f64::max);
    s.map(|v| v / max)
}

/// A filter on the lens: its transmission per wavelength. Generic shapes after the classic yellow, orange, red
/// and green filters (a long-pass edge, or a band for green).
pub fn filter_transmission(id: &str) -> Option<[f64; SAMPLES]> {
    let edge = |at: f64, width: f64| -> [f64; SAMPLES] { std::array::from_fn(|i| 1.0 / (1.0 + (-(wavelength(i) - at) / width).exp())) };
    Some(match id {
        "none" => [1.0; SAMPLES],
        "yellow" => edge(500.0, 12.0),
        "orange" => edge(555.0, 10.0),
        "red" => edge(600.0, 8.0),
        "green" => {
            let (rise, fall) = (edge(490.0, 12.0), edge(590.0, 12.0));
            std::array::from_fn(|i| 0.8 * rise[i] * (1.0 - fall[i]))
        }
        _ => return None,
    })
}

/// A black body at `t` kelvin (Planck's law), 100 at 560 nm. `c2` is the second radiation constant in m·K:
/// 1.4388e-2 today; CIE illuminant A is defined at 2,848 K with the older 1.435e-2 (2,856 K today).
pub fn planck(t: f64, c2: f64) -> [f64; SAMPLES] {
    let at = |nm: f64| {
        let l = nm * 1e-9;
        l.powi(-5) / ((c2 / (l * t)).exp() - 1.0)
    };
    let norm = at(560.0);
    std::array::from_fn(|i| 100.0 * at(wavelength(i)) / norm)
}

/// CIE daylight at a correlated colour temperature `t` (4,000–25,000 K), 100 at 560 nm: its chromaticity from
/// the temperature, then S0 + M1·S1 + M2·S2, as CIE 15 defines it.
pub fn daylight(t: f64) -> [f64; SAMPLES] {
    let t = t.clamp(4000.0, 25000.0);
    let x = if t <= 7000.0 {
        -4.6070e9 / t.powi(3) + 2.9678e6 / t.powi(2) + 0.09911e3 / t + 0.244063
    } else {
        -2.0064e9 / t.powi(3) + 1.9018e6 / t.powi(2) + 0.24748e3 / t + 0.237040
    };
    let y = -3.0 * x * x + 2.870 * x - 0.275;
    let m = 0.0241 + 0.2562 * x - 0.7341 * y;
    let (m1, m2) = ((-1.3515 - 1.7703 * x + 5.9114 * y) / m, (0.0300 - 31.4424 * x + 30.0717 * y) / m);
    let raw: [f64; SAMPLES] = std::array::from_fn(|i| DAYLIGHT_S0[i] + m1 * DAYLIGHT_S1[i] + m2 * DAYLIGHT_S2[i]);
    // 560 nm is sample 18.
    std::array::from_fn(|i| 100.0 * raw[i] / raw[18])
}

/// The light a scene was lit by, by id: `daylight` (D65, the default), `tungsten` (CIE illuminant A, a
/// household bulb at 2,856 K), or `white-balance` — the temperature the edit's white balance names, as a black
/// body below 5,000 K and daylight above (the split CIE's colour-rendering index uses). `None` for a light this
/// version doesn't know.
pub fn light(id: &str, temperature_k: f64) -> Option<[f64; SAMPLES]> {
    match id {
        "daylight" => Some(D65),
        "tungsten" => Some(planck(2848.0, 1.435e-2)),
        "white-balance" if temperature_k < 5000.0 => Some(planck(temperature_k.max(1000.0), 1.4388e-2)),
        "white-balance" => Some(daylight(temperature_k)),
        _ => None,
    }
}

/// The coefficient grid `mono_lut` reads, computed once: `GRID`³ colours over the unit cube.
pub const GRID: usize = 17;

/// The brightness each colour is fitted at, as a reflectance: middle grey's.
const SURFACE_Y: f64 = 0.18;

/// The scale a colour of the cube is fitted at: its luminance brought to middle grey's, its largest channel
/// kept under 0.95. Light is linear, so any scale gives the same exposure once it is divided back out — but not
/// the same spectrum: the family's spectrum for a colour depends on its brightness, and a colour scaled up
/// until a channel reaches 1 is brighter and more saturated than any surface, which the family can only meet
/// with odd shapes — a red that rises again in the violet, unseen by the eye and seen by blue-sensitive film.
/// At a surface's brightness, a red is a red: low in the blue and violet.
fn fit_scale(rgb: [f64; 3]) -> f64 {
    let y = REC2020_TO_XYZ[1][0] * rgb[0] + REC2020_TO_XYZ[1][1] * rgb[1] + REC2020_TO_XYZ[1][2] * rgb[2];
    let max = rgb.iter().cloned().fold(0.0, f64::max);
    if y <= 1e-6 || max <= 1e-6 {
        return 1.0;
    }
    (SURFACE_Y / y).min(0.95 / max)
}

/// For each colour of the cube: its coefficients, fitted at `fit_scale`, and that scale.
fn grid() -> &'static [([f64; 3], f64)] {
    static GRID_COEFFICIENTS: std::sync::OnceLock<Vec<([f64; 3], f64)>> = std::sync::OnceLock::new();
    GRID_COEFFICIENTS.get_or_init(|| {
        let at = |i: usize| i as f64 / (GRID - 1) as f64;
        (0..GRID * GRID * GRID)
            .map(|n| {
                let rgb = [at(n % GRID), at(n / GRID % GRID), at(n / (GRID * GRID))];
                let k = fit_scale(rgb);
                (coefficients(rgb.map(|v| v * k)), k)
            })
            .collect()
    })
}

/// The exposure black-and-white film `film` behind filter `filter` gets from each colour of the unit cube
/// (linear Rec.2020, white-balanced), `GRID`³ values, red fastest, white at 1. Each colour is a surface — the
/// reflectance whose colour under D65 it is, which is what white balance makes of it — lit by `light`
/// (`light()`): film isn't white-balanced, so under tungsten it gets far less blue than red, and the meter
/// (white at 1) makes up the rest. A colour beyond the cube is scaled into it by its largest channel, which then
/// scales the exposure: light is linear. `None` for a film or filter this version doesn't know.
pub fn mono_lut(film: &str, filter: &str, light: &[f64; SAMPLES]) -> Option<Vec<f32>> {
    let (s, f) = (film_sensitivity(film)?, filter_transmission(filter)?);
    let weight: [f64; SAMPLES] = std::array::from_fn(|i| light[i] * s[i] * f[i]);
    let total: f64 = weight.iter().sum();
    let exposure = |(c, k): ([f64; 3], f64)| (0..SAMPLES).map(|i| reflectance(c, i) * weight[i]).sum::<f64>() / total / k;
    let grid = grid();
    // White is a flat spectrum, so it is its own exposure whatever the film; dividing by it only takes out the
    // fit's last thousandth.
    let white = exposure(grid[GRID * GRID * GRID - 1]);
    Some(grid.iter().map(|&g| (exposure(g) / white) as f32).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectral_data::REFERENCE;

    /// Our fit lands on colour-science's spectra: same colour, same family, so the same curve.
    #[test]
    fn matches_colour_science() {
        for (rgb, reference) in REFERENCE {
            let c = coefficients(rgb);
            let worst = (0..SAMPLES).map(|i| (reflectance(c, i) - reference[i]).abs()).fold(0.0, f64::max);
            assert!(worst < 0.01, "{rgb:?}: worst {worst}");
        }
    }

    /// Across the cube: every spectrum is a valid reflectance and has the colour it was fitted to.
    #[test]
    fn spectra_are_reflectances_of_their_colours() {
        let white = white();
        // Colours real surfaces have (the cube's corners are beyond any reflectance: more saturated than a surface
        // can be, they end at the nearest the family reaches).
        for &rgb in &[[0.5, 0.08, 0.06], [0.1, 0.4, 0.1], [0.08, 0.12, 0.5], [0.6, 0.55, 0.1], [0.3, 0.3, 0.3], [0.02, 0.02, 0.02], [0.95, 0.95, 0.95]] {
            let c = coefficients(rgb);
            assert!((0..SAMPLES).all(|i| (0.0..=1.0).contains(&reflectance(c, i))));
            let (got, want) = (lab(xyz(c), white), lab(rec2020_to_xyz(rgb), white));
            let de = ((got[0] - want[0]).powi(2) + (got[1] - want[1]).powi(2) + (got[2] - want[2]).powi(2)).sqrt();
            assert!(de < 1.0, "{rgb:?}: ΔE {de}");
        }
    }

    fn at(lut: &[f32], rgb: [usize; 3]) -> f32 {
        lut[rgb[0] + rgb[1] * GRID + rgb[2] * GRID * GRID]
    }

    /// Greys are their own exposure; orthochromatic film renders a red darker than panchromatic does, and a
    /// blue lighter; a red filter lightens red and darkens blue.
    #[test]
    fn films_and_filters_behave_as_their_kinds() {
        let pan = mono_lut("panchromatic", "none", &D65).unwrap();
        let ortho = mono_lut("orthochromatic", "none", &D65).unwrap();
        let red_filter = mono_lut("panchromatic", "red", &D65).unwrap();
        let g = GRID - 1;
        assert!((at(&pan, [g, g, g]) - 1.0).abs() < 1e-6);
        let mid = g / 2;
        assert!((at(&pan, [mid, mid, mid]) - 0.5).abs() < 0.02, "mid grey {}", at(&pan, [mid, mid, mid]));
        // A red and a blue at the table's points nearest surface colours (about 0.5, 0.06, 0.06 and the like).
        let (red, blue) = ([mid, 1, 1], [1, 2, mid]);
        assert!(at(&ortho, red) < 0.5 * at(&pan, red), "ortho red {} pan red {}", at(&ortho, red), at(&pan, red));
        assert!(at(&ortho, blue) > at(&pan, blue));
        assert!(at(&red_filter, red) > at(&pan, red) && at(&red_filter, blue) < at(&pan, blue));
        assert!(mono_lut("panchromatic", "purple", &D65).is_none() && mono_lut("velvia", "none", &D65).is_none());
    }

    /// Everyday colours as each kind renders them, measured: blue-sensitive film darkens skin by more than half
    /// a stop and lifts the sky; orthochromatic film takes a red down to near black; a red filter darkens a blue
    /// sky by more than a stop.
    #[test]
    fn everyday_colours_go_as_the_films_do() {
        let e = |film: &str, filter: &str, rgb: [f64; 3]| {
            let lut = mono_lut(film, filter, &D65).unwrap();
            let m = rgb.iter().cloned().fold(0.0, f64::max);
            m * at(&lut, rgb.map(|v| (v / m * (GRID - 1) as f64).round() as usize)) as f64
        };
        let (skin, sky, red) = ([0.45, 0.30, 0.20], [0.25, 0.42, 0.75], [0.55, 0.06, 0.05]);
        let pan = |c| e("panchromatic", "none", c);
        assert!(e("blue-sensitive", "none", skin) < 0.7 * pan(skin), "{} vs {}", e("blue-sensitive", "none", skin), pan(skin));
        assert!(e("blue-sensitive", "none", sky) > pan(sky));
        assert!(e("orthochromatic", "none", red) < 0.4 * pan(red), "{} vs {}", e("orthochromatic", "none", red), pan(red));
        assert!(e("panchromatic", "red", sky) < 0.5 * pan(sky), "{} vs {}", e("panchromatic", "red", sky), pan(sky));
    }

    /// Planck's law at CIE A's defining temperature and constant is CIE A; CIE's daylight formula at D50's temperature
    /// is colour-science's D50.
    #[test]
    fn illuminants_match_cie() {
        use crate::spectral_data::{D50, ILLUMINANT_A};
        let a = planck(2848.0, 1.435e-2);
        let worst = (0..SAMPLES).map(|i| (a[i] - ILLUMINANT_A[i]).abs() / ILLUMINANT_A[i]).fold(0.0, f64::max);
        assert!(worst < 1e-3, "A: worst relative {worst}");
        let d50 = daylight(5003.0 * 1.4388 / 1.4380);
        let worst = (0..SAMPLES).map(|i| (d50[i] - D50[i]).abs()).fold(0.0, f64::max);
        assert!(worst < 0.1, "D50: worst {worst}");
        assert!(light("white-balance", 3000.0).unwrap()[0] < light("white-balance", 6500.0).unwrap()[0]);
        assert!(light("candle", 6500.0).is_none());
    }

    /// Under tungsten, film gets a red's light and little of a blue's: panchromatic film renders a red lighter and a
    /// blue sky darker than in daylight, against the same metered grey.
    #[test]
    fn tungsten_lightens_reds_and_darkens_blues() {
        let (red, sky) = ([0.55, 0.06, 0.05], [0.25, 0.42, 0.75]);
        let e = |light: &[f64; SAMPLES], rgb: [f64; 3]| {
            let lut = mono_lut("panchromatic", "none", light).unwrap();
            let m = rgb.iter().cloned().fold(0.0, f64::max);
            m * at(&lut, rgb.map(|v| (v / m * (GRID - 1) as f64).round() as usize)) as f64
        };
        let tungsten = planck(2848.0, 1.435e-2);
        eprintln!("tungsten against daylight: red {:+.2} stop, sky {:+.2} stop", (e(&tungsten, red) / e(&D65, red)).log2(), (e(&tungsten, sky) / e(&D65, sky)).log2());
        assert!(e(&tungsten, red) > 1.2 * e(&D65, red), "{} vs {}", e(&tungsten, red), e(&D65, red));
        assert!(e(&tungsten, sky) < 0.9 * e(&D65, sky), "{} vs {}", e(&tungsten, sky), e(&D65, sky));
        let mid = (GRID - 1) / 2;
        assert!((at(&mono_lut("panchromatic", "none", &tungsten).unwrap(), [mid, mid, mid]) - 0.5).abs() < 0.02);
    }

    /// A film's filter factor behind one of our filters, in light `light`.
    fn factor(s: &[f64; SAMPLES], filter: &str, light: &[f64; SAMPLES]) -> f64 {
        let t = filter_transmission(filter).unwrap();
        let open: f64 = (0..SAMPLES).map(|i| light[i] * s[i]).sum();
        open / (0..SAMPLES).map(|i| light[i] * s[i] * t[i]).sum::<f64>()
    }

    /// Each stock's filter factors through our generic filters land near its maker's table: red (Wratten 25) and
    /// orange (21) within 0.45 stop, yellow (8) within 0.65, our yellow being deeper than an 8; our green is
    /// broader than a 58, so it isn't compared. SFX 200's
    /// table is what fixes how ILFORD's spectrograms are read: as drawn, the film's relative sensitivity; divided
    /// by the tungsten light the title names, its red factor would be 9 where ILFORD says 2.8.
    #[test]
    fn stocks_filter_factors_match_their_sheets() {
        let tungsten = planck(3200.0, 1.4388e-2);
        let checks: [(&str, &str, &[f64; SAMPLES], f64); 9] = [
            ("sfx-200", "yellow", &D65, 2.0),
            ("sfx-200", "orange", &D65, 2.4),
            ("sfx-200", "red", &D65, 2.8),
            ("tri-x-400", "yellow", &D65, 2.0),
            ("tri-x-400", "red", &D65, 8.0),
            ("tri-x-400", "yellow", &tungsten, 1.5),
            ("tri-x-400", "red", &tungsten, 5.0),
            ("double-x", "yellow", &D65, 1.5),
            ("double-x", "red", &D65, 8.0),
        ];
        for (film, filter, light, sheet) in checks {
            let ours = factor(&film_sensitivity(film).unwrap(), filter, light);
            let off = (ours / sheet).log2();
            eprintln!("{film} behind {filter}: {ours:.1} (sheet {sheet}), {off:+.2} stop");
            // Our yellow is a long-pass at 500 nm, between a Wratten 8 and a 15: compared with the 8, loosely.
            let within = if filter == "yellow" { 0.65 } else { 0.45 };
            assert!(off.abs() < within, "{film} {filter}: {ours} against {sheet}");
        }
    }

    /// HP5 Plus, a classic panchromatic film, renders yellow darker and a blue sky lighter than the eye sees them.
    #[test]
    fn hp5_sees_more_blue_than_the_eye() {
        let lut = mono_lut("hp5-plus", "none", &D65).unwrap();
        let e = |rgb: [f64; 3]| {
            let m = rgb.iter().cloned().fold(0.0, f64::max);
            m * at(&lut, rgb.map(|v| (v / m * (GRID - 1) as f64).round() as usize)) as f64
        };
        let lum = |rgb: [f64; 3]| REC2020_TO_XYZ[1][0] * rgb[0] + REC2020_TO_XYZ[1][1] * rgb[1] + REC2020_TO_XYZ[1][2] * rgb[2];
        let (yellow, sky) = ([0.6, 0.5, 0.05], [0.25, 0.42, 0.75]);
        assert!(e(yellow) < 0.8 * lum(yellow) && e(sky) > 1.15 * lum(sky), "yellow {} / {}, sky {} / {}", e(yellow), lum(yellow), e(sky), lum(sky));
    }

    #[test]
    fn the_grid_is_quick_enough() {
        let t = std::time::Instant::now();
        let _ = mono_lut("panchromatic", "none", &D65);
        eprintln!("first table: {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    }
}
