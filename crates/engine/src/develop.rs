// SPDX-License-Identifier: Apache-2.0
//! The develop pass: source (camera-native linear RGBA16F) + uniforms + tone LUT → display-linear RGBA16F.
//! Bind group layout mirrors web/src/lib/gpu/develop.ts.

use anyhow::{Result, bail};
use wgpu::util::DeviceExt;

use crate::compute::{B, Kernel, Res, texture, view};
use crate::{Gpu, RgbaF32};
use sidecar_color::params::{DevelopParams, MAX_MASKS};
use sidecar_color::spline::LUT_SIZE;

/// Layers of develop's whole-frame maps: local contrast, Dehaze's transmission, the airlight.
pub const MAP_LAYERS: u32 = 3;

pub struct DevelopPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    mask_layout: wgpu::BindGroupLayout,
    mask_sampler: wgpu::Sampler,
    /// What an unused mask slot binds: 1×1, zero. Also the adjacency map's binding when there is none.
    no_mask: wgpu::Texture,
    /// The adjacency map's passes (`client/density_down.wgsl`, `client/density_blur.wgsl`).
    density_down: Kernel,
    density_blur: Kernel,
    /// Every layer's curve the identity, for a develop whose layers have none.
    identity_curves: wgpu::Texture,
    /// Texture and Clarity's map: the guided filter's coefficients, packed (`client/local_pack.wgsl`), and
    /// what binds when there is none (1×1, RGBA32F).
    guided: crate::guided::GuidedFilter,
    local_pack: Kernel,
    /// What the maps' binding holds when none is needed: 1×1, `MAP_LAYERS` layers of zero.
    no_maps: wgpu::Texture,
    /// Dehaze's passes (`crate::haze`).
    haze: crate::haze::Haze,
    /// The camera profile pass (`client/profile.wgsl`).
    profile: Kernel,
    /// The defringe pass (`client/defringe.wgsl`).
    defringe: Kernel,
}

/// What one develop takes beside its uniform and tone curve: the masks' uniforms, a texture per layer in the
/// frame `params.frame` describes — `None` for a slot whose mask isn't there (it then weighs nothing) — the
/// layers' curves (`Packed::mask_curves`; empty for none), black and white's table (`Packed::mono_lut`; empty
/// in colour), and its developer's adjacency map over the whole frame (`DevelopPass::density_map`). A develop
/// of the whole frame makes its own map when it needs one; a tile has to be given it.
pub struct Masks<'a> {
    pub params: sidecar_color::params::MaskParams,
    pub textures: Vec<Option<&'a wgpu::Texture>>,
    pub curves: &'a [f32],
    pub mono: &'a [f32],
    pub density: Option<&'a wgpu::Texture>,
    /// Texture and Clarity's map over the whole frame (`DevelopPass::local_map`); like the adjacency map, a
    /// develop of the whole frame makes its own and a tile has to be given it.
    pub local: Option<&'a wgpu::Texture>,
    /// Dehaze's map and airlight over the whole frame (`DevelopPass::haze_map`); the same rule.
    pub haze: Option<(&'a wgpu::Texture, &'a wgpu::Texture)>,
    /// The camera profile pass (`Packed::profile`), run over the source first: per pixel, so a tile needs nothing
    /// of the whole frame for it.
    pub profile: Option<&'a sidecar_color::params::ProfilePass>,
    /// The defringe pass's uniform (`Packed::defringe`), run over the source first.
    pub defringe: Option<&'a [u8]>,
    /// A spectral colour stock's two tables (`Packed::slide_exposure` and the stock profile's response table);
    /// both empty unless one is being rendered.
    pub slide_exposure: &'a [f32],
    pub slide_response: &'a [f32],
}

impl Masks<'_> {
    pub fn none() -> Self {
        Self {
            params: bytemuck::Zeroable::zeroed(),
            textures: Vec::new(),
            curves: &[],
            mono: &[],
            density: None,
            local: None,
            haze: None,
            profile: None,
            defringe: None,
            slide_exposure: &[],
            slide_response: &[],
        }
    }
}

/// Whether a develop with this uniform reads the haze map: Dehaze is not 0.
pub fn needs_haze_map(uniform: &[u8]) -> bool {
    let p: &DevelopParams = bytemuck::from_bytes(uniform);
    p.local[3] != 0.0
}

/// Whether a develop with this uniform reads the local-contrast map: Texture or Clarity is on.
pub fn needs_local_map(uniform: &[u8]) -> bool {
    let p: &DevelopParams = bytemuck::from_bytes(uniform);
    p.local[2] > 0.5
}

/// Whether a develop with this uniform reads the adjacency map: black and white, a developer with adjacency, and
/// not the map's own pass.
pub fn needs_density_map(uniform: &[u8]) -> bool {
    let p: &DevelopParams = bytemuck::from_bytes(uniform);
    p.mono[0] > 0.5 && p.mono[2] < 0.5 && p.developer[0] != 0.0
}

/// Black and white's table as a 3D texture, `side`³; a single zero in colour.
fn mono_texture(gpu: &Gpu, data: &[f32]) -> wgpu::Texture {
    let side = (data.len() as f64).cbrt().round() as u32;
    let (side, data) = if side > 0 && (side as usize).pow(3) == data.len() {
        (side, data)
    } else {
        (1, &[0.0f32][..])
    };
    cube_texture(gpu, "mono table", wgpu::TextureFormat::R32Float, side, data)
}

/// One of a spectral stock's tables as a 3D texture of four-float entries, `side`³; a single zero entry when
/// there is no stock, so the binding is always there.
fn slide_texture(gpu: &Gpu, label: &str, data: &[f32]) -> wgpu::Texture {
    let side = (data.len() as f64 / 4.0).cbrt().round() as u32;
    let (side, data) = if side > 0 && (side as usize).pow(3) * 4 == data.len() {
        (side, data)
    } else {
        (1, &[0.0f32; 4][..])
    };
    cube_texture(gpu, label, wgpu::TextureFormat::Rgba32Float, side, data)
}

fn cube_texture(
    gpu: &Gpu,
    label: &str,
    format: wgpu::TextureFormat,
    side: u32,
    data: &[f32],
) -> wgpu::Texture {
    gpu.device.create_texture_with_data(
        &gpu.queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: side,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(data),
    )
}

fn curves_texture(gpu: &Gpu, data: &[f32]) -> wgpu::Texture {
    gpu.device.create_texture_with_data(
        &gpu.queue,
        &wgpu::TextureDescriptor {
            label: Some("layer curves"),
            size: wgpu::Extent3d {
                width: LUT_SIZE as u32,
                height: MAX_MASKS as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(data),
    )
}

impl DevelopPass {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        let source = sidecar_shaders::compose("client/develop.wgsl")?;
        let module = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("develop"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        let layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("develop"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D1,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D3,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // A spectral colour stock's exposure and response tables.
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D3,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 6,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D3,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // The whole-frame maps (`MAP_LAYERS`): Texture and Clarity's, Dehaze's, the airlight.
                    wgpu::BindGroupLayoutEntry {
                        binding: 7,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2Array,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });
        // Mirrors web/src/lib/gpu/develop.ts: the mask uniforms, a linear sampler, and a texture per slot.
        let texture = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let mut mask_entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ];
        mask_entries.extend((0..MAX_MASKS as u32).map(|i| texture(2 + i)));
        mask_entries.push(wgpu::BindGroupLayoutEntry {
            binding: 2 + MAX_MASKS as u32,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
        let mask_layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("develop masks"),
                entries: &mask_entries,
            });
        let pipeline_layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("develop"),
                bind_group_layouts: &[Some(&layout), Some(&mask_layout)],
                ..Default::default()
            });
        let pipeline = gpu
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("develop"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba16Float,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
        let mask_sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("masks"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let no_mask = crate::upload_rgba16(
            gpu,
            &RgbaF32 {
                width: 1,
                height: 1,
                data: vec![0.0; 4],
            },
        );
        let identity: Vec<f32> = (0..MAX_MASKS)
            .flat_map(|_| (0..LUT_SIZE).map(|i| i as f32 / (LUT_SIZE - 1) as f32))
            .collect();
        let identity_curves = curves_texture(gpu, &identity);
        let store = B::Store(wgpu::TextureFormat::Rgba16Float);
        let density_down = Kernel::new(
            gpu,
            "client/density_down.wgsl",
            &[B::Uniform, B::Tex, store],
        )?;
        let density_blur = Kernel::new(
            gpu,
            "client/density_blur.wgsl",
            &[B::Uniform, B::Tex, store],
        )?;
        let local_pack = Kernel::new(
            gpu,
            "client/local_pack.wgsl",
            &[
                B::Uniform,
                B::Tex,
                B::Tex,
                B::Store(wgpu::TextureFormat::Rgba32Float),
            ],
        )?;
        let no_maps = gpu.device.create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("no maps"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: MAP_LAYERS,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytemuck::cast_slice(&[0.0f32; 4 * MAP_LAYERS as usize]),
        );
        let profile = Kernel::new(
            gpu,
            "client/profile.wgsl",
            &[
                B::Uniform,
                B::Tex,
                B::Tex3,
                B::Tex3,
                B::Store(wgpu::TextureFormat::Rgba16Float),
            ],
        )?;
        let defringe = Kernel::new(
            gpu,
            "client/defringe.wgsl",
            &[
                B::Uniform,
                B::Tex,
                B::Store(wgpu::TextureFormat::Rgba16Float),
            ],
        )?;
        Ok(Self {
            pipeline,
            layout,
            mask_layout,
            mask_sampler,
            no_mask,
            density_down,
            density_blur,
            identity_curves,
            guided: crate::guided::GuidedFilter::new(gpu)?,
            local_pack,
            no_maps,
            haze: crate::haze::Haze::new(gpu)?,
            profile,
            defringe,
        })
    }

    /// Run the develop pass once, without masks, and read the result back.
    pub fn run(&self, gpu: &Gpu, source: &RgbaF32, uniform: &[u8], lut: &[f32]) -> Result<RgbaF32> {
        let src_tex = crate::upload_rgba16(gpu, source);
        let out = self.render(gpu, &src_tex, uniform, lut, &Masks::none())?;
        crate::read_rgba16(gpu, &out)
    }

    /// The whole frame `whole` averaged down to the adjacency map's size (`sidecar_color::developer::map_size`):
    /// what `density_map` develops. The loupe lens-corrects it before.
    pub fn density_source(&self, gpu: &Gpu, whole: &wgpu::Texture) -> wgpu::Texture {
        let frame = [whole.width(), whole.height()];
        let [w, h] = sidecar_color::developer::map_size(frame);
        let small = texture(gpu, w, h, wgpu::TextureFormat::Rgba16Float);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let k = sidecar_color::developer::map_scale(frame) as f32;
        self.density_down.run(
            gpu,
            &mut enc,
            &[k, 0.0, 0.0, 0.0],
            [w, h],
            &[Res::View(&view(whole)), Res::View(&view(&small))],
        );
        gpu.queue.submit([enc.finish()]);
        small
    }

    /// The adjacency map for a frame `frame` pixels across, from `small` (`density_source`'s, at the map's size):
    /// the film's log exposure, as this develop sees it through the whole frame's masks, blurred over the
    /// developer's reach.
    pub fn density_map(
        &self,
        gpu: &Gpu,
        small: &wgpu::Texture,
        frame: [u32; 2],
        uniform: &[u8],
        lut: &[f32],
        masks: &Masks,
    ) -> Result<wgpu::Texture> {
        let mut own: DevelopParams = *bytemuck::from_bytes(uniform);
        own.mono[2] = 1.0;
        // The film's exposure, without Texture, Clarity or Dehaze: the browser's map is made the same way.
        own.local = [0.0; 4];
        let mut whole = masks.params;
        whole.frame = [0.0; 4];
        let whole = Masks {
            params: whole,
            textures: masks.textures.clone(),
            curves: masks.curves,
            mono: masks.mono,
            density: None,
            ..Masks::none()
        };
        let exposure = self.render(gpu, small, bytemuck::bytes_of(&own), lut, &whole)?;
        let (w, h) = (small.width(), small.height());
        let (across, map) = (
            texture(gpu, w, h, wgpu::TextureFormat::Rgba16Float),
            texture(gpu, w, h, wgpu::TextureFormat::Rgba16Float),
        );
        let sigma = sidecar_color::developer::map_sigma(own.developer[1], frame);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        self.density_blur.run(
            gpu,
            &mut enc,
            &[sigma, 1.0, 0.0, 0.0],
            [w, h],
            &[Res::View(&view(&exposure)), Res::View(&view(&across))],
        );
        self.density_blur.run(
            gpu,
            &mut enc,
            &[sigma, 0.0, 1.0, 0.0],
            [w, h],
            &[Res::View(&view(&across)), Res::View(&view(&map))],
        );
        gpu.queue.submit([enc.finish()]);
        Ok(map)
    }

    /// Texture and Clarity's map for a frame `frame` pixels across, from `small` (`density_source`'s: the whole
    /// frame averaged down to at most 1,024 on the long edge): two self-guided filters of its log luminance, the
    /// texture band's and the clarity band's (`sidecar_color::params::TEXTURE_RADIUS` and the rest, shares of
    /// the long edge), their coefficients side by side in one RGBA32F texel. It depends on the photo alone, not
    /// on the edit, so a caller may keep it while the sliders move.
    pub fn local_map(&self, gpu: &Gpu, small: &wgpu::Texture, frame: [u32; 2]) -> wgpu::Texture {
        use crate::guided::{GUIDE, Guided};
        use sidecar_color::params::{CLARITY_EPS, CLARITY_RADIUS, TEXTURE_EPS, TEXTURE_RADIUS};
        let size = [small.width(), small.height()];
        // Radii in the map's own texels.
        let texels =
            frame[0].max(frame[1]) as f64 / sidecar_color::developer::map_scale(frame) as f64;
        let band = |radius: f64, eps: f64| {
            self.guided
                .coefficients(
                    gpu,
                    small,
                    small,
                    size,
                    Guided {
                        radius: (radius * texels).max(1.0) as f32,
                        eps: eps as f32,
                        subsample: 1,
                        channel: GUIDE,
                        log_guide: true,
                    },
                )
                .0
        };
        let (fine, mid) = (
            band(TEXTURE_RADIUS, TEXTURE_EPS),
            band(CLARITY_RADIUS, CLARITY_EPS),
        );
        let map = texture(gpu, size[0], size[1], wgpu::TextureFormat::Rgba32Float);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        self.local_pack.run(
            gpu,
            &mut enc,
            &[0.0f32; 4],
            size,
            &[
                Res::View(&view(&fine)),
                Res::View(&view(&mid)),
                Res::View(&view(&map)),
            ],
        );
        gpu.queue.submit([enc.finish()]);
        map
    }

    /// Dehaze's map for a frame `frame` pixels across from `small` (`density_source`'s), under this uniform's
    /// white balance and camera matrix: the transmission's guided coefficients and the airlight (`crate::haze`).
    pub fn haze_map(
        &self,
        gpu: &Gpu,
        small: &wgpu::Texture,
        frame: [u32; 2],
        uniform: &[u8],
    ) -> (wgpu::Texture, wgpu::Texture) {
        let p: &DevelopParams = bytemuck::from_bytes(uniform);
        self.haze
            .map(gpu, &self.guided, small, frame, p.cam_to_work, p.wb_mul)
    }

    /// The whole-frame maps as develop binds them: one array of `MAP_LAYERS` (the fragment stage allows sixteen
    /// sampled textures, and develop is at that), layer 0 the local-contrast map, 1 Dehaze's, and the airlight in
    /// texel (0, 0) of layer 2. `None` when neither is wanted.
    fn maps(
        gpu: &Gpu,
        local: Option<&wgpu::Texture>,
        haze: Option<(&wgpu::Texture, &wgpu::Texture)>,
    ) -> Option<wgpu::Texture> {
        let any = local.or(haze.map(|h| h.0))?;
        let array = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("develop maps"),
            size: wgpu::Extent3d {
                width: any.width(),
                height: any.height(),
                depth_or_array_layers: MAP_LAYERS,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let mut copy = |from: &wgpu::Texture, layer: u32| {
            enc.copy_texture_to_texture(
                from.as_image_copy(),
                wgpu::TexelCopyTextureInfo {
                    texture: &array,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: from.width().min(any.width()),
                    height: from.height().min(any.height()),
                    depth_or_array_layers: 1,
                },
            );
        };
        if let Some(l) = local {
            copy(l, 0);
        }
        if let Some((m, a)) = haze {
            copy(m, 1);
            copy(a, 2);
        }
        gpu.queue.submit([enc.finish()]);
        Some(array)
    }

    /// `src` through the camera profile pass: its HueSatMap and LookTable, as a new texture.
    pub fn apply_profile(
        &self,
        gpu: &Gpu,
        src: &wgpu::Texture,
        p: &sidecar_color::params::ProfilePass,
    ) -> wgpu::Texture {
        let u: &sidecar_color::params::ProfileParams = bytemuck::from_bytes(&p.uniform);
        // A table as a 3D texture: saturation across, hue down, value deep; a single zero texel when absent.
        let table = |dims: [u32; 4], data: &[f32]| {
            let (size, data) =
                if dims[0] > 0 && data.len() == (dims[0] * dims[1] * dims[2] * 4) as usize {
                    ([dims[1], dims[0], dims[2]], data)
                } else {
                    ([1, 1, 1], &[0.0f32; 4][..])
                };
            gpu.device.create_texture_with_data(
                &gpu.queue,
                &wgpu::TextureDescriptor {
                    label: Some("profile table"),
                    size: wgpu::Extent3d {
                        width: size[0],
                        height: size[1],
                        depth_or_array_layers: size[2],
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: wgpu::TextureFormat::Rgba32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                bytemuck::cast_slice(data),
            )
        };
        let (hs, lt) = (table(u.hue_sat, &p.hue_sat), table(u.look, &p.look));
        let out = texture(
            gpu,
            src.width(),
            src.height(),
            wgpu::TextureFormat::Rgba16Float,
        );
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        self.profile.run(
            gpu,
            &mut enc,
            u,
            [src.width(), src.height()],
            &[
                Res::View(&view(src)),
                Res::View(&view(&hs)),
                Res::View(&view(&lt)),
                Res::View(&view(&out)),
            ],
        );
        gpu.queue.submit([enc.finish()]);
        out
    }

    /// `src` with its fringes taken out (`client/defringe.wgsl`), as a new texture. `frame_long` is the whole frame's
    /// long edge in pixels when `src` is a tile of it, so the reach is the whole frame's; 0 for a whole frame.
    pub fn apply_defringe(
        &self,
        gpu: &Gpu,
        src: &wgpu::Texture,
        uniform: &[u8],
        frame_long: f32,
    ) -> wgpu::Texture {
        let mut u: sidecar_color::params::DefringeParams = bytemuck::pod_read_unaligned(uniform);
        u.reach[3] = frame_long;
        let out = texture(
            gpu,
            src.width(),
            src.height(),
            wgpu::TextureFormat::Rgba16Float,
        );
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        self.defringe.run(
            gpu,
            &mut enc,
            &u,
            [src.width(), src.height()],
            &[Res::View(&view(src)), Res::View(&view(&out))],
        );
        gpu.queue.submit([enc.finish()]);
        out
    }

    /// Develop a source texture into a new RGBA16F texture (display-linear).
    pub fn render(
        &self,
        gpu: &Gpu,
        src_tex: &wgpu::Texture,
        uniform: &[u8],
        lut: &[f32],
        masks: &Masks,
    ) -> Result<wgpu::Texture> {
        let expected = std::mem::size_of::<DevelopParams>();
        if uniform.len() != expected {
            bail!(
                "develop uniform must be {expected} bytes, got {}",
                uniform.len()
            );
        }
        // Fringes first, as the camera recorded them, then the camera profile: develop reads the result, as the
        // browser's does.
        let frame_long = if masks.params.frame[2] > 0.0 {
            masks.params.frame[2].max(masks.params.frame[3])
        } else {
            0.0
        };
        let defringed = masks
            .defringe
            .map(|u| self.apply_defringe(gpu, src_tex, u, frame_long));
        let src_tex = defringed.as_ref().unwrap_or(src_tex);
        let profiled = masks.profile.map(|p| self.apply_profile(gpu, src_tex, p));
        let src_tex = profiled.as_ref().unwrap_or(src_tex);
        // The whole frame makes its own adjacency map; a tile without one goes without its edges.
        let own_map = match (
            masks.density,
            needs_density_map(uniform) && masks.params.frame[2] == 0.0,
        ) {
            (None, true) => {
                let small = self.density_source(gpu, src_tex);
                Some(self.density_map(
                    gpu,
                    &small,
                    [src_tex.width(), src_tex.height()],
                    uniform,
                    lut,
                    masks,
                )?)
            }
            _ => None,
        };
        let density = masks.density.or(own_map.as_ref()).unwrap_or(&self.no_mask);
        let own_local =
            (masks.local.is_none() && needs_local_map(uniform) && masks.params.frame[2] == 0.0)
                .then(|| {
                    let small = self.density_source(gpu, src_tex);
                    self.local_map(gpu, &small, [src_tex.width(), src_tex.height()])
                });
        let own_haze = (masks.haze.is_none()
            && needs_haze_map(uniform)
            && masks.params.frame[2] == 0.0)
            .then(|| {
                let small = self.density_source(gpu, src_tex);
                self.haze_map(gpu, &small, [src_tex.width(), src_tex.height()], uniform)
            });
        let own_maps = Self::maps(
            gpu,
            masks.local.or(own_local.as_ref()),
            masks.haze.or(own_haze.as_ref().map(|(m, a)| (m, a))),
        );
        let maps = own_maps.as_ref().unwrap_or(&self.no_maps);
        let (w, h) = (src_tex.width(), src_tex.height());
        let size = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        let d = &gpu.device;
        let lut_tex = d.create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("tone lut"),
                size: wgpu::Extent3d {
                    width: lut.len() as u32,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D1,
                format: wgpu::TextureFormat::R32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytemuck::cast_slice(lut),
        );
        let ubo = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("develop params"),
            contents: uniform,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let out = d.create_texture(&wgpu::TextureDescriptor {
            label: Some("developed"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let mono_tex = mono_texture(gpu, masks.mono);
        let slide_exposures = slide_texture(gpu, "slide exposures", masks.slide_exposure);
        let slide_responses = slide_texture(gpu, "slide responses", masks.slide_response);
        let bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("develop"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ubo.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &src_tex.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &lut_tex.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(
                        &mono_tex.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(
                        &density.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(
                        &slide_exposures.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(
                        &slide_responses.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&maps.create_view(
                        &wgpu::TextureViewDescriptor {
                            dimension: Some(wgpu::TextureViewDimension::D2Array),
                            ..Default::default()
                        },
                    )),
                },
            ],
        });
        let mask_ubo = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("develop masks"),
            contents: bytemuck::bytes_of(&masks.params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let views: Vec<wgpu::TextureView> = (0..MAX_MASKS)
            .map(|i| {
                masks
                    .textures
                    .get(i)
                    .copied()
                    .flatten()
                    .unwrap_or(&self.no_mask)
                    .create_view(&Default::default())
            })
            .collect();
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: mask_ubo.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&self.mask_sampler),
            },
        ];
        entries.extend(views.iter().enumerate().map(|(i, v)| wgpu::BindGroupEntry {
            binding: 2 + i as u32,
            resource: wgpu::BindingResource::TextureView(v),
        }));
        let own_curves =
            (masks.curves.len() == MAX_MASKS * LUT_SIZE).then(|| curves_texture(gpu, masks.curves));
        let curves_view = own_curves
            .as_ref()
            .unwrap_or(&self.identity_curves)
            .create_view(&Default::default());
        entries.push(wgpu::BindGroupEntry {
            binding: 2 + MAX_MASKS as u32,
            resource: wgpu::BindingResource::TextureView(&curves_view),
        });
        let mask_bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("develop masks"),
            layout: &self.mask_layout,
            entries: &entries,
        });
        let mut enc = d.create_command_encoder(&Default::default());
        {
            let out_view = out.create_view(&Default::default());
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("develop"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &out_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.set_bind_group(1, &mask_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        gpu.queue.submit([enc.finish()]);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn develops_on_the_gpu() {
        let gpu = match crate::test_gpu() {
            Ok(g) => g,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        eprintln!(
            "adapter: {} ({:?})",
            gpu.adapter_info.name, gpu.adapter_info.backend
        );
        let pass = DevelopPass::new(gpu).unwrap();
        // Identity-ish uniforms: unit WB, unit matrices, default tone.
        let mut u = vec![0u8; std::mem::size_of::<sidecar_color::params::DevelopParams>()];
        let put = |u: &mut Vec<u8>, off: usize, v: &[f32]| {
            u[off..off + v.len() * 4].copy_from_slice(bytemuck::cast_slice(v))
        };
        put(&mut u, 0, &[1.0, 1.0, 1.0, 1.0]);
        put(
            &mut u,
            16,
            &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        );
        put(
            &mut u,
            64,
            &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
        );
        put(&mut u, 112, &[-10.0, 6.5, 7.0, 0.5]);
        let lut: Vec<f32> = (0..4096).map(|i| i as f32 / 4095.0).collect();
        let src = RgbaF32 {
            width: 4,
            height: 2,
            data: [0.18f32, 0.18, 0.18, 0.0].repeat(8),
        };
        let out = pass.run(gpu, &src, &u, &lut).unwrap();
        assert_eq!(out.data.len(), 32);
        assert!(
            out.data[0] > 0.01 && out.data[0] < 1.0,
            "{:?}",
            &out.data[..4]
        );
    }

    /// Identity colour, default tone, black and white on panchromatic film with a developer of the given effects.
    fn mono_uniform(developer: [f32; 4]) -> Vec<u8> {
        let mut p: DevelopParams = bytemuck::Zeroable::zeroed();
        p.wb_mul = [1.0; 3];
        p.exposure = 1.0;
        p.cam_to_work = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        p.work_to_disp = p.cam_to_work;
        p.tone = [-10.0, 6.5, 7.0, 0.5];
        p.mono = [1.0, sidecar_color::spectral::GRID as f32, 0.0, 0.0];
        p.developer = developer;
        bytemuck::bytes_of(&p).to_vec()
    }

    /// A developer with adjacency lightens the bright side of an edge and darkens the dark side against their
    /// interiors; the standard one leaves both sides flat. A tile given the whole frame's map develops as the
    /// same part of the whole does.
    #[test]
    fn adjacency_sharpens_edges_and_a_tile_matches_the_whole() {
        let Ok(gpu) = crate::test_gpu() else {
            return eprintln!("skipping: no GPU");
        };
        let pass = DevelopPass::new(gpu).unwrap();
        let (w, h) = (2048u32, 64u32);
        let src = RgbaF32 {
            width: w,
            height: h,
            data: (0..w * h)
                .flat_map(|i| {
                    let v = if i % w < w / 2 { 0.03 } else { 0.4 };
                    [v, v, v, 0.0]
                })
                .collect(),
        };
        let lut: Vec<f32> = (0..4096).map(|i| i as f32 / 4095.0).collect();
        let table = sidecar_color::spectral::mono_lut(
            "panchromatic",
            "none",
            &sidecar_color::spectral::light("daylight", 6500.0).unwrap(),
        )
        .unwrap();
        let tex = crate::upload_rgba16(gpu, &src);
        let masks = Masks {
            mono: &table,
            ..Masks::none()
        };
        let at = |img: &RgbaF32, x: u32| img.data[((32 * img.width + x) * 4) as usize];
        // Reach: 0.1 mm on 36 mm, so about 5.7 pixels here; the edge at 1024.
        let standard = mono_uniform([0.0, 0.1 / 36.0, 1.0, 0.0]);
        let flat = crate::read_rgba16(
            gpu,
            &pass.render(gpu, &tex, &standard, &lut, &masks).unwrap(),
        )
        .unwrap();
        assert_eq!(at(&flat, 1030), at(&flat, 1900));
        assert_eq!(at(&flat, 1018), at(&flat, 100));
        let sharp = mono_uniform([0.35, 0.1 / 36.0, 1.0, 0.0]);
        let whole = crate::read_rgba16(gpu, &pass.render(gpu, &tex, &sharp, &lut, &masks).unwrap())
            .unwrap();
        assert!(
            at(&whole, 1027) > at(&whole, 1900) * 1.02,
            "bright side {} interior {}",
            at(&whole, 1027),
            at(&whole, 1900)
        );
        assert!(
            at(&whole, 1021) < at(&whole, 100) * 0.98,
            "dark side {} interior {}",
            at(&whole, 1021),
            at(&whole, 100)
        );
        eprintln!(
            "edge: bright {:.4} → {:.4}, dark {:.4} → {:.4}",
            at(&whole, 1900),
            at(&whole, 1027),
            at(&whole, 100),
            at(&whole, 1021)
        );
        assert!(
            (at(&whole, 1900) - at(&flat, 1900)).abs() < 2e-3,
            "interiors as they were"
        );
        // A tile at 960, 128 wide, its place in the frame and the whole frame's map given.
        let map = pass
            .density_map(
                gpu,
                &pass.density_source(gpu, &tex),
                [w, h],
                &sharp,
                &lut,
                &masks,
            )
            .unwrap();
        let cut = RgbaF32 {
            width: 128,
            height: h,
            data: (0..h)
                .flat_map(|y| {
                    src.data[((y * w + 960) * 4) as usize..((y * w + 1088) * 4) as usize].to_vec()
                })
                .collect(),
        };
        let mut placed = masks.params;
        placed.frame = [960.0, 0.0, w as f32, h as f32];
        let tile_masks = Masks {
            params: placed,
            density: Some(&map),
            ..Masks {
                mono: &table,
                ..Masks::none()
            }
        };
        let tile = crate::read_rgba16(
            gpu,
            &pass
                .render(
                    gpu,
                    &crate::upload_rgba16(gpu, &cut),
                    &sharp,
                    &lut,
                    &tile_masks,
                )
                .unwrap(),
        )
        .unwrap();
        let worst = (0..128)
            .map(|x| (at(&tile, x) - at(&whole, 960 + x)).abs())
            .fold(0.0, f32::max);
        assert!(worst < 1e-3, "worst difference {worst}");
    }

    /// Clarity raises middle-scale detail in stops and leaves a hard edge with little halo; at rest the pass is the
    /// one before, bit for bit; a tile given the whole frame's map develops as that part of the whole does.
    #[test]
    fn clarity_raises_middle_detail_without_halos() {
        let Ok(gpu) = crate::test_gpu() else {
            return eprintln!("skipping: no GPU");
        };
        let pass = DevelopPass::new(gpu).unwrap();
        let (w, h) = (2048u32, 128u32);
        // Left: a ripple of ±0.15 stop, 60 px across, round mid grey. Right: two flats meeting at a 3.3-stop edge.
        let level = |x: u32| -> f32 {
            if x < 700 {
                0.18 * (0.15 * (x as f32 * std::f32::consts::TAU / 60.0).sin()).exp2()
            } else if x < 1400 {
                0.05
            } else {
                0.5
            }
        };
        let src = RgbaF32 {
            width: w,
            height: h,
            data: (0..w * h)
                .flat_map(|i| {
                    let v = level(i % w);
                    [1.3 * v, v, 0.8 * v, 0.0]
                })
                .collect(),
        };
        let uniform = |local: [f32; 4]| {
            let mut p: DevelopParams = bytemuck::Zeroable::zeroed();
            p.wb_mul = [1.0; 3];
            p.exposure = 1.0;
            p.cam_to_work = [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ];
            p.work_to_disp = p.cam_to_work;
            p.tone = [-10.0, 6.5, 7.0, 0.5];
            p.local = local;
            bytemuck::bytes_of(&p).to_vec()
        };
        let lut: Vec<f32> = (0..4096).map(|i| i as f32 / 4095.0).collect();
        let tex = crate::upload_rgba16(gpu, &src);
        let develop = |u: &[u8]| {
            crate::read_rgba16(
                gpu,
                &pass.render(gpu, &tex, u, &lut, &Masks::none()).unwrap(),
            )
            .unwrap()
        };
        let at = |img: &RgbaF32, x: u32| img.data[((64 * img.width + x) * 4 + 1) as usize];
        let rest = develop(&uniform([0.0, 0.0, 0.0, 0.0]));
        let off = develop(&uniform([0.0, 0.0, 1.0, 0.0]));
        assert!(
            (0..w).all(|x| at(&rest, x) == at(&off, x)),
            "both bands at 0 change nothing"
        );
        let clear = develop(&uniform([0.0, 1.0, 1.0, 0.0]));
        // The ripple's swing, in stops of the output, over two periods away from the ends.
        let swing = |img: &RgbaF32| {
            let v: Vec<f32> = (300..420).map(|x| at(img, x).log2()).collect();
            v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min)
        };
        let (before, after) = (swing(&rest), swing(&clear));
        // Each side of the edge, 8 px from it, against its own interior.
        let halo = |img: &RgbaF32| {
            (
                at(img, 1392) / at(img, 1000) - 1.0,
                at(img, 1408) / at(img, 1800) - 1.0,
            )
        };
        eprintln!(
            "ripple {before:.3} → {after:.3} stops; edge {:?} → {:?}",
            halo(&rest),
            halo(&clear)
        );
        assert!(
            after > 1.6 * before,
            "clarity raised the ripple {before} → {after}"
        );
        // A 3.3-stop edge at full Clarity: the guided filter's windows that straddle it leave about a tenth of a
        // stop on the dark side, 8 px out — measured, and bounded here so it can't grow unnoticed.
        let (d, b) = halo(&clear);
        assert!(d.abs() < 0.1 && b.abs() < 0.05, "halo {d} {b}");
        let fine = develop(&uniform([1.0, 0.0, 1.0, 0.0]));
        eprintln!(
            "texture: ripple {:.3} stops, edge {:?}",
            swing(&fine),
            halo(&fine)
        );
        let (d, b) = halo(&fine);
        assert!(d.abs() < 0.05 && b.abs() < 0.05, "texture halo {d} {b}");
        // A tile at 1300, 256 wide, given the whole frame's map.
        let small = pass.density_source(gpu, &tex);
        let map = pass.local_map(gpu, &small, [w, h]);
        let cut = RgbaF32 {
            width: 256,
            height: h,
            data: (0..h)
                .flat_map(|y| {
                    src.data[((y * w + 1300) * 4) as usize..((y * w + 1556) * 4) as usize].to_vec()
                })
                .collect(),
        };
        let mut params: sidecar_color::params::MaskParams = bytemuck::Zeroable::zeroed();
        params.frame = [1300.0, 0.0, w as f32, h as f32];
        let tile = crate::read_rgba16(
            gpu,
            &pass
                .render(
                    gpu,
                    &crate::upload_rgba16(gpu, &cut),
                    &uniform([0.0, 1.0, 1.0, 0.0]),
                    &lut,
                    &Masks {
                        params,
                        local: Some(&map),
                        ..Masks::none()
                    },
                )
                .unwrap(),
        )
        .unwrap();
        let worst = (0..256)
            .map(|x| (at(&tile, x) - at(&clear, 1300 + x)).abs())
            .fold(0.0, f32::max);
        assert!(worst < 1e-3, "worst difference {worst}");
    }

    /// A scene hazed by Koschmieder's model over a depth ramp under a sky, with a known airlight: Dehaze finds the airlight,
    /// and its develop is much nearer the haze-free scene's than the hazy one's is; −100 takes it further away; a
    /// tile given the whole frame's map develops as that part of the whole does.
    #[test]
    fn dehaze_recovers_a_hazed_scene() {
        let Ok(gpu) = crate::test_gpu() else {
            return eprintln!("skipping: no GPU");
        };
        let pass = DevelopPass::new(gpu).unwrap();
        let (w, h) = (1024u32, 512u32);
        let air = [0.55f32, 0.6, 0.65];
        let hash = |mut s: u64| {
            s = s.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s % 10_000) as f32 / 10_000.0
        };
        // Coloured cells 32 px square, each with one channel near black, as outdoor scenes have (the prior).
        let clear = |x: u32, y: u32| -> [f32; 3] {
            let k = ((y / 32) * 64 + x / 32) as u64;
            let mut c = [
                0.05 + 0.4 * hash(k * 3),
                0.05 + 0.4 * hash(k * 3 + 1),
                0.05 + 0.4 * hash(k * 3 + 2),
            ];
            c[(hash(k * 7) * 3.0) as usize % 3] = 0.01;
            c
        };
        // Depth grows left to right: transmission from 0.95 to 0.25.
        let t_at = |x: u32| (-(0.05 + 1.33 * x as f32 / w as f32)).exp();
        let image = |hazy: bool| RgbaF32 {
            width: w,
            height: h,
            data: (0..w * h)
                .flat_map(|i| {
                    let (x, y) = (i % w, i / w);
                    // The top eighth is sky: nothing but air, hazy or not.
                    let t = if y < h / 8 {
                        0.0
                    } else if hazy {
                        t_at(x)
                    } else {
                        1.0
                    };
                    let j = clear(x, y);
                    [0, 1, 2]
                        .map(|c| j[c] * t + air[c] * (1.0 - t))
                        .into_iter()
                        .chain([0.0])
                        .collect::<Vec<_>>()
                })
                .collect(),
        };
        let uniform = |dehaze: f32| {
            let mut p: DevelopParams = bytemuck::Zeroable::zeroed();
            p.wb_mul = [1.0; 3];
            p.exposure = 1.0;
            p.cam_to_work = [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ];
            p.work_to_disp = p.cam_to_work;
            p.tone = [-10.0, 6.5, 7.0, 0.5];
            p.local = [0.0, 0.0, 0.0, dehaze];
            bytemuck::bytes_of(&p).to_vec()
        };
        let lut: Vec<f32> = (0..4096).map(|i| i as f32 / 4095.0).collect();
        let (hazy, truth) = (
            crate::upload_rgba16(gpu, &image(true)),
            crate::upload_rgba16(gpu, &image(false)),
        );
        let develop = |tex: &wgpu::Texture, u: &[u8]| {
            crate::read_rgba16(
                gpu,
                &pass.render(gpu, tex, u, &lut, &Masks::none()).unwrap(),
            )
            .unwrap()
        };
        let reference = develop(&truth, &uniform(0.0));
        // Mean absolute difference from the haze-free render, away from the frame's edges.
        let err = |img: &RgbaF32| {
            let mut e = 0.0;
            let mut n = 0.0;
            for y in (40..h - 40).step_by(4) {
                for x in (40..w - 40).step_by(4) {
                    for c in 0..3 {
                        let i = ((y * w + x) * 4 + c) as usize;
                        e += (img.data[i] - reference.data[i]).abs() as f64;
                        n += 1.0;
                    }
                }
            }
            e / n
        };
        let as_is = err(&develop(&hazy, &uniform(0.0)));
        let removed = err(&develop(&hazy, &uniform(1.0)));
        let added = err(&develop(&hazy, &uniform(-1.0)));
        let small = pass.density_source(gpu, &hazy);
        let (map, found) = pass.haze_map(gpu, &small, [w, h], &uniform(1.0));
        let found = crate::read_rgba32(gpu, &found).unwrap();
        eprintln!(
            "airlight {:?} (truth {air:?}); error from the clear scene: hazy {as_is:.4}, dehazed {removed:.4}, doubled {added:.4}",
            &found.data[..3]
        );
        for (got, want) in found.data.iter().zip(air) {
            assert!(
                (got / want - 1.0).abs() < 0.1,
                "airlight {:?}",
                &found.data[..3]
            );
        }
        assert!(
            removed < 0.5 * as_is,
            "dehazed {removed} against hazy {as_is}"
        );
        assert!(added > 1.2 * as_is, "doubled {added} against hazy {as_is}");
        // A tile at 600, 200 wide, given the whole frame's map.
        let whole = develop(&hazy, &uniform(1.0));
        let src = image(true);
        let cut = RgbaF32 {
            width: 200,
            height: h,
            data: (0..h)
                .flat_map(|y| {
                    src.data[((y * w + 600) * 4) as usize..((y * w + 800) * 4) as usize].to_vec()
                })
                .collect(),
        };
        let mut params: sidecar_color::params::MaskParams = bytemuck::Zeroable::zeroed();
        params.frame = [600.0, 0.0, w as f32, h as f32];
        let found_tex =
            crate::haze::one_texel(gpu, [found.data[0], found.data[1], found.data[2], 1.0]);
        let tile = crate::read_rgba16(
            gpu,
            &pass
                .render(
                    gpu,
                    &crate::upload_rgba16(gpu, &cut),
                    &uniform(1.0),
                    &lut,
                    &Masks {
                        params,
                        haze: Some((&map, &found_tex)),
                        ..Masks::none()
                    },
                )
                .unwrap(),
        )
        .unwrap();
        let at = |img: &RgbaF32, x: u32| img.data[((256 * img.width + x) * 4 + 1) as usize];
        let worst = (0..200)
            .map(|x| (at(&tile, x) - at(&whole, 600 + x)).abs())
            .fold(0.0, f32::max);
        assert!(worst < 1e-3, "worst difference {worst}");
    }

    /// The camera profile pass does what `sidecar_color::dcp::apply` does on the CPU: a table that turns hues 30°
    /// and halves saturation, through the matrices `pack` makes from a forward matrix, on a few camera colours.
    #[test]
    fn the_profile_pass_matches_the_cpu() {
        use sidecar_color::dcp::{HsvTable, Profile};
        let Ok(gpu) = crate::test_gpu() else {
            return eprintln!("skipping: no GPU");
        };
        let pass = DevelopPass::new(gpu).unwrap();
        let dims = [12u32, 6, 1];
        let table = HsvTable {
            dims,
            data: (0..dims[0] * dims[1])
                .flat_map(|_| [30.0, 0.5, 1.0])
                .collect(),
            srgb: false,
        };
        let mut camera = sidecar_color::camera::SourceSpace::Srgb.camera_color();
        camera.as_shot_mul = [2.0, 1.0, 1.5];
        camera.profile = Some(Box::new(Profile {
            name: "test".into(),
            forward: vec![(
                6504.0,
                [[0.6, 0.25, 0.11], [0.25, 0.73, 0.02], [0.02, 0.1, 0.70]],
            )],
            hue_sat: vec![(6504.0, table.clone())],
            look: None,
        }));
        let edit = sidecar_color::params::EditParams {
            camera_profile: Some(sidecar_color::params::FILE_PROFILE.into()),
            ..Default::default()
        };
        let packed = sidecar_color::params::pack(
            &edit,
            &camera,
            sidecar_color::SourceKind::Raw,
            sidecar_color::DisplaySpace::Srgb,
        );
        let p = packed.profile.expect("a profile pass");
        let u: &sidecar_color::params::ProfileParams = bytemuck::from_bytes(&p.uniform);
        // The last is neutral once white-balanced by [2, 1, 1.5].
        let colours = [
            [0.2f32, 0.3, 0.1],
            [0.05, 0.1, 0.3],
            [0.3, 0.3, 0.2],
            [0.05, 0.1, 0.1 / 1.5],
        ];
        let src = RgbaF32 {
            width: 4,
            height: 1,
            data: colours
                .iter()
                .flat_map(|c| [c[0], c[1], c[2], 0.0])
                .collect(),
        };
        let out = crate::read_rgba16(
            gpu,
            &pass.apply_profile(gpu, &crate::upload_rgba16(gpu, &src), &p),
        )
        .unwrap();
        let m = |a: &[[f32; 4]; 3]| -> [[f64; 3]; 3] {
            std::array::from_fn(|r| std::array::from_fn(|c| a[c][r] as f64))
        };
        let (to, from) = (m(&u.to_prophoto), m(&u.from_prophoto));
        for (i, c) in colours.iter().enumerate() {
            let w = [0, 1, 2].map(|k| c[k] as f64 * u.wb[k] as f64);
            let pp = sidecar_color::mat::apply(&to, &w);
            let back = sidecar_color::mat::apply(&from, &sidecar_color::dcp::apply(&table, pp));
            let want = [0, 1, 2].map(|k| back[k] / u.wb[k] as f64);
            for (k, want) in want.iter().enumerate() {
                let got = out.data[i * 4 + k] as f64;
                assert!(
                    (got - want).abs() < 2e-3 * want.abs().max(0.05),
                    "{c:?}: {got} against {want}"
                );
            }
        }
        // A neutral stays neutral (no hue to turn, no saturation to halve).
        let grey = &out.data[12..15];
        assert!(
            (grey[0] - 0.05).abs() < 1e-3
                && (grey[1] - 0.1).abs() < 1e-3
                && (grey[2] - 0.1 / 1.5).abs() < 1e-3,
            "{grey:?}"
        );
    }

    /// Defringe takes a purple fringe beside a black-to-white edge to neutral at its own luminance, and leaves a
    /// large purple patch, with no edge within reach of its middle, as it was.
    #[test]
    fn defringe_takes_fringes_and_leaves_purple_things() {
        let Ok(gpu) = crate::test_gpu() else {
            return eprintln!("skipping: no GPU");
        };
        let pass = DevelopPass::new(gpu).unwrap();
        let (w, h) = (512u32, 256u32);
        let purple = [0.45f32, 0.25, 0.65];
        let src = RgbaF32 {
            width: w,
            height: h,
            data: (0..w * h)
                .flat_map(|i| {
                    let (x, y) = (i % w, i / w);
                    let c = if y < 128 {
                        // An edge at x = 256, a fringe in the two pixels beside it on the bright side.
                        if x < 256 {
                            [0.02, 0.02, 0.02]
                        } else if x < 258 {
                            purple
                        } else {
                            [0.8, 0.8, 0.8]
                        }
                    } else {
                        // A purple thing, 200 px wide, on mid grey.
                        if (156..356).contains(&x) {
                            purple
                        } else {
                            [0.2, 0.2, 0.2]
                        }
                    };
                    [c[0], c[1], c[2], 0.0]
                })
                .collect(),
        };
        let mut d: DevelopParams = bytemuck::Zeroable::zeroed();
        d.wb_mul = [1.0; 3];
        d.cam_to_work = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        let u = sidecar_color::params::Defringe::uniform(
            Some(&sidecar_color::params::Defringe {
                purple: 100.0,
                ..Default::default()
            }),
            &d,
        )
        .unwrap();
        let out = crate::read_rgba16(
            gpu,
            &pass.apply_defringe(gpu, &crate::upload_rgba16(gpu, &src), &u, 0.0),
        )
        .unwrap();
        let at =
            |x: u32, y: u32| &out.data[((y * w + x) * 4) as usize..((y * w + x) * 4 + 3) as usize];
        let spread = |c: &[f32]| {
            c.iter().cloned().fold(f32::MIN, f32::max) - c.iter().cloned().fold(f32::MAX, f32::min)
        };
        let fringe = at(256, 64);
        eprintln!("fringe {fringe:?}, purple thing {:?}", at(256, 192));
        assert!(spread(fringe) < 0.02, "the fringe is neutral: {fringe:?}");
        let y = 0.2627 * purple[0] + 0.678 * purple[1] + 0.0593 * purple[2];
        assert!((fringe[1] - y).abs() < 0.01, "at its own luminance");
        let thing = at(256, 192);
        assert!(
            (thing[0] - 0.45).abs() < 1e-3 && (thing[2] - 0.65).abs() < 1e-3,
            "the purple thing keeps its colour: {thing:?}"
        );
    }

    /// The develop pass renders a spectral colour stock as `slide::Slide` does on the CPU: the same two tables,
    /// read the same way. Stand-in standards, so these are not real PROVIA 100F colours — the chain is what is
    /// being checked. At amount 0 the pass is the neutral rendering, bit for bit.
    #[test]
    fn a_spectral_stock_develops_as_its_chain_does() {
        use sidecar_color::params::DevelopParams;
        let Ok(gpu) = crate::test_gpu() else {
            return eprintln!("skipping: no GPU");
        };
        let pass = DevelopPass::new(gpu).unwrap();
        let stock = sidecar_color::slide::slide("provia-100f").unwrap();
        let (meter, light) = sidecar_color::slide::stand_in_standards();
        // 33³ keeps the table's own interpolation error well under what is being measured here.
        let profile = stock.profile_sized(&meter, &light, 33);
        let exposures = stock.exposure_lut(&light);
        let colours: [[f32; 3]; 5] = [
            [0.18, 0.18, 0.18],
            [0.04, 0.04, 0.04],
            [0.6, 0.6, 0.6],
            [0.45, 0.12, 0.1],
            [0.1, 0.2, 0.5],
        ];
        let src = RgbaF32 {
            width: colours.len() as u32,
            height: 1,
            data: colours
                .iter()
                .flat_map(|c| [c[0], c[1], c[2], 0.0])
                .collect(),
        };
        let mut p: DevelopParams = bytemuck::Zeroable::zeroed();
        p.wb_mul = [1.0; 3];
        p.exposure = 1.0;
        p.cam_to_work = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        p.work_to_disp = p.cam_to_work;
        p.tone = [-10.0, 6.5, 7.0, 0.5];
        p.slide = [
            1.0,
            sidecar_color::spectral::GRID as f32,
            profile.side as f32,
            0.0,
        ];
        p.slide_log_h = [
            profile.offsets[0],
            profile.offsets[1],
            profile.offsets[2],
            profile.scale,
        ];
        // The tone controls at their defaults, in a developed file's terms: the identity, so what comes out is
        // the chain's own colour.
        p.slide_tone = [0.0, 0.0, 1.0, 1.0];
        let lut: Vec<f32> = (0..4096).map(|i| i as f32 / 4095.0).collect();
        let tex = crate::upload_rgba16(gpu, &src);
        let masks = Masks {
            slide_exposure: &exposures,
            slide_response: &profile.response,
            ..Masks::none()
        };
        let on = bytemuck::bytes_of(&p).to_vec();
        let out =
            crate::read_rgba16(gpu, &pass.render(gpu, &tex, &on, &lut, &masks).unwrap()).unwrap();
        for (i, c) in colours.iter().enumerate() {
            let want = stock.render(c.map(|v| v as f64), &light, &meter, &light);
            let got = [out.data[i * 4], out.data[i * 4 + 1], out.data[i * 4 + 2]];
            eprintln!("{c:?} → gpu {got:?}, chain {want:?}");
            for k in 0..3 {
                // In stops, so a dark slide is held to the same relative accuracy as a bright one. rgba16float
                // output and the table's 33³ lattice are what the margin is for.
                let off = (got[k].max(1e-6) as f64 / want[k].max(1e-6)).log2().abs();
                assert!(
                    off < 0.06,
                    "{c:?} channel {k}: gpu {} chain {} ({off:+.3} stop)",
                    got[k],
                    want[k]
                );
            }
        }
        // A stock's output is a rendered picture, so the tone controls grade it — the identity at their
        // defaults (asserted above, against the chain), and they must actually bite when moved. Before this
        // they reached the picture only through the neutral rendering a full Amount had already replaced, so
        // Contrast, Blacks and the curve did nothing at all.
        let mut harder = p;
        harder.slide_tone = [0.0, 0.0, 1.6, 1.0]; // Contrast up
        let contrasty = crate::read_rgba16(
            gpu,
            &pass
                .render(gpu, &tex, bytemuck::bytes_of(&harder), &lut, &masks)
                .unwrap(),
        )
        .unwrap();
        let grey = |img: &RgbaF32| img.data[1]; // the mid-grey patch's green
        let dark = |img: &RgbaF32| img.data[4 + 1]; // the near-black patch's green
        assert!(
            dark(&contrasty) < dark(&out) - 1e-4,
            "Contrast must darken the shadows: {} against {}",
            dark(&contrasty),
            dark(&out)
        );
        eprintln!(
            "contrast 1.6: grey {} -> {}, shadow {} -> {}",
            grey(&out),
            grey(&contrasty),
            dark(&out),
            dark(&contrasty)
        );
        let mut lifted = p;
        lifted.slide_tone = [0.6, 0.0, 1.0, 1.0]; // Blacks up
        let raised = crate::read_rgba16(
            gpu,
            &pass
                .render(gpu, &tex, bytemuck::bytes_of(&lifted), &lut, &masks)
                .unwrap(),
        )
        .unwrap();
        assert!(
            dark(&raised) > dark(&out) + 1e-4,
            "Blacks must lift the shadows: {} against {}",
            dark(&raised),
            dark(&out)
        );

        // Amount 0 is the neutral rendering, untouched.
        let mut off_params = p;
        off_params.slide = [0.0; 4];
        let off = crate::read_rgba16(
            gpu,
            &pass
                .render(gpu, &tex, bytemuck::bytes_of(&off_params), &lut, &masks)
                .unwrap(),
        )
        .unwrap();
        let plain = crate::read_rgba16(
            gpu,
            &pass
                .render(
                    gpu,
                    &tex,
                    bytemuck::bytes_of(&off_params),
                    &lut,
                    &Masks::none(),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            off.data, plain.data,
            "a stock at amount 0 renders as if it were not there"
        );
    }
}
