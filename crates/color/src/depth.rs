// SPDX-License-Identifier: Apache-2.0
//! Lens blur and haze by distance (DESIGN.md §15, "Depth effects"): the uniform the depth passes take
//! (`shaders/depth/effects.wgsl`), from the edit, the photo's airlight, and where the texture being processed
//! sits in the frame the depth map covers. One packing for the browser (through wasm) and the worker.

use crate::params::EditParams;

/// The largest lens blur radius, as a fraction of the picture's long edge (`LensBlur::amount` 100).
pub const MAX_BLUR: f64 = 0.025;
/// Haze at ±100: the optical depth at the farthest point. At 3, the farthest point keeps 5 % of its own light.
pub const MAX_HAZE: f64 = 3.0;
/// The blur gathers at a resolution where the largest disc's radius is at most this many texels, so a wide
/// blur costs the same as a narrow one, at any picture size.
pub const GATHER_RADIUS: f64 = 16.0;

/// Byte-for-byte mirror of `struct DepthParams` in shaders/depth/effects.wgsl (64 bytes); checked by naga.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DepthParams {
    /// xy: this texture's offset in the depth map's frame, in its pixels; zw: that frame's size.
    pub frame: [f32; 4],
    /// rgb: the airlight in camera RGB; w: the optical depth at the farthest point, + adds haze, − removes it.
    pub haze: [f32; 4],
    /// x: the largest blur radius in this texture's pixels; y: focus as nearness 0…1; z: half the in-focus
    /// band; w: 1 when blurring.
    pub blur: [f32; 4],
    /// x: how many of this texture's pixels a low-resolution texel covers, each way.
    pub scale: [f32; 4],
}

/// The model's output, one edge: what the depth map's own resolution is, whatever size it is stretched to
/// (`sidecar_worker::masks::DEPTH_EDGE`).
pub const DEPTH_MODEL_EDGE: f64 = 518.0;

/// The longest edge a refined depth map is rendered at. The map's detail comes from the guide, so a larger one
/// would keep finding edges — but it is kept for as long as the photo is open and sampled bilinearly like the
/// map it replaces, and 2,048 is already four times the model's own resolution, which is where the halo around
/// hair and thin branches comes from. Beyond it the photo is read by point samples anyway, which would start
/// to alias on fine texture.
pub const REFINE_MAX_EDGE: u32 = 2048;

/// The size to refine a depth map to, for a photo of `frame` pixels: the frame, with its long edge capped at
/// `REFINE_MAX_EDGE` and its proportions kept.
pub fn refined_size(frame: [u32; 2]) -> [u32; 2] {
    let long = frame[0].max(frame[1]);
    if long <= REFINE_MAX_EDGE || long == 0 {
        return frame;
    }
    let scale = REFINE_MAX_EDGE as f64 / long as f64;
    [((frame[0] as f64 * scale).round() as u32).max(1), ((frame[1] as f64 * scale).round() as u32).max(1)]
}

/// How different two luminances must be, in the photo's linear light, before the refinement stops letting one
/// pixel's depth speak for the other. Small enough that an edge is an edge; large enough that film grain,
/// sensor noise and a texture's own variation do not chop a smooth surface into pieces.
pub const REFINE_SIGMA: f64 = 0.08;

/// Byte-for-byte mirror of `struct DepthRefineParams` in shaders/client/depth_refine.wgsl (16 bytes); checked
/// by naga.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DepthRefineParams {
    /// The distance between taps, in the texture being refined: one texel of the model's own map.
    pub spacing: f32,
    /// `REFINE_SIGMA`.
    pub sigma_range: f32,
    pub _pad: [f32; 2],
}

impl DepthRefineParams {
    /// For a texture whose frame is `frame` pixels: the depth map has `DEPTH_MODEL_EDGE` texels along the
    /// frame's long edge however far it has been stretched, so that is how far apart the taps go. Below one
    /// pixel there is nothing left to repair and the pass copies.
    pub fn for_frame(frame: [u32; 2]) -> Self {
        let long = frame[0].max(frame[1]) as f64;
        Self { spacing: (long / DEPTH_MODEL_EDGE) as f32, sigma_range: REFINE_SIGMA as f32, _pad: [0.0; 2] }
    }

    /// Whether refining this frame would do anything: a frame at or below the model's own resolution is
    /// already as sharp as the map gets.
    pub fn worth_it(&self) -> bool {
        self.spacing >= 1.0
    }
}

impl DepthParams {
    pub fn blurs(&self) -> bool {
        self.blur[3] > 0.5
    }

    /// The low-resolution textures' scale down, ≥ 1.
    pub fn scale(&self) -> u32 {
        self.scale[0].max(1.0) as u32
    }

    /// How far the blur can reach, in this texture's pixels: the margin a tile needs around it to come out as
    /// it would in the whole picture. 0 without blur.
    pub fn reach(&self) -> u32 {
        if self.blurs() { self.blur[0].ceil() as u32 + 2 * self.scale() } else { 0 }
    }

    /// The same effects for a texture at `offset` in the frame (a tile with its margin).
    pub fn at(&self, offset: [f32; 2]) -> Self {
        Self { frame: [offset[0], offset[1], self.frame[2], self.frame[3]], ..*self }
    }
}

/// The depth passes' uniform for a texture at `offset` (pixels) in a frame `frame` pixels big, the frame the
/// depth map covers and the blur is sized by. `airlight` is the photo's, in camera RGB (the worker estimates it
/// with the depth mask). `None` when the edit has neither effect: the passes don't run.
pub fn pack(edit: &EditParams, airlight: [f32; 3], frame: [u32; 2], offset: [f32; 2]) -> Option<DepthParams> {
    let blur = edit.lens_blur.filter(|b| b.amount > 0.0);
    let haze = edit.haze.clamp(-100.0, 100.0);
    if blur.is_none() && haze == 0.0 {
        return None;
    }
    let long = frame[0].max(frame[1]) as f64;
    let (blur, k) = match blur {
        Some(b) => {
            let radius = b.amount.clamp(0.0, 100.0) / 100.0 * MAX_BLUR * long;
            let k = (radius / GATHER_RADIUS).ceil().max(1.0);
            ([radius as f32, (b.focus.clamp(0.0, 100.0) / 100.0) as f32, (b.range.clamp(0.0, 100.0) / 200.0) as f32, 1.0], k)
        }
        None => ([0.0; 4], 1.0),
    };
    Some(DepthParams {
        frame: [offset[0], offset[1], frame[0] as f32, frame[1] as f32],
        haze: [airlight[0], airlight[1], airlight[2], (haze / 100.0 * MAX_HAZE) as f32],
        blur,
        scale: [k as f32, 0.0, 0.0, 0.0],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::LensBlur;

    #[test]
    fn layout_matches_wgsl() {
        let src = sidecar_shaders::compose("depth/effects.wgsl").unwrap();
        let module = naga::front::wgsl::parse_str(&src).unwrap();
        let (_, ty) = module.types.iter().find(|(_, t)| t.name.as_deref() == Some("DepthParams")).unwrap();
        let naga::TypeInner::Struct { members, span } = &ty.inner else { panic!("not a struct") };
        assert_eq!(*span as usize, std::mem::size_of::<DepthParams>());
        for (name, off) in [
            ("frame", std::mem::offset_of!(DepthParams, frame)),
            ("haze", std::mem::offset_of!(DepthParams, haze)),
            ("blur", std::mem::offset_of!(DepthParams, blur)),
            ("scale", std::mem::offset_of!(DepthParams, scale)),
        ] {
            assert_eq!(members.iter().find(|m| m.name.as_deref() == Some(name)).unwrap().offset as usize, off, "{name}");
        }
    }

    #[test]
    fn refine_layout_matches_wgsl() {
        let src = sidecar_shaders::compose("client/depth_refine.wgsl").unwrap();
        let module = naga::front::wgsl::parse_str(&src).unwrap();
        let (_, ty) = module.types.iter().find(|(_, t)| t.name.as_deref() == Some("DepthRefineParams")).unwrap();
        let naga::TypeInner::Struct { members, span } = &ty.inner else { panic!("not a struct") };
        assert_eq!(*span as usize, std::mem::size_of::<DepthRefineParams>());
        for (name, off) in [
            ("spacing", std::mem::offset_of!(DepthRefineParams, spacing)),
            ("sigma_range", std::mem::offset_of!(DepthRefineParams, sigma_range)),
        ] {
            assert_eq!(members.iter().find(|m| m.name.as_deref() == Some(name)).unwrap().offset as usize, off, "{name}");
        }
    }

    /// The taps are a texel of the model's own map apart, whatever the frame is stretched to, and a frame no
    /// bigger than the model's output has nothing to repair.
    #[test]
    fn refinement_is_spaced_by_the_models_own_texel() {
        let p = DepthRefineParams::for_frame([6000, 4000]);
        assert!((p.spacing - 6000.0 / 518.0).abs() < 1e-4, "{}", p.spacing);
        assert!(p.worth_it());
        assert!((DepthRefineParams::for_frame([400, 518]).spacing - 1.0).abs() < 1e-6, "the long edge decides");
        assert!(!DepthRefineParams::for_frame([400, 300]).worth_it(), "already at the map's own resolution");
    }

    /// A refined map is the frame, capped on its long edge and keeping its proportions.
    #[test]
    fn refined_size_caps_the_long_edge() {
        assert_eq!(refined_size([1600, 1000]), [1600, 1000], "under the cap, as it is");
        assert_eq!(refined_size([6000, 4000]), [2048, 1365]);
        assert_eq!(refined_size([4000, 6000]), [1365, 2048], "portrait too");
        assert!(DepthRefineParams::for_frame(refined_size([6000, 4000])).worth_it(), "and still worth refining");
    }

    /// Nothing to do is no pass; the blur is sized by the frame's long edge and gathered at a scale that keeps
    /// its disc within `GATHER_RADIUS`; haze is signed.
    #[test]
    fn packs_from_the_edit() {
        let air = [0.5, 0.6, 0.7];
        assert_eq!(pack(&EditParams::default(), air, [6000, 4000], [0.0; 2]), None);
        let off = EditParams { lens_blur: Some(LensBlur { amount: 0.0, ..Default::default() }), ..Default::default() };
        assert_eq!(pack(&off, air, [6000, 4000], [0.0; 2]), None, "a blur of 0 is none");
        let edit = EditParams { lens_blur: Some(LensBlur { amount: 100.0, focus: 80.0, range: 20.0 }), haze: -50.0, ..Default::default() };
        let p = pack(&edit, air, [6000, 4000], [10.0, 20.0]).unwrap();
        assert_eq!(p.blur, [150.0, 0.8, 0.1, 1.0]);
        assert_eq!(p.scale(), 10, "150 px gathered at a tenth: 15 texels");
        assert_eq!(p.haze, [0.5, 0.6, 0.7, -1.5]);
        assert_eq!(p.frame, [10.0, 20.0, 6000.0, 4000.0]);
        assert_eq!(p.reach(), 170);
        let proxy = pack(&edit, air, [1500, 1000], [0.0; 2]).unwrap();
        assert_eq!((proxy.blur[0], proxy.scale()), (37.5, 3), "the same blur on a proxy, in its pixels");
        let haze_only = pack(&EditParams { haze: 30.0, ..Default::default() }, air, [100, 100], [0.0; 2]).unwrap();
        assert!(!haze_only.blurs() && haze_only.reach() == 0);
    }
}
