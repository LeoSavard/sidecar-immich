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
}

impl CameraColor {
    fn xyz_to_camera_at(&self, temperature_k: f64) -> M3 {
        match self.calibrations.as_slice() {
            [only] => only.xyz_to_camera,
            [a, b, ..] => {
                let (lo, hi) = if a.temperature_k <= b.temperature_k { (a, b) } else { (b, a) };
                let inv = |t: f64| 1.0 / t;
                let w = ((inv(temperature_k) - inv(hi.temperature_k)) / (inv(lo.temperature_k) - inv(hi.temperature_k))).clamp(0.0, 1.0);
                mat::lerp(&lo.xyz_to_camera, &hi.xyz_to_camera, w)
            }
            [] => panic!("camera has no colour calibration"),
        }
    }

    /// White point chromaticity implied by the as-shot multipliers.
    pub fn as_shot_xy(&self) -> [f64; 2] {
        let m = self.as_shot_mul;
        let neutral = [m[1] / m[0], 1.0, m[1] / m[2]];
        let mut xy = mat::D65;
        for _ in 0..30 {
            let (t, _) = xy_to_temp_tint(xy);
            let xyz = mat::apply(&mat::inverse(&self.xyz_to_camera_at(t)), &neutral);
            xy = mat::xyz_to_xy(&xyz);
        }
        xy
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
    let cam_wb_to_xyz = mat::mul(&mat::inverse(&cm), &mat::diag([1.0 / mul[0], 1.0, 1.0 / mul[2]]));
    let mut m = mat::mul(&mat::XYZ_TO_REC2020, &mat::mul(&mat::bradford(white_xy, mat::D65), &cam_wb_to_xyz));
    let white = mat::apply(&m, &[1.0, 1.0, 1.0]);
    for (row, w) in m.iter_mut().zip(white) {
        for v in row.iter_mut() {
            *v /= w;
        }
    }
    ResolvedWhiteBalance { temperature_k: t, tint, mul, cam_to_rec2020: m }
}


#[cfg(test)]
pub(crate) fn nikon_d850() -> CameraColor {
    // Adobe coefficients as distributed in LibRaw's colordata table (XYZ → camera, ×10000).
    let m = [10405.0, -3755.0, -1270.0, -5461.0, 13787.0, 1793.0, -1040.0, 2015.0, 6785.0].map(|v| v / 10000.0);
    CameraColor {
        as_shot_mul: [2.0996, 1.0, 1.4453],
        calibrations: vec![Calibration { temperature_k: 6504.0, xyz_to_camera: [[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]] }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_maps_to_neutral_and_as_shot_round_trips() {
        let cam = nikon_d850();
        let xy = cam.as_shot_xy();
        let wb = resolve(&cam, xy);
        assert!((wb.mul[0] - 2.0996).abs() < 1e-3 && (wb.mul[2] - 1.4453).abs() < 1e-3, "{:?}", wb.mul);
        let w = mat::apply(&wb.cam_to_rec2020, &[1.0, 1.0, 1.0]);
        assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-9));
        assert!((3000.0..8000.0).contains(&wb.temperature_k), "{}", wb.temperature_k);
    }
}
