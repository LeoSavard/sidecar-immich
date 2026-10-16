// SPDX-License-Identifier: Apache-2.0
//! The develop pass: source (camera-native linear RGBA16F) + uniforms + tone LUT → display-linear RGBA16F.
//! Bind group layout mirrors web/src/lib/gpu/develop.ts.

use anyhow::{Result, bail};
use wgpu::util::DeviceExt;

use crate::compute::{B, Kernel, Res, texture, view};
use crate::{Gpu, RgbaF32};
use sidecar_color::params::{DevelopParams, MAX_MASKS};
use sidecar_color::spline::LUT_SIZE;

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
            slide_exposure: &[],
            slide_response: &[],
        }
    }
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
    let (side, data) = if side > 0 && (side as usize).pow(3) == data.len() { (side, data) } else { (1, &[0.0f32][..]) };
    cube_texture(gpu, "mono table", wgpu::TextureFormat::R32Float, side, data)
}

/// One of a spectral stock's tables as a 3D texture of four-float entries, `side`³; a single zero entry when
/// there is no stock, so the binding is always there.
fn slide_texture(gpu: &Gpu, label: &str, data: &[f32]) -> wgpu::Texture {
    let side = (data.len() as f64 / 4.0).cbrt().round() as u32;
    let (side, data) = if side > 0 && (side as usize).pow(3) * 4 == data.len() { (side, data) } else { (1, &[0.0f32; 4][..]) };
    cube_texture(gpu, label, wgpu::TextureFormat::Rgba32Float, side, data)
}

fn cube_texture(gpu: &Gpu, label: &str, format: wgpu::TextureFormat, side: u32, data: &[f32]) -> wgpu::Texture {
    gpu.device.create_texture_with_data(
        &gpu.queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: side },
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
            size: wgpu::Extent3d { width: LUT_SIZE as u32, height: MAX_MASKS as u32, depth_or_array_layers: 1 },
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
        let module = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("develop"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
            ],
        });
        // Mirrors web/src/lib/gpu/develop.ts: the mask uniforms, a linear sampler, and a texture per slot.
        let texture = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let mut mask_entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
        ];
        mask_entries.extend((0..MAX_MASKS as u32).map(|i| texture(2 + i)));
        mask_entries.push(wgpu::BindGroupLayoutEntry {
            binding: 2 + MAX_MASKS as u32,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        });
        let mask_layout = gpu.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("develop masks"), entries: &mask_entries });
        let pipeline_layout = gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("develop"),
            bind_group_layouts: &[Some(&layout), Some(&mask_layout)],
            ..Default::default()
        });
        let pipeline = gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
        let no_mask = crate::upload_rgba16(gpu, &RgbaF32 { width: 1, height: 1, data: vec![0.0; 4] });
        let identity: Vec<f32> = (0..MAX_MASKS).flat_map(|_| (0..LUT_SIZE).map(|i| i as f32 / (LUT_SIZE - 1) as f32)).collect();
        let identity_curves = curves_texture(gpu, &identity);
        let store = B::Store(wgpu::TextureFormat::Rgba16Float);
        let density_down = Kernel::new(gpu, "client/density_down.wgsl", &[B::Uniform, B::Tex, store])?;
        let density_blur = Kernel::new(gpu, "client/density_blur.wgsl", &[B::Uniform, B::Tex, store])?;
        Ok(Self { pipeline, layout, mask_layout, mask_sampler, no_mask, density_down, density_blur, identity_curves })
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
        self.density_down.run(gpu, &mut enc, &[k, 0.0, 0.0, 0.0], [w, h], &[Res::View(&view(whole)), Res::View(&view(&small))]);
        gpu.queue.submit([enc.finish()]);
        small
    }

    /// The adjacency map for a frame `frame` pixels across, from `small` (`density_source`'s, at the map's size):
    /// the film's log exposure, as this develop sees it through the whole frame's masks, blurred over the
    /// developer's reach.
    pub fn density_map(&self, gpu: &Gpu, small: &wgpu::Texture, frame: [u32; 2], uniform: &[u8], lut: &[f32], masks: &Masks) -> Result<wgpu::Texture> {
        let mut own: DevelopParams = *bytemuck::from_bytes(uniform);
        own.mono[2] = 1.0;
        let mut whole = masks.params;
        whole.frame = [0.0; 4];
        let whole = Masks { params: whole, textures: masks.textures.clone(), curves: masks.curves, mono: masks.mono, density: None, ..Masks::none() };
        let exposure = self.render(gpu, small, bytemuck::bytes_of(&own), lut, &whole)?;
        let (w, h) = (small.width(), small.height());
        let (across, map) = (texture(gpu, w, h, wgpu::TextureFormat::Rgba16Float), texture(gpu, w, h, wgpu::TextureFormat::Rgba16Float));
        let sigma = sidecar_color::developer::map_sigma(own.developer[1], frame);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        self.density_blur.run(gpu, &mut enc, &[sigma, 1.0, 0.0, 0.0], [w, h], &[Res::View(&view(&exposure)), Res::View(&view(&across))]);
        self.density_blur.run(gpu, &mut enc, &[sigma, 0.0, 1.0, 0.0], [w, h], &[Res::View(&view(&across)), Res::View(&view(&map))]);
        gpu.queue.submit([enc.finish()]);
        Ok(map)
    }

    /// Develop a source texture into a new RGBA16F texture (display-linear).
    pub fn render(&self, gpu: &Gpu, src_tex: &wgpu::Texture, uniform: &[u8], lut: &[f32], masks: &Masks) -> Result<wgpu::Texture> {
        let expected = std::mem::size_of::<DevelopParams>();
        if uniform.len() != expected {
            bail!("develop uniform must be {expected} bytes, got {}", uniform.len());
        }
        // The whole frame makes its own adjacency map; a tile without one goes without its edges.
        let own_map = match (masks.density, needs_density_map(uniform) && masks.params.frame[2] == 0.0) {
            (None, true) => {
                let small = self.density_source(gpu, src_tex);
                Some(self.density_map(gpu, &small, [src_tex.width(), src_tex.height()], uniform, lut, masks)?)
            }
            _ => None,
        };
        let density = masks.density.or(own_map.as_ref()).unwrap_or(&self.no_mask);
        let (w, h) = (src_tex.width(), src_tex.height());
        let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
        let d = &gpu.device;
        let lut_tex = d.create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("tone lut"),
                size: wgpu::Extent3d { width: lut.len() as u32, height: 1, depth_or_array_layers: 1 },
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let mono_tex = mono_texture(gpu, masks.mono);
        let slide_exposures = slide_texture(gpu, "slide exposures", masks.slide_exposure);
        let slide_responses = slide_texture(gpu, "slide responses", masks.slide_response);
        let bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("develop"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubo.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&src_tex.create_view(&Default::default())),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&lut_tex.create_view(&Default::default())),
                },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&mono_tex.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&density.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&slide_exposures.create_view(&Default::default())) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&slide_responses.create_view(&Default::default())) },
            ],
        });
        let mask_ubo = d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("develop masks"),
            contents: bytemuck::bytes_of(&masks.params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let views: Vec<wgpu::TextureView> =
            (0..MAX_MASKS).map(|i| masks.textures.get(i).copied().flatten().unwrap_or(&self.no_mask).create_view(&Default::default())).collect();
        let mut entries = vec![
            wgpu::BindGroupEntry { binding: 0, resource: mask_ubo.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.mask_sampler) },
        ];
        entries.extend(views.iter().enumerate().map(|(i, v)| wgpu::BindGroupEntry { binding: 2 + i as u32, resource: wgpu::BindingResource::TextureView(v) }));
        let own_curves = (masks.curves.len() == MAX_MASKS * LUT_SIZE).then(|| curves_texture(gpu, masks.curves));
        let curves_view = own_curves.as_ref().unwrap_or(&self.identity_curves).create_view(&Default::default());
        entries.push(wgpu::BindGroupEntry { binding: 2 + MAX_MASKS as u32, resource: wgpu::BindingResource::TextureView(&curves_view) });
        let mask_bind = d.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("develop masks"), layout: &self.mask_layout, entries: &entries });
        let mut enc = d.create_command_encoder(&Default::default());
        {
            let out_view = out.create_view(&Default::default());
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("develop"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &out_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
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
        eprintln!("adapter: {} ({:?})", gpu.adapter_info.name, gpu.adapter_info.backend);
        let pass = DevelopPass::new(gpu).unwrap();
        // Identity-ish uniforms: unit WB, unit matrices, default tone.
        let mut u = vec![0u8; std::mem::size_of::<sidecar_color::params::DevelopParams>()];
        let put = |u: &mut Vec<u8>, off: usize, v: &[f32]| u[off..off + v.len() * 4].copy_from_slice(bytemuck::cast_slice(v));
        put(&mut u, 0, &[1.0, 1.0, 1.0, 1.0]);
        put(&mut u, 16, &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        put(&mut u, 64, &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        put(&mut u, 112, &[-10.0, 6.5, 7.0, 0.5]);
        let lut: Vec<f32> = (0..4096).map(|i| i as f32 / 4095.0).collect();
        let src = RgbaF32 { width: 4, height: 2, data: [0.18f32, 0.18, 0.18, 0.0].repeat(8) };
        let out = pass.run(gpu, &src, &u, &lut).unwrap();
        assert_eq!(out.data.len(), 32);
        assert!(out.data[0] > 0.01 && out.data[0] < 1.0, "{:?}", &out.data[..4]);
    }

    /// Identity colour, default tone, black and white on panchromatic film with a developer of the given effects.
    fn mono_uniform(developer: [f32; 4]) -> Vec<u8> {
        let mut p: DevelopParams = bytemuck::Zeroable::zeroed();
        p.wb_mul = [1.0; 3];
        p.exposure = 1.0;
        p.cam_to_work = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
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
        let Ok(gpu) = crate::test_gpu() else { return eprintln!("skipping: no GPU") };
        let pass = DevelopPass::new(gpu).unwrap();
        let (w, h) = (2048u32, 64u32);
        let src = RgbaF32 { width: w, height: h, data: (0..w * h).flat_map(|i| { let v = if i % w < w / 2 { 0.03 } else { 0.4 }; [v, v, v, 0.0] }).collect() };
        let lut: Vec<f32> = (0..4096).map(|i| i as f32 / 4095.0).collect();
        let table = sidecar_color::spectral::mono_lut("panchromatic", "none", &sidecar_color::spectral::light("daylight", 6500.0).unwrap()).unwrap();
        let tex = crate::upload_rgba16(gpu, &src);
        let masks = Masks { mono: &table, ..Masks::none() };
        let at = |img: &RgbaF32, x: u32| img.data[((32 * img.width + x) * 4) as usize];
        // Reach: 0.1 mm on 36 mm, so about 5.7 pixels here; the edge at 1024.
        let standard = mono_uniform([0.0, 0.1 / 36.0, 1.0, 0.0]);
        let flat = crate::read_rgba16(gpu, &pass.render(gpu, &tex, &standard, &lut, &masks).unwrap()).unwrap();
        assert_eq!(at(&flat, 1030), at(&flat, 1900));
        assert_eq!(at(&flat, 1018), at(&flat, 100));
        let sharp = mono_uniform([0.35, 0.1 / 36.0, 1.0, 0.0]);
        let whole = crate::read_rgba16(gpu, &pass.render(gpu, &tex, &sharp, &lut, &masks).unwrap()).unwrap();
        assert!(at(&whole, 1027) > at(&whole, 1900) * 1.02, "bright side {} interior {}", at(&whole, 1027), at(&whole, 1900));
        assert!(at(&whole, 1021) < at(&whole, 100) * 0.98, "dark side {} interior {}", at(&whole, 1021), at(&whole, 100));
        eprintln!("edge: bright {:.4} → {:.4}, dark {:.4} → {:.4}", at(&whole, 1900), at(&whole, 1027), at(&whole, 100), at(&whole, 1021));
        assert!((at(&whole, 1900) - at(&flat, 1900)).abs() < 2e-3, "interiors as they were");
        // A tile at 960, 128 wide, its place in the frame and the whole frame's map given.
        let map = pass.density_map(gpu, &pass.density_source(gpu, &tex), [w, h], &sharp, &lut, &masks).unwrap();
        let cut = RgbaF32 { width: 128, height: h, data: (0..h).flat_map(|y| src.data[((y * w + 960) * 4) as usize..((y * w + 1088) * 4) as usize].to_vec()).collect() };
        let mut placed = masks.params;
        placed.frame = [960.0, 0.0, w as f32, h as f32];
        let tile_masks = Masks { params: placed, density: Some(&map), ..Masks { mono: &table, ..Masks::none() } };
        let tile = crate::read_rgba16(gpu, &pass.render(gpu, &crate::upload_rgba16(gpu, &cut), &sharp, &lut, &tile_masks).unwrap()).unwrap();
        let worst = (0..128).map(|x| (at(&tile, x) - at(&whole, 960 + x)).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-3, "worst difference {worst}");
    }

    /// The develop pass renders a spectral colour stock as `slide::Slide` does on the CPU: the same two tables,
    /// read the same way. Stand-in standards, so these are not real PROVIA 100F colours — the chain is what is
    /// being checked. At amount 0 the pass is the neutral rendering, bit for bit.
    #[test]
    fn a_spectral_stock_develops_as_its_chain_does() {
        use sidecar_color::params::DevelopParams;
        let Ok(gpu) = crate::test_gpu() else { return eprintln!("skipping: no GPU") };
        let pass = DevelopPass::new(gpu).unwrap();
        let stock = sidecar_color::slide::slide("provia-100f").unwrap();
        let (meter, light) = sidecar_color::slide::stand_in_standards();
        // 33³ keeps the table's own interpolation error well under what is being measured here.
        let profile = stock.profile_sized(&meter, &light, 33);
        let exposures = stock.exposure_lut(&light);
        let colours: [[f32; 3]; 5] = [[0.18, 0.18, 0.18], [0.04, 0.04, 0.04], [0.6, 0.6, 0.6], [0.45, 0.12, 0.1], [0.1, 0.2, 0.5]];
        let src = RgbaF32 {
            width: colours.len() as u32,
            height: 1,
            data: colours.iter().flat_map(|c| [c[0], c[1], c[2], 0.0]).collect(),
        };
        let mut p: DevelopParams = bytemuck::Zeroable::zeroed();
        p.wb_mul = [1.0; 3];
        p.exposure = 1.0;
        p.cam_to_work = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        p.work_to_disp = p.cam_to_work;
        p.tone = [-10.0, 6.5, 7.0, 0.5];
        p.slide = [1.0, sidecar_color::spectral::GRID as f32, profile.side as f32, 0.0];
        p.slide_log_h = [profile.offsets[0], profile.offsets[1], profile.offsets[2], profile.scale];
        let lut: Vec<f32> = (0..4096).map(|i| i as f32 / 4095.0).collect();
        let tex = crate::upload_rgba16(gpu, &src);
        let masks = Masks { slide_exposure: &exposures, slide_response: &profile.response, ..Masks::none() };
        let on = bytemuck::bytes_of(&p).to_vec();
        let out = crate::read_rgba16(gpu, &pass.render(gpu, &tex, &on, &lut, &masks).unwrap()).unwrap();
        for (i, c) in colours.iter().enumerate() {
            let want = stock.render(c.map(|v| v as f64), &light, &meter, &light);
            let got = [out.data[i * 4], out.data[i * 4 + 1], out.data[i * 4 + 2]];
            eprintln!("{c:?} → gpu {got:?}, chain {want:?}");
            for k in 0..3 {
                // In stops, so a dark slide is held to the same relative accuracy as a bright one. rgba16float
                // output and the table's 33³ lattice are what the margin is for.
                let off = (got[k].max(1e-6) as f64 / want[k].max(1e-6)).log2().abs();
                assert!(off < 0.06, "{c:?} channel {k}: gpu {} chain {} ({off:+.3} stop)", got[k], want[k]);
            }
        }
        // Amount 0 is the neutral rendering, untouched.
        let mut off_params = p;
        off_params.slide = [0.0; 4];
        let off = crate::read_rgba16(gpu, &pass.render(gpu, &tex, bytemuck::bytes_of(&off_params), &lut, &masks).unwrap()).unwrap();
        let plain = crate::read_rgba16(gpu, &pass.render(gpu, &tex, bytemuck::bytes_of(&off_params), &lut, &Masks::none()).unwrap()).unwrap();
        assert_eq!(off.data, plain.data, "a stock at amount 0 renders as if it were not there");
    }
}
