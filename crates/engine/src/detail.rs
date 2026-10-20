// SPDX-License-Identifier: Apache-2.0
//! Detail passes on camera-native linear RGB: à-trous wavelet denoising in a variance-stabilized opponent space
//! (Starck & Murtagh), and Richardson–Lucy deconvolution of luminance with a Gaussian PSF. Used for 1:1 loupe
//! tiles and for export, with the same noise estimate for both.

use anyhow::Result;
use wgpu::TextureFormat as F;

use crate::compute::{B, Kernel, Res, texture, view};
use crate::{Gpu, read_rgba16};

/// Mirrors `struct DetailParams` in shaders/detail/common.wgsl (32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DetailParams {
    pub size: [u32; 2],
    pub step: u32,
    pub axis: u32,
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}

/// Noise of each à-trous B3-spline band relative to white noise at scale 0 (Starck & Murtagh).
const BAND_NOISE: [f32; 5] = [0.889, 0.200, 0.086, 0.041, 0.020];
/// Chroma channels carry about √6 times the stabilized luma noise.
const CHROMA_NOISE: f32 = 2.45;

pub struct Detail {
    to_opp: Kernel,
    from_opp: Kernel,
    blur: Kernel,
    shrink: Kernel,
    band: Kernel,
    gauss: Kernel,
    luma: Kernel,
    ratio: Kernel,
    update: Kernel,
    apply: Kernel,
    crop: Kernel,
}

pub struct DetailSettings {
    /// 0–100.
    pub denoise_luminance: f32,
    pub denoise_chroma: f32,
    pub sharpen_amount: f32,
    /// Gaussian sigma in pixels.
    pub sharpen_radius: f32,
    pub sharpen_iterations: u32,
}

impl DetailSettings {
    pub fn is_noop(&self) -> bool {
        self.denoise_luminance <= 0.0 && self.denoise_chroma <= 0.0 && self.sharpen_amount <= 0.0
    }

    /// Pixels of context a tile needs so its centre matches a full-image render.
    pub fn margin(&self) -> u32 {
        let denoise = if self.denoise_luminance > 0.0 || self.denoise_chroma > 0.0 {
            64
        } else {
            0
        };
        let sharpen = if self.sharpen_amount > 0.0 {
            (3.0 * self.sharpen_radius).ceil() as u32 * 2 * self.sharpen_iterations.min(10)
        } else {
            0
        };
        (denoise + sharpen).clamp(8, 160)
    }
}

impl Detail {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        let k = |e: &str, b: &[B]| Kernel::new(gpu, e, b);
        Ok(Self {
            to_opp: k(
                "detail/to_opponent.wgsl",
                &[B::Uniform, B::Tex, B::Store(F::Rgba32Float)],
            )?,
            from_opp: k(
                "detail/from_opponent.wgsl",
                &[B::Uniform, B::Tex, B::Tex, B::Store(F::Rgba16Float)],
            )?,
            blur: k(
                "detail/atrous_blur.wgsl",
                &[B::Uniform, B::Tex, B::Store(F::Rgba32Float)],
            )?,
            shrink: k(
                "detail/shrink.wgsl",
                &[B::Uniform, B::Tex, B::Tex, B::Tex, B::Store(F::Rgba32Float)],
            )?,
            band: k(
                "detail/detail_band.wgsl",
                &[B::Uniform, B::Tex, B::Tex, B::Store(F::Rgba16Float)],
            )?,
            gauss: k(
                "detail/gauss.wgsl",
                &[B::Uniform, B::Tex, B::Store(F::R32Float)],
            )?,
            luma: k(
                "detail/rl_luma.wgsl",
                &[B::Uniform, B::Tex, B::Store(F::R32Float)],
            )?,
            ratio: k(
                "detail/rl_ratio.wgsl",
                &[B::Uniform, B::Tex, B::Tex, B::Store(F::R32Float)],
            )?,
            update: k(
                "detail/rl_update.wgsl",
                &[B::Uniform, B::Tex, B::Tex, B::Store(F::R32Float)],
            )?,
            apply: k(
                "detail/rl_apply.wgsl",
                &[B::Uniform, B::Tex, B::Tex, B::Tex, B::Store(F::Rgba16Float)],
            )?,
            crop: k(
                "detail/crop.wgsl",
                &[B::Uniform, B::Tex, B::Store(F::Rgba16Float)],
            )?,
        })
    }

    fn p(size: [u32; 2]) -> DetailParams {
        DetailParams {
            size,
            ..Default::default()
        }
    }

    /// Copy `w × h` pixels starting at `(x, y)` into a new RGBA16F texture.
    pub fn crop(
        &self,
        gpu: &Gpu,
        src: &wgpu::Texture,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    ) -> wgpu::Texture {
        let out = texture(gpu, w, h, F::Rgba16Float);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let params = DetailParams {
            a: x as f32,
            b: y as f32,
            ..Self::p([w, h])
        };
        self.crop.run(
            gpu,
            &mut enc,
            &params,
            [w, h],
            &[Res::View(&view(src)), Res::View(&view(&out))],
        );
        gpu.queue.submit([enc.finish()]);
        out
    }

    /// Noise level of the stabilized luma at scale 0, from the median absolute detail of a central crop.
    pub fn noise_sigma(&self, gpu: &Gpu, src: &wgpu::Texture) -> Result<f32> {
        let (w, h) = (src.width().min(1024), src.height().min(1024));
        let tile = self.crop(
            gpu,
            src,
            (src.width() - w) / 2,
            (src.height() - h) / 2,
            w,
            h,
        );
        let size = [w, h];
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let opp = texture(gpu, w, h, F::Rgba32Float);
        self.to_opp.run(
            gpu,
            &mut enc,
            &Self::p(size),
            size,
            &[Res::View(&view(&tile)), Res::View(&view(&opp))],
        );
        let tmp = texture(gpu, w, h, F::Rgba32Float);
        let coarse = texture(gpu, w, h, F::Rgba32Float);
        self.blur.run(
            gpu,
            &mut enc,
            &DetailParams {
                step: 1,
                axis: 0,
                ..Self::p(size)
            },
            size,
            &[Res::View(&view(&opp)), Res::View(&view(&tmp))],
        );
        self.blur.run(
            gpu,
            &mut enc,
            &DetailParams {
                step: 1,
                axis: 1,
                ..Self::p(size)
            },
            size,
            &[Res::View(&view(&tmp)), Res::View(&view(&coarse))],
        );
        let band = texture(gpu, w, h, F::Rgba16Float);
        self.band.run(
            gpu,
            &mut enc,
            &Self::p(size),
            size,
            &[
                Res::View(&view(&opp)),
                Res::View(&view(&coarse)),
                Res::View(&view(&band)),
            ],
        );
        gpu.queue.submit([enc.finish()]);
        let mut d: Vec<f32> = read_rgba16(gpu, &band)?
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[0])
            .collect();
        let mid = d.len() / 2;
        let (_, median, _) = d.select_nth_unstable_by(mid, f32::total_cmp);
        Ok((*median / 0.6745 / BAND_NOISE[0]).max(1e-5))
    }

    pub fn denoise(
        &self,
        gpu: &Gpu,
        src: &wgpu::Texture,
        sigma: f32,
        luminance: f32,
        chroma: f32,
    ) -> wgpu::Texture {
        let (w, h) = (src.width(), src.height());
        let size = [w, h];
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let mut fine = texture(gpu, w, h, F::Rgba32Float);
        self.to_opp.run(
            gpu,
            &mut enc,
            &Self::p(size),
            size,
            &[Res::View(&view(src)), Res::View(&view(&fine))],
        );
        let mut acc = texture(gpu, w, h, F::Rgba32Float);
        let (kl, kc) = (
            3.0 * luminance.clamp(0.0, 100.0) / 100.0,
            4.0 * chroma.clamp(0.0, 100.0) / 100.0,
        );
        for (j, noise) in BAND_NOISE.iter().enumerate() {
            let step = 1u32 << j;
            let tmp = texture(gpu, w, h, F::Rgba32Float);
            let coarse = texture(gpu, w, h, F::Rgba32Float);
            self.blur.run(
                gpu,
                &mut enc,
                &DetailParams {
                    step,
                    axis: 0,
                    ..Self::p(size)
                },
                size,
                &[Res::View(&view(&fine)), Res::View(&view(&tmp))],
            );
            self.blur.run(
                gpu,
                &mut enc,
                &DetailParams {
                    step,
                    axis: 1,
                    ..Self::p(size)
                },
                size,
                &[Res::View(&view(&tmp)), Res::View(&view(&coarse))],
            );
            let next_acc = texture(gpu, w, h, F::Rgba32Float);
            let t = sigma * noise;
            self.shrink.run(
                gpu,
                &mut enc,
                &DetailParams {
                    a: kl * t,
                    b: kc * t * CHROMA_NOISE,
                    ..Self::p(size)
                },
                size,
                &[
                    Res::View(&view(&fine)),
                    Res::View(&view(&coarse)),
                    Res::View(&view(&acc)),
                    Res::View(&view(&next_acc)),
                ],
            );
            acc = next_acc;
            fine = coarse;
        }
        let out = texture(gpu, w, h, F::Rgba16Float);
        self.from_opp.run(
            gpu,
            &mut enc,
            &Self::p(size),
            size,
            &[
                Res::View(&view(&fine)),
                Res::View(&view(&acc)),
                Res::View(&view(&out)),
            ],
        );
        gpu.queue.submit([enc.finish()]);
        out
    }

    pub fn sharpen(
        &self,
        gpu: &Gpu,
        src: &wgpu::Texture,
        amount: f32,
        sigma: f32,
        iterations: u32,
    ) -> wgpu::Texture {
        let (w, h) = (src.width(), src.height());
        let size = [w, h];
        let sigma = sigma.clamp(0.3, 3.0);
        let radius = ((3.0 * sigma).ceil() as u32).clamp(1, 9);
        let g = |axis: u32| DetailParams {
            step: radius,
            axis,
            a: sigma,
            ..Self::p(size)
        };
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let observed = texture(gpu, w, h, F::R32Float);
        self.luma.run(
            gpu,
            &mut enc,
            &Self::p(size),
            size,
            &[Res::View(&view(src)), Res::View(&view(&observed))],
        );
        let mut estimate = texture(gpu, w, h, F::R32Float);
        self.luma.run(
            gpu,
            &mut enc,
            &Self::p(size),
            size,
            &[Res::View(&view(src)), Res::View(&view(&estimate))],
        );
        for _ in 0..iterations.clamp(1, 30) {
            let (b1, b2, ratio, c1, c2, next) = (
                texture(gpu, w, h, F::R32Float),
                texture(gpu, w, h, F::R32Float),
                texture(gpu, w, h, F::R32Float),
                texture(gpu, w, h, F::R32Float),
                texture(gpu, w, h, F::R32Float),
                texture(gpu, w, h, F::R32Float),
            );
            self.gauss.run(
                gpu,
                &mut enc,
                &g(0),
                size,
                &[Res::View(&view(&estimate)), Res::View(&view(&b1))],
            );
            self.gauss.run(
                gpu,
                &mut enc,
                &g(1),
                size,
                &[Res::View(&view(&b1)), Res::View(&view(&b2))],
            );
            self.ratio.run(
                gpu,
                &mut enc,
                &Self::p(size),
                size,
                &[
                    Res::View(&view(&observed)),
                    Res::View(&view(&b2)),
                    Res::View(&view(&ratio)),
                ],
            );
            self.gauss.run(
                gpu,
                &mut enc,
                &g(0),
                size,
                &[Res::View(&view(&ratio)), Res::View(&view(&c1))],
            );
            self.gauss.run(
                gpu,
                &mut enc,
                &g(1),
                size,
                &[Res::View(&view(&c1)), Res::View(&view(&c2))],
            );
            self.update.run(
                gpu,
                &mut enc,
                &Self::p(size),
                size,
                &[
                    Res::View(&view(&estimate)),
                    Res::View(&view(&c2)),
                    Res::View(&view(&next)),
                ],
            );
            estimate = next;
        }
        let out = texture(gpu, w, h, F::Rgba16Float);
        self.apply.run(
            gpu,
            &mut enc,
            &DetailParams {
                a: amount.clamp(0.0, 100.0) / 100.0,
                ..Self::p(size)
            },
            size,
            &[
                Res::View(&view(src)),
                Res::View(&view(&observed)),
                Res::View(&view(&estimate)),
                Res::View(&view(&out)),
            ],
        );
        gpu.queue.submit([enc.finish()]);
        out
    }

    /// Apply denoise then sharpening as configured; returns the source unchanged when both are off.
    pub fn apply(
        &self,
        gpu: &Gpu,
        src: wgpu::Texture,
        sigma: f32,
        s: &DetailSettings,
    ) -> wgpu::Texture {
        let mut cur = src;
        if s.denoise_luminance > 0.0 || s.denoise_chroma > 0.0 {
            cur = self.denoise(gpu, &cur, sigma, s.denoise_luminance, s.denoise_chroma);
        }
        if s.sharpen_amount > 0.0 {
            cur = self.sharpen(
                gpu,
                &cur,
                s.sharpen_amount,
                s.sharpen_radius,
                s.sharpen_iterations,
            );
        }
        cur
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_params_layout_matches_wgsl() {
        let src = sidecar_shaders::compose("detail/common.wgsl").unwrap();
        let module = naga::front::wgsl::parse_str(&src).unwrap();
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("DetailParams"))
            .unwrap();
        let naga::TypeInner::Struct { span, .. } = &ty.inner else {
            panic!()
        };
        assert_eq!(*span as usize, std::mem::size_of::<DetailParams>());
    }

    #[test]
    fn denoise_reduces_noise_and_keeps_the_mean() {
        let Ok(gpu) = crate::test_gpu() else { return };
        let detail = Detail::new(gpu).unwrap();
        let (w, h) = (256u32, 256u32);
        let mut state = 12345u64;
        let mut rnd = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % 10_000) as f32 / 10_000.0 - 0.5
        };
        let data: Vec<f32> = (0..w * h)
            .flat_map(|_| {
                let n = 0.02 * rnd();
                [0.2 + n, 0.2 + 0.02 * rnd(), 0.2 + 0.02 * rnd(), 0.0]
            })
            .collect();
        let img = crate::RgbaF32 {
            width: w,
            height: h,
            data,
        };
        let tex = crate::upload_rgba16(gpu, &img);
        let sigma = detail.noise_sigma(gpu, &tex).unwrap();
        assert!(sigma > 0.0);
        let out = read_rgba16(gpu, &detail.denoise(gpu, &tex, sigma, 100.0, 100.0)).unwrap();
        let stats = |d: &[f32]| {
            let g: Vec<f32> = d.as_chunks::<4>().0.iter().map(|p| p[1]).collect();
            let mean = g.iter().sum::<f32>() / g.len() as f32;
            let var = g.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / g.len() as f32;
            (mean, var.sqrt())
        };
        let (m0, s0) = stats(&img.data);
        let (m1, s1) = stats(&out.data);
        assert!((m1 - m0).abs() < 0.005, "mean {m0} → {m1}");
        assert!(s1 < s0 * 0.6, "noise {s0} → {s1}");
    }
}
