// SPDX-License-Identifier: Apache-2.0
//! Camera white balance and camera → working-space matrices, following the DNG specification's model:
//! XYZ → camera colour matrices at one or two calibration illuminants, interpolated in inverse temperature.

use serde::{Deserialize, Serialize};

use crate::cct::xy_to_temp_tint;
use crate::mat::{self, M3, V3};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Calibration {
    /// Correlated colour temperature of the calibration illuminant.
    pub temperature_k: f64,
    /// XYZ → camera RGB, rows.
    pub xyz_to_camera: M3,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraColor {
    /// As-shot white balance multipliers, camera RGB.
    pub as_shot_mul: V3,
    /// One or two calibrations. With two, they are interpolated by white balance temperature.
    pub calibrations: Vec<Calibration>,
    /// `BaselineExposure` (DNG 50730), in stops, where the file states it. This is **data, not a guess**:
    /// where it is present it says how far under the rendering's middle grey this maker places a correct
    /// exposure, and it replaces `BASELINE_EV`, which is the measured stand-in for files that say nothing.
    /// Apple's ProRAW declares 4.13 — three stops more than the stand-in, and a photo rendered without it is
    /// three stops dark. Absent for anything but a DNG read through `linear_dng`.
    #[serde(default)]
    pub baseline_exposure: Option<f64>,
}

impl CameraColor {
    fn xyz_to_camera_at(&self, temperature_k: f64) -> M3 {
        match self.calibrations.as_slice() {
            [only] => only.xyz_to_camera,
            [a, b, ..] => {
                let (lo, hi) = if a.temperature_k <= b.temperature_k {
                    (a, b)
                } else {
                    (b, a)
                };
                let inv = |t: f64| 1.0 / t;
                let w = ((inv(temperature_k) - inv(hi.temperature_k))
                    / (inv(lo.temperature_k) - inv(hi.temperature_k)))
                .clamp(0.0, 1.0);
                mat::lerp(&lo.xyz_to_camera, &hi.xyz_to_camera, w)
            }
            // A file that states no matrix at all (a linear DNG without ColorMatrix1, say) still has to
            // open: treat its camera RGB as sRGB primaries, as the LibRaw path does for an unknown camera.
            // This also runs in the browser's wasm, where a panic takes the whole packer down.
            [] => SourceSpace::Srgb.xyz_to_rgb(),
        }
    }

    /// White point chromaticity implied by the as-shot multipliers.
    pub fn as_shot_xy(&self) -> [f64; 2] {
        let m = self.as_shot_mul;
        self.neutral_xy([m[1] / m[0], 1.0, m[1] / m[2]])
    }

    /// The white point under which `neutral`, a camera RGB, is grey: what the white balance eyedropper sets
    /// from the patch it samples. Iterates because the colour matrix depends on the temperature it finds.
    pub fn neutral_xy(&self, neutral: V3) -> [f64; 2] {
        let mut xy = mat::D65;
        for _ in 0..30 {
            let (t, _) = xy_to_temp_tint(xy);
            let xyz = mat::apply(&mat::inverse(&self.xyz_to_camera_at(t)), &neutral);
            xy = mat::xyz_to_xy(&xyz);
        }
        xy
    }
}

/// The colour space a developed (non-RAW) image already lives in, decoded by the worker or by the browser. All
/// have a D65 white; sRGB and Display P3 share the sRGB transfer function, Adobe RGB (1998) has its own.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SourceSpace {
    #[default]
    Srgb,
    DisplayP3,
    AdobeRgb,
}

impl SourceSpace {
    /// Guess from an ICC profile description. Anything unrecognised is treated as sRGB, which is what an
    /// untagged JPEG almost always is.
    pub fn from_profile(description: &str) -> Self {
        let d = description.to_ascii_lowercase();
        if description.contains("P3") {
            Self::DisplayP3
        } else if d.contains("adobe rgb") || d.contains("adobergb") {
            Self::AdobeRgb
        } else {
            Self::Srgb
        }
    }

    /// From what a file says about itself: its ICC profile's description when it has one, else EXIF. A camera set
    /// to Adobe RGB writes its JPEGs with no profile at all and marks them in EXIF instead, with `ColorSpace`
    /// 0xFFFF ("uncalibrated") and `InteropIndex` "R03" — the DCF option-file rule — or, on some bodies,
    /// `ColorSpace` 2. Read as sRGB, such a photo comes out desaturated (the review's D11).
    pub fn from_tags(profile: &str, color_space: u32, interop: &str) -> Self {
        if !profile.trim().is_empty() {
            return Self::from_profile(profile);
        }
        match (color_space, interop.trim()) {
            (2, _) | (0xFFFF, "R03") => Self::AdobeRgb,
            _ => Self::Srgb,
        }
    }

    /// Encoded value (0…1) → linear light: the space's transfer function. Adobe RGB's is a pure power of
    /// 563/256 (2.2 in round numbers), with no linear segment.
    pub fn decode(self, v: f32) -> f32 {
        match self {
            Self::Srgb | Self::DisplayP3 => {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            }
            Self::AdobeRgb => v.max(0.0).powf(563.0 / 256.0),
        }
    }

    /// XYZ → this space, derived from the working-space constants (or the space's own primaries) rather than
    /// written out again, so there is one source of truth for each.
    pub fn xyz_to_rgb(self) -> mat::M3 {
        match self {
            Self::Srgb => mat::mul(&mat::REC2020_TO_SRGB, &mat::XYZ_TO_REC2020),
            Self::DisplayP3 => mat::mul(&mat::REC2020_TO_P3, &mat::XYZ_TO_REC2020),
            Self::AdobeRgb => mat::inverse(&mat::rgb_to_xyz(mat::ADOBE_RGB_PRIMARIES, mat::D65)),
        }
    }

    /// A `CameraColor` that makes the existing colour pipeline treat this space as the camera's native one.
    pub fn camera_color(self) -> CameraColor {
        CameraColor {
            as_shot_mul: [1.0, 1.0, 1.0],
            calibrations: vec![Calibration {
                temperature_k: 6504.0,
                xyz_to_camera: self.xyz_to_rgb(),
            }],
            baseline_exposure: None,
        }
    }
}

/// EXIF LightSource → correlated colour temperature, for DNG calibration illuminants.
pub fn illuminant_temperature(code: u16) -> Option<f64> {
    Some(match code {
        17 | 3 => 2856.0,
        24 => 3200.0,
        15 => 3450.0,
        14 => 4150.0,
        2 => 4200.0,
        18 => 4874.0,
        13 => 5000.0,
        23 => 5003.0,
        1 | 4 | 9 => 5500.0,
        20 => 5503.0,
        12 => 6430.0,
        10 | 21 => 6504.0,
        19 => 6774.0,
        11 | 22 => 7504.0,
        _ => return None,
    })
}

/// XYZ → camera for one DNG calibration, composed as the DNG specification composes it ("Mapping Camera Color
/// Space to CIE XYZ Space"): `AnalogBalance · CameraCalibration · ColorMatrix`. A `ColorMatrix` alone maps XYZ
/// to the *reference* camera; the other two carry it to this unit's values. A calibration that is absent (or
/// all zero) is the identity, and so is a balance that isn't three positive gains.
///
/// Apple's ProRAW is the case that makes this matter: it records `AsShotNeutral` as 1 1 1 and the shot's
/// white balance in `AnalogBalance` (2.92 1 1.65 on an iPhone 17 Pro), so a neutral read against the bare
/// `ColorMatrix` came out at 2,786 K and tint −158 on a photo whose white is ordinary daylight.
pub fn dng_xyz_to_camera(
    color_matrix: &mat::M3,
    calibration: Option<&mat::M3>,
    analog_balance: Option<[f64; 3]>,
) -> mat::M3 {
    let cc = calibration
        .filter(|m| !is_zero(m))
        .copied()
        .unwrap_or(mat::diag([1.0; 3]));
    let ab = analog_balance
        .filter(|b| b.iter().all(|v| *v > 0.0 && v.is_finite()))
        .unwrap_or([1.0; 3]);
    mat::mul(&mat::diag(ab), &mat::mul(&cc, color_matrix))
}

fn rows3(m: &[[f32; 3]; 4]) -> [[f64; 3]; 3] {
    [0, 1, 2].map(|r| m[r].map(|v| v as f64))
}

fn is_zero(m: &[[f64; 3]; 3]) -> bool {
    m.iter().flatten().all(|v| v.abs() < 1e-9)
}

/// The colour data LibRaw reads from a file, as the server's shim (`sc_info`) and the browser's LibRaw build
/// (mobile/libraw/wasm_api.cpp, in these camelCase names) hand it over. One mapping to a `CameraColor` for both.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibrawColor {
    pub cam_mul: [f32; 4],
    pub pre_mul: [f32; 4],
    /// XYZ → camera, from LibRaw's colour data (rows; the fourth is a fourth colour's, unused).
    pub cam_xyz: [[f32; 3]; 4],
    /// EXIF LightSource codes of the DNG calibration illuminants; 0 where there is none.
    pub dng_illuminant: [u16; 2],
    pub dng_colormatrix: [[[f32; 3]; 4]; 2],
    /// DNG `AnalogBalance`, per colour; zero (or absent, from an older build) is no balance.
    #[serde(default)]
    pub dng_analogbalance: [f32; 4],
    /// DNG `CameraCalibration1/2`, beside each `dng_colormatrix`; zero (or absent) is the identity.
    #[serde(default)]
    pub dng_calibration: [[[f32; 4]; 4]; 2],
}

impl LibrawColor {
    /// DNG calibrations where the file has them, LibRaw's matrix for the model otherwise, sRGB primaries for a
    /// camera nobody knows; as-shot multipliers from the camera, then LibRaw's daylight ones.
    pub fn camera_color(&self) -> CameraColor {
        let mut calibrations = Vec::new();
        let ab = self.dng_analogbalance;
        let ab = [ab[0] as f64, ab[1] as f64, ab[2] as f64];
        for n in 0..2 {
            let m = rows3(&self.dng_colormatrix[n]);
            let cc = [0, 1, 2].map(|r| [0, 1, 2].map(|c| self.dng_calibration[n][r][c] as f64));
            if let (Some(t), false) = (illuminant_temperature(self.dng_illuminant[n]), is_zero(&m))
            {
                calibrations.push(Calibration {
                    temperature_k: t,
                    xyz_to_camera: dng_xyz_to_camera(&m, Some(&cc), Some(ab)),
                });
            }
        }
        if calibrations.is_empty() {
            let m = rows3(&self.cam_xyz);
            let m = if is_zero(&m) {
                // Unknown camera: treat camera RGB as sRGB primaries (XYZ → linear sRGB).
                [
                    [3.2404542, -1.5371385, -0.4985314],
                    [-0.9692660, 1.8760108, 0.0415560],
                    [0.0556434, -0.2040259, 1.0572252],
                ]
            } else {
                m
            };
            calibrations.push(Calibration {
                temperature_k: 6504.0,
                xyz_to_camera: m,
            });
        }
        let pick = |m: [f32; 4]| {
            (m[0] > 0.0 && m[1] > 0.0 && m[2] > 0.0)
                .then(|| [m[0] as f64 / m[1] as f64, 1.0, m[2] as f64 / m[1] as f64])
        };
        let as_shot_mul = pick(self.cam_mul)
            .or_else(|| pick(self.pre_mul))
            .unwrap_or([2.0, 1.0, 1.5]);
        CameraColor {
            as_shot_mul,
            calibrations,
            baseline_exposure: None,
        }
    }
}

pub struct ResolvedWhiteBalance {
    pub temperature_k: f64,
    pub tint: f64,
    /// Multipliers normalized so green = 1.
    pub mul: V3,
    pub cam_to_rec2020: M3,
}

/// Resolve a white point into shader inputs: multipliers and the camera → linear Rec.2020 matrix, normalized
/// so that a white-balanced neutral (1, 1, 1) maps to Rec.2020 (1, 1, 1).
pub fn resolve(camera: &CameraColor, white_xy: [f64; 2]) -> ResolvedWhiteBalance {
    let (t, tint) = xy_to_temp_tint(white_xy);
    let cm = camera.xyz_to_camera_at(t);
    let neutral = mat::apply(&cm, &mat::xy_to_xyz(white_xy));
    let mul = [neutral[1] / neutral[0], 1.0, neutral[1] / neutral[2]];
    let cam_wb_to_xyz = mat::mul(
        &mat::inverse(&cm),
        &mat::diag([1.0 / mul[0], 1.0, 1.0 / mul[2]]),
    );
    let mut m = mat::mul(
        &mat::XYZ_TO_REC2020,
        &mat::mul(&mat::bradford(white_xy, mat::D65), &cam_wb_to_xyz),
    );
    let white = mat::apply(&m, &[1.0, 1.0, 1.0]);
    for (row, w) in m.iter_mut().zip(white) {
        for v in row.iter_mut() {
            *v /= w;
        }
    }
    ResolvedWhiteBalance {
        temperature_k: t,
        tint,
        mul,
        cam_to_rec2020: m,
    }
}

#[cfg(test)]
pub(crate) fn nikon_d850() -> CameraColor {
    // Adobe coefficients as distributed in LibRaw's colordata table (XYZ → camera, ×10000).
    let m = [
        10405.0, -3755.0, -1270.0, -5461.0, 13787.0, 1793.0, -1040.0, 2015.0, 6785.0,
    ]
    .map(|v| v / 10000.0);
    CameraColor {
        as_shot_mul: [2.0996, 1.0, 1.4453],
        calibrations: vec![Calibration {
            temperature_k: 6504.0,
            xyz_to_camera: [[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]],
        }],
        baseline_exposure: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D11: a camera's Adobe RGB JPEG is known by its EXIF when it has no profile; a profile always wins.
    /// Apple's ProRAW keeps the shot's white balance in `AnalogBalance` and writes 1 1 1 as `AsShotNeutral`
    /// (an iPhone 17 Pro frame's tags, verbatim). Read against the bare `ColorMatrix` its white came out at
    /// 2,786 K and tint −158, off the end of the slider, on a photo whose white is ordinary daylight; composed
    /// as the DNG specification composes it, it reads as daylight.
    #[test]
    fn proraw_white_balance_comes_from_its_analog_balance() {
        let cm1 = [
            [1.299852848, -0.6473193765, -0.2318986207],
            [-0.4558799858, 1.513552667, -0.02695739455],
            [-0.04014902193, 0.1442424505, 0.6399207116],
        ];
        let cm2 = [
            [0.9567329279, -0.3796998561, -0.1292831898],
            [-0.4220122099, 1.30898881, 0.0894335062],
            [-0.09921766815, 0.2101706116, 0.467231214],
        ];
        let colour = |ab: Option<[f64; 3]>| CameraColor {
            as_shot_mul: [1.0, 1.0, 1.0],
            calibrations: vec![
                Calibration {
                    temperature_k: 2856.0,
                    xyz_to_camera: dng_xyz_to_camera(&cm1, None, ab),
                },
                Calibration {
                    temperature_k: 6504.0,
                    xyz_to_camera: dng_xyz_to_camera(&cm2, None, ab),
                },
            ],
            baseline_exposure: None,
        };
        let shot = |c: &CameraColor| resolve(c, c.as_shot_xy());
        let bare = shot(&colour(None));
        assert!(
            bare.tint < -100.0,
            "the bug this guards: {} K {}",
            bare.temperature_k,
            bare.tint
        );
        let composed = shot(&colour(Some([2.921630961, 1.0, 1.649902266])));
        assert!(
            (5500.0..8000.0).contains(&composed.temperature_k) && composed.tint.abs() < 50.0,
            "{} K {}",
            composed.temperature_k,
            composed.tint
        );
        // No calibration and no balance is the matrix itself.
        assert_eq!(
            dng_xyz_to_camera(&cm1, Some(&[[0.0; 3]; 3]), Some([0.0; 3])),
            cm1
        );
    }

    /// The phone's LibRaw build from before `AnalogBalance` and `CameraCalibration` were passed still parses.
    #[test]
    fn libraw_colour_without_the_dng_balance_fields_still_reads() {
        let json = r#"{"camMul":[2,1,1.5,0],"preMul":[2,1,1.5,0],"camXyz":[[1,0,0],[0,1,0],[0,0,1],[0,0,0]],
            "dngIlluminant":[0,0],"dngColormatrix":[[[0,0,0],[0,0,0],[0,0,0],[0,0,0]],[[0,0,0],[0,0,0],[0,0,0],[0,0,0]]]}"#;
        let c: LibrawColor = serde_json::from_str(json).unwrap();
        assert_eq!(c.dng_analogbalance, [0.0; 4]);
        assert_eq!(c.camera_color().as_shot_mul, [2.0, 1.0, 1.5]);
    }

    #[test]
    fn adobe_rgb_is_read_from_exif_when_there_is_no_profile() {
        use SourceSpace::*;
        assert_eq!(SourceSpace::from_tags("", 0xFFFF, "R03"), AdobeRgb);
        assert_eq!(SourceSpace::from_tags("", 2, ""), AdobeRgb);
        assert_eq!(SourceSpace::from_tags("", 1, "R98"), Srgb);
        assert_eq!(SourceSpace::from_tags("", 0xFFFF, ""), Srgb);
        assert_eq!(SourceSpace::from_tags("", 0, ""), Srgb);
        assert_eq!(SourceSpace::from_tags("Adobe RGB (1998)", 0, ""), AdobeRgb);
        assert_eq!(
            SourceSpace::from_tags("Display P3", 0xFFFF, "R03"),
            DisplayP3
        );
        assert_eq!(
            SourceSpace::from_tags("sRGB IEC61966-2.1", 0xFFFF, "R03"),
            Srgb
        );
    }

    /// Adobe RGB's green is far wider than sRGB's, and its white is still D65.
    #[test]
    fn adobe_rgb_is_wider_than_srgb_with_the_same_white() {
        let to_srgb = |space: SourceSpace| {
            mat::mul(
                &SourceSpace::Srgb.xyz_to_rgb(),
                &mat::inverse(&space.xyz_to_rgb()),
            )
        };
        let m = to_srgb(SourceSpace::AdobeRgb);
        let white = mat::apply(&m, &[1.0, 1.0, 1.0]);
        assert!(white.iter().all(|w| (w - 1.0).abs() < 1e-3), "{white:?}");
        let green = mat::apply(&m, &[0.0, 1.0, 0.0]);
        assert!(
            green[0] < -0.3 && green[2] < -0.03,
            "pure Adobe green is outside sRGB: {green:?}"
        );
        assert!((SourceSpace::AdobeRgb.decode(0.5) - 0.5f32.powf(2.199_218_8)).abs() < 1e-6);
        assert!((SourceSpace::Srgb.decode(0.5) - 0.214_041).abs() < 1e-5);
    }

    #[test]
    fn neutral_maps_to_neutral_and_as_shot_round_trips() {
        let cam = nikon_d850();
        let xy = cam.as_shot_xy();
        let wb = resolve(&cam, xy);
        assert!(
            (wb.mul[0] - 2.0996).abs() < 1e-3 && (wb.mul[2] - 1.4453).abs() < 1e-3,
            "{:?}",
            wb.mul
        );
        let w = mat::apply(&wb.cam_to_rec2020, &[1.0, 1.0, 1.0]);
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-9));
        assert!(
            (3000.0..8000.0).contains(&wb.temperature_k),
            "{}",
            wb.temperature_k
        );
    }

    /// A patch picked as neutral comes out grey once its white point is resolved, through the same temperature
    /// and tint the sliders store.
    #[test]
    fn a_picked_patch_resolves_to_grey() {
        let cam = nikon_d850();
        let patch = [0.21, 0.40, 0.33];
        let (t, tint) = crate::cct::xy_to_temp_tint(cam.neutral_xy(patch));
        let wb = resolve(&cam, crate::cct::temp_tint_to_xy(t, tint));
        let balanced = [patch[0] * wb.mul[0], patch[1], patch[2] * wb.mul[2]];
        let out = mat::apply(&wb.cam_to_rec2020, &balanced);
        assert!(
            out.iter().all(|v| (v / out[1] - 1.0).abs() < 1e-3),
            "{out:?} at {t:.0} K {tint:+.1}"
        );
    }

    /// The JSON the browser's LibRaw build writes (mobile/libraw/wasm_api.cpp) is what `LibrawColor` reads, field
    /// for field, and maps as the server's shim does: a camera's matrix, its as-shot multipliers over green.
    #[test]
    fn libraw_color_reads_the_browser_builds_json() {
        let json = r#"{"camMul":[2.0996,1,1.4453,0],"preMul":[2,1,1.3,0],
            "camXyz":[[1.0405,-0.3755,-0.127],[-0.5461,1.3787,0.1793],[-0.104,0.2015,0.6785],[0,0,0]],
            "dngIlluminant":[0,0],"dngColormatrix":[[[0,0,0],[0,0,0],[0,0,0],[0,0,0]],[[0,0,0],[0,0,0],[0,0,0],[0,0,0]]]}"#;
        let color: LibrawColor = serde_json::from_str(json).unwrap();
        let cam = color.camera_color();
        let d850 = nikon_d850();
        assert_eq!(cam.as_shot_mul, [2.0996f32 as f64, 1.0, 1.4453f32 as f64]);
        assert_eq!(cam.calibrations.len(), 1);
        for (a, b) in cam.calibrations[0]
            .xyz_to_camera
            .iter()
            .flatten()
            .zip(d850.calibrations[0].xyz_to_camera.iter().flatten())
        {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
        // Two DNG calibrations, and no multipliers from the camera: LibRaw's daylight ones stand in.
        let mut dng = color.clone();
        dng.cam_mul = [0.0; 4];
        dng.dng_illuminant = [17, 21];
        dng.dng_colormatrix = [color.cam_xyz, color.cam_xyz];
        let cam = dng.camera_color();
        assert_eq!(
            cam.calibrations
                .iter()
                .map(|c| c.temperature_k)
                .collect::<Vec<_>>(),
            [2856.0, 6504.0]
        );
        assert_eq!(cam.as_shot_mul, [2.0, 1.0, 1.3f32 as f64]);
    }

    /// A file with no colour matrix resolves as sRGB instead of panicking, and its neutral stays neutral.
    #[test]
    fn no_calibration_resolves_as_srgb() {
        let cam = CameraColor {
            as_shot_mul: [1.0, 1.0, 1.0],
            calibrations: Vec::new(),
            baseline_exposure: None,
        };
        let wb = resolve(&cam, cam.as_shot_xy());
        assert!(
            wb.mul.iter().all(|m| (m - 1.0).abs() < 1e-6),
            "{:?}",
            wb.mul
        );
        let w = mat::apply(&wb.cam_to_rec2020, &[1.0, 1.0, 1.0]);
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-9), "{w:?}");
    }
}
