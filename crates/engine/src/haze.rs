// SPDX-License-Identifier: Apache-2.0
//! Dehaze without depth (DESIGN.md §7, "Dehaze"): the dark channel prior (He, Sun and Tang, CVPR 2009 / TPAMI
//! 2011) over the whole frame at the adjacency map's size, its transmission refined by the guided filter. The
//! passes are `client/haze_*.wgsl`, which the browser composes the same way (`web/src/lib/gpu/haze.ts`); develop
//! applies the result (`develop/haze.wgsl`).
//!
//! 1. The dark channel: each texel's smallest white-balanced working-space channel, its minimum over a patch.
//! 2. The airlight: the colour where that is greatest — one workgroup's reduction, so nothing is read back.
//! 3. The normalised dark channel (each channel over the airlight's) and its patch minimum: t = 1 − ω·that.
//! 4. The guided filter's coefficients for t against the picture's log luminance, which develop applies at
//!    each pixel against its own: the transmission's edges at full resolution.

use anyhow::Result;
use sidecar_uniform::Uniform;
use wgpu::TextureFormat as F;
use wgpu::util::DeviceExt;

use crate::Gpu;
use crate::compute::{B, Kernel, Res, texture, view};
use crate::guided::{Guided, GuidedFilter};

/// The dehaze passes' uniform (80 bytes). Its WGSL declaration, `shaders/uniforms/haze_params.wgsl`, is generated
/// from this struct (`sidecar-uniform`); the `uniforms` test checks the file and every offset.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable, Uniform)]
pub struct HazeParams {
    /// Camera RGB → linear Rec.2020, as develop's: the dark channel compares working-space channels.
    #[uniform(wgsl = "mat3x3f")]
    pub cam_to_work: [[f32; 4]; 3],
    /// The white balance's camera multipliers, w unused.
    pub wb: [f32; 4],
    /// The patch minimum's radius in texels.
    pub radius: u32,
    /// The minimum pass's axis: 0 across, 1 down.
    pub axis: u32,
    /// How much of the haze the coarse transmission accounts for (He et al.'s ω).
    pub omega: f32,
    pub _pad: u32,
}

/// The dark channel's patch, as a share of the long edge (He et al.'s 15 px on pictures about 600 across).
pub const PATCH: f64 = 0.012;
/// The guided refinement's radius as a share of the long edge, and its ε in log₂ units squared.
pub const REFINE_RADIUS: f64 = 0.04;
pub const REFINE_EPS: f32 = 0.01;
pub const OMEGA: f32 = 0.95;

pub struct Haze {
    dark: Kernel,
    min: Kernel,
    air: Kernel,
    trans: Kernel,
    unit: wgpu::Texture,
}

impl Haze {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        let wide = B::Store(F::Rgba32Float);
        Ok(Self {
            dark: Kernel::new(
                gpu,
                "client/haze_dark.wgsl",
                &[B::Uniform, B::Tex, B::Tex, wide],
            )?,
            min: Kernel::new(gpu, "client/haze_min.wgsl", &[B::Uniform, B::Tex, wide])?,
            air: Kernel::new(gpu, "client/haze_air.wgsl", &[B::Uniform, B::Tex, wide])?,
            trans: Kernel::new(
                gpu,
                "client/haze_trans.wgsl",
                &[B::Uniform, B::Tex, B::Store(F::Rgba16Float)],
            )?,
            unit: one_texel(gpu, [1.0; 4]),
        })
    }

    /// The transmission's guided coefficients (RGBA32F, mean a and b in x and y, at `small`'s size) and the
    /// airlight (1×1 RGBA32F, working space before exposure), for a frame `frame` pixels across averaged down
    /// to `small` and developed with `cam_to_work` and `wb`.
    pub fn map(
        &self,
        gpu: &Gpu,
        guided: &GuidedFilter,
        small: &wgpu::Texture,
        frame: [u32; 2],
        cam_to_work: [[f32; 4]; 3],
        wb: [f32; 3],
    ) -> (wgpu::Texture, wgpu::Texture) {
        let size = [small.width(), small.height()];
        let texels =
            frame[0].max(frame[1]) as f64 / sidecar_color::developer::map_scale(frame) as f64;
        let p = HazeParams {
            cam_to_work,
            wb: [wb[0], wb[1], wb[2], 0.0],
            radius: ((PATCH * texels).round() as u32).max(1),
            axis: 0,
            omega: OMEGA,
            _pad: 0,
        };
        let wide = || texture(gpu, size[0], size[1], F::Rgba32Float);
        let (a, b) = (wide(), wide());
        let air = texture(gpu, 1, 1, F::Rgba32Float);
        let coarse = texture(gpu, size[0], size[1], F::Rgba16Float);
        let (sv, av, bv, airv, unitv, cv) = (
            view(small),
            view(&a),
            view(&b),
            view(&air),
            view(&self.unit),
            view(&coarse),
        );
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let minimum = |enc: &mut wgpu::CommandEncoder| {
            self.min
                .run(gpu, enc, &p, size, &[Res::View(&av), Res::View(&bv)]);
            let down = HazeParams { axis: 1, ..p };
            self.min
                .run(gpu, enc, &down, size, &[Res::View(&bv), Res::View(&av)]);
        };
        // The dark channel against an airlight of 1, its patch minimum, the airlight from it.
        self.dark.run(
            gpu,
            &mut enc,
            &p,
            size,
            &[Res::View(&sv), Res::View(&unitv), Res::View(&av)],
        );
        minimum(&mut enc);
        self.air.run(
            gpu,
            &mut enc,
            &p,
            [16, 16],
            &[Res::View(&av), Res::View(&airv)],
        );
        // The normalised dark channel, its patch minimum, the coarse transmission.
        self.dark.run(
            gpu,
            &mut enc,
            &p,
            size,
            &[Res::View(&sv), Res::View(&airv), Res::View(&av)],
        );
        minimum(&mut enc);
        self.trans
            .run(gpu, &mut enc, &p, size, &[Res::View(&av), Res::View(&cv)]);
        gpu.queue.submit([enc.finish()]);
        let (coef, _) = guided.coefficients(
            gpu,
            small,
            &coarse,
            size,
            Guided {
                radius: (REFINE_RADIUS * texels).max(1.0) as f32,
                eps: REFINE_EPS,
                subsample: 1,
                channel: 0,
                log_guide: true,
            },
        );
        (coef, air)
    }
}

/// A 1×1 RGBA32F texture holding `v`.
pub fn one_texel(gpu: &Gpu, v: [f32; 4]) -> wgpu::Texture {
    gpu.device.create_texture_with_data(
        &gpu.queue,
        &wgpu::TextureDescriptor {
            label: Some("one texel"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: F::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(&v),
    )
}
