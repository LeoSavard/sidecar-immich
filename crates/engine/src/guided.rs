// SPDX-License-Identifier: Apache-2.0
//! The guided filter (He, Sun and Tang, "Guided Image Filtering", ECCV 2010 / TPAMI 2013), in its fast
//! subsampled form (He and Sun, "Fast Guided Filter", 2015): an edge-preserving filter of one channel of an
//! input, steered by the luminance of a guide image, in linear time whatever the radius. It is a primitive, not
//! a feature — dehaze refines its transmission with it, the sky mask its edge, mask edges their feathering — so
//! it takes textures and returns one, and the same four WGSL passes run in the browser
//! (`client/guided_*.wgsl`).
//!
//! The statistics are taken on a grid `subsample` times coarser than the output, each texel the mean of its
//! footprint; box means, the linear coefficients and their box means follow on that grid; the last pass applies
//! them at full resolution against the full-resolution guide, which is where the edges come from.

use anyhow::Result;
use sidecar_uniform::Uniform;
use wgpu::TextureFormat as F;

use crate::Gpu;
use crate::compute::{B, Kernel, Res, texture, view};

/// The guided filter's passes' uniform (32 bytes). Its WGSL declaration, `shaders/uniforms/guided_params.wgsl`,
/// is generated from this struct (`sidecar-uniform`); the `uniforms` test checks the file and every offset.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable, Uniform)]
pub struct GuidedParams {
    /// The statistics grid, in texels.
    pub low: [u32; 2],
    /// The box radius on that grid, in its texels.
    pub radius: u32,
    /// The box pass's axis: 0 across, 1 down.
    pub axis: u32,
    /// The regularisation ε, in the guide's units squared: a window whose guide varies less than this is
    /// treated as flat and smoothed; one that varies more keeps its edge.
    pub eps: f32,
    /// Which channel of the input is filtered, 0…3.
    pub channel: u32,
    /// 1 to guide by log₂ luminance, so ε is relative to the level rather than absolute.
    pub log_guide: u32,
    pub _pad: u32,
}

/// How to filter: sizes are in the output's pixels.
#[derive(Clone, Copy, Debug)]
pub struct Guided {
    /// The window's radius, in output pixels.
    pub radius: f32,
    /// The regularisation ε, in the guide's units squared (log₂ units squared with `log_guide`).
    pub eps: f32,
    /// How much coarser the statistics grid is than the output; 1 is the exact filter.
    pub subsample: u32,
    pub channel: u32,
    pub log_guide: bool,
}

pub struct GuidedFilter {
    stats: Kernel,
    boxed: Kernel,
    coef: Kernel,
    apply: Kernel,
    sampler: wgpu::Sampler,
}

impl GuidedFilter {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        let wide = B::Store(F::Rgba32Float);
        Ok(Self {
            stats: Kernel::new(
                gpu,
                "client/guided_stats.wgsl",
                &[B::Uniform, B::TexF, B::TexF, B::Samp, wide],
            )?,
            boxed: Kernel::new(gpu, "client/guided_box.wgsl", &[B::Uniform, B::Tex, wide])?,
            coef: Kernel::new(gpu, "client/guided_coef.wgsl", &[B::Uniform, B::Tex, wide])?,
            apply: Kernel::new(
                gpu,
                "client/guided_apply.wgsl",
                &[
                    B::Uniform,
                    B::TexF,
                    B::Tex,
                    B::Samp,
                    B::Store(F::Rgba16Float),
                ],
            )?,
            sampler: gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        })
    }

    /// The statistics grid for an output of `out` pixels.
    pub fn grid(out: [u32; 2], subsample: u32) -> [u32; 2] {
        let s = subsample.max(1);
        [out[0].div_ceil(s).max(1), out[1].div_ceil(s).max(1)]
    }

    /// `input`'s chosen channel filtered against `guide`'s luminance, as a new RGBA16F texture `out` pixels big
    /// with the result in every colour channel. `guide` and `input` are sampled across their whole frame, so
    /// either may be another size than `out` — a low-resolution map upsampled against a full-resolution photo
    /// is the usual case.
    pub fn filter(
        &self,
        gpu: &Gpu,
        guide: &wgpu::Texture,
        input: &wgpu::Texture,
        out: [u32; 2],
        how: Guided,
    ) -> wgpu::Texture {
        let low = Self::grid(out, how.subsample);
        let s = how.subsample.max(1) as f32;
        let p = GuidedParams {
            low,
            radius: ((how.radius / s).round() as u32).max(1),
            axis: 0,
            eps: how.eps.max(1e-12),
            channel: how.channel.min(3),
            log_guide: how.log_guide as u32,
            _pad: 0,
        };
        let wide = || texture(gpu, low[0], low[1], F::Rgba32Float);
        let (a, b) = (wide(), wide());
        let result = texture(gpu, out[0].max(1), out[1].max(1), F::Rgba16Float);
        let (gv, iv, av, bv, rv) = (view(guide), view(input), view(&a), view(&b), view(&result));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let samp = Res::Sampler(&self.sampler);
        self.stats.run(
            gpu,
            &mut enc,
            &p,
            low,
            &[Res::View(&gv), Res::View(&iv), samp, Res::View(&av)],
        );
        // Box the statistics (a → b → a), take the coefficients (a → b), box those (b → a → b).
        let boxed = |enc: &mut wgpu::CommandEncoder, from, to, axis| {
            let p = GuidedParams { axis, ..p };
            self.boxed
                .run(gpu, enc, &p, low, &[Res::View(from), Res::View(to)]);
        };
        boxed(&mut enc, &av, &bv, 0);
        boxed(&mut enc, &bv, &av, 1);
        self.coef
            .run(gpu, &mut enc, &p, low, &[Res::View(&av), Res::View(&bv)]);
        boxed(&mut enc, &bv, &av, 0);
        boxed(&mut enc, &av, &bv, 1);
        self.apply.run(
            gpu,
            &mut enc,
            &p,
            [out[0].max(1), out[1].max(1)],
            &[
                Res::View(&gv),
                Res::View(&bv),
                Res::Sampler(&self.sampler),
                Res::View(&rv),
            ],
        );
        gpu.queue.submit([enc.finish()]);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RgbaF32, read_rgba16, upload_rgba16};

    const LUMA: [f64; 3] = [0.2627, 0.6780, 0.0593];

    /// The exact filter (subsample 1, linear guide) in f64, from the paper's equations, for comparison.
    fn reference(guide: &RgbaF32, input: &RgbaF32, r: i64, eps: f64) -> Vec<f64> {
        let (w, h) = (guide.width as i64, guide.height as i64);
        let px = |img: &RgbaF32, x: i64, y: i64, c: usize| {
            img.data[((y * w + x) * 4) as usize + c] as f64
        };
        let i: Vec<f64> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (y, x)))
            .map(|(y, x)| (0..3).map(|c| px(guide, x, y, c) * LUMA[c]).sum())
            .collect();
        let p: Vec<f64> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (y, x)))
            .map(|(y, x)| px(input, x, y, 0))
            .collect();
        let boxed = |v: &[f64]| -> Vec<f64> {
            let mut out = vec![0.0; v.len()];
            for y in 0..h {
                for x in 0..w {
                    let (mut s, mut n) = (0.0, 0.0);
                    for yy in (y - r).max(0)..=(y + r).min(h - 1) {
                        for xx in (x - r).max(0)..=(x + r).min(w - 1) {
                            s += v[(yy * w + xx) as usize];
                            n += 1.0;
                        }
                    }
                    out[(y * w + x) as usize] = s / n;
                }
            }
            out
        };
        let ip: Vec<f64> = i.iter().zip(&p).map(|(a, b)| a * b).collect();
        let ii: Vec<f64> = i.iter().map(|a| a * a).collect();
        let (mi, mp, mip, mii) = (boxed(&i), boxed(&p), boxed(&ip), boxed(&ii));
        let a: Vec<f64> = (0..i.len())
            .map(|k| (mip[k] - mi[k] * mp[k]) / ((mii[k] - mi[k] * mi[k]).max(0.0) + eps))
            .collect();
        let b: Vec<f64> = (0..i.len()).map(|k| mp[k] - a[k] * mi[k]).collect();
        let (ma, mb) = (boxed(&a), boxed(&b));
        (0..i.len()).map(|k| ma[k] * i[k] + mb[k]).collect()
    }

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 3]) -> RgbaF32 {
        RgbaF32 {
            width: w,
            height: h,
            data: (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .flat_map(|(x, y)| {
                    let [r, g, b] = f(x, y);
                    [r, g, b, 1.0]
                })
                .collect(),
        }
    }

    /// The exact filter on the GPU is the paper's, to half precision: a noisy step guided by a clean one.
    #[test]
    fn the_gpu_filter_is_the_papers() {
        let Ok(gpu) = crate::test_gpu() else {
            return eprintln!("skipping: no GPU");
        };
        let (w, h) = (96u32, 64u32);
        let guide = image(w, h, |x, y| {
            let v = if x + y / 3 > 48 { 0.8 } else { 0.1 };
            [v, v * 0.9, v * 1.1]
        });
        // A noisy copy of the step, as the input: the filter should denoise it without moving the edge.
        let input = image(w, h, |x, y| {
            let v = if x + y / 3 > 48 { 1.0 } else { 0.0 };
            let noise = (((x * 7919 + y * 104729) % 101) as f32 / 101.0 - 0.5) * 0.3;
            [v + noise, 0.0, 0.0]
        });
        let f = GuidedFilter::new(gpu).unwrap();
        let (r, eps) = (4, 1e-3);
        let out = f.filter(
            gpu,
            &upload_rgba16(gpu, &guide),
            &upload_rgba16(gpu, &input),
            [w, h],
            Guided {
                radius: r as f32,
                eps: eps as f32,
                subsample: 1,
                channel: 0,
                log_guide: false,
            },
        );
        let got = read_rgba16(gpu, &out).unwrap();
        // The reference runs on what the GPU actually read: both inputs went up as half floats.
        let half = |img: &RgbaF32| RgbaF32 {
            data: img
                .data
                .iter()
                .map(|v| half::f16::from_f32(*v).to_f32())
                .collect(),
            ..img.clone()
        };
        let want = reference(&half(&guide), &half(&input), r, eps);
        let worst = want
            .iter()
            .enumerate()
            .map(|(k, v)| (got.data[k * 4] as f64 - v).abs())
            .fold(0.0, f64::max);
        eprintln!("guided filter: largest difference from the reference {worst:.2e}");
        assert!(
            worst < 2e-3,
            "largest difference from the reference: {worst}"
        );
    }

    /// What makes it a guided filter: across the guide's edge the input's step survives; on either flat side
    /// its noise is smoothed away. And a constant input stays exactly that constant.
    #[test]
    fn it_keeps_the_guides_edges_and_smooths_between_them() {
        let Ok(gpu) = crate::test_gpu() else {
            return eprintln!("skipping: no GPU");
        };
        let (w, h) = (256u32, 32u32);
        let guide = image(w, h, |x, _| if x < 128 { [0.05; 3] } else { [0.6; 3] });
        let noisy = image(w, h, |x, y| {
            let v = if x < 128 { 0.2 } else { 0.9 };
            let noise = (((x * 7919 + y * 104729) % 97) as f32 / 97.0 - 0.5) * 0.2;
            [v + noise, 0.0, 0.0]
        });
        let f = GuidedFilter::new(gpu).unwrap();
        for (subsample, log_guide) in [(1, false), (4, false), (4, true)] {
            let how = Guided {
                radius: 16.0,
                eps: if log_guide { 0.05 } else { 1e-3 },
                subsample,
                channel: 0,
                log_guide,
            };
            let out = read_rgba16(
                gpu,
                &f.filter(
                    gpu,
                    &upload_rgba16(gpu, &guide),
                    &upload_rgba16(gpu, &noisy),
                    [w, h],
                    how,
                ),
            )
            .unwrap();
            let at = |x: u32| out.data[((16 * w + x) * 4) as usize];
            let spread = |from: u32, to: u32| {
                let v: Vec<f32> = (from..to).map(at).collect();
                v.iter().cloned().fold(f32::MIN, f32::max)
                    - v.iter().cloned().fold(f32::MAX, f32::min)
            };
            assert!(
                spread(20, 100) < 0.05 && spread(156, 236) < 0.05,
                "{how:?}: the flat sides are smoothed (noise ±0.1 in)"
            );
            assert!(
                (at(126) - 0.2).abs() < 0.05 && (at(130) - 0.9).abs() < 0.05,
                "{how:?}: the step stays where the guide's is: {} | {}",
                at(126),
                at(130)
            );
        }
        let flat = image(w, h, |_, _| [0.37, 0.0, 0.0]);
        let out = read_rgba16(
            gpu,
            &f.filter(
                gpu,
                &upload_rgba16(gpu, &guide),
                &upload_rgba16(gpu, &flat),
                [w, h],
                Guided {
                    radius: 8.0,
                    eps: 1e-4,
                    subsample: 2,
                    channel: 0,
                    log_guide: false,
                },
            ),
        )
        .unwrap();
        assert!(
            out.data.chunks(4).all(|p| (p[0] - 0.37).abs() < 1e-3),
            "a constant input is left alone"
        );
    }
}
// SPDX-License-Identifier: Apache-2.0
//! The guided filter (He, Sun and Tang, "Guided Image Filtering", ECCV 2010 / TPAMI 2013), in its fast
//! subsampled form (He and Sun, "Fast Guided Filter", 2015): an edge-preserving filter of one channel of an
//! input, steered by the luminance of a guide image, in linear time whatever the radius. It is a primitive, not
//! a feature — dehaze refines its transmission with it, the sky mask its edge, mask edges their feathering — so
//! it takes textures and returns one, and the same four WGSL passes run in the browser
//! (`client/guided_*.wgsl`).
//!
//! The statistics are taken on a grid `subsample` times coarser than the output, each texel the mean of its
//! footprint; box means, the linear coefficients and their box means follow on that grid; the last pass applies
//! them at full resolution against the full-resolution guide, which is where the edges come from.

use anyhow::Result;
use sidecar_uniform::Uniform;
use wgpu::TextureFormat as F;

use crate::Gpu;
use crate::compute::{B, Kernel, Res, texture, view};

/// The guided filter's passes' uniform (32 bytes). Its WGSL declaration, `shaders/uniforms/guided_params.wgsl`,
/// is generated from this struct (`sidecar-uniform`); the `uniforms` test checks the file and every offset.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable, Uniform)]
pub struct GuidedParams {
    /// The statistics grid, in texels.
    pub low: [u32; 2],
    /// The box radius on that grid, in its texels.
    pub radius: u32,
    /// The box pass's axis: 0 across, 1 down.
    pub axis: u32,
    /// The regularisation ε, in the guide's units squared: a window whose guide varies less than this is
    /// treated as flat and smoothed; one that varies more keeps its edge.
    pub eps: f32,
    /// Which channel of the input is filtered, 0…3; `GUIDE` filters the guide value itself.
    pub channel: u32,
    /// 1 to guide by log₂ luminance, so ε is relative to the level rather than absolute.
    pub log_guide: u32,
    pub _pad: u32,
}

/// How to filter: sizes are in the output's pixels.
#[derive(Clone, Copy, Debug)]
pub struct Guided {
    /// The window's radius, in output pixels.
    pub radius: f32,
    /// The regularisation ε, in the guide's units squared (log₂ units squared with `log_guide`).
    pub eps: f32,
    /// How much coarser the statistics grid is than the output; 1 is the exact filter.
    pub subsample: u32,
    pub channel: u32,
    pub log_guide: bool,
}

/// `Guided::channel` for a self-guided filter: the input is the guide's own value (log luminance with
/// `log_guide`).
pub const GUIDE: u32 = 4;

pub struct GuidedFilter {
    stats: Kernel,
    boxed: Kernel,
    coef: Kernel,
    apply: Kernel,
    sampler: wgpu::Sampler,
}

impl GuidedFilter {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        let wide = B::Store(F::Rgba32Float);
        Ok(Self {
            stats: Kernel::new(
                gpu,
                "client/guided_stats.wgsl",
                &[B::Uniform, B::TexF, B::TexF, B::Samp, wide],
            )?,
            boxed: Kernel::new(gpu, "client/guided_box.wgsl", &[B::Uniform, B::Tex, wide])?,
            coef: Kernel::new(gpu, "client/guided_coef.wgsl", &[B::Uniform, B::Tex, wide])?,
            apply: Kernel::new(
                gpu,
                "client/guided_apply.wgsl",
                &[
                    B::Uniform,
                    B::TexF,
                    B::Tex,
                    B::Samp,
                    B::Store(F::Rgba16Float),
                ],
            )?,
            sampler: gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        })
    }

    /// The statistics grid for an output of `out` pixels.
    pub fn grid(out: [u32; 2], subsample: u32) -> [u32; 2] {
        let s = subsample.max(1);
        [out[0].div_ceil(s).max(1), out[1].div_ceil(s).max(1)]
    }

    /// `input`'s chosen channel filtered against `guide`'s luminance, as a new RGBA16F texture `out` pixels big
    /// with the result in every colour channel. `guide` and `input` are sampled across their whole frame, so
    /// either may be another size than `out` — a low-resolution map upsampled against a full-resolution photo
    /// is the usual case.
    pub fn filter(
        &self,
        gpu: &Gpu,
        guide: &wgpu::Texture,
        input: &wgpu::Texture,
        out: [u32; 2],
        how: Guided,
    ) -> wgpu::Texture {
        let (coef, p) = self.coefficients(gpu, guide, input, out, how);
        let result = texture(gpu, out[0].max(1), out[1].max(1), F::Rgba16Float);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        self.apply.run(
            gpu,
            &mut enc,
            &p,
            [out[0].max(1), out[1].max(1)],
            &[
                Res::View(&view(guide)),
                Res::View(&view(&coef)),
                Res::Sampler(&self.sampler),
                Res::View(&view(&result)),
            ],
        );
        gpu.queue.submit([enc.finish()]);
        result
    }

    /// The filter's smoothed linear coefficients on its statistics grid (`grid(out, subsample)`): an RGBA32F
    /// texture holding mean(a) and mean(b) in x and y, so that q = mean(a)·I + mean(b) at any resolution —
    /// what `filter` applies, and what a pass that applies them itself (develop's local contrast) takes.
    pub fn coefficients(
        &self,
        gpu: &Gpu,
        guide: &wgpu::Texture,
        input: &wgpu::Texture,
        out: [u32; 2],
        how: Guided,
    ) -> (wgpu::Texture, GuidedParams) {
        let low = Self::grid(out, how.subsample);
        let s = how.subsample.max(1) as f32;
        let p = GuidedParams {
            low,
            radius: ((how.radius / s).round() as u32).max(1),
            axis: 0,
            eps: how.eps.max(1e-12),
            channel: how.channel.min(GUIDE),
            log_guide: how.log_guide as u32,
            _pad: 0,
        };
        let wide = || texture(gpu, low[0], low[1], F::Rgba32Float);
        let (a, b) = (wide(), wide());
        let (gv, iv, av, bv) = (view(guide), view(input), view(&a), view(&b));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let samp = Res::Sampler(&self.sampler);
        self.stats.run(
            gpu,
            &mut enc,
            &p,
            low,
            &[Res::View(&gv), Res::View(&iv), samp, Res::View(&av)],
        );
        // Box the statistics (a → b → a), take the coefficients (a → b), box those (b → a → b).
        let boxed = |enc: &mut wgpu::CommandEncoder, from, to, axis| {
            let p = GuidedParams { axis, ..p };
            self.boxed
                .run(gpu, enc, &p, low, &[Res::View(from), Res::View(to)]);
        };
        boxed(&mut enc, &av, &bv, 0);
        boxed(&mut enc, &bv, &av, 1);
        self.coef
            .run(gpu, &mut enc, &p, low, &[Res::View(&av), Res::View(&bv)]);
        boxed(&mut enc, &bv, &av, 0);
        boxed(&mut enc, &av, &bv, 1);
        gpu.queue.submit([enc.finish()]);
        (b, p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RgbaF32, read_rgba16, upload_rgba16};

    const LUMA: [f64; 3] = [0.2627, 0.6780, 0.0593];

    /// The exact filter (subsample 1, linear guide) in f64, from the paper's equations, for comparison.
    fn reference(guide: &RgbaF32, input: &RgbaF32, r: i64, eps: f64) -> Vec<f64> {
        let (w, h) = (guide.width as i64, guide.height as i64);
        let px = |img: &RgbaF32, x: i64, y: i64, c: usize| {
            img.data[((y * w + x) * 4) as usize + c] as f64
        };
        let i: Vec<f64> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (y, x)))
            .map(|(y, x)| (0..3).map(|c| px(guide, x, y, c) * LUMA[c]).sum())
            .collect();
        let p: Vec<f64> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (y, x)))
            .map(|(y, x)| px(input, x, y, 0))
            .collect();
        let boxed = |v: &[f64]| -> Vec<f64> {
            let mut out = vec![0.0; v.len()];
            for y in 0..h {
                for x in 0..w {
                    let (mut s, mut n) = (0.0, 0.0);
                    for yy in (y - r).max(0)..=(y + r).min(h - 1) {
                        for xx in (x - r).max(0)..=(x + r).min(w - 1) {
                            s += v[(yy * w + xx) as usize];
                            n += 1.0;
                        }
                    }
                    out[(y * w + x) as usize] = s / n;
                }
            }
            out
        };
        let ip: Vec<f64> = i.iter().zip(&p).map(|(a, b)| a * b).collect();
        let ii: Vec<f64> = i.iter().map(|a| a * a).collect();
        let (mi, mp, mip, mii) = (boxed(&i), boxed(&p), boxed(&ip), boxed(&ii));
        let a: Vec<f64> = (0..i.len())
            .map(|k| (mip[k] - mi[k] * mp[k]) / ((mii[k] - mi[k] * mi[k]).max(0.0) + eps))
            .collect();
        let b: Vec<f64> = (0..i.len()).map(|k| mp[k] - a[k] * mi[k]).collect();
        let (ma, mb) = (boxed(&a), boxed(&b));
        (0..i.len()).map(|k| ma[k] * i[k] + mb[k]).collect()
    }

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 3]) -> RgbaF32 {
        RgbaF32 {
            width: w,
            height: h,
            data: (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .flat_map(|(x, y)| {
                    let [r, g, b] = f(x, y);
                    [r, g, b, 1.0]
                })
                .collect(),
        }
    }

    /// The exact filter on the GPU is the paper's, to half precision: a noisy step guided by a clean one.
    #[test]
    fn the_gpu_filter_is_the_papers() {
        let Ok(gpu) = crate::test_gpu() else {
            return eprintln!("skipping: no GPU");
        };
        let (w, h) = (96u32, 64u32);
        let guide = image(w, h, |x, y| {
            let v = if x + y / 3 > 48 { 0.8 } else { 0.1 };
            [v, v * 0.9, v * 1.1]
        });
        // A noisy copy of the step, as the input: the filter should denoise it without moving the edge.
        let input = image(w, h, |x, y| {
            let v = if x + y / 3 > 48 { 1.0 } else { 0.0 };
            let noise = (((x * 7919 + y * 104729) % 101) as f32 / 101.0 - 0.5) * 0.3;
            [v + noise, 0.0, 0.0]
        });
        let f = GuidedFilter::new(gpu).unwrap();
        let (r, eps) = (4, 1e-3);
        let out = f.filter(
            gpu,
            &upload_rgba16(gpu, &guide),
            &upload_rgba16(gpu, &input),
            [w, h],
            Guided {
                radius: r as f32,
                eps: eps as f32,
                subsample: 1,
                channel: 0,
                log_guide: false,
            },
        );
        let got = read_rgba16(gpu, &out).unwrap();
        // The reference runs on what the GPU actually read: both inputs went up as half floats.
        let half = |img: &RgbaF32| RgbaF32 {
            data: img
                .data
                .iter()
                .map(|v| half::f16::from_f32(*v).to_f32())
                .collect(),
            ..img.clone()
        };
        let want = reference(&half(&guide), &half(&input), r, eps);
        let worst = want
            .iter()
            .enumerate()
            .map(|(k, v)| (got.data[k * 4] as f64 - v).abs())
            .fold(0.0, f64::max);
        eprintln!("guided filter: largest difference from the reference {worst:.2e}");
        assert!(
            worst < 2e-3,
            "largest difference from the reference: {worst}"
        );
    }

    /// What makes it a guided filter: across the guide's edge the input's step survives; on either flat side
    /// its noise is smoothed away. And a constant input stays exactly that constant.
    #[test]
    fn it_keeps_the_guides_edges_and_smooths_between_them() {
        let Ok(gpu) = crate::test_gpu() else {
            return eprintln!("skipping: no GPU");
        };
        let (w, h) = (256u32, 32u32);
        let guide = image(w, h, |x, _| if x < 128 { [0.05; 3] } else { [0.6; 3] });
        let noisy = image(w, h, |x, y| {
            let v = if x < 128 { 0.2 } else { 0.9 };
            let noise = (((x * 7919 + y * 104729) % 97) as f32 / 97.0 - 0.5) * 0.2;
            [v + noise, 0.0, 0.0]
        });
        let f = GuidedFilter::new(gpu).unwrap();
        for (subsample, log_guide) in [(1, false), (4, false), (4, true)] {
            let how = Guided {
                radius: 16.0,
                eps: if log_guide { 0.05 } else { 1e-3 },
                subsample,
                channel: 0,
                log_guide,
            };
            let out = read_rgba16(
                gpu,
                &f.filter(
                    gpu,
                    &upload_rgba16(gpu, &guide),
                    &upload_rgba16(gpu, &noisy),
                    [w, h],
                    how,
                ),
            )
            .unwrap();
            let at = |x: u32| out.data[((16 * w + x) * 4) as usize];
            let spread = |from: u32, to: u32| {
                let v: Vec<f32> = (from..to).map(at).collect();
                v.iter().cloned().fold(f32::MIN, f32::max)
                    - v.iter().cloned().fold(f32::MAX, f32::min)
            };
            assert!(
                spread(20, 100) < 0.05 && spread(156, 236) < 0.05,
                "{how:?}: the flat sides are smoothed (noise ±0.1 in)"
            );
            assert!(
                (at(126) - 0.2).abs() < 0.05 && (at(130) - 0.9).abs() < 0.05,
                "{how:?}: the step stays where the guide's is: {} | {}",
                at(126),
                at(130)
            );
        }
        let flat = image(w, h, |_, _| [0.37, 0.0, 0.0]);
        let out = read_rgba16(
            gpu,
            &f.filter(
                gpu,
                &upload_rgba16(gpu, &guide),
                &upload_rgba16(gpu, &flat),
                [w, h],
                Guided {
                    radius: 8.0,
                    eps: 1e-4,
                    subsample: 2,
                    channel: 0,
                    log_guide: false,
                },
            ),
        )
        .unwrap();
        assert!(
            out.data.chunks(4).all(|p| (p[0] - 0.37).abs() < 1e-3),
            "a constant input is left alone"
        );
    }
}
