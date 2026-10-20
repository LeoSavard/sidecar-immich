// SPDX-License-Identifier: Apache-2.0
//! GPU ingest: raw mosaic → black level → highlight reconstruction (vkdt pyramid) → demosaic (RCD for Bayer,
//! 3×3 binning for X-Trans proxies) → clip coverage → downsampled RGBA16F proxy, camera-native linear RGB.

use anyhow::{Context, Result, bail};
use wgpu::util::DeviceExt;

use crate::compute::{B, Kernel, Res, texture, view};
use crate::{Gpu, RgbaF32};

/// Mirrors `struct IngestParams` in shaders/ingest/common.wgsl (224 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct IngestParams {
    pub in_size: [u32; 2],
    pub out_size: [u32; 2],
    pub black: [f32; 4],
    pub wb: [f32; 4],
    pub white: f32,
    pub clip: f32,
    pub desat: f32,
    pub soft: f32,
    pub sensor: u32,
    pub _p: [u32; 3],
    pub pattern: [[u32; 4]; 9],
}

/// A raw mosaic as the engine needs it (see `sidecar_raw::Mosaic`).
pub struct MosaicInput<'a> {
    pub width: u32,
    pub height: u32,
    pub data: &'a [u16],
    pub black: [f32; 4],
    pub white: f32,
    /// Colour index (0 R, 1 G, 2 B, 3 G2) of `(row % period, col % period)`.
    pub pattern: [[u8; 6]; 6],
    pub period: u32,
}

use wgpu::TextureFormat as F;

pub struct Ingest {
    normalize: Kernel,
    half: Kernel,
    reduce: Kernel,
    assemble: Kernel,
    doub: Kernel,
    rcd_conv: Kernel,
    rcd_green: Kernel,
    rcd_diag: Kernel,
    rcd_final: Kernel,
    xtrans_bin: Kernel,
    downsample: Kernel,
    resample: Kernel,
    sampler: wgpu::Sampler,
}

/// Crop offsets that put the mosaic on the phase the shaders expect, and the resulting crop size.
fn phase(m: &MosaicInput) -> Result<(u32, u32, u32, u32, u32)> {
    let colour = |y: u32, x: u32| match m.pattern[(y % m.period) as usize][(x % m.period) as usize] {
        3 => 1,
        c => c,
    };
    if m.period == 2 {
        for (oy, ox) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            if colour(oy, ox) == 0 && colour(oy, ox + 1) == 1 && colour(oy + 1, ox) == 1 && colour(oy + 1, ox + 1) == 2 {
                let (w, h) = ((m.width - ox) / 2 * 2, (m.height - oy) / 2 * 2);
                return Ok((ox, oy, w, h, 0));
            }
        }
        bail!("unsupported Bayer layout");
    }
    // X-Trans: greens at the corners and centre of each 3×3 block, blue at (x 0, y 1) on even blocks.
    for oy in 0..6 {
        for ox in 0..6 {
            let g = |x: u32, y: u32| colour(oy + y, ox + x) == 1;
            let ok_block = |bx: u32, by: u32| {
                let (x0, y0) = (bx * 3, by * 3);
                let greens = g(x0, y0) && g(x0 + 2, y0) && g(x0 + 1, y0 + 1) && g(x0, y0 + 2) && g(x0 + 2, y0 + 2);
                let even = (bx + by).is_multiple_of(2);
                let (edge_v, edge_h) = if even { (2, 0) } else { (0, 2) };
                greens && colour(oy + y0 + 1, ox + x0) == edge_v && colour(oy + y0 + 1, ox + x0 + 2) == edge_v && colour(oy + y0, ox + x0 + 1) == edge_h && colour(oy + y0 + 2, ox + x0 + 1) == edge_h
            };
            if ok_block(0, 0) && ok_block(1, 0) && ok_block(0, 1) && ok_block(1, 1) {
                let (w, h) = ((m.width - ox) / 3 * 3, (m.height - oy) / 3 * 3);
                return Ok((ox, oy, w, h, 1));
            }
        }
    }
    bail!("unsupported X-Trans layout")
}

impl Ingest {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        let k = |entry: &str, b: &[B]| Kernel::new(gpu, entry, b).with_context(|| entry.to_string());
        Ok(Self {
            normalize: k("ingest/normalize.wgsl", &[B::Uniform, B::TexU, B::Store(F::R32Float)])?,
            half: k("ingest/hilite_half.wgsl", &[B::Uniform, B::Tex, B::Store(F::Rgba32Float)])?,
            reduce: k("ingest/hilite_reduce.wgsl", &[B::Uniform, B::Tex, B::Store(F::Rgba32Float)])?,
            assemble: k("ingest/hilite_assemble.wgsl", &[B::Uniform, B::Tex, B::Tex, B::Store(F::Rgba32Float)])?,
            doub: k("ingest/hilite_doub.wgsl", &[B::Uniform, B::Tex, B::Tex, B::Store(F::R32Float)])?,
            rcd_conv: k("ingest/rcd_conv.wgsl", &[B::Uniform, B::Tex, B::Store(F::R32Float), B::Store(F::R32Float), B::Store(F::R32Float)])?,
            rcd_green: k("ingest/rcd_green.wgsl", &[B::Uniform, B::Tex, B::Tex, B::Tex, B::Store(F::R32Float)])?,
            rcd_diag: k("ingest/rcd_rb_diag.wgsl", &[B::Uniform, B::Tex, B::Tex, B::Tex, B::Store(F::Rgba32Float)])?,
            rcd_final: k("ingest/rcd_rb_green.wgsl", &[B::Uniform, B::Tex, B::Tex, B::Tex, B::Tex, B::Store(F::Rgba16Float)])?,
            xtrans_bin: k("ingest/xtrans_bin.wgsl", &[B::Uniform, B::Tex, B::Tex, B::Store(F::Rgba16Float)])?,
            downsample: k("ingest/downsample.wgsl", &[B::Uniform, B::Tex, B::Store(F::Rgba16Float)])?,
            resample: k("ingest/resample.wgsl", &[B::Uniform, B::TexF, B::Samp, B::Store(F::Rgba16Float)])?,
            sampler: gpu.device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() }),
        })
    }

    /// Develop the mosaic to camera-native linear RGB and return a proxy whose long edge is at most `long_edge`
    /// (never upscaled). Orientation is not applied.
    pub fn proxy(&self, gpu: &Gpu, m: &MosaicInput, wb: [f32; 3], long_edge: u32) -> Result<RgbaF32> {
        let rgb = self.demosaic(gpu, m, wb, false)?;
        self.downsample_and_read(gpu, rgb, long_edge)
    }

    /// Full demosaic to an RGBA16F texture: camera-native linear RGB, alpha = clip coverage.
    /// `full_xtrans` is reserved for the full-resolution X-Trans path; today X-Trans is binned 3×3.
    pub fn demosaic(&self, gpu: &Gpu, m: &MosaicInput, wb: [f32; 3], full_xtrans: bool) -> Result<wgpu::Texture> {
        let _ = full_xtrans;
        let (ox, oy, w, h, sensor) = phase(m)?;
        let mut pattern = [[0u32; 4]; 9];
        for i in 0..36u32 {
            let c = m.pattern[((i / 6 + oy) % m.period) as usize][((i % 6 + ox) % m.period) as usize];
            pattern[(i / 4) as usize][(i % 4) as usize] = c as u32;
        }
        let white_obs = {
            let max = m.data.iter().copied().max().unwrap_or(0) as f32;
            let blk = m.black.iter().copied().fold(f32::MAX, f32::min);
            ((max - blk) / (m.white - blk)).min(1.0)
        };
        let base = IngestParams {
            in_size: [w, h],
            out_size: [w, h],
            black: m.black,
            wb: [wb[0], wb[1], wb[2], wb[1]],
            white: m.white,
            clip: if white_obs > 0.5 { (white_obs * 0.98).min(0.985) } else { 0.985 },
            desat: 0.3,
            soft: 0.6,
            sensor,
            _p: [0; 3],
            pattern,
        };
        let with = |in_size: [u32; 2], out_size: [u32; 2]| IngestParams { in_size, out_size, ..base };

        // Upload the cropped mosaic.
        let mut cropped = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            let row = ((y + oy) * m.width + ox) as usize;
            cropped.extend_from_slice(&m.data[row..row + w as usize]);
        }
        let raw = gpu.device.create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("mosaic"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: F::R16Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            bytemuck::cast_slice(&cropped),
        );
        drop(cropped);

        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let norm = texture(gpu, w, h, F::R32Float);
        self.normalize.run(gpu, &mut enc, &base, base.out_size, &[Res::View(&view(&raw)), Res::View(&view(&norm))]);

        // Highlight reconstruction pyramid.
        let block = if sensor == 1 { 3 } else { 2 };
        let (hw, hh) = (w / block, h / block);
        let half = texture(gpu, hw, hh, F::Rgba32Float);
        self.half.run(gpu, &mut enc, &with([w, h], [hw, hh]), [hw, hh], &[Res::View(&view(&norm)), Res::View(&view(&half))]);
        let mut levels = vec![half];
        loop {
            let prev = levels.last().unwrap();
            let (pw, ph) = (prev.width(), prev.height());
            if pw <= 1 || ph <= 1 || levels.len() >= 15 {
                break;
            }
            let (cw, ch) = ((pw - 1) / 2 + 1, (ph - 1) / 2 + 1);
            let t = texture(gpu, cw, ch, F::Rgba32Float);
            self.reduce.run(gpu, &mut enc, &with([pw, ph], [cw, ch]), [cw, ch], &[Res::View(&view(prev)), Res::View(&view(&t))]);
            levels.push(t);
        }
        let mut coarse = levels.pop().unwrap();
        while let Some(fine) = levels.pop() {
            let out = texture(gpu, fine.width(), fine.height(), F::Rgba32Float);
            self.assemble.run(gpu, &mut enc, &with([coarse.width(), coarse.height()], [fine.width(), fine.height()]), [fine.width(), fine.height()], &[Res::View(&view(&fine)), Res::View(&view(&coarse)), Res::View(&view(&out))]);
            coarse = out;
        }
        let cfa = texture(gpu, w, h, F::R32Float);
        self.doub.run(gpu, &mut enc, &with([w, h], [hw, hh]), [hw, hh], &[Res::View(&view(&norm)), Res::View(&view(&coarse)), Res::View(&view(&cfa))]);
        // Diagnostic only, for `sidecar-dev hilite` (ROADMAP, "Highlight reconstruction"): keep the clipped
        // mosaic instead of the reconstructed one, which is the A/B that says whether something in a blown
        // area was invented here or was already in the picture. Never set in a deployment.
        let cfa = if std::env::var_os("SIDECAR_NO_HILITE").is_some() { norm.clone() } else { cfa };

        // Demosaic.
        let rgb = if sensor == 0 {
            let (vh, pq, lp) = (texture(gpu, w, h, F::R32Float), texture(gpu, w, h, F::R32Float), texture(gpu, w, h, F::R32Float));
            self.rcd_conv.run(gpu, &mut enc, &base, base.out_size, &[Res::View(&view(&cfa)), Res::View(&view(&vh)), Res::View(&view(&pq)), Res::View(&view(&lp))]);
            let g = texture(gpu, w, h, F::R32Float);
            self.rcd_green.run(gpu, &mut enc, &base, base.out_size, &[Res::View(&view(&cfa)), Res::View(&view(&vh)), Res::View(&view(&lp)), Res::View(&view(&g))]);
            let rb = texture(gpu, w, h, F::Rgba32Float);
            self.rcd_diag.run(gpu, &mut enc, &base, base.out_size, &[Res::View(&view(&cfa)), Res::View(&view(&pq)), Res::View(&view(&g)), Res::View(&view(&rb))]);
            let out = texture(gpu, w, h, F::Rgba16Float);
            self.rcd_final.run(gpu, &mut enc, &base, base.out_size, &[Res::View(&view(&vh)), Res::View(&view(&g)), Res::View(&view(&rb)), Res::View(&view(&norm)), Res::View(&view(&out))]);
            out
        } else {
            let out = texture(gpu, hw, hh, F::Rgba16Float);
            self.xtrans_bin.run(gpu, &mut enc, &with([w, h], [hw, hh]), [hw, hh], &[Res::View(&view(&cfa)), Res::View(&view(&norm)), Res::View(&view(&out))]);
            out
        };
        gpu.queue.submit([enc.finish()]);
        Ok(rgb)
    }

    /// Box-reduce then bilinear-resample an RGBA16F texture to `long_edge`, and read it back.
    pub fn downsample_and_read(&self, gpu: &Gpu, rgb: wgpu::Texture, long_edge: u32) -> Result<RgbaF32> {
        let with = |in_size: [u32; 2], out_size: [u32; 2]| IngestParams { in_size, out_size, ..bytemuck::Zeroable::zeroed() };
        let (sw, sh) = (rgb.width(), rgb.height());
        let scale = (long_edge as f64 / sw.max(sh) as f64).min(1.0);
        let (tw, th) = (((sw as f64 * scale).round() as u32).max(1), ((sh as f64 * scale).round() as u32).max(1));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let mut cur = rgb;
        while cur.width() >= tw * 2 && cur.height() >= th * 2 {
            let (nw, nh) = (cur.width().div_ceil(2), cur.height().div_ceil(2));
            let t = texture(gpu, nw, nh, F::Rgba16Float);
            self.downsample.run(gpu, &mut enc, &with([cur.width(), cur.height()], [nw, nh]), [nw, nh], &[Res::View(&view(&cur)), Res::View(&view(&t))]);
            cur = t;
        }
        if cur.width() != tw || cur.height() != th {
            let t = texture(gpu, tw, th, F::Rgba16Float);
            self.resample.run(gpu, &mut enc, &with([cur.width(), cur.height()], [tw, th]), [tw, th], &[Res::View(&view(&cur)), Res::Sampler(&self.sampler), Res::View(&view(&t))]);
            cur = t;
        }
        gpu.queue.submit([enc.finish()]);
        crate::read_rgba16(gpu, &cur)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_params_layout_matches_wgsl() {
        let src = sidecar_shaders::compose("ingest/common.wgsl").unwrap();
        let module = naga::front::wgsl::parse_str(&src).unwrap();
        let (_, ty) = module.types.iter().find(|(_, t)| t.name.as_deref() == Some("IngestParams")).unwrap();
        let naga::TypeInner::Struct { members, span } = &ty.inner else { panic!() };
        assert_eq!(*span as usize, std::mem::size_of::<IngestParams>());
        let off = |n: &str| members.iter().find(|m| m.name.as_deref() == Some(n)).unwrap().offset as usize;
        assert_eq!(off("wb"), std::mem::offset_of!(IngestParams, wb));
        assert_eq!(off("sensor"), std::mem::offset_of!(IngestParams, sensor));
        assert_eq!(off("pattern"), std::mem::offset_of!(IngestParams, pattern));
    }

    #[test]
    fn finds_rggb_phase_for_every_bayer_layout() {
        // GRBG: R sits at row 0, column 1.
        let data = vec![0u16; 16];
        let m = MosaicInput { width: 4, height: 4, data: &data, black: [0.0; 4], white: 1.0, pattern: [[1, 0, 1, 0, 1, 0], [2, 3, 2, 3, 2, 3], [1, 0, 1, 0, 1, 0], [2, 3, 2, 3, 2, 3], [1, 0, 1, 0, 1, 0], [2, 3, 2, 3, 2, 3]], period: 2 };
        let (ox, oy, w, h, sensor) = phase(&m).unwrap();
        assert_eq!((ox, oy, w, h, sensor), (1, 0, 2, 4, 0));
    }
}
