// SPDX-License-Identifier: Apache-2.0
//! GPU engine for the worker: a headless wgpu device (Vulkan on the server) and the develop pass, compiled
//! from the same composed WGSL the browser uses.

use anyhow::{Context, Result};

pub mod compute;
pub mod detail;
pub mod develop;
pub mod focus;
pub mod ingest;
pub mod lens;
pub use develop::DevelopPass;
pub use focus::focus_energy;
pub use ingest::{Ingest, MosaicInput};

/// Linear RGBA image in f32, row-major, top-left origin.
#[derive(Clone, Debug)]
pub struct RgbaF32 {
    pub width: u32,
    pub height: u32,
    pub data: Vec<f32>,
}

pub(crate) fn area_weights(src: u32, dst: u32) -> Vec<Vec<(usize, f32)>> {
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

    /// Apply a LibRaw orientation. `flip` is a bit set — 4 transposes, 2 mirrors top to bottom, 1 mirrors
    /// left to right — so 3 is 180°, 5 is 90° counter-clockwise, 6 is 90° clockwise, and 1, 2, 4 and 7 are the
    /// mirrored EXIF orientations.
    pub fn oriented(self, flip: i32) -> RgbaF32 {
        let flip = flip & 7;
        if flip == 0 {
            return self;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let transpose = flip & 4 != 0;
        let (nw, nh) = if transpose { (h, w) } else { (w, h) };
        let mut data = Vec::with_capacity(self.data.len());
        for ny in 0..nh {
            for nx in 0..nw {
                let (mut x, mut y) = if transpose { (ny, nx) } else { (nx, ny) };
                if flip & 2 != 0 {
                    y = h - 1 - y;
                }
                if flip & 1 != 0 {
                    x = w - 1 - x;
                }
                let i = (y * w + x) * 4;
                data.extend_from_slice(&self.data[i..i + 4]);
            }
        }
        RgbaF32 { width: nw as u32, height: nh as u32, data }
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

/// Where a rectangle of the image `oriented(flip)` produces lies in the image before orientation, for a
/// source of `sw` × `sh`. Both are `(x, y, width, height)` in pixels.
pub fn source_rect(flip: i32, sw: u32, sh: u32, (x, y, w, h): (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    let (mut x, mut y, w, h) = if flip & 4 != 0 { (y, x, h, w) } else { (x, y, w, h) };
    if flip & 2 != 0 {
        y = sh - y - h;
    }
    if flip & 1 != 0 {
        x = sw - x - w;
    }
    (x, y, w, h)
}

/// Single-channel image in f32, row-major, top-left origin.
#[derive(Clone, Debug)]
pub struct Mono {
    pub width: u32,
    pub height: u32,
    pub data: Vec<f32>,
}

impl Mono {
    /// Max-pooled downsample: unlike `RgbaF32::downsampled`'s area-weighted average, the brightest sample in
    /// each destination cell survives instead of being blended away. Never upscales. See docs/ROADMAP.md,
    /// "Focus peaking measures the wrong thing" — a single sharp pixel has to survive.
    pub fn max_pooled(&self, long_edge: u32) -> Mono {
        let scale = (long_edge as f64 / self.width.max(self.height) as f64).min(1.0);
        if scale >= 1.0 {
            return self.clone();
        }
        let (dw, dh) = (((self.width as f64 * scale).round() as u32).max(1), ((self.height as f64 * scale).round() as u32).max(1));
        let (wx, wy) = (area_weights(self.width, dw), area_weights(self.height, dh));
        let sw = self.width as usize;
        let mut tmp = vec![f32::NEG_INFINITY; dw as usize * self.height as usize];
        for y in 0..self.height as usize {
            for (x, ws) in wx.iter().enumerate() {
                let slot = &mut tmp[y * dw as usize + x];
                for &(s, _) in ws {
                    *slot = slot.max(self.data[y * sw + s]);
                }
            }
        }
        let mut data = vec![f32::NEG_INFINITY; dw as usize * dh as usize];
        for (y, ws) in wy.iter().enumerate() {
            for x in 0..dw as usize {
                let slot = &mut data[y * dw as usize + x];
                for &(s, _) in ws {
                    *slot = slot.max(tmp[s * dw as usize + x]);
                }
            }
        }
        Mono { width: dw, height: dh, data }
    }

    /// Display-ready `r8unorm` bytes: clamp to 0…1 and round.
    pub fn to_r8_bytes(&self) -> Vec<u8> {
        self.data.iter().map(|&v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect()
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
            // What full-resolution work wants, clamped to what this adapter offers: software Vulkan (CI) caps
            // allocations at 2 GB and would otherwise refuse the device outright.
            let have = adapter.limits();
            let limits = wgpu::Limits {
                max_texture_dimension_2d: have.max_texture_dimension_2d.min(16384),
                max_storage_textures_per_shader_stage: have.max_storage_textures_per_shader_stage.min(8),
                max_buffer_size: have.max_buffer_size.min(u32::MAX as u64),
                ..wgpu::Limits::default()
            };
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor { label: Some("sidecar"), required_limits: limits, ..Default::default() })
                .await
                .context("requesting GPU device")?;
            Ok(Self { adapter_info, device, queue })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3 × 2 image whose every pixel is its own index, so any remap is visible.
    fn indexed() -> RgbaF32 {
        RgbaF32 { width: 3, height: 2, data: (0..6).flat_map(|i| [i as f32; 4]).collect() }
    }

    /// The area filter keeps the mean, and turns per-pixel clip flags in alpha into the fraction clipped.
    #[test]
    fn downsampling_averages_and_turns_clip_flags_into_coverage() {
        let img = RgbaF32 { width: 4, height: 2, data: [[1.0, 0.0, 0.0, 1.0], [0.5, 0.5, 0.5, 0.0]].concat().repeat(4) };
        let small = img.downsampled(2);
        assert_eq!((small.width, small.height), (2, 1));
        assert!((small.data[0] - 0.75).abs() < 1e-6 && (small.data[3] - 0.5).abs() < 1e-6, "{:?}", &small.data[..4]);
    }

    #[test]
    fn rotations_and_mirrors_land_where_exif_says() {
        let rows = |img: RgbaF32| -> Vec<Vec<u32>> {
            img.data.chunks(4 * img.width as usize).map(|r| r.chunks(4).map(|p| p[0] as u32).collect()).collect()
        };
        // 0 1 2
        // 3 4 5
        assert_eq!(rows(indexed().oriented(0)), [[0, 1, 2], [3, 4, 5]]);
        assert_eq!(rows(indexed().oriented(1)), [[2, 1, 0], [5, 4, 3]], "EXIF 2, mirrored left to right");
        assert_eq!(rows(indexed().oriented(3)), [[5, 4, 3], [2, 1, 0]], "180°");
        assert_eq!(rows(indexed().oriented(2)), [[3, 4, 5], [0, 1, 2]], "EXIF 4, mirrored top to bottom");
        assert_eq!(rows(indexed().oriented(6)), [[3, 0], [4, 1], [5, 2]], "90° clockwise");
        assert_eq!(rows(indexed().oriented(5)), [[2, 5], [1, 4], [0, 3]], "90° counter-clockwise");
        assert_eq!(rows(indexed().oriented(4)), [[0, 3], [1, 4], [2, 5]], "EXIF 5, transposed");
        assert_eq!(rows(indexed().oriented(7)), [[5, 2], [4, 1], [3, 0]], "EXIF 7, transversed");
    }

    /// The loupe cuts its tile out of the unoriented image with this, then orients it: the two must agree.
    #[test]
    fn source_rect_is_the_inverse_of_oriented() {
        for flip in 0..8 {
            let src = indexed();
            let out = src.clone().oriented(flip);
            for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy, sw, sh) = source_rect(flip, 3, 2, (x, y, 1, 1));
                assert_eq!((sw, sh), (1, 1));
                let want = out.data[((y * out.width + x) * 4) as usize];
                assert_eq!(src.data[((sy * 3 + sx) * 4) as usize], want, "flip {flip} at {x},{y}");
            }
        }
    }
}
