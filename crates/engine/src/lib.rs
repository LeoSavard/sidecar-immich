// SPDX-License-Identifier: Apache-2.0
//! GPU engine for the worker: a headless wgpu device (Vulkan on the server) and the develop pass, compiled
//! from the same composed WGSL the browser uses.

use anyhow::{Context, Result};

pub mod compute;
pub mod detail;
pub mod develop;
pub mod ingest;
pub use develop::DevelopPass;
pub use ingest::{Ingest, MosaicInput};

/// Linear RGBA image in f32, row-major, top-left origin.
#[derive(Clone, Debug)]
pub struct RgbaF32 {
    pub width: u32,
    pub height: u32,
    pub data: Vec<f32>,
}

fn area_weights(src: u32, dst: u32) -> Vec<Vec<(usize, f32)>> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|d| {
            let (a, b) = (d as f64 * scale, (d as f64 + 1.0) * scale);
            let mut w = Vec::new();
            let mut s = a.floor() as u32;
            while (s as f64) < b && s < src {
                let overlap = (b.min(s as f64 + 1.0) - a.max(s as f64)).max(0.0);
                if overlap > 0.0 {
                    w.push((s as usize, (overlap / scale) as f32));
                }
                s += 1;
            }
            w
        })
        .collect()
}

impl RgbaF32 {
    /// Area-filtered downsample so the long edge is at most `long_edge` (never upscales).
    pub fn downsampled(&self, long_edge: u32) -> RgbaF32 {
        let scale = (long_edge as f64 / self.width.max(self.height) as f64).min(1.0);
        if scale >= 1.0 {
            return self.clone();
        }
        let (dw, dh) = (((self.width as f64 * scale).round() as u32).max(1), ((self.height as f64 * scale).round() as u32).max(1));
        let (wx, wy) = (area_weights(self.width, dw), area_weights(self.height, dh));
        let sw = self.width as usize;
        let mut tmp = vec![0f32; dw as usize * self.height as usize * 4];
        for y in 0..self.height as usize {
            for (x, ws) in wx.iter().enumerate() {
                for &(s, w) in ws {
                    for c in 0..4 {
                        tmp[(y * dw as usize + x) * 4 + c] += self.data[(y * sw + s) * 4 + c] * w;
                    }
                }
            }
        }
        let mut data = vec![0f32; dw as usize * dh as usize * 4];
        for (y, ws) in wy.iter().enumerate() {
            for &(s, w) in ws {
                for x in 0..dw as usize {
                    for c in 0..4 {
                        data[(y * dw as usize + x) * 4 + c] += tmp[(s * dw as usize + x) * 4 + c] * w;
                    }
                }
            }
        }
        RgbaF32 { width: dw, height: dh, data }
    }

    /// Apply a LibRaw orientation: 3 = 180°, 5 = 90° counter-clockwise, 6 = 90° clockwise.
    pub fn oriented(self, flip: i32) -> RgbaF32 {
        let (w, h) = (self.width as usize, self.height as usize);
        let px = |x: usize, y: usize| &self.data[(y * w + x) * 4..(y * w + x) * 4 + 4];
        match flip {
            3 => {
                let mut data = Vec::with_capacity(self.data.len());
                for y in (0..h).rev() {
                    for x in (0..w).rev() {
                        data.extend_from_slice(px(x, y));
                    }
                }
                RgbaF32 { width: self.width, height: self.height, data }
            }
            5 | 6 => {
                let mut data = Vec::with_capacity(self.data.len());
                for ny in 0..w {
                    for nx in 0..h {
                        let (x, y) = if flip == 6 { (ny, h - 1 - nx) } else { (w - 1 - ny, nx) };
                        data.extend_from_slice(px(x, y));
                    }
                }
                RgbaF32 { width: self.height, height: self.width, data }
            }
            _ => self,
        }
    }

    pub fn to_f16_bytes(&self) -> Vec<u8> {
        let half: Vec<half::f16> = self.data.iter().map(|&v| half::f16::from_f32(v)).collect();
        bytemuck::cast_slice(&half).to_vec()
    }

    pub fn from_f16_bytes(width: u32, height: u32, bytes: &[u8]) -> Self {
        let half: &[half::f16] = bytemuck::cast_slice(bytes);
        Self { width, height, data: half.iter().map(|v| v.to_f32()).collect() }
    }
}

/// Upload linear RGBA as an RGBA16F texture.
pub fn upload_rgba16(gpu: &Gpu, img: &RgbaF32) -> wgpu::Texture {
    use wgpu::util::DeviceExt;
    gpu.device.create_texture_with_data(
        &gpu.queue,
        &wgpu::TextureDescriptor {
            label: Some("rgba16"),
            size: wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &img.to_f16_bytes(),
    )
}

/// Read an RGBA16F texture back to the CPU.
pub fn read_rgba16(gpu: &Gpu, tex: &wgpu::Texture) -> Result<RgbaF32> {
    let (w, h) = (tex.width(), tex.height());
    let bytes_per_row = (8 * w).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (bytes_per_row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    gpu.device.poll(wgpu::PollType::wait_indefinitely())?;
    rx.recv()??;
    let mapped = slice.get_mapped_range()?;
    let mut bytes = Vec::with_capacity((8 * w * h) as usize);
    for row in 0..h {
        let start = (row * bytes_per_row) as usize;
        bytes.extend_from_slice(&mapped[start..start + (8 * w) as usize]);
    }
    drop(mapped);
    buf.unmap();
    Ok(RgbaF32::from_f16_bytes(w, h, &bytes))
}

pub struct Gpu {
    pub adapter_info: wgpu::AdapterInfo,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl Gpu {
    pub fn new_headless() -> Result<Self> {
        pollster::block_on(async {
            let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
            desc.backends = wgpu::Backends::VULKAN | wgpu::Backends::METAL;
            let instance = wgpu::Instance::new(desc);
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    ..Default::default()
                })
                .await
                .context("no GPU adapter")?;
            let adapter_info = adapter.get_info();
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("sidecar"),
                    required_limits: wgpu::Limits { max_texture_dimension_2d: 16384, max_storage_textures_per_shader_stage: 8, max_buffer_size: u32::MAX as u64, ..wgpu::Limits::default() },
                    ..Default::default()
                })
                .await
                .context("requesting GPU device")?;
            Ok(Self { adapter_info, device, queue })
        })
    }
}
