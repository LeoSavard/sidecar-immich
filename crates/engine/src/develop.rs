// SPDX-License-Identifier: Apache-2.0
//! The develop pass: source (camera-native linear RGBA16F) + uniforms + tone LUT → display-linear RGBA16F.
//! Bind group layout mirrors web/src/lib/gpu/develop.ts.

use anyhow::{Result, bail};
use wgpu::util::DeviceExt;

use crate::{Gpu, RgbaF32};

pub struct DevelopPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
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
            ],
        });
        let pipeline_layout = gpu.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("develop"),
            bind_group_layouts: &[Some(&layout)],
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
        Ok(Self { pipeline, layout })
    }

    /// Run the develop pass once and read the result back.
    pub fn run(&self, gpu: &Gpu, source: &RgbaF32, uniform: &[u8], lut: &[f32]) -> Result<RgbaF32> {
        let src_tex = crate::upload_rgba16(gpu, source);
        let out = self.render(gpu, &src_tex, uniform, lut)?;
        crate::read_rgba16(gpu, &out)
    }

    /// Develop a source texture into a new RGBA16F texture (display-linear).
    pub fn render(&self, gpu: &Gpu, src_tex: &wgpu::Texture, uniform: &[u8], lut: &[f32]) -> Result<wgpu::Texture> {
        if uniform.len() != 272 {
            bail!("develop uniform must be 272 bytes, got {}", uniform.len());
        }
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
            ],
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
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind, &[]);
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
        let gpu = match Gpu::new_headless() {
            Ok(g) => g,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        eprintln!("adapter: {} ({:?})", gpu.adapter_info.name, gpu.adapter_info.backend);
        let pass = DevelopPass::new(&gpu).unwrap();
        // Identity-ish uniforms: unit WB, unit matrices, default tone.
        let mut u = vec![0u8; 272];
        let put = |u: &mut Vec<u8>, off: usize, v: &[f32]| u[off..off + v.len() * 4].copy_from_slice(bytemuck::cast_slice(v));
        put(&mut u, 0, &[1.0, 1.0, 1.0, 1.0]);
        put(&mut u, 16, &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        put(&mut u, 64, &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        put(&mut u, 112, &[-10.0, 6.5, 7.0, 0.5]);
        let lut: Vec<f32> = (0..4096).map(|i| i as f32 / 4095.0).collect();
        let src = RgbaF32 { width: 4, height: 2, data: [0.18f32, 0.18, 0.18, 0.0].repeat(8) };
        let out = pass.run(&gpu, &src, &u, &lut).unwrap();
        assert_eq!(out.data.len(), 32);
        assert!(out.data[0] > 0.01 && out.data[0] < 1.0, "{:?}", &out.data[..4]);
    }
}
