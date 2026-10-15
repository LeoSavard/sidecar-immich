// SPDX-License-Identifier: Apache-2.0
//! Edit parameters as stored in `sc:Sidecar`, and their packing into the develop uniform buffer.

use serde::{Deserialize, Serialize};

use crate::camera::{self, CameraColor};
use crate::mat::{self, M3};
use crate::spline::{HIGHLIGHT_SHADOW_SCALE, LUT_SIZE, monotone_lut, tone_curve_lut};
use crate::tone;

/// 2: Highlights and Shadows stopped sharing points with the manual curve (`Tone::highlights`/`shadows`,
/// `migrate_highlight_shadow_anchors`) — an edit saved under version 1 needs its curve read differently, not
/// just a field that defaults to off, so this is a real bump rather than an additive field. See
/// docs/ROADMAP.md, "Curve editor: Highlights and Shadows stop sharing points with the manual curve".
pub const PROCESS_VERSION: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct EditParams {
    pub process_version: u32,
    pub white_balance: WhiteBalance,
    pub exposure: Exposure,
    pub tone: Tone,
    pub mixer: Mixer,
    /// Global chroma gain, −100…100, 0 at default — the same OKLCh scale `MaskAdjustment.saturation` uses, as
    /// the baseline every mask's own saturation adds to (`DevelopParams.saturation`,
    /// `shaders/develop/masks.wgsl`'s `mask_start`). Left out of the serialized edit at 0, so an edit that
    /// never touches it keeps the ETag it already had.
    #[serde(skip_serializing_if = "is_zero")]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub saturation: f64,
    /// Film rendering instead of the neutral one (DESIGN.md §7, "Film"), for a RAW. Absent means neutral, and
    /// is left out of the serialized edit, so every edit made before it renders and hashes as it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub film: Option<Film>,
    pub detail: Detail,
    /// Absent means off: photos edited before this field existed render unchanged. See DESIGN.md §15.
    pub lens: Option<Lens>,
    /// Crop, straighten, keystone and turns, after lens correction (`crate::geometry`). Absent means none, and
    /// is left out of the serialized edit so every edit made before it keeps its ETag.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub geometry: Option<crate::geometry::Geometry>,
    /// Adjustments through masks, at most `MAX_MASKS`. Left out of the serialized edit when empty, so an edit
    /// without masks keeps the ETag — and the history ids and export records keyed by it — it always had.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<MaskAdjustment>>", optional))]
    pub masks: Vec<MaskAdjustment>,
    /// A lens blur by distance, from the depth mask (DESIGN.md §15, "Depth effects"). Absent is none, and is
    /// left out of the serialized edit.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub lens_blur: Option<LensBlur>,
    /// Haze by distance, from the depth mask: −100 takes it away, 100 adds it; 0, left out of the serialized
    /// edit, is none.
    #[serde(skip_serializing_if = "is_zero")]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub haze: f64,
}

/// A lens blur by distance: how far it reaches, where the focus is, and how deep the sharp band around it is.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct LensBlur {
    /// 0–100: the blur at the farthest from the focus, up to `crate::depth::MAX_BLUR` of the long edge.
    pub amount: f64,
    /// 0–100: the distance in focus, as the depth mask reads it (0 farthest, 100 nearest).
    pub focus: f64,
    /// 0–100: how much of the depth range around the focus stays sharp.
    pub range: f64,
}

impl Default for LensBlur {
    fn default() -> Self {
        Self { amount: 50.0, focus: 100.0, range: 10.0 }
    }
}

impl Default for EditParams {
    fn default() -> Self {
        Self {
            process_version: PROCESS_VERSION,
            white_balance: WhiteBalance::default(),
            exposure: Exposure::default(),
            tone: Tone::default(),
            mixer: Mixer::default(),
            saturation: 0.0,
            film: None,
            detail: Detail::default(),
            lens: None,
            geometry: None,
            masks: Vec::new(),
            lens_blur: None,
            haze: 0.0,
        }
    }
}

impl EditParams {
    /// Bring an edit stored under an older `PROCESS_VERSION` to the current meaning, and stamp it current.
    /// Idempotent: an edit already at this version is left as it is (a point a person later drew at exactly
    /// x = 0.75 is theirs, not an old Highlights anchor). A newer version is never lowered.
    pub fn migrate(&mut self) {
        if self.process_version < 2 {
            self.migrate_highlight_shadow_anchors();
        }
        self.process_version = self.process_version.max(PROCESS_VERSION);
    }

    /// An edit saved under `PROCESS_VERSION` 1 stored Highlights/Shadows as curve points at x≈0.25/0.75
    /// (`CURVE_ANCHORS` in the web app, before this version): pulls those two points' offsets out into
    /// `tone.highlights`/`shadows` and removes them from `tone.curve`, so a slider that was at, say, +20 keeps
    /// reading +20 — its strength carries forward even though the curve no longer shares a point with it.
    /// Called through `migrate`, which `sidecar_xmp` runs on the current edit and on every history version
    /// it hands out; an edit built fresh (no anchor points to find) is unaffected either way.
    fn migrate_highlight_shadow_anchors(&mut self) {
        let anchor_offset = |curve: &[[f64; 2]], x: f64| {
            curve.iter().find(|p| (p[0] - x).abs() < 1e-6).map(|p| (p[1] - x) / HIGHLIGHT_SHADOW_SCALE * 100.0)
        };
        if let Some(v) = anchor_offset(&self.tone.curve, 0.25) {
            self.tone.shadows = v;
        }
        if let Some(v) = anchor_offset(&self.tone.curve, 0.75) {
            self.tone.highlights = v;
        }
        self.tone.curve.retain(|p| (p[0] - 0.25).abs() >= 1e-6 && (p[0] - 0.75).abs() >= 1e-6);
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub enum WhiteBalanceMode {
    #[default]
    AsShot,
    Custom,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct Tone {
    pub black_ev: f64,
    pub white_ev: f64,
    pub contrast: f64,
    /// Control points over display-encoded values, `[x, y]` in 0…1. Manually drawn only, as of
    /// `PROCESS_VERSION` 2 — Highlights and Shadows are `highlights`/`shadows` below, not points in here.
    pub curve: Vec<[f64; 2]>,
    /// −100…100 each: a smooth additive bump on top of `curve` (`spline::tone_curve_lut`), not a point in it,
    /// so drawing a custom curve can never have a slider silently steal or move one of its points.
    #[serde(skip_serializing_if = "is_zero")]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub highlights: f64,
    #[serde(skip_serializing_if = "is_zero")]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub shadows: f64,
}

/// Where the default tone curve puts black and white, in stops from middle grey. The Blacks and Whites sliders
/// are offsets from these (the web app's `view.blacks` / `view.whites` mirror them).
pub const DEFAULT_BLACK_EV: f64 = -8.0;
pub const DEFAULT_WHITE_EV: f64 = 4.5;

impl Default for Tone {
    fn default() -> Self {
        Self {
            black_ev: DEFAULT_BLACK_EV,
            white_ev: DEFAULT_WHITE_EV,
            contrast: 1.0,
            curve: vec![[0.0, 0.0], [1.0, 1.0]],
            highlights: 0.0,
            shadows: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct MixerBand {
    pub hue: f64,
    pub saturation: f64,
    pub luminance: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
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

/// Film rendering: each of red, green and blue goes through the characteristic curve on its own, as a colour
/// film's three emulsion layers each have their own, instead of one curve on luminance with hue held. That is
/// what makes highlights fade toward white and lifts midtone saturation the way film does. Before the curves,
/// each layer's development holds its neighbours back (interlayer effects, from the inhibitors couplers
/// release), modelled as a spread in log exposure away from the pixel's mean: more separation, more colour,
/// neutrals untouched. Only for a RAW — a developed file was already rendered by whatever wrote it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct Film {
    /// 0–100: how far from the neutral rendering toward film's.
    pub amount: f64,
    /// 0–100: interlayer effects, at 100 a spread of `FILM_INTERLAYER` in log exposure.
    pub interlayer: f64,
}

impl Default for Film {
    fn default() -> Self {
        Self { amount: 100.0, interlayer: 45.0 }
    }
}

/// Interlayer 100 → each channel moves this far again from the mean of the three, in log exposure.
pub const FILM_INTERLAYER: f64 = 0.6;

/// Distortion, lateral CA and vignetting from the photo's profile, applied by the lens pass before develop
/// (`crate::lens`, DESIGN.md §15).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
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

/// A mask layer (DESIGN.md §15): a mask — one the models made, refined or not by brush strokes, or one painted
/// from nothing — and the adjustments it carries. Each value is added, weighted by the mask, to what the global
/// edit does there — `ev = global.ev + Σ wₖ · maskₖ.ev` — so a layer reaches every control the global edit has
/// for light and colour.
///
/// Every field past `tint` is left out of the serialized edit at its default: a layer that uses none of them
/// is written as 0.4 wrote it, keeps its ETag, and still renders in 0.4, which carries layers with fields it
/// doesn't know and skips rendering them.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct MaskAdjustment {
    /// Which mask, by name: one of `MASK_NAMES`, which the worker's models make and cache (never stored
    /// here), or `brush`, which starts empty and is only what the strokes paint.
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
    /// −100…100, as the global sliders: they move this layer's white and black points from the global ones.
    #[serde(skip_serializing_if = "is_zero")]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub whites: f64,
    #[serde(skip_serializing_if = "is_zero")]
    #[cfg_attr(feature = "ts", ts(as = "Option<f64>", optional))]
    pub blacks: f64,
    /// A curve over display-encoded values, applied after the global curve where the mask is. Highlights and
    /// Shadows move its 75 % and 25 % points, as the global sliders move the global curve's. Empty is none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<[f64; 2]>>", optional))]
    pub curve: Vec<[f64; 2]>,
    /// Colour mixer values added to the global ones where the mask is.
    #[serde(skip_serializing_if = "is_default_mixer")]
    #[cfg_attr(feature = "ts", ts(as = "Option<Mixer>", optional))]
    pub mixer: Mixer,
    /// Only where the mask's value lies in this range: a band of distance, for `depth`. None is the mask as
    /// it is. Applied before the strokes, so painting still adds to or takes from what the range selects.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub range: Option<MaskRange>,
    /// The layer's edge feathered and moved, after the range and the strokes. None is the mask as painted.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub edge: Option<MaskEdge>,
    /// Brush strokes over the mask, in the order they were painted.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<Stroke>>", optional))]
    pub strokes: Vec<Stroke>,
    /// Fields a newer Sidecar wrote (another kind of mask, a shape), kept as they are so saving here doesn't
    /// lose them. An entry this version can't render is carried along and skipped (`renders`).
    #[serde(flatten)]
    #[cfg_attr(feature = "ts", ts(skip))]
    pub other: serde_json::Map<String, serde_json::Value>,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn is_default_mixer(m: &Mixer) -> bool {
    *m == Mixer::default()
}

impl Default for MaskAdjustment {
    fn default() -> Self {
        Self {
            mask: String::new(),
            invert: false,
            amount: 100.0,
            exposure: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            temperature: 0.0,
            tint: 0.0,
            whites: 0.0,
            blacks: 0.0,
            curve: Vec::new(),
            mixer: Mixer::default(),
            range: None,
            edge: None,
            strokes: Vec::new(),
            other: Default::default(),
        }
    }
}

/// A window over a mask's values, each 0…100: the layer applies fully where the mask is between `from` and
/// `to`, and fades out over `feather` beyond either end. On the depth mask, where near is 100, it selects a
/// band of distance.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct MaskRange {
    pub from: f64,
    pub to: f64,
    pub feather: f64,
}

impl MaskRange {
    /// As the brush composite shader takes it (`CompositeParams.range`): low and high ends and feather as
    /// 0…1, and w = 1 for "applied". `None` is all zeros: the mask as it is.
    pub fn uniform(range: Option<&MaskRange>) -> [f32; 4] {
        match range {
            Some(r) => {
                let (a, b) = (r.from.clamp(0.0, 100.0), r.to.clamp(0.0, 100.0));
                [(a.min(b) / 100.0) as f32, (a.max(b) / 100.0) as f32, (r.feather.clamp(0.0, 100.0) / 100.0) as f32, 1.0]
            }
            None => [0.0; 4],
        }
    }
}

/// A layer's edge (DESIGN.md §15, "Mask edges"): the mask blurred over `width` and thresholded again, which
/// moves the edge by `shift` and softens it by `feather`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct MaskEdge {
    /// 0–100: how far the edge reaches, up to `MAX_EDGE` of the long edge.
    pub width: f64,
    /// 0–100: how soft it is across that width; 0 is a crisp edge, only moved and smoothed.
    pub feather: f64,
    /// −100–100: the edge pushed inward (−) or outward (+), within the width.
    pub shift: f64,
}

impl Default for MaskEdge {
    fn default() -> Self {
        Self { width: 20.0, feather: 100.0, shift: 0.0 }
    }
}

/// The widest an edge reaches, as a fraction of the mask's long edge.
pub const MAX_EDGE: f64 = 0.05;

/// The edge passes for a mask of some size (`MaskEdge::plan`): averaged down by `scale`, blurred over
/// `radius` texels of that, then thresholded at `threshold` with a ramp `ramp` wide.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EdgePlan {
    pub scale: u32,
    pub small: [u32; 2],
    pub radius: f32,
    pub threshold: f32,
    pub ramp: f32,
}

impl MaskEdge {
    /// The passes for a mask `size` pixels big; `None` when the edge reaches less than half a pixel.
    pub fn plan(&self, size: [u32; 2]) -> Option<EdgePlan> {
        let reach = self.width.clamp(0.0, 100.0) / 100.0 * MAX_EDGE * size[0].max(size[1]) as f64;
        if reach < 0.5 {
            return None;
        }
        let scale = (reach / 16.0).ceil().max(1.0) as u32;
        Some(EdgePlan {
            scale,
            small: size.map(|v| v.div_ceil(scale)),
            radius: (reach / scale as f64) as f32,
            threshold: (0.5 - self.shift.clamp(-100.0, 100.0) / 100.0 * 0.45) as f32,
            ramp: (self.feather.clamp(0.0, 100.0) / 100.0).max(0.02) as f32,
        })
    }
}

/// Byte-for-byte mirror of `struct EdgeParams` in shaders/client/mask_edge.wgsl (48 bytes); checked by naga.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EdgeParams {
    pub size: [f32; 2],
    pub mode: u32,
    pub scale: f32,
    pub dir: [f32; 2],
    pub radius: f32,
    pub _pad: f32,
    pub ramp: [f32; 4],
}

impl EdgePlan {
    /// Pass 0: average the mask down into `small`.
    pub fn down(&self) -> EdgeParams {
        EdgeParams { size: self.small.map(|v| v as f32), mode: 0, scale: self.scale as f32, ..Default::default() }
    }
    /// Pass 1: blur across (`down` false) or down.
    pub fn blur(&self, down: bool) -> EdgeParams {
        EdgeParams { size: self.small.map(|v| v as f32), mode: 1, dir: if down { [0.0, 1.0] } else { [1.0, 0.0] }, radius: self.radius, ..Default::default() }
    }
    /// Pass 2: threshold into the mask, `size` pixels.
    pub fn threshold(&self, size: [u32; 2]) -> EdgeParams {
        EdgeParams { size: size.map(|v| v as f32), mode: 2, ramp: [self.threshold, self.ramp, 0.0, 0.0], ..Default::default() }
    }
}

/// One brush stroke on a mask. Coordinates are in the photo as it is shown before lens correction — the
/// frame the models' masks are made in — so a stroke stays on what it was painted over whatever the lens
/// correction, crop or straighten later do.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub struct Stroke {
    /// Take the mask away instead of painting it in.
    pub erase: bool,
    /// Brush radius, as a fraction of the photo's long edge.
    pub radius: f64,
    /// 0 is a hard edge; 1 fades from the centre out.
    pub feather: f64,
    /// How much one stroke lays down, 0…1; painting over it again adds more.
    pub flow: f64,
    /// The path, `[x, y]` as fractions of the photo's width and height. One point is a dab.
    pub points: Vec<[f64; 2]>,
}

impl Default for Stroke {
    fn default() -> Self {
        Self { erase: false, radius: 0.05, feather: 0.5, flow: 1.0, points: Vec::new() }
    }
}

/// The masks the models make, which an adjustment can name. Kept in step with the worker's `masks::NAMES`.
pub const MASK_NAMES: [&str; 7] = ["subject", "skin", "face-skin", "body-skin", "hair", "clothes", "depth"];
/// A mask painted from nothing: only its strokes.
pub const BRUSH: &str = "brush";

impl MaskAdjustment {
    /// Whether this version knows how to render it: a mask it knows by name, and nothing a newer version
    /// added to say it is something else.
    pub fn renders(&self) -> bool {
        (self.uses_model() || self.mask == BRUSH) && self.other.is_empty()
    }

    /// Whether its mask starts from one the models make.
    pub fn uses_model(&self) -> bool {
        MASK_NAMES.contains(&self.mask.as_str())
    }

    fn has_curve(&self) -> bool {
        !self.curve.is_empty() && self.curve != [[0.0, 0.0], [1.0, 1.0]]
    }
}

/// One mask layer as the shader takes it. Mirrors `struct MaskLayer` in shaders/develop/masks.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaskLayer {
    /// rgb: white-balance gains in camera space at full weight; w: exposure in stops.
    pub gain: [f32; 4],
    /// x: contrast, y: saturation (each −1…1), z: amount (0…1), w: 1 to invert.
    pub tone: [f32; 4],
    /// The tone map's parameters at full weight, as `DevelopParams::tone`.
    pub tone_map: [f32; 4],
    /// x: 1 when it has a curve (row `i` of the layer curves), y: 1 when it moves the mixer, z: 1 when it
    /// moves the tone map.
    pub flags: [f32; 4],
    /// Added to `DevelopParams::mixer` at full weight.
    pub mixer: [[f32; 4]; 8],
}

/// Byte-for-byte mirror of `struct MaskParams` in shaders/develop/masks.wgsl (1,568 bytes); checked by naga.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaskParams {
    /// xy: where the texture being developed sits in the frame the masks cover, in its pixels; zw: that
    /// frame's size (0 for the texture's own size — a whole image). A loupe tile sets both.
    pub frame: [f32; 4],
    pub count: u32,
    pub _pad: [u32; 3],
    pub layers: [MaskLayer; MAX_MASKS],
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
pub enum SourceKind {
    /// Metadata cached before this field existed is RAW.
    #[default]
    Raw,
    Developed,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, export_to = "edit/"))]
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
    /// Global chroma gain − 1 (OKLCh) — the mask-saturation baseline every layer's own saturation adds to.
    pub saturation: f32,
    pub _pad1: f32,
    pub mixer: [[f32; 4]; 8],
    /// Film rendering: amount 0…1 (0 is the neutral rendering), interlayer spread in log exposure, unused ×2.
    pub film: [f32; 4],
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
    /// The layers' own curves, `MAX_MASKS` rows of `LUT_SIZE`: row `i` is `edit.masks[i]`'s, the identity
    /// for a layer without one.
    pub mask_curves: Vec<f32>,
}

/// Row-major 3×3 → WGSL `mat3x3f` columns.
fn to_mat3(m: &M3) -> [[f32; 4]; 3] {
    [0, 1, 2].map(|j| [m[0][j] as f32, m[1][j] as f32, m[2][j] as f32, 0.0])
}

/// The tone map's parameters for these tone settings and exposure — the global edit's, or a layer's.
fn tone_params(edit: &EditParams, developed: bool) -> [f32; 4] {
    let t = &edit.tone;
    if developed {
        display_tone(edit)
    } else {
        let k = (t.contrast * CONTRAST_SCALE).max(0.5);
        [t.black_ev as f32, t.white_ev as f32, k as f32, tone::pivot_for(t.black_ev, t.white_ev, k) as f32]
    }
}

fn mixer_params(mixer: &Mixer) -> [[f32; 4]; 8] {
    mixer.bands().map(|b| {
        [
            (b.hue.clamp(-100.0, 100.0) / 100.0 * 30f64.to_radians()) as f32,
            (b.saturation.clamp(-100.0, 100.0) / 100.0) as f32,
            (b.luminance.clamp(-100.0, 100.0) / 100.0 * 0.15) as f32,
            0.0,
        ]
    })
}

/// The identity curve, as a LUT row.
fn identity_lut() -> Vec<f32> {
    (0..LUT_SIZE).map(|i| i as f32 / (LUT_SIZE - 1) as f32).collect()
}

/// A layer at full weight: its white and black points moved from the global ones as the global sliders move
/// them (Whites ±100 is ∓2 EV, Blacks ±100 is ∓3 EV), and exposure added, which a developed file's shoulder
/// follows.
fn layer_edit(edit: &EditParams, m: &MaskAdjustment) -> EditParams {
    let mut e = edit.clone();
    e.tone.white_ev -= m.whites.clamp(-100.0, 100.0) / 100.0 * 2.0;
    e.tone.black_ev -= m.blacks.clamp(-100.0, 100.0) / 100.0 * 3.0;
    e.exposure.ev += m.exposure.clamp(-5.0, 5.0);
    e
}

pub fn pack(edit: &EditParams, camera: &CameraColor, source: SourceKind, display: DisplaySpace) -> Packed {
    let white_xy = match edit.white_balance.mode {
        WhiteBalanceMode::AsShot => camera.as_shot_xy(),
        WhiteBalanceMode::Custom => crate::cct::temp_tint_to_xy(edit.white_balance.temperature_k, edit.white_balance.tint),
    };
    let wb = camera::resolve(camera, white_xy);
    let developed = source == SourceKind::Developed;
    let tone = tone_params(edit, developed);
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
        saturation: (edit.saturation.clamp(-100.0, 100.0) / 100.0) as f32,
        _pad1: 0.0,
        mixer: mixer_params(&edit.mixer),
        film: match (&edit.film, developed) {
            (Some(f), false) => [(f.amount.clamp(0.0, 100.0) / 100.0) as f32, (f.interlayer.clamp(0.0, 100.0) / 100.0 * FILM_INTERLAYER) as f32, 0.0, 0.0],
            _ => [0.0; 4],
        },
    };
    let mut masks = MaskParams { frame: [0.0; 4], count: 0, _pad: [0; 3], layers: [MaskLayer::default(); MAX_MASKS] };
    let mut mask_curves = Vec::with_capacity(MAX_MASKS * LUT_SIZE);
    for i in 0..MAX_MASKS {
        let m = edit.masks.get(i);
        match m.filter(|m| m.renders() && m.has_curve()) {
            Some(m) => mask_curves.extend(monotone_lut(&m.curve, LUT_SIZE)),
            None => mask_curves.extend(identity_lut()),
        }
        let Some(m) = m else { continue };
        masks.count += 1;
        // Kept in its slot, so texture `i` stays `edit.masks[i]`'s, at no weight.
        if !m.renders() {
            continue;
        }
        let g = mask_gains(camera, &wb, m);
        let unit = |v: f64| (v.clamp(-100.0, 100.0) / 100.0) as f32;
        let tone_map = tone_params(&layer_edit(edit, m), developed);
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        masks.layers[i] = MaskLayer {
            gain: [g[0] as f32, g[1] as f32, g[2] as f32, m.exposure.clamp(-5.0, 5.0) as f32],
            tone: [unit(m.contrast), unit(m.saturation), (m.amount.clamp(0.0, 100.0) / 100.0) as f32, flag(m.invert)],
            tone_map,
            flags: [flag(m.has_curve()), flag(m.mixer != Mixer::default()), flag(tone_map != tone), 0.0],
            mixer: mixer_params(&m.mixer),
        };
    }
    let lut = tone_curve_lut(&edit.tone.curve, edit.tone.highlights, edit.tone.shadows, LUT_SIZE);
    Packed { uniform: bytemuck::bytes_of(&params).to_vec(), lut, temperature_k: wb.temperature_k, tint: wb.tint, masks, mask_curves }
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
        assert_eq!(layouter[handle].size as usize, 288);
        let rust = [
            ("wb_mul", std::mem::offset_of!(DevelopParams, wb_mul)),
            ("exposure", std::mem::offset_of!(DevelopParams, exposure)),
            ("cam_to_work", std::mem::offset_of!(DevelopParams, cam_to_work)),
            ("work_to_disp", std::mem::offset_of!(DevelopParams, work_to_disp)),
            ("tone", std::mem::offset_of!(DevelopParams, tone)),
            ("hl_strength", std::mem::offset_of!(DevelopParams, hl_strength)),
            ("display_referred", std::mem::offset_of!(DevelopParams, display_referred)),
            ("saturation", std::mem::offset_of!(DevelopParams, saturation)),
            ("mixer", std::mem::offset_of!(DevelopParams, mixer)),
            ("film", std::mem::offset_of!(DevelopParams, film)),
        ];
        for (name, off) in rust {
            let m = members.iter().find(|m| m.name.as_deref() == Some(name)).unwrap();
            assert_eq!(m.offset as usize, off, "{name}");
        }
        let p = pack(&EditParams::default(), &crate::camera::nikon_d850(), SourceKind::Raw, DisplaySpace::Srgb);
        assert_eq!(p.uniform.len(), 288);
        assert_eq!(p.lut.len(), LUT_SIZE);
        // exposure sits at byte 12
        assert_eq!(f32::from_le_bytes(p.uniform[12..16].try_into().unwrap()), 1.0);
    }

    #[test]
    fn global_saturation_packs_to_the_same_scale_a_mask_layer_uses() {
        let cam = crate::camera::nikon_d850();
        let neutral = pack(&EditParams::default(), &cam, SourceKind::Raw, DisplaySpace::Srgb);
        let params: &DevelopParams = bytemuck::from_bytes(&neutral.uniform);
        assert_eq!(params.saturation, 0.0);

        let edit = EditParams { saturation: 50.0, ..Default::default() };
        let boosted = pack(&edit, &cam, SourceKind::Raw, DisplaySpace::Srgb);
        let params: &DevelopParams = bytemuck::from_bytes(&boosted.uniform);
        assert_eq!(params.saturation, 0.5, "same -100..100 -> -1..1 scale as MaskAdjustment.saturation's `unit`");
    }

    #[test]
    fn film_packs_for_a_raw_and_never_for_a_developed_file() {
        let cam = crate::camera::nikon_d850();
        let at = |edit: &EditParams, source| *bytemuck::from_bytes::<DevelopParams>(&pack(edit, &cam, source, DisplaySpace::Srgb).uniform);
        assert_eq!(at(&EditParams::default(), SourceKind::Raw).film, [0.0; 4], "absent is the neutral rendering");
        let edit = EditParams { film: Some(Film { amount: 50.0, interlayer: 100.0 }), ..Default::default() };
        assert_eq!(at(&edit, SourceKind::Raw).film, [0.5, FILM_INTERLAYER as f32, 0.0, 0.0]);
        assert_eq!(at(&edit, SourceKind::Developed).film, [0.0; 4]);
    }

    #[test]
    fn edge_params_layout_matches_wgsl() {
        let src = sidecar_shaders::compose("client/mask_edge.wgsl").unwrap();
        let module = naga::front::wgsl::parse_str(&src).unwrap();
        let (_, ty) = module.types.iter().find(|(_, t)| t.name.as_deref() == Some("EdgeParams")).unwrap();
        let naga::TypeInner::Struct { members, span } = &ty.inner else { panic!("not a struct") };
        assert_eq!(*span as usize, std::mem::size_of::<EdgeParams>());
        for (name, off) in [("size", std::mem::offset_of!(EdgeParams, size)), ("mode", std::mem::offset_of!(EdgeParams, mode)), ("scale", std::mem::offset_of!(EdgeParams, scale)), ("dir", std::mem::offset_of!(EdgeParams, dir)), ("radius", std::mem::offset_of!(EdgeParams, radius)), ("ramp", std::mem::offset_of!(EdgeParams, ramp))] {
            assert_eq!(members.iter().find(|m| m.name.as_deref() == Some(name)).unwrap().offset as usize, off, "{name}");
        }
    }

    /// An edge's reach follows the mask's size; it is blurred at a scale that keeps the radius within 16
    /// texels; shifting out lowers the threshold; an edge too narrow to see is none.
    #[test]
    fn edge_plans_follow_the_mask_size() {
        let edge = MaskEdge { width: 100.0, feather: 50.0, shift: 100.0 };
        let p = edge.plan([4000, 3000]).unwrap();
        assert_eq!((p.scale, p.small, p.radius), (13, [308, 231], 200.0 / 13.0));
        assert!((p.threshold - 0.05).abs() < 1e-6 && p.ramp == 0.5);
        let proxy = edge.plan([1000, 750]).unwrap();
        assert_eq!((proxy.scale, proxy.radius), (4, 12.5), "the same edge on a proxy, in its pixels");
        assert_eq!(MaskEdge { width: 0.0, ..edge }.plan([4000, 3000]), None);
        assert_eq!(MaskEdge { shift: -100.0, ..edge }.plan([100, 100]).unwrap().threshold, 0.95);
    }

    /// A layer's range stays out of the edit until set, packs low end first, and a depth layer renders.
    #[test]
    fn a_mask_range_is_left_out_until_set_and_packs_in_order() {
        let plain = MaskAdjustment { mask: "depth".into(), ..Default::default() };
        assert!(serde_json::to_value(&plain).unwrap().get("range").is_none());
        let r = MaskRange { from: 80.0, to: 20.0, feather: 10.0 };
        assert_eq!(MaskRange::uniform(Some(&r)), [0.2, 0.8, 0.1, 1.0]);
        assert_eq!(MaskRange::uniform(None), [0.0; 4]);
        let ranged = MaskAdjustment { range: Some(r), ..plain };
        assert!(ranged.renders());
        assert_eq!(serde_json::to_value(&ranged).unwrap()["range"], serde_json::json!({ "from": 80.0, "to": 20.0, "feather": 10.0 }));
    }

    #[test]
    fn film_is_left_out_until_chosen() {
        assert!(serde_json::to_value(EditParams::default()).unwrap().get("film").is_none());
        let e = EditParams { film: Some(Film::default()), ..Default::default() };
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["film"], serde_json::json!({ "amount": 100.0, "interlayer": 45.0 }));
        assert_eq!(serde_json::from_value::<EditParams>(json).unwrap(), e);
    }

    #[test]
    fn saturation_is_left_out_until_set() {
        let json = serde_json::to_value(EditParams::default()).unwrap();
        assert!(json.get("saturation").is_none());
        let e = EditParams { saturation: 25.0, ..Default::default() };
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["saturation"], 25.0);
        let back: EditParams = serde_json::from_value(json).unwrap();
        assert_eq!(back, e);
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
        assert_eq!(m.layers[0].gain, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(m.layers[0].tone, [0.5, 0.0, 0.5, 1.0]);
        assert_eq!(m.layers[0].flags, [0.0; 4], "exposure alone leaves a RAW's tone map, curve and mixer");
        let [r, g, b, _] = m.layers[1].gain;
        assert!(r > 1.0 && b < 1.0 && (g - 1.0).abs() < 1e-6, "warmer: more red, less blue, green fixed: {:?}", m.layers[1].gain);
    }

    #[test]
    fn mask_layout_matches_wgsl() {
        let src = sidecar_shaders::compose("develop/masks.wgsl").unwrap();
        let module = naga::front::wgsl::parse_str(&src).unwrap();
        let (_, ty) = module.types.iter().find(|(_, t)| t.name.as_deref() == Some("MaskParams")).unwrap();
        let naga::TypeInner::Struct { members, span } = &ty.inner else { panic!("not a struct") };
        assert_eq!(*span as usize, std::mem::size_of::<MaskParams>());
        assert_eq!(std::mem::size_of::<MaskParams>(), 1568);
        let off = |n: &str| members.iter().find(|m| m.name.as_deref() == Some(n)).unwrap().offset as usize;
        assert_eq!(off("frame"), std::mem::offset_of!(MaskParams, frame));
        assert_eq!(off("count"), std::mem::offset_of!(MaskParams, count));
        assert_eq!(off("layers"), std::mem::offset_of!(MaskParams, layers));
        let (_, ty) = module.types.iter().find(|(_, t)| t.name.as_deref() == Some("MaskLayer")).unwrap();
        let naga::TypeInner::Struct { members, span } = &ty.inner else { panic!("not a struct") };
        assert_eq!(*span as usize, std::mem::size_of::<MaskLayer>());
        let off = |n: &str| members.iter().find(|m| m.name.as_deref() == Some(n)).unwrap().offset as usize;
        assert_eq!(off("gain"), std::mem::offset_of!(MaskLayer, gain));
        assert_eq!(off("tone"), std::mem::offset_of!(MaskLayer, tone));
        assert_eq!(off("tone_map"), std::mem::offset_of!(MaskLayer, tone_map));
        assert_eq!(off("flags"), std::mem::offset_of!(MaskLayer, flags));
        assert_eq!(off("mixer"), std::mem::offset_of!(MaskLayer, mixer));
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

    /// A layer that uses only what 0.4 knew is written exactly as 0.4 wrote it: a newer field at its default
    /// adds nothing, so its ETag and 0.4's ability to render it are unchanged.
    #[test]
    fn a_layer_with_only_old_fields_serializes_as_before() {
        let m = MaskAdjustment { mask: "subject".into(), exposure: 0.5, ..Default::default() };
        let json = serde_json::to_value(&m).unwrap();
        let keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["amount", "contrast", "exposure", "invert", "mask", "saturation", "temperature", "tint"]);
        let back: MaskAdjustment = serde_json::from_value(json).unwrap();
        assert!(back.renders() && back.other.is_empty());
    }

    #[test]
    fn a_layer_reaches_the_tone_map_curve_and_mixer() {
        let cam = crate::camera::nikon_d850();
        let mut e = EditParams::default();
        let mut mixer = Mixer::default();
        mixer.blue.saturation = 50.0;
        e.masks.push(MaskAdjustment {
            mask: BRUSH.into(),
            whites: 50.0,
            blacks: -30.0,
            curve: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            mixer,
            strokes: vec![Stroke { points: vec![[0.5, 0.5]], ..Default::default() }],
            ..Default::default()
        });
        assert!(e.masks[0].renders());
        let p = pack(&e, &cam, SourceKind::Raw, DisplaySpace::Srgb);
        let l = p.masks.layers[0];
        assert_eq!(l.flags, [1.0, 1.0, 1.0, 0.0]);
        assert_eq!((l.tone_map[0], l.tone_map[1]), ((DEFAULT_BLACK_EV + 0.9) as f32, (DEFAULT_WHITE_EV - 1.0) as f32));
        assert_eq!(l.mixer[5][1], 0.5);
        assert_eq!(p.mask_curves.len(), MAX_MASKS * LUT_SIZE);
        assert!(p.mask_curves[LUT_SIZE / 2] > 0.55, "row 0 is the layer's curve");
        assert!((p.mask_curves[LUT_SIZE + LUT_SIZE / 2] - 0.5).abs() < 1e-3, "row 1 is the identity");
    }

    #[test]
    fn geometry_is_left_out_until_set() {
        let json = serde_json::to_value(EditParams::default()).unwrap();
        assert!(json.get("geometry").is_none());
        let e = EditParams { geometry: Some(crate::geometry::Geometry { angle: 2.5, ..Default::default() }), ..Default::default() };
        let back: EditParams = serde_json::from_value(serde_json::to_value(&e).unwrap()).unwrap();
        assert_eq!(back, e);
    }

    /// A `PROCESS_VERSION` 1 edit with a manually-drawn point plus Highlights at +20 and Shadows untouched:
    /// migration must carry the +20 forward into the new field, leave the manual point alone, and remove only
    /// the anchor point — not reinterpret the hand-drawn one as an anchor just because it's nearby-ish.
    #[test]
    fn migration_carries_old_anchor_points_into_the_new_fields() {
        let mut e = EditParams::default();
        e.tone.curve = vec![[0.0, 0.0], [0.3, 0.22], [0.75, 0.75 + 20.0 / 100.0 * HIGHLIGHT_SHADOW_SCALE], [1.0, 1.0]];
        e.migrate_highlight_shadow_anchors();
        assert!((e.tone.highlights - 20.0).abs() < 1e-9);
        assert_eq!(e.tone.shadows, 0.0);
        assert_eq!(e.tone.curve, vec![[0.0, 0.0], [0.3, 0.22], [1.0, 1.0]]);
    }

    #[test]
    fn migration_is_a_no_op_without_anchor_points() {
        let mut e = EditParams::default();
        e.tone.curve = vec![[0.0, 0.0], [0.4, 0.5], [1.0, 1.0]];
        let before = e.clone();
        e.migrate_highlight_shadow_anchors();
        assert_eq!(e, before);
    }

    /// The whole point: migrating must reproduce the exact LUT value the old mechanism baked at the anchor,
    /// not just a numerically-equal slider reading.
    #[test]
    fn migrated_edit_packs_the_same_value_at_the_anchor_as_the_old_mechanism_did() {
        let mut e = EditParams::default();
        let old_y = 0.25 + (-35.0_f64) / 100.0 * HIGHLIGHT_SHADOW_SCALE;
        e.tone.curve = vec![[0.0, 0.0], [0.25, old_y], [1.0, 1.0]];
        let before = pack(&e, &crate::camera::nikon_d850(), SourceKind::Raw, DisplaySpace::Srgb).lut;

        e.migrate_highlight_shadow_anchors();
        let after = pack(&e, &crate::camera::nikon_d850(), SourceKind::Raw, DisplaySpace::Srgb).lut;

        let i = (0.25 * (LUT_SIZE - 1) as f64).round() as usize;
        assert!((before[i] - after[i]).abs() < 1e-3, "before={}, after={}", before[i], after[i]);
    }
}
