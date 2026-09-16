// SPDX-License-Identifier: Apache-2.0
//! Edit parameters as stored in `sc:Sidecar`, and their packing into the develop uniform buffer.

use serde::{Deserialize, Serialize};

use crate::camera::{self, CameraColor};
use crate::mat::{self, M3};
use crate::spline::{LUT_SIZE, monotone_lut};
use crate::tone;

pub const PROCESS_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct EditParams {
    pub process_version: u32,
    pub white_balance: WhiteBalance,
    pub exposure: Exposure,
    pub tone: Tone,
    pub mixer: Mixer,
    pub detail: Detail,
}

impl Default for EditParams {
    fn default() -> Self {
        Self {
            process_version: PROCESS_VERSION,
            white_balance: WhiteBalance::default(),
            exposure: Exposure::default(),
            tone: Tone::default(),
            mixer: Mixer::default(),
            detail: Detail::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum WhiteBalanceMode {
    #[default]
    AsShot,
    Custom,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct WhiteBalance {
    pub mode: WhiteBalanceMode,
    pub temperature_k: f64,
    pub tint: f64,
}

impl Default for WhiteBalance {
    fn default() -> Self {
        Self { mode: WhiteBalanceMode::AsShot, temperature_k: 5500.0, tint: 0.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Exposure {
    pub ev: f64,
    pub highlight_blend: f64,
}

impl Default for Exposure {
    fn default() -> Self {
        Self { ev: 0.0, highlight_blend: 0.5 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Tone {
    pub black_ev: f64,
    pub white_ev: f64,
    pub contrast: f64,
    /// Control points over display-encoded values, `[x, y]` in 0…1.
    pub curve: Vec<[f64; 2]>,
}

impl Default for Tone {
    fn default() -> Self {
        Self { black_ev: -8.0, white_ev: 4.5, contrast: 1.0, curve: vec![[0.0, 0.0], [1.0, 1.0]] }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MixerBand {
    pub hue: f64,
    pub saturation: f64,
    pub luminance: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Mixer {
    pub red: MixerBand,
    pub orange: MixerBand,
    pub yellow: MixerBand,
    pub green: MixerBand,
    pub aqua: MixerBand,
    pub blue: MixerBand,
    pub purple: MixerBand,
    pub magenta: MixerBand,
}

impl Mixer {
    pub fn bands(&self) -> [MixerBand; 8] {
        [self.red, self.orange, self.yellow, self.green, self.aqua, self.blue, self.purple, self.magenta]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Detail {
    pub sharpen_amount: f64,
    pub sharpen_radius: f64,
    pub sharpen_iterations: u32,
    pub denoise_luminance: f64,
    pub denoise_chroma: f64,
}

impl Default for Detail {
    fn default() -> Self {
        Self { sharpen_amount: 0.0, sharpen_radius: 1.0, sharpen_iterations: 10, denoise_luminance: 0.0, denoise_chroma: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DisplaySpace {
    #[default]
    Srgb,
    DisplayP3,
}

/// Byte-for-byte mirror of `struct DevelopParams` in shaders/develop/params.wgsl. A `mat3x3f` is three
/// columns, each padded to 16 bytes. The test below parses the WGSL with naga and checks every offset.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DevelopParams {
    pub wb_mul: [f32; 3],
    pub exposure: f32,
    pub cam_to_work: [[f32; 4]; 3],
    pub work_to_disp: [[f32; 4]; 3],
    pub tone: [f32; 4],
    pub hl_strength: f32,
    pub _pad0: f32,
    pub _pad1: [f32; 2],
    pub mixer: [[f32; 4]; 8],
}

/// Contrast slider value → logistic steepness.
pub const CONTRAST_SCALE: f64 = 7.0;

pub struct Packed {
    pub uniform: Vec<u8>,
    pub lut: Vec<f32>,
    pub temperature_k: f64,
    pub tint: f64,
}

/// Row-major 3×3 → WGSL `mat3x3f` columns.
fn to_mat3(m: &M3) -> [[f32; 4]; 3] {
    [0, 1, 2].map(|j| [m[0][j] as f32, m[1][j] as f32, m[2][j] as f32, 0.0])
}

pub fn pack(edit: &EditParams, camera: &CameraColor, display: DisplaySpace) -> Packed {
    let white_xy = match edit.white_balance.mode {
        WhiteBalanceMode::AsShot => camera.as_shot_xy(),
        WhiteBalanceMode::Custom => crate::cct::temp_tint_to_xy(edit.white_balance.temperature_k, edit.white_balance.tint),
    };
    let wb = camera::resolve(camera, white_xy);
    let t = &edit.tone;
    let k = (t.contrast * CONTRAST_SCALE).max(0.5);
    let pivot = tone::pivot_for(t.black_ev, t.white_ev, k);
    let bands = edit.mixer.bands();
    let params = DevelopParams {
        wb_mul: wb.mul.map(|v| v as f32),
        exposure: 2f64.powf(edit.exposure.ev) as f32,
        cam_to_work: to_mat3(&wb.cam_to_rec2020),
        work_to_disp: to_mat3(match display {
            DisplaySpace::Srgb => &mat::REC2020_TO_SRGB,
            DisplaySpace::DisplayP3 => &mat::REC2020_TO_P3,
        }),
        tone: [t.black_ev as f32, t.white_ev as f32, k as f32, pivot as f32],
        hl_strength: edit.exposure.highlight_blend.clamp(0.0, 1.0) as f32,
        _pad0: 0.0,
        _pad1: [0.0; 2],
        mixer: bands.map(|b| {
            [
                (b.hue.clamp(-100.0, 100.0) / 100.0 * 30f64.to_radians()) as f32,
                (b.saturation.clamp(-100.0, 100.0) / 100.0) as f32,
                (b.luminance.clamp(-100.0, 100.0) / 100.0 * 0.15) as f32,
                0.0,
            ]
        }),
    };
    Packed { uniform: bytemuck::bytes_of(&params).to_vec(), lut: monotone_lut(&t.curve, LUT_SIZE), temperature_k: wb.temperature_k, tint: wb.tint }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_layout_matches_wgsl() {
        let src = sidecar_shaders::compose("develop/params.wgsl").unwrap();
        let module = naga::front::wgsl::parse_str(&src).unwrap();
        let mut layouter = naga::proc::Layouter::default();
        layouter.update(module.to_ctx()).unwrap();
        let (handle, ty) = module.types.iter().find(|(_, t)| t.name.as_deref() == Some("DevelopParams")).unwrap();
        let naga::TypeInner::Struct { members, span } = &ty.inner else { panic!("not a struct") };
        assert_eq!(*span as usize, std::mem::size_of::<DevelopParams>());
        assert_eq!(layouter[handle].size as usize, 272);
        let rust = [
            ("wb_mul", std::mem::offset_of!(DevelopParams, wb_mul)),
            ("exposure", std::mem::offset_of!(DevelopParams, exposure)),
            ("cam_to_work", std::mem::offset_of!(DevelopParams, cam_to_work)),
            ("work_to_disp", std::mem::offset_of!(DevelopParams, work_to_disp)),
            ("tone", std::mem::offset_of!(DevelopParams, tone)),
            ("hl_strength", std::mem::offset_of!(DevelopParams, hl_strength)),
            ("mixer", std::mem::offset_of!(DevelopParams, mixer)),
        ];
        for (name, off) in rust {
            let m = members.iter().find(|m| m.name.as_deref() == Some(name)).unwrap();
            assert_eq!(m.offset as usize, off, "{name}");
        }
        let p = pack(&EditParams::default(), &crate::camera::nikon_d850(), DisplaySpace::Srgb);
        assert_eq!(p.uniform.len(), 272);
        assert_eq!(p.lut.len(), LUT_SIZE);
        // exposure sits at byte 12
        assert_eq!(f32::from_le_bytes(p.uniform[12..16].try_into().unwrap()), 1.0);
    }

    #[test]
    fn edit_params_round_trip_through_json() {
        let json = r#"{"exposure":{"ev":0.65},"mixer":{"blue":{"luminance":-35}}}"#;
        let e: EditParams = serde_json::from_str(json).unwrap();
        assert_eq!(e.exposure.ev, 0.65);
        assert_eq!(e.mixer.blue.luminance, -35.0);
        assert_eq!(e.tone.white_ev, 4.5);
    }
}
