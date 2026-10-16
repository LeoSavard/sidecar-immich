// SPDX-License-Identifier: Apache-2.0
//! Brush strokes on masks (DESIGN.md §15), with the browser's shaders: `client/brush.wgsl` draws a stroke's
//! coverage, `client/brush_composite.wgsl` paints or erases it onto the mask, and `client/lens_mask.wgsl`
//! moves the finished mask through the lens correction. Masks are one channel, `r8unorm`.

use anyhow::Result;
use sidecar_color::lens::LensParams;
use sidecar_color::params::Stroke;
use wgpu::util::DeviceExt;

use crate::Gpu;

const MASK: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// A stroke in a target's pixels.
pub struct StrokePx {
    pub erase: bool,
    pub radius: f32,
    pub feather: f32,
    pub flow: f32,
    /// Segments, both ends: `[ax, ay, bx, by]`. A single point is a segment from itself to itself.
    pub segments: Vec<[f32; 4]>,
}

/// `stroke` in a target of `size` pixels; `place` takes its points (fractions of the photo as shown) to
/// fractions of the target — the identity, or the way back to the sensor's orientation.
pub fn stroke_px(stroke: &Stroke, size: [u32; 2], place: impl Fn([f64; 2]) -> [f64; 2]) -> StrokePx {
    let px = |p: [f64; 2]| {
        let q = place(p);
        [(q[0] * size[0] as f64) as f32, (q[1] * size[1] as f64) as f32]
    };
    let points: Vec<[f32; 2]> = stroke.points.iter().map(|&p| px(p)).collect();
    let segments = match points.as_slice() {
        [] => Vec::new(),
        [only] => vec![[only[0], only[1], only[0], only[1]]],
        many => many.windows(2).map(|w| [w[0][0], w[0][1], w[1][0], w[1][1]]).collect(),
    };
    StrokePx {
        erase: stroke.erase,
        radius: (stroke.radius.max(0.0) * size[0].max(size[1]) as f64) as f32,
        feather: stroke.feather.clamp(0.0, 1.0) as f32,
        flow: stroke.flow.clamp(0.0, 1.0) as f32,
        segments,
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BrushParams {
    size: [f32; 2],
    radius: f32,
    feather: f32,
    flow: f32,
    _pad: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CompositeParams {
    size: [f32; 2],
    mode: u32,
    _pad: u32,
}

pub struct Brush {
    stroke: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    lens: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    /// What "no base" and "no coverage" read: 1×1, zero.
    empty: wgpu::Texture,
}

fn pipeline(gpu: &Gpu, entry: &str, blend: Option<wgpu::BlendState>, buffers: &[Option<wgpu::VertexBufferLayout>]) -> Result<wgpu::RenderPipeline> {
    let src = sidecar_shaders::compose(entry)?;
    let module = gpu.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(entry), source: wgpu::ShaderSource::Wgsl(src.into()) });
    Ok(gpu.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry),
        layout: None,
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers, compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState { format: MASK, blend, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    }))
}

fn mask_texture(gpu: &Gpu, size: [u32; 2]) -> wgpu::Texture {
    gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mask"),
        size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: MASK,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

impl Brush {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        let max = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Max };
        let segment = wgpu::VertexBufferLayout {
            array_stride: 16,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 0 }],
        };
        let stroke = pipeline(gpu, "client/brush.wgsl", Some(wgpu::BlendState { color: max, alpha: max }), &[Some(segment)])?;
        let composite = pipeline(gpu, "client/brush_composite.wgsl", None, &[])?;
        let lens = pipeline(gpu, "client/lens_mask.wgsl", None, &[])?;
        let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let empty = mask_texture(gpu, [1, 1]);
        Ok(Self { stroke, composite, lens, sampler, empty })
    }

    /// A full-screen triangle, or with `segments` a capsule quad per segment.
    fn draw(&self, enc: &mut wgpu::CommandEncoder, target: &wgpu::Texture, clear: bool, pipeline: &wgpu::RenderPipeline, bind: &wgpu::BindGroup, segments: Option<(&wgpu::Buffer, u32)>) {
        let view = target.create_view(&Default::default());
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: if clear { wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT) } else { wgpu::LoadOp::Load }, store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind, &[]);
        match segments {
            Some((buffer, n)) => {
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..6, 0..n);
            }
            None => pass.draw(0..3, 0..1),
        }
    }

    fn composite(&self, gpu: &Gpu, enc: &mut wgpu::CommandEncoder, target: &wgpu::Texture, base: &wgpu::Texture, coverage: &wgpu::Texture, mode: u32) {
        let params = CompositeParams { size: [target.width() as f32, target.height() as f32], mode, _pad: 0 };
        let ubo = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(&params), usage: wgpu::BufferUsages::UNIFORM });
        let (bv, cv) = (base.create_view(&Default::default()), coverage.create_view(&Default::default()));
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.composite.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubo.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&bv) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&cv) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.draw(enc, target, false, &self.composite, &bind, None);
    }

    /// A mask of `size`: `base` (the models' mask, any size; `None` starts empty), then each stroke in order.
    pub fn paint(&self, gpu: &Gpu, base: Option<&wgpu::Texture>, size: [u32; 2], strokes: &[StrokePx]) -> wgpu::Texture {
        let (mut current, mut spare) = (mask_texture(gpu, size), mask_texture(gpu, size));
        let coverage = mask_texture(gpu, size);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        self.composite(gpu, &mut enc, &current, base.unwrap_or(&self.empty), &self.empty, 2);
        for s in strokes.iter().filter(|s| !s.segments.is_empty()) {
            let params = BrushParams { size: [size[0] as f32, size[1] as f32], radius: s.radius, feather: s.feather, flow: s.flow, _pad: [0.0; 3] };
            let ubo = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(&params), usage: wgpu::BufferUsages::UNIFORM });
            let segments = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&s.segments), usage: wgpu::BufferUsages::VERTEX });
            let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.stroke.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubo.as_entire_binding() }],
            });
            self.draw(&mut enc, &coverage, true, &self.stroke, &bind, Some((&segments, s.segments.len() as u32)));
            self.composite(gpu, &mut enc, &spare, &current, &coverage, if s.erase { 1 } else { 0 });
            std::mem::swap(&mut current, &mut spare);
        }
        gpu.queue.submit([enc.finish()]);
        current
    }

    /// A mask moved through the lens correction (`params` packed for its own size), as a new mask.
    pub fn correct(&self, gpu: &Gpu, src: &wgpu::Texture, params: &LensParams) -> wgpu::Texture {
        let out = mask_texture(gpu, [src.width(), src.height()]);
        let ubo = gpu.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(params), usage: wgpu::BufferUsages::UNIFORM });
        let sv = src.create_view(&Default::default());
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.lens.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubo.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&sv) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        self.draw(&mut enc, &out, true, &self.lens, &bind, None);
        gpu.queue.submit([enc.finish()]);
        out
    }
}

/// An `r8unorm` texture's values, 0…1 (tests and tools).
pub fn read_mask(gpu: &Gpu, tex: &wgpu::Texture) -> Result<Vec<f32>> {
    let (w, h) = (tex.width(), tex.height());
    let row = w.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
    rx.recv()??;
    let data = slice.get_mapped_range()?;
    Ok((0..h).flat_map(|y| data[(y * row) as usize..(y * row + w) as usize].iter().map(|&v| v as f32 / 255.0).collect::<Vec<_>>()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hard stroke paints its capsule and nothing else; an erase stroke over half of it takes that half
    /// away; a base mask shows through where nothing painted.
    #[test]
    fn strokes_paint_and_erase() {
        let Ok(gpu) = crate::test_gpu() else { return eprintln!("skipping: no GPU") };
        let brush = Brush::new(gpu).unwrap();
        let size = [100u32, 50u32];
        let paint = Stroke { radius: 0.05, feather: 0.0, flow: 1.0, points: vec![[0.2, 0.5], [0.8, 0.5]], ..Default::default() };
        let erase = Stroke { erase: true, radius: 0.1, feather: 0.0, flow: 1.0, points: vec![[0.75, 0.5]] };
        let strokes: Vec<StrokePx> = [paint, erase].iter().map(|s| stroke_px(s, size, |p| p)).collect();
        assert_eq!(strokes[0].radius, 5.0);
        let mut base = crate::RgbaF32 { width: 10, height: 10, data: vec![0.0; 400] };
        base.data[0] = 1.0;
        let base_tex = crate::upload_rgba16(gpu, &base);
        let mask = read_mask(gpu, &brush.paint(gpu, Some(&base_tex), size, &strokes)).unwrap();
        let at = |x: u32, y: u32| mask[(y * size[0] + x) as usize];
        assert!(at(50, 25) > 0.99, "on the stroke");
        assert!(at(50, 31) < 0.01, "past its radius");
        assert!(at(16, 25) > 0.99 && at(14, 25) < 0.01, "a round cap 5 px past the first point");
        assert!(at(75, 25) < 0.01, "erased");
        assert!(at(0, 0) > 0.6, "the base where nothing was painted");
    }

    /// The mask lens pass moves a mask as the picture's lens pass moves the picture's green channel.
    #[test]
    fn a_mask_moves_with_the_lens_correction() {
        let Ok(gpu) = crate::test_gpu() else { return eprintln!("skipping: no GPU") };
        let brush = Brush::new(gpu).unwrap();
        let lens = crate::lens::LensPass::new(gpu).unwrap();
        let profile = sidecar_color::lens::LensProfile {
            id: "t".into(),
            label: "t".into(),
            center: [0.5, 0.5],
            distortion: Some(sidecar_color::lens::Curve(vec![[0.0, 1.0], [1.0, 0.9]])),
            ca_red: None,
            ca_blue: None,
            vignetting: None,
            required: false,
        };
        let (w, h) = (120u32, 80u32);
        let e = sidecar_color::lens::effective(&profile, &Default::default(), 1.5);
        let params = sidecar_color::lens::pack(&e, [0.5, 0.5], [w, h], 0, [0.0; 2], [0.0; 2]);
        let img = crate::RgbaF32 { width: w, height: h, data: (0..w * h).flat_map(|i| { let v = if (i % w) / 10 % 2 == 0 { 1.0 } else { 0.0 }; [v, v, v, 0.0] }).collect() };
        let picture = crate::read_rgba16(gpu, &lens.apply(gpu, &crate::upload_rgba16(gpu, &img), &params, [w, h])).unwrap();
        let mask = read_mask(gpu, &brush.correct(gpu, &crate::upload_rgba16(gpu, &img), &params)).unwrap();
        let worst = (0..(w * h) as usize).map(|i| (picture.data[i * 4 + 1] - mask[i]).abs()).fold(0.0, f32::max);
        let mean = (0..(w * h) as usize).map(|i| (picture.data[i * 4 + 1] - mask[i]).abs()).sum::<f32>() / (w * h) as f32;
        // Bilinear against bicubic on hard stripes: the edges differ by a fraction, the rest not at all.
        assert!(mean < 0.03 && worst < 0.6, "mean {mean}, worst {worst}");
    }
}
