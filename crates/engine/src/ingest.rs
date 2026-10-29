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
    pub clip: [f32; 4],
    pub white: f32,
    pub desat: f32,
    pub _unused: f32,
    pub sensor: u32,
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
    /// Where each colour saturates, as a fraction of `white` above its own black: `Mosaic::clip`, measured
    /// from the samples; see [`clip_levels`].
    pub clip: [f32; 4],
}

use wgpu::TextureFormat as F;

/// The thresholds the shaders compare a normalized sample against, one per CFA colour: what the decode
/// measured, `Mosaic::clip`.
///
/// What it replaces, and what `SIDECAR_CLIP_LEGACY=1` brings back for comparison, is the heuristic the
/// pipeline used until 2026-10-28: *one* number for all four colours, 98 % of the brightest sample anywhere
/// in the mosaic, capped at 98.5 % (D6 and D7 in docs/research/pipeline-review-2026-10.md). A single sample
/// decided it, and both ways it was wrong. On `IMG_0735.CR2` one stuck green pixel at 83.27 % of the range
/// put the threshold at 0.8161, above every channel's real maximum, so a frame with 0.68 % of its samples at
/// saturation had exactly one pixel of its proxy flagged and highlight reconstruction did nothing. On
/// `IMG_1768.CR2`, which clips nowhere, the brightest sample reaches 61.70 % and the threshold was 0.6047.
///
/// Measured over 222 raws from seven formats and thirteen camera models, the measurement is stricter than
/// the heuristic in 822 of 827 channels and looser in 5, and those 5 are channels that genuinely saturate
/// early. `sidecar-dev levels` prints both.
/// Does any sample in the frame actually reach its own colour's saturation? Highlight reconstruction has
/// one job — estimate the channels a pixel lost from the one it kept — so a frame that lost nothing has
/// nothing for it to do, and the whole pyramid is skipped rather than run to produce a no-op.
///
/// Worth asking only because the thresholds are now measured. The heuristic this replaced set them at 98 %
/// of the brightest sample *present*, so the brightest sample always cleared its own threshold and every
/// frame had "clipped" pixels by construction — reconstruction ran on all of them, and on a frame with
/// nothing blown it replaced the real top of the picture with an estimate. Measured instead, **46 % of a
/// 217-file sample reach saturation nowhere at all** (`sidecar-dev levels`).
///
/// Stops at the first saturated sample, so a frame that does need the pass pays almost nothing to find out.
pub fn any_saturated(m: &MosaicInput, clip: [f32; 4]) -> bool {
    // In the samples' own units, so the inner loop is an integer compare: a frame is tens of millions of
    // them, and asking this must cost much less than the pass it saves.
    let level = |c: usize| -> u32 {
        let span = m.white - m.black[c];
        if span <= 0.0 {
            return u32::MAX;
        }
        (m.black[c] + clip[c] * span).ceil().max(0.0) as u32
    };
    let levels: [u32; 4] = std::array::from_fn(level);
    // One threshold per column for each of the pattern's distinct rows, so the period's arithmetic is paid
    // `period` times rather than once per sample.
    let rows: Vec<Vec<u32>> = (0..m.period as usize)
        .map(|py| {
            (0..m.width as usize)
                .map(|x| levels[m.pattern[py][x % m.period as usize] as usize])
                .collect()
        })
        .collect();
    m.data
        .chunks_exact(m.width as usize)
        .enumerate()
        .any(|(y, row)| {
            let want = &rows[y % m.period as usize];
            row.iter().zip(want).any(|(&v, &t)| v as u32 >= t)
        })
}

pub fn clip_levels(m: &MosaicInput) -> [f32; 4] {
    if std::env::var_os("SIDECAR_CLIP_LEGACY").is_none() {
        return m.clip;
    }
    let max = m.data.iter().copied().max().unwrap_or(0) as f32;
    let blk = m.black.iter().copied().fold(f32::MAX, f32::min);
    let white_obs = ((max - blk) / (m.white - blk)).min(1.0);
    [if white_obs > 0.5 {
        (white_obs * 0.98).min(0.985)
    } else {
        0.985
    }; 4]
}

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
    let colour = |y: u32, x: u32| match m.pattern[(y % m.period) as usize][(x % m.period) as usize]
    {
        3 => 1,
        c => c,
    };
    if m.period == 2 {
        for (oy, ox) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            if colour(oy, ox) == 0
                && colour(oy, ox + 1) == 1
                && colour(oy + 1, ox) == 1
                && colour(oy + 1, ox + 1) == 2
            {
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
                let greens = g(x0, y0)
                    && g(x0 + 2, y0)
                    && g(x0 + 1, y0 + 1)
                    && g(x0, y0 + 2)
                    && g(x0 + 2, y0 + 2);
                let even = (bx + by).is_multiple_of(2);
                let (edge_v, edge_h) = if even { (2, 0) } else { (0, 2) };
                greens
                    && colour(oy + y0 + 1, ox + x0) == edge_v
                    && colour(oy + y0 + 1, ox + x0 + 2) == edge_v
                    && colour(oy + y0, ox + x0 + 1) == edge_h
                    && colour(oy + y0 + 2, ox + x0 + 1) == edge_h
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
        let k =
            |entry: &str, b: &[B]| Kernel::new(gpu, entry, b).with_context(|| entry.to_string());
        Ok(Self {
            normalize: k(
                "ingest/normalize.wgsl",
                &[B::Uniform, B::TexU, B::Store(F::R32Float)],
            )?,
            half: k(
                "ingest/hilite_half.wgsl",
                &[B::Uniform, B::Tex, B::Store(F::Rgba32Float)],
            )?,
            reduce: k(
                "ingest/hilite_reduce.wgsl",
                &[B::Uniform, B::Tex, B::Store(F::Rgba32Float)],
            )?,
            assemble: k(
                "ingest/hilite_assemble.wgsl",
                &[B::Uniform, B::Tex, B::Tex, B::Store(F::Rgba32Float)],
            )?,
            doub: k(
                "ingest/hilite_doub.wgsl",
                &[B::Uniform, B::Tex, B::Tex, B::Store(F::R32Float)],
            )?,
            rcd_conv: k(
                "ingest/rcd_conv.wgsl",
                &[
                    B::Uniform,
                    B::Tex,
                    B::Store(F::R32Float),
                    B::Store(F::R32Float),
                    B::Store(F::R32Float),
                ],
            )?,
            rcd_green: k(
                "ingest/rcd_green.wgsl",
                &[B::Uniform, B::Tex, B::Tex, B::Tex, B::Store(F::R32Float)],
            )?,
            rcd_diag: k(
                "ingest/rcd_rb_diag.wgsl",
                &[B::Uniform, B::Tex, B::Tex, B::Tex, B::Store(F::Rgba32Float)],
            )?,
            rcd_final: k(
                "ingest/rcd_rb_green.wgsl",
                &[
                    B::Uniform,
                    B::Tex,
                    B::Tex,
                    B::Tex,
                    B::Tex,
                    B::Store(F::Rgba16Float),
                ],
            )?,
            xtrans_bin: k(
                "ingest/xtrans_bin.wgsl",
                &[B::Uniform, B::Tex, B::Tex, B::Store(F::Rgba16Float)],
            )?,
            downsample: k(
                "ingest/downsample.wgsl",
                &[B::Uniform, B::Tex, B::Store(F::Rgba16Float)],
            )?,
            resample: k(
                "ingest/resample.wgsl",
                &[B::Uniform, B::TexF, B::Samp, B::Store(F::Rgba16Float)],
            )?,
            sampler: gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        })
    }

    /// Develop the mosaic to camera-native linear RGB and return a proxy whose long edge is at most `long_edge`
    /// (never upscaled). Orientation is not applied.
    pub fn proxy(
        &self,
        gpu: &Gpu,
        m: &MosaicInput,
        wb: [f32; 3],
        long_edge: u32,
    ) -> Result<RgbaF32> {
        let rgb = self.demosaic(gpu, m, wb, false)?;
        self.downsample_and_read(gpu, rgb, long_edge)
    }

    /// Full demosaic to an RGBA16F texture: camera-native linear RGB, alpha = clip coverage.
    /// `full_xtrans` is reserved for the full-resolution X-Trans path; today X-Trans is binned 3×3.
    pub fn demosaic(
        &self,
        gpu: &Gpu,
        m: &MosaicInput,
        wb: [f32; 3],
        full_xtrans: bool,
    ) -> Result<wgpu::Texture> {
        self.demosaic_at(gpu, m, wb, full_xtrans, clip_levels(m))
    }

    /// [`Self::demosaic`] with the saturation thresholds given rather than chosen, one per CFA colour.
    /// [`clip_levels`] decides which the pipeline uses; tests pin them.
    pub fn demosaic_at(
        &self,
        gpu: &Gpu,
        m: &MosaicInput,
        wb: [f32; 3],
        full_xtrans: bool,
        clip: [f32; 4],
    ) -> Result<wgpu::Texture> {
        let _ = full_xtrans;
        let (ox, oy, w, h, sensor) = phase(m)?;
        let mut pattern = [[0u32; 4]; 9];
        for i in 0..36u32 {
            let c =
                m.pattern[((i / 6 + oy) % m.period) as usize][((i % 6 + ox) % m.period) as usize];
            pattern[(i / 4) as usize][(i % 4) as usize] = c as u32;
        }
        let base = IngestParams {
            in_size: [w, h],
            out_size: [w, h],
            black: m.black,
            wb: [wb[0], wb[1], wb[2], wb[1]],
            clip,
            white: m.white,
            desat: 0.3,
            _unused: 0.0,
            sensor,
            pattern,
        };
        let with = |in_size: [u32; 2], out_size: [u32; 2]| IngestParams {
            in_size,
            out_size,
            ..base
        };

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
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
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
        self.normalize.run(
            gpu,
            &mut enc,
            &base,
            base.out_size,
            &[Res::View(&view(&raw)), Res::View(&view(&norm))],
        );

        // Highlight reconstruction, for a frame that has something to reconstruct. `SIDECAR_NO_HILITE` is
        // diagnostic only, for `sidecar-dev hilite` (ROADMAP, "Highlight reconstruction"): it keeps the
        // clipped mosaic, which is the A/B that says whether something in a blown area was invented here or
        // was already in the picture. Never set in a deployment.
        let reconstruct = std::env::var_os("SIDECAR_NO_HILITE").is_none() && any_saturated(m, clip);
        let block = if sensor == 1 { 3 } else { 2 };
        let (hw, hh) = (w / block, h / block);
        let cfa = if !reconstruct {
            norm.clone()
        } else {
            let half = texture(gpu, hw, hh, F::Rgba32Float);
            self.half.run(
                gpu,
                &mut enc,
                &with([w, h], [hw, hh]),
                [hw, hh],
                &[Res::View(&view(&norm)), Res::View(&view(&half))],
            );
            let mut levels = vec![half];
            loop {
                let prev = levels.last().unwrap();
                let (pw, ph) = (prev.width(), prev.height());
                if pw <= 1 || ph <= 1 || levels.len() >= 15 {
                    break;
                }
                let (cw, ch) = ((pw - 1) / 2 + 1, (ph - 1) / 2 + 1);
                let t = texture(gpu, cw, ch, F::Rgba32Float);
                self.reduce.run(
                    gpu,
                    &mut enc,
                    &with([pw, ph], [cw, ch]),
                    [cw, ch],
                    &[Res::View(&view(prev)), Res::View(&view(&t))],
                );
                levels.push(t);
            }
            let mut coarse = levels.pop().unwrap();
            while let Some(fine) = levels.pop() {
                let out = texture(gpu, fine.width(), fine.height(), F::Rgba32Float);
                self.assemble.run(
                    gpu,
                    &mut enc,
                    &with(
                        [coarse.width(), coarse.height()],
                        [fine.width(), fine.height()],
                    ),
                    [fine.width(), fine.height()],
                    &[
                        Res::View(&view(&fine)),
                        Res::View(&view(&coarse)),
                        Res::View(&view(&out)),
                    ],
                );
                coarse = out;
            }
            let cfa = texture(gpu, w, h, F::R32Float);
            self.doub.run(
                gpu,
                &mut enc,
                &with([w, h], [hw, hh]),
                [hw, hh],
                &[
                    Res::View(&view(&norm)),
                    Res::View(&view(&coarse)),
                    Res::View(&view(&cfa)),
                ],
            );
            cfa
        };

        // Demosaic.
        let rgb = if sensor == 0 {
            let (vh, pq, lp) = (
                texture(gpu, w, h, F::R32Float),
                texture(gpu, w, h, F::R32Float),
                texture(gpu, w, h, F::R32Float),
            );
            self.rcd_conv.run(
                gpu,
                &mut enc,
                &base,
                base.out_size,
                &[
                    Res::View(&view(&cfa)),
                    Res::View(&view(&vh)),
                    Res::View(&view(&pq)),
                    Res::View(&view(&lp)),
                ],
            );
            let g = texture(gpu, w, h, F::R32Float);
            self.rcd_green.run(
                gpu,
                &mut enc,
                &base,
                base.out_size,
                &[
                    Res::View(&view(&cfa)),
                    Res::View(&view(&vh)),
                    Res::View(&view(&lp)),
                    Res::View(&view(&g)),
                ],
            );
            let rb = texture(gpu, w, h, F::Rgba32Float);
            self.rcd_diag.run(
                gpu,
                &mut enc,
                &base,
                base.out_size,
                &[
                    Res::View(&view(&cfa)),
                    Res::View(&view(&pq)),
                    Res::View(&view(&g)),
                    Res::View(&view(&rb)),
                ],
            );
            let out = texture(gpu, w, h, F::Rgba16Float);
            self.rcd_final.run(
                gpu,
                &mut enc,
                &base,
                base.out_size,
                &[
                    Res::View(&view(&vh)),
                    Res::View(&view(&g)),
                    Res::View(&view(&rb)),
                    Res::View(&view(&norm)),
                    Res::View(&view(&out)),
                ],
            );
            out
        } else {
            let out = texture(gpu, hw, hh, F::Rgba16Float);
            self.xtrans_bin.run(
                gpu,
                &mut enc,
                &with([w, h], [hw, hh]),
                [hw, hh],
                &[
                    Res::View(&view(&cfa)),
                    Res::View(&view(&norm)),
                    Res::View(&view(&out)),
                ],
            );
            out
        };
        gpu.queue.submit([enc.finish()]);
        Ok(rgb)
    }

    /// Box-reduce then bilinear-resample an RGBA16F texture to `long_edge`, and read it back.
    pub fn downsample_and_read(
        &self,
        gpu: &Gpu,
        rgb: wgpu::Texture,
        long_edge: u32,
    ) -> Result<RgbaF32> {
        let with = |in_size: [u32; 2], out_size: [u32; 2]| IngestParams {
            in_size,
            out_size,
            ..bytemuck::Zeroable::zeroed()
        };
        let (sw, sh) = (rgb.width(), rgb.height());
        let scale = (long_edge as f64 / sw.max(sh) as f64).min(1.0);
        let (tw, th) = (
            ((sw as f64 * scale).round() as u32).max(1),
            ((sh as f64 * scale).round() as u32).max(1),
        );
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let mut cur = rgb;
        while cur.width() >= tw * 2 && cur.height() >= th * 2 {
            let (nw, nh) = (cur.width().div_ceil(2), cur.height().div_ceil(2));
            let t = texture(gpu, nw, nh, F::Rgba16Float);
            self.downsample.run(
                gpu,
                &mut enc,
                &with([cur.width(), cur.height()], [nw, nh]),
                [nw, nh],
                &[Res::View(&view(&cur)), Res::View(&view(&t))],
            );
            cur = t;
        }
        if cur.width() != tw || cur.height() != th {
            let t = texture(gpu, tw, th, F::Rgba16Float);
            self.resample.run(
                gpu,
                &mut enc,
                &with([cur.width(), cur.height()], [tw, th]),
                [tw, th],
                &[
                    Res::View(&view(&cur)),
                    Res::Sampler(&self.sampler),
                    Res::View(&view(&t)),
                ],
            );
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
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("IngestParams"))
            .unwrap();
        let naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!()
        };
        assert_eq!(*span as usize, std::mem::size_of::<IngestParams>());
        let off = |n: &str| {
            members
                .iter()
                .find(|m| m.name.as_deref() == Some(n))
                .unwrap()
                .offset as usize
        };
        assert_eq!(off("wb"), std::mem::offset_of!(IngestParams, wb));
        assert_eq!(off("clip"), std::mem::offset_of!(IngestParams, clip));
        assert_eq!(off("white"), std::mem::offset_of!(IngestParams, white));
        assert_eq!(off("sensor"), std::mem::offset_of!(IngestParams, sensor));
        assert_eq!(off("pattern"), std::mem::offset_of!(IngestParams, pattern));
    }

    /// D14: half a frame blown, beside a measured half with no blue in it at all. Before, the fill's ratios
    /// against the estimate's empty blue multiplied level by level until `exp` overflowed and NaN covered the
    /// frame. Every value must now be finite and bounded (16× clip on the mosaic, which the demosaic overshoots
    /// by up to half again on the step between the halves), the blown half no darker than what the sensor
    /// recorded there, and the measured half left exactly as it was. On Bayer and on X-Trans.
    ///
    /// Run twice: with one threshold for every colour, as the heuristic gives, and with blue saturating far
    /// below the others, as a measurement often says (D6/D7). Uneven thresholds are the new way to break the
    /// fill — blue is then "clipped" over most of the frame while red and green are measurements — and the
    /// bound has to hold either way.
    #[test]
    fn highlight_fill_stays_bounded_beside_a_channel_the_estimate_lacks() {
        let Ok(gpu) = crate::test_gpu() else {
            eprintln!("skipped: no GPU");
            return;
        };
        let ing = Ingest::new(gpu).unwrap();
        let bayer = std::array::from_fn(|y| {
            std::array::from_fn(|x| match (y % 2, x % 2) {
                (0, 0) => 0,
                (1, 1) => 2,
                (0, _) => 1,
                _ => 3,
            })
        });
        // The vkdt block phase `phase` looks for: greens on each 3×3 block's corners and centre, blue left and
        // right of the centre on even blocks and red above and below it, swapped on odd ones.
        let xtrans = std::array::from_fn(|y| {
            std::array::from_fn(|x| {
                let (bx, by, i, j) = (x / 3, y / 3, x % 3, y % 3);
                let (side, cap) = if (bx + by) % 2 == 0 { (2, 0) } else { (0, 2) };
                match (i, j) {
                    (1, 1) => 1,
                    (1, _) => cap,
                    (_, 1) => side,
                    _ => 1,
                }
            })
        });
        for (name, pattern, period) in [("Bayer", bayer, 2), ("X-Trans", xtrans, 6)] {
            let (w, h) = (288u32, 288u32);
            let white = 4000.0;
            let data: Vec<u16> = (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    let c = pattern[(y % period) as usize][(x % period) as usize];
                    match (y < h / 2, c) {
                        (true, _) => white as u16,
                        (false, 2) => 0,
                        (false, _) => 1200,
                    }
                })
                .collect();
            let m = MosaicInput {
                width: w,
                height: h,
                data: &data,
                black: [0.0; 4],
                white,
                pattern,
                period,
                clip: [0.98; 4],
            };
            // Three threshold sets: even, a measured uneven one, and an extreme one kept for the bound
            // alone. `reaches_white` says whether the blown half is also asserted to render at white.
            //
            // The extreme set is excluded from that second property because it **diverges by GPU**, which
            // is worth writing down rather than rediscovering. On lavapipe (what CI uses, and reproducible
            // here with `VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json`) exactly one texel of some
            // 34,000 comes back at 0.67 where an NVIDIA card gives 1.0. What triggers it is **red's
            // threshold sitting above green's**, not blue and not the size of the spread: `[0.98, 0.98,
            // 0.25, 0.98]` passes with blue wildly uneven, while `[0.995, 0.98, 0.98, 0.98]` fails, and
            // `[0.995, 0.918, …]` (a wider gap) passes where `[0.995, 0.98, …]` (a narrower one) fails. It
            // is a knife-edge, not a systematic break. In the failing texel red and green sit exactly on
            // their per-channel caps (`HILITE_HEADROOM * clip`) on *both* GPUs and only the unclamped
            // channel differs, so whatever diverges is upstream in the estimate or its scale and the clamp
            // merely hides it in the other two.
            //
            // The bound is what this test is named for (D14) and it holds on both: the divergent value is
            // finite and well inside the headroom. The measurement cannot produce that set in any case —
            // over 217 files the widest spread between the four thresholds is 0.077 and 86 % have none at
            // all — so the second set below is `IMG_4197.CR2`'s own, which is the widest really measured.
            for (clip, reaches_white) in [
                ([0.98; 4], true),
                ([0.995, 0.918, 0.995, 0.995], true),
                ([0.995, 0.98, 0.25, 0.98], false),
            ] {
                let out = ing
                    .demosaic_at(gpu, &m, [2.0, 1.0, 1.5], false, clip)
                    .unwrap();
                let img = crate::read_rgba16(gpu, &out).unwrap();
                let (ow, oh) = (img.width, img.height);
                let edge = ow / 32;
                let even = clip[0] == clip[2];
                for (i, p) in img.data.chunks(4).enumerate() {
                    let (x, y) = (i as u32 % ow, i as u32 / ow);
                    assert!(
                        p[..3].iter().all(|v| v.is_finite() && *v <= 32.0),
                        "{name} {clip:?} ({x}, {y}): {p:?}"
                    );
                    let inside = (edge..ow - edge).contains(&x);
                    if reaches_white && inside && (edge..oh / 2 - edge).contains(&y) {
                        assert!(
                            p[..3].iter().all(|v| *v >= 0.99),
                            "{name} {clip:?} blown ({x}, {y}): {p:?}"
                        );
                    }
                    // With blue declared saturated at 0.25 the measured half is no longer measured, so only
                    // the even-threshold run can ask for it back untouched.
                    if even && inside && (oh / 2 + edge..oh - edge).contains(&y) {
                        let want = [0.3, 0.3, 0.0];
                        assert!(
                            (0..3).all(|c| (p[c] - want[c]).abs() < 1e-3),
                            "{name} measured ({x}, {y}): {p:?}"
                        );
                    }
                }
            }
        }
    }

    /// A frame that reaches saturation nowhere has nothing for the fill to estimate, so the pass is skipped
    /// rather than run to produce a no-op. The heuristic this replaced could not tell the difference: it put
    /// the threshold just under the brightest sample present, so every frame cleared its own.
    #[test]
    fn a_frame_that_clips_nowhere_has_nothing_to_reconstruct() {
        const BAYER: [[u8; 6]; 6] = [
            [0, 1, 0, 1, 0, 1],
            [3, 2, 3, 2, 3, 2],
            [0, 1, 0, 1, 0, 1],
            [3, 2, 3, 2, 3, 2],
            [0, 1, 0, 1, 0, 1],
            [3, 2, 3, 2, 3, 2],
        ];
        let frame = |data: &'static [u16]| MosaicInput {
            width: 4,
            height: 4,
            data,
            black: [0.0; 4],
            white: 1000.0,
            pattern: BAYER,
            period: 2,
            clip: [0.98; 4],
        };
        static AT_SATURATION: [u16; 16] = [980; 16];
        static WELL_UNDER: [u16; 16] = [300; 16];
        assert!(
            any_saturated(&frame(&AT_SATURATION), [0.98; 4]),
            "980 of 1000 is at the threshold"
        );
        assert!(
            !any_saturated(&frame(&WELL_UNDER), [0.98; 4]),
            "a frame whose brightest sample is a third of the range lost nothing"
        );

        // Each sample is measured against its own colour's threshold, and a threshold it does not reach
        // leaves it a measurement. The high sample sits at (1, 1), which this pattern makes colour 2.
        static ONE_HIGH: [u16; 16] = [
            300, 300, 300, 300, 300, 990, 300, 300, 300, 300, 300, 300, 300, 300, 300, 300,
        ];
        assert!(any_saturated(&frame(&ONE_HIGH), [0.98; 4]));
        assert!(!any_saturated(&frame(&ONE_HIGH), [0.995; 4]));
        assert!(
            !any_saturated(&frame(&ONE_HIGH), [0.98, 0.98, 0.995, 0.98]),
            "raising only that colour's threshold is enough to leave the frame unsaturated"
        );
    }

    #[test]
    fn finds_rggb_phase_for_every_bayer_layout() {
        // GRBG: R sits at row 0, column 1.
        let data = vec![0u16; 16];
        let m = MosaicInput {
            width: 4,
            height: 4,
            data: &data,
            black: [0.0; 4],
            white: 1.0,
            clip: [0.995; 4],
            pattern: [
                [1, 0, 1, 0, 1, 0],
                [2, 3, 2, 3, 2, 3],
                [1, 0, 1, 0, 1, 0],
                [2, 3, 2, 3, 2, 3],
                [1, 0, 1, 0, 1, 0],
                [2, 3, 2, 3, 2, 3],
            ],
            period: 2,
        };
        let (ox, oy, w, h, sensor) = phase(&m).unwrap();
        assert_eq!((ox, oy, w, h, sensor), (1, 0, 2, 4, 0));
    }
}
