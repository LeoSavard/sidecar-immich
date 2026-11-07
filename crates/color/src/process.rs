// SPDX-License-Identifier: Apache-2.0
//! Alternative processes (DESIGN.md §7, "Alternative processes"): a black-and-white rendering printed as a
//! cyanotype, or made as a tintype — each process's own colours and tonal range, computed from a model of its
//! materials under D65 rather than picked.
//!
//! **Cyanotype.** Iron salts on paper, turned to Prussian blue where light reached them through the negative: the
//! print darkens subtractively, its reflectance `paper(λ) · 10^(−D · a(λ))` (Beer–Lambert). Prussian blue's
//! absorbance is its broad intervalence band in the red, centred near 700 nm (modelled as a Gaussian of σ 110 nm),
//! over a weak absorbance everywhere (15 % of the peak): it takes the red and much of the green and leaves the
//! blue. The model is anchored to what is measured of the prints themselves — a cyanotype's maximum *visual*
//! density, about 1.3 (`CYANO_VISUAL_DMAX`) — by solving for the peak density that gives it; the spectrum's shape
//! then decides only the colour. The paper is a cotton rag, reflectance 0.80 in the violet rising to 0.86 in the
//! red.
//!
//! **Tintype.** Wet collodion on black-japanned iron, a direct positive: silver deposits are the light tones, the
//! lacquer the dark ones, side by side at a scale the eye can't separate, so the plate mixes the two additively.
//! Black japan reflects about 4 %; collodion silver a warm grey, 30 % in the violet rising to 50 % in the red.
//! Collodion saw only blue light: the film type to pair it with is Blue-sensitive.
//!
//! Both are generic processes, not named products, so the materials are approximated from their published
//! characteristics. Each process's lightest surface is shown at 0.9 of white (the viewer adapts to the brightest
//! thing in view) with its own contrast ratio kept: a tintype is a dim object, but it is looked at as the
//! brightest thing there.

use crate::mat;
use crate::spectral::SAMPLES;
use crate::spectral_data::{CMF, D65};

/// A cyanotype's maximum visual (luminance) reflection density, as measured on prints.
pub const CYANO_VISUAL_DMAX: f64 = 1.3;
/// Where the lightest surface of a process is shown.
pub const VIEWED_WHITE: f64 = 0.9;

fn wavelength(i: usize) -> f64 {
    380.0 + 10.0 * i as f64
}

/// Linear Rec.2020 of a reflectance under D65, white (reflectance 1) at Y = 1.
fn colour_of(r: impl Fn(f64) -> f64) -> [f64; 3] {
    let mut xyz = [0.0; 3];
    let mut norm = 0.0;
    for i in 0..SAMPLES {
        let e = D65[i] * r(wavelength(i));
        for k in 0..3 {
            xyz[k] += e * CMF[i][k];
        }
        norm += D65[i] * CMF[i][1];
    }
    mat::apply(&mat::XYZ_TO_REC2020, &xyz.map(|v| v / norm))
}

const LUMA_2020: [f64; 3] = [0.2627, 0.6780, 0.0593];
fn luminance(c: [f64; 3]) -> f64 {
    (0..3).map(|k| LUMA_2020[k] * c[k]).sum()
}

fn paper(l: f64) -> f64 {
    0.80 + 0.06 * (l - 380.0) / 350.0
}

fn prussian_blue(l: f64) -> f64 {
    0.15 + 0.85 * (-0.5 * ((l - 700.0) / 110.0).powi(2)).exp()
}

/// The peak density at which the print's visual density is `CYANO_VISUAL_DMAX`, by bisection: visual density
/// rises with it monotonically.
fn cyano_peak_density() -> f64 {
    let white = luminance(colour_of(paper));
    let visual = |d: f64| {
        -(luminance(colour_of(|l| paper(l) * 10f64.powf(-d * prussian_blue(l)))) / white).log10()
    };
    let (mut lo, mut hi) = (0.0, 10.0);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if visual(mid) < CYANO_VISUAL_DMAX {
            lo = mid
        } else {
            hi = mid
        }
    }
    0.5 * (lo + hi)
}

/// `DevelopParams.process`, `process_dark` and `process_light` for `process` (`cyanotype`, `tintype`; anything
/// else is none).
pub fn uniform(process: &str) -> Option<[[f32; 4]; 3]> {
    let f = |v: [f64; 3], w: f64| [v[0] as f32, v[1] as f32, v[2] as f32, w as f32];
    match process {
        "cyanotype" => {
            let dmax = cyano_peak_density();
            let white = colour_of(paper);
            let dark = colour_of(|l| paper(l) * 10f64.powf(-dmax * prussian_blue(l)));
            let scale = VIEWED_WHITE / luminance(white);
            let (white, dark) = (white.map(|v| v * scale), dark.map(|v| v * scale));
            // Each channel's absorbance per unit density, fitted at the darkest: Beer–Lambert per channel.
            let k = [0, 1, 2].map(|c| -(dark[c] / white[c]).log10() / dmax);
            let k_y = -(luminance(dark) / luminance(white)).log10() / dmax;
            Some([
                [1.0, dmax as f32, k_y as f32, 0.0],
                f(k, luminance(dark)),
                f(white, luminance(white)),
            ])
        }
        "tintype" => {
            let silver = colour_of(|l| 0.30 + 0.20 * (l - 380.0) / 350.0);
            let japan = colour_of(|_| 0.04);
            let scale = VIEWED_WHITE / luminance(silver);
            let (silver, japan) = (silver.map(|v| v * scale), japan.map(|v| v * scale));
            Some([
                [2.0, 0.0, 0.0, 0.0],
                f(japan, luminance(japan)),
                f(silver, luminance(silver)),
            ])
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OKLab hue in degrees of a linear Rec.2020 colour (Ottosson's matrices, through linear sRGB).
    fn hue(c: [f32; 3]) -> f32 {
        let [r, g, b] = mat::apply(&mat::REC2020_TO_SRGB, &c.map(f64::from));
        let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
        let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
        let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
        let a = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
        let bb = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
        bb.atan2(a).to_degrees().rem_euclid(360.0) as f32
    }

    /// A cyanotype's deepest tone is Prussian blue — a blue hue, its red far below its blue — on a near-white
    /// paper, and its tonal range is the visual density it was anchored to (1.3, about 4.3 stops).
    #[test]
    fn cyanotype_is_prussian_blue_on_white() {
        let [p, k, white] = uniform("cyanotype").unwrap();
        assert_eq!(p[0], 1.0);
        let dark = [0, 1, 2].map(|c| white[c] * 10f32.powf(-p[1] * k[c]));
        println!("paper {white:?}, deepest {dark:?}, hue {}", hue(dark));
        assert!((white[3] - 0.9).abs() < 1e-4);
        assert!(dark[2] > 3.0 * dark[0], "deep blue: {dark:?}");
        let h = hue(dark);
        assert!((220.0..275.0).contains(&h), "a blue hue: {h}");
        let density = (white[3] / k[3]).log10();
        assert!(
            (density - CYANO_VISUAL_DMAX as f32).abs() < 0.01,
            "visual Dmax {density}"
        );
        assert!(k[0] > k[2], "red absorbed more than blue");
    }

    /// A tintype runs from japan black to a warm grey, about three and a half stops.
    #[test]
    fn tintype_is_warm_silver_on_black() {
        let [p, japan, silver] = uniform("tintype").unwrap();
        assert_eq!(p[0], 2.0);
        assert!(silver[0] > silver[2], "warm: {silver:?}");
        let stops = (silver[3] / japan[3]).log2();
        assert!((3.0..4.0).contains(&stops), "{stops} stops");
        assert!(uniform("silver-gelatin").is_none());
    }
}
