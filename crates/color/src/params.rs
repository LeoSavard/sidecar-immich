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
    /// Absent means off: photos edited before this field existed render unchanged. See DESIGN.md §15.
    pub lens: Option<Lens>,
    /// Adjustments through masks, at most `MAX_MASKS`. Left out of the serialized edit when empty, so an edit
    /// without masks keeps the ETag — and the history ids and export records keyed by it — it always had.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<MaskAdjustment>,
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
            lens: None,
            masks: Vec::new(),
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

/// Where the default tone curve puts black and white, in stops from middle grey. The Blacks and Whites sliders
/// are offsets from these (the web app's `view.blacks` / `view.whites` mirror them).
pub const DEFAULT_BLACK_EV: f64 = -8.0;
pub const DEFAULT_WHITE_EV: f64 = 4.5;

impl Default for Tone {
    fn default() -> Self {
        Self { black_ev: DEFAULT_BLACK_EV, white_ev: DEFAULT_WHITE_EV, contrast: 1.0, curve: vec![[0.0, 0.0], [1.0, 1.0]] }
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

/// Distortion, lateral CA and vignetting from the photo's profile, applied by the lens pass before develop
/// (`crate::lens`, DESIGN.md §15).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Lens {
    /// `LensProfile::id` of the profile it was set against: `dng`, `sony`, `fujifilm` or `lensfun:<maker>/<model>`.
    pub profile: String,
    /// Correction strength, 0…200; 100 is the profile's full correction.
    pub distortion: f64,
    pub lateral_ca: f64,
    pub vignetting: f64,
}

impl Default for Lens {
    fn default() -> Self {
        Self { profile: String::new(), distortion: 100.0, lateral_ca: 100.0, vignetting: 100.0 }
    }
}

/// Masks the develop pass takes at once: texture bindings, and the uniform array's length.
pub const MAX_MASKS: usize = 8;

/// An adjustment applied through a mask (DESIGN.md §15): each value is added, weighted by the mask, to what
/// the global edit does there — `ev = global.ev + Σ wₖ · maskₖ.ev`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct MaskAdjustment {
    /// Which mask, by name: `subject`, `skin`, `face-skin`, `body-skin`, `hair`, `clothes`. The raster is
    /// cached by the worker, never stored here.
    pub mask: String,
    /// Apply to everything the mask leaves out: the background, for `subject`.
    pub invert: bool,
    /// How much of the mask applies, 0…100.
    pub amount: f64,
    /// Stops, added to the global exposure.
    pub exposure: f64,
    /// −100…100 each.
    pub contrast: f64,
    pub saturation: f64,
    /// Warmer (+) or cooler (−), and more magenta (+) or green (−), relative to the global white balance.
    pub temperature: f64,
    pub tint: f64,
    /// Fields a newer Sidecar wrote (another kind of mask, a shape), kept as they are so saving here doesn't
    /// lose them. An entry this version can't render is carried along and skipped (`renders`).
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

impl Default for MaskAdjustment {
    fn default() -> Self {
        Self { mask: String::new(), invert: false, amount: 100.0, exposure: 0.0, contrast: 0.0, saturation: 0.0, temperature: 0.0, tint: 0.0, other: Default::default() }
    }
}

/// The masks the models make, which an adjustment can name. Kept in step with the worker's `masks::NAMES`.
pub const MASK_NAMES: [&str; 6] = ["subject", "skin", "face-skin", "body-skin", "hair", "clothes"];

impl MaskAdjustment {
    /// Whether this version knows how to render it: a mask it knows by name, and nothing a newer version
    /// added to say it is something else.
    pub fn renders(&self) -> bool {
        MASK_NAMES.contains(&self.mask.as_str()) && self.other.is_empty()
    }
}

/// One mask's adjustment as the shader takes it. Mirrors `struct MaskAdjust` in shaders/develop/masks.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaskAdjust {
    /// rgb: white-balance gains in camera space at full weight; w: exposure in stops.
    pub gain: [f32; 4],
    /// x: contrast, y: saturation (each −1…1), z: amount (0…1), w: 1 to invert.
    pub tone: [f32; 4],
}

/// Byte-for-byte mirror of `struct MaskParams` in shaders/develop/masks.wgsl (288 bytes); checked by naga.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaskParams {
    /// xy: where the texture being developed sits in the frame the masks cover, in its pixels; zw: that
    /// frame's size (0 for the texture's own size — a whole image). A loupe tile sets both.
    pub frame: [f32; 4],
    pub count: u32,
    pub _pad: [u32; 3],
    pub adjust: [MaskAdjust; MAX_MASKS],
}

/// Relative white balance for a mask: gains in camera space that move the global white point `(t, tint)` by
/// the mask's temperature and tint. Temperature ±100 is ±40 mired, tint ±100 is ±30 tint units. The matrix
/// is kept from the global white balance; a local shift is small enough for that to hold.
fn mask_gains(camera: &CameraColor, global: &camera::ResolvedWhiteBalance, m: &MaskAdjustment) -> [f64; 3] {
    if m.temperature == 0.0 && m.tint == 0.0 {
        return [1.0; 3];
    }
    // Warmer means assuming a bluer light: a higher temperature, a lower mired value.
    let mired = (1e6 / global.temperature_k - 0.4 * m.temperature.clamp(-100.0, 100.0)).clamp(20.0, 1000.0);
    let tint = global.tint + 0.3 * m.tint.clamp(-100.0, 100.0);
    let local = camera::resolve(camera, crate::cct::temp_tint_to_xy(1e6 / mired, tint));
    [0, 1, 2].map(|i| local.mul[i] / global.mul[i])
}

/// What the source pixels are, which decides how the develop pass renders them.
///
/// A RAW is scene-referred: sensor light, which the base tone curve renders for display. A JPEG, HEIC, PNG
/// or TIFF is display-referred: whatever wrote it already rendered it — tone curve, white clip, gamut — and
/// none of that can be undone. Rendering it again as scene light tone-maps it twice (white at 82 %, the top
/// fifth of the range pressed into a tenth), so a developed file gets controls that act on the picture it
/// is: identity at default settings, and no step that promises recovery the file can't give.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    /// Metadata cached before this field existed is RAW.
    #[default]
    Raw,
    Developed,
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
    /// 0 for a RAW, 1 for a developed file — see `SourceKind`.
    pub display_referred: f32,
    pub _pad1: [f32; 2],
    pub mixer: [[f32; 4]; 8],
}

/// Contrast slider value → logistic steepness.
pub const CONTRAST_SCALE: f64 = 7.0;

/// A developed file's tone settings, from the same stored fields a RAW uses so the editor's sliders need no
/// second meaning: Blacks and Whites as −1…1 (the slider over 100), Contrast as the S-curve's exponent, and
/// the shoulder's knee, which only drops below white once exposure pushes the picture past it.
fn display_tone(edit: &EditParams) -> [f32; 4] {
    let t = &edit.tone;
    let blacks = ((DEFAULT_BLACK_EV - t.black_ev) / 3.0).clamp(-1.0, 1.0);
    let whites = ((DEFAULT_WHITE_EV - t.white_ev) / 2.0).clamp(-1.0, 1.0);
    let knee = if edit.exposure.ev > 0.0 { (1.0 - 0.3 * edit.exposure.ev).clamp(0.4, 1.0) } else { 1.0 };
    [blacks as f32, whites as f32, t.contrast.max(0.1) as f32, knee as f32]
}

pub struct Packed {
    pub uniform: Vec<u8>,
    pub lut: Vec<f32>,
    pub temperature_k: f64,
    pub tint: f64,
    /// `MaskParams` for the edit's masks, frame left at the whole image; texture `i` is `edit.masks[i]`'s.
    pub masks: MaskParams,
}

/// Row-major 3×3 → WGSL `mat3x3f` columns.
fn to_mat3(m: &M3) -> [[f32; 4]; 3] {
    [0, 1, 2].map(|j| [m[0][j] as f32, m[1][j] as f32, m[2][j] as f32, 0.0])
}

pub fn pack(edit: &EditParams, camera: &CameraColor, source: SourceKind, display: DisplaySpace) -> Packed {
    let white_xy = match edit.white_balance.mode {
        WhiteBalanceMode::AsShot => camera.as_shot_xy(),
        WhiteBalanceMode::Custom => crate::cct::temp_tint_to_xy(edit.white_balance.temperature_k, edit.white_balance.tint),
    };
    let wb = camera::resolve(camera, white_xy);
    let t = &edit.tone;
    let developed = source == SourceKind::Developed;
    let tone = if developed {
        display_tone(edit)
    } else {
        let k = (t.contrast * CONTRAST_SCALE).max(0.5);
        [t.black_ev as f32, t.white_ev as f32, k as f32, tone::pivot_for(t.black_ev, t.white_ev, k) as f32]
    };
    let bands = edit.mixer.bands();
    let params = DevelopParams {
        wb_mul: wb.mul.map(|v| v as f32),
        exposure: 2f64.powf(edit.exposure.ev) as f32,
        cam_to_work: to_mat3(&wb.cam_to_rec2020),
        work_to_disp: to_mat3(match display {
            DisplaySpace::Srgb => &mat::REC2020_TO_SRGB,
            DisplaySpace::DisplayP3 => &mat::REC2020_TO_P3,
        }),
        tone,
        // Highlight blending neutralises what white balance does to sensor-clipped pixels; a developed file
        // has no sensor clipping to speak of.
        hl_strength: if developed { 0.0 } else { edit.exposure.highlight_blend.clamp(0.0, 1.0) as f32 },
        display_referred: if developed { 1.0 } else { 0.0 },
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
    let mut masks = MaskParams { frame: [0.0; 4], count: 0, _pad: [0; 3], adjust: [MaskAdjust::default(); MAX_MASKS] };
    for (slot, m) in masks.adjust.iter_mut().zip(edit.masks.iter().take(MAX_MASKS)) {
        masks.count += 1;
        // Kept in its slot, so texture `i` stays `edit.masks[i]`'s, at no weight.
        if !m.renders() {
            continue;
        }
        let g = mask_gains(camera, &wb, m);
        let unit = |v: f64| (v.clamp(-100.0, 100.0) / 100.0) as f32;
        *slot = MaskAdjust {
            gain: [g[0] as f32, g[1] as f32, g[2] as f32, m.exposure.clamp(-5.0, 5.0) as f32],
            tone: [unit(m.contrast), unit(m.saturation), (m.amount.clamp(0.0, 100.0) / 100.0) as f32, if m.invert { 1.0 } else { 0.0 }],
        };
    }
    Packed { uniform: bytemuck::bytes_of(&params).to_vec(), lut: monotone_lut(&t.curve, LUT_SIZE), temperature_k: wb.temperature_k, tint: wb.tint, masks }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default settings on a developed file must be the identity: every tone value at its neutral, and the
    /// RAW-only highlight blend off whatever the stored strength says.
    #[test]
    fn a_developed_file_packs_neutral_at_default() {
        let p = pack(&EditParams::default(), &crate::camera::nikon_d850(), SourceKind::Developed, DisplaySpace::Srgb);
        let params: &DevelopParams = bytemuck::from_bytes(&p.uniform);
        assert_eq!(params.tone, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!((params.hl_strength, params.display_referred), (0.0, 1.0));

        let mut edit = EditParams::default();
        edit.tone.white_ev = 2.5;
        edit.tone.black_ev = -5.0;
        edit.exposure.ev = 1.0;
        let p = pack(&edit, &crate::camera::nikon_d850(), SourceKind::Developed, DisplaySpace::Srgb);
        let params: &DevelopParams = bytemuck::from_bytes(&p.uniform);
        assert_eq!(params.tone, [-1.0, 1.0, 1.0, 0.7], "Blacks −100, Whites +100, shoulder at 0.7 for +1 EV");
    }

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
            ("display_referred", std::mem::offset_of!(DevelopParams, display_referred)),
            ("mixer", std::mem::offset_of!(DevelopParams, mixer)),
        ];
        for (name, off) in rust {
            let m = members.iter().find(|m| m.name.as_deref() == Some(name)).unwrap();
            assert_eq!(m.offset as usize, off, "{name}");
        }
        let p = pack(&EditParams::default(), &crate::camera::nikon_d850(), SourceKind::Raw, DisplaySpace::Srgb);
        assert_eq!(p.uniform.len(), 272);
        assert_eq!(p.lut.len(), LUT_SIZE);
        // exposure sits at byte 12
        assert_eq!(f32::from_le_bytes(p.uniform[12..16].try_into().unwrap()), 1.0);
    }

    /// An edit without masks serializes as it did before masks existed: its ETag, and everything keyed by it,
    /// must not change.
    #[test]
    fn an_edit_without_masks_serializes_as_before() {
        let json = serde_json::to_value(EditParams::default()).unwrap();
        assert!(json.get("masks").is_none());
        let mut e = EditParams::default();
        e.masks.push(MaskAdjustment { mask: "subject".into(), exposure: 0.5, ..Default::default() });
        let back: EditParams = serde_json::from_value(serde_json::to_value(&e).unwrap()).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn masks_pack_their_gains_and_weights() {
        let cam = crate::camera::nikon_d850();
        let mut e = EditParams::default();
        e.masks.push(MaskAdjustment { mask: "subject".into(), exposure: 1.0, contrast: 50.0, amount: 50.0, invert: true, ..Default::default() });
        e.masks.push(MaskAdjustment { mask: "skin".into(), temperature: 50.0, ..Default::default() });
        let m = pack(&e, &cam, SourceKind::Raw, DisplaySpace::Srgb).masks;
        assert_eq!(m.count, 2);
        assert_eq!(m.adjust[0].gain, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(m.adjust[0].tone, [0.5, 0.0, 0.5, 1.0]);
        let [r, g, b, _] = m.adjust[1].gain;
        assert!(r > 1.0 && b < 1.0 && (g - 1.0).abs() < 1e-6, "warmer: more red, less blue, green fixed: {:?}", m.adjust[1].gain);
    }

    #[test]
    fn mask_layout_matches_wgsl() {
        let src = sidecar_shaders::compose("develop/masks.wgsl").unwrap();
        let module = naga::front::wgsl::parse_str(&src).unwrap();
        let (_, ty) = module.types.iter().find(|(_, t)| t.name.as_deref() == Some("MaskParams")).unwrap();
        let naga::TypeInner::Struct { members, span } = &ty.inner else { panic!("not a struct") };
        assert_eq!(*span as usize, std::mem::size_of::<MaskParams>());
        assert_eq!(std::mem::size_of::<MaskParams>(), 288);
        let off = |n: &str| members.iter().find(|m| m.name.as_deref() == Some(n)).unwrap().offset as usize;
        assert_eq!(off("frame"), std::mem::offset_of!(MaskParams, frame));
        assert_eq!(off("count"), std::mem::offset_of!(MaskParams, count));
        assert_eq!(off("adjust"), std::mem::offset_of!(MaskParams, adjust));
    }

    #[test]
    fn edit_params_round_trip_through_json() {
        let json = r#"{"exposure":{"ev":0.65},"mixer":{"blue":{"luminance":-35}}}"#;
        let e: EditParams = serde_json::from_str(json).unwrap();
        assert_eq!(e.exposure.ev, 0.65);
        assert_eq!(e.mixer.blue.luminance, -35.0);
        assert_eq!(e.tone.white_ev, 4.5);
        assert_eq!(e.lens, None);
    }

    #[test]
    fn lens_absent_by_default_present_when_set() {
        assert_eq!(EditParams::default().lens, None);

        let json = r#"{"lens":{"profile":"fujifilm/xf16mmf14","distortion":80}}"#;
        let e: EditParams = serde_json::from_str(json).unwrap();
        let lens = e.lens.clone().expect("lens present");
        assert_eq!(lens.profile, "fujifilm/xf16mmf14");
        assert_eq!(lens.distortion, 80.0);
        assert_eq!(lens.lateral_ca, 100.0);
        assert_eq!(lens.vignetting, 100.0);

        let round_tripped: EditParams = serde_json::from_value(serde_json::to_value(&e).unwrap()).unwrap();
        assert_eq!(round_tripped, e);
    }
}
