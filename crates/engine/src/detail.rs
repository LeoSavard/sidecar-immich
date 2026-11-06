// SPDX-License-Identifier: Apache-2.0
//! Detail passes on camera-native linear RGB: à-trous wavelet denoising in a variance-stabilized opponent space
//! (Starck & Murtagh), and Richardson–Lucy deconvolution of luminance with a Gaussian PSF. Used for 1:1 loupe
//! tiles and for export, with the same noise estimate for both.

use anyhow::Result;
use sidecar_uniform::Uniform;
use wgpu::TextureFormat as F;

use crate::compute::{B, Kernel, Res, texture, view};
use crate::noise::NoiseModel;
use crate::{Gpu, read_rgba16};

/// The detail passes' uniform (32 bytes). Its WGSL declaration,
/// `shaders/uniforms/detail_params.wgsl`, is generated from this struct (`sidecar-uniform`); the
/// `uniforms` test checks the file and every offset.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable, Uniform)]
pub struct DetailParams {
    pub size: [u32; 2],
    /// À-trous dilation (1, 2, 4, …) or blur radius.
    pub step: u32,
    /// 0 = horizontal, 1 = vertical.
    pub axis: u32,
    /// Pass-specific, as are `b`, `c` and `d`.
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
}

/// Detail per layer's uniform (`detail/strength.wgsl`, 160 bytes). Its WGSL declaration,
/// `shaders/uniforms/detail_layers.wgsl`, is generated from this struct (`sidecar-uniform`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable, Uniform)]
pub struct DetailLayers {
    /// Per mask slot: luminance noise reduction and sharpening to add (−1…1), the layer's amount (0…1), and 1
    /// when it is inverted. All zero for a slot without a layer.
    pub layers: [[f32; 4]; 8],
    /// Where the texture being detailed sits in the masks' frame: x, y, and the frame's width and height.
    pub frame: [f32; 4],
    /// The global noise reduction and sharpening (0…1), unused ×2.
    pub base: [f32; 4],
}

/// One mask layer's detail: its slot among the masks develop binds, what it adds (−100…100 each, as the
/// global sliders), and how its mask applies.
#[derive(Clone, Copy, Debug, Default)]
pub struct LayerDetail {
    pub slot: usize,
    pub noise: f32,
    pub sharpness: f32,
    /// 0…1.
    pub amount: f32,
    pub invert: bool,
}

/// The masks a layered detail reads: one texture per slot (`None` for a slot without one), in a frame of
/// `frame[2] × frame[3]` pixels, of which the texture being detailed starts at `frame[0], frame[1]`.
pub struct DetailMasks<'a> {
    pub textures: Vec<Option<&'a wgpu::Texture>>,
    pub frame: [f32; 4],
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
    strength: Kernel,
    sampler: wgpu::Sampler,
    no_mask: wgpu::Texture,
}

pub struct DetailSettings {
    /// 0–100.
    pub denoise_luminance: f32,
    pub denoise_chroma: f32,
    pub sharpen_amount: f32,
    /// Gaussian sigma in pixels.
    pub sharpen_radius: f32,
    pub sharpen_iterations: u32,
    /// Stabilize with this photo's measured noise model (`noise`), not the fixed transform an edit saved
    /// before `PROCESS_VERSION` 8 was denoised with.
    pub noise_model: bool,
    /// Mask layers that change the noise reduction or the sharpening where they are.
    pub layers: Vec<LayerDetail>,
}

/// A photo's noise, measured once and kept with its source: the noise level after the fixed transform, and
/// the photo's noise model with the level after its own transform (about 1, by construction) when the model
/// could be measured.
#[derive(Clone, Copy, Debug)]
pub struct Noise {
    pub fixed_sigma: f32,
    pub model: Option<(NoiseModel, f32)>,
}

/// The stabilization a denoise runs under.
#[derive(Clone, Copy, Debug)]
pub enum Stabilization {
    /// `y = 2·sqrt(luminance + ε)`, and the noise level measured after it.
    Fixed(f32),
    /// The generalized Anscombe transform for this model, and the noise level measured after it.
    Model(NoiseModel, f32),
}

impl Stabilization {
    fn params(&self, size: [u32; 2]) -> DetailParams {
        match *self {
            Self::Fixed(_) => DetailParams {
                size,
                ..Default::default()
            },
            Self::Model(m, _) => DetailParams {
                size,
                a: m.gain,
                b: m.read,
                c: 1.0,
                ..Default::default()
            },
        }
    }

    fn sigma(&self) -> f32 {
        match *self {
            Self::Fixed(s) | Self::Model(_, s) => s,
        }
    }
}

impl Noise {
    /// The stabilization to denoise with: the photo's model when asked for and measured, the fixed one
    /// otherwise.
    pub fn stabilization(&self, model: bool) -> Stabilization {
        match self.model {
            Some((m, sigma)) if model => Stabilization::Model(m, sigma),
            _ => Stabilization::Fixed(self.fixed_sigma),
        }
    }
}

impl DetailSettings {
    /// The most luminance noise reduction any pixel gets: the global amount plus every layer that adds.
    fn most_denoise(&self) -> f32 {
        self.denoise_luminance
            + self
                .layers
                .iter()
                .map(|l| l.noise.max(0.0) * l.amount)
                .sum::<f32>()
    }

    fn most_sharpen(&self) -> f32 {
        self.sharpen_amount
            + self
                .layers
                .iter()
                .map(|l| l.sharpness.max(0.0) * l.amount)
                .sum::<f32>()
    }

    pub fn is_noop(&self) -> bool {
        self.most_denoise() <= 0.0 && self.denoise_chroma <= 0.0 && self.most_sharpen() <= 0.0
    }

    /// Pixels of context a tile needs so its centre matches a full-image render.
    pub fn margin(&self) -> u32 {
        let denoise = if self.most_denoise() > 0.0 || self.denoise_chroma > 0.0 {
            64
        } else {
            0
        };
        let sharpen = if self.most_sharpen() > 0.0 {
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
                &[
                    B::Uniform,
                    B::Tex,
                    B::Tex,
                    B::Tex,
                    B::Store(F::Rgba32Float),
                    B::Tex,
                ],
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
                &[
                    B::Uniform,
                    B::Tex,
                    B::Tex,
                    B::Tex,
                    B::Store(F::Rgba16Float),
                    B::Tex,
                ],
            )?,
            crop: k(
                "detail/crop.wgsl",
                &[B::Uniform, B::Tex, B::Store(F::Rgba16Float)],
            )?,
            strength: k(
                "detail/strength.wgsl",
                &[
                    B::Uniform,
                    B::Samp,
                    B::TexF,
                    B::TexF,
                    B::TexF,
                    B::TexF,
                    B::TexF,
                    B::TexF,
                    B::TexF,
                    B::TexF,
                    B::Store(F::Rgba32Float),
                ],
            )?,
            sampler: gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            no_mask: crate::upload_rgba16(
                gpu,
                &crate::RgbaF32 {
                    width: 1,
                    height: 1,
                    data: vec![0.0; 4],
                },
            ),
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

    /// The photo's noise (`Noise`), from a central crop of at most 1024²: its noise model (`noise::estimate`
    /// on the crop's luminance), and the noise level of the stabilized luma at scale 0 under each transform,
    /// from the median absolute detail.
    pub fn noise(&self, gpu: &Gpu, src: &wgpu::Texture) -> Result<Noise> {
        let (w, h) = (src.width().min(1024), src.height().min(1024));
        let tile = self.crop(
            gpu,
            src,
            (src.width() - w) / 2,
            (src.height() - h) / 2,
            w,
            h,
        );
        let lum: Vec<f32> = read_rgba16(gpu, &tile)?
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| (p[0] + p[1] + p[2]) / 3.0)
            .collect();
        let fixed_sigma = self.stabilized_sigma(gpu, &tile, &Stabilization::Fixed(0.0))?;
        let model = match crate::noise::estimate(&lum, w as usize, h as usize) {
            Some((m, _)) => {
                let sigma = self.stabilized_sigma(gpu, &tile, &Stabilization::Model(m, 0.0))?;
                Some((m, sigma))
            }
            None => None,
        };
        Ok(Noise { fixed_sigma, model })
    }

    /// Noise level of the stabilized luma at scale 0 under `stab`, from the median absolute detail of `tile`.
    fn stabilized_sigma(
        &self,
        gpu: &Gpu,
        tile: &wgpu::Texture,
        stab: &Stabilization,
    ) -> Result<f32> {
        let (w, h) = (tile.width(), tile.height());
        let size = [w, h];
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let opp = texture(gpu, w, h, F::Rgba32Float);
        self.to_opp.run(
            gpu,
            &mut enc,
            &stab.params(size),
            size,
            &[Res::View(&view(tile)), Res::View(&view(&opp))],
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

    /// Denoise `src`: luminance by the strength map's red at each pixel (`strength`, see `strength_map`), chroma
    /// by `chroma` (0…100) everywhere.
    pub fn denoise(
        &self,
        gpu: &Gpu,
        src: &wgpu::Texture,
        stab: &Stabilization,
        strength: &wgpu::Texture,
        chroma: f32,
    ) -> wgpu::Texture {
        let (w, h) = (src.width(), src.height());
        let size = [w, h];
        let sigma = stab.sigma();
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let mut fine = texture(gpu, w, h, F::Rgba32Float);
        self.to_opp.run(
            gpu,
            &mut enc,
            &stab.params(size),
            size,
            &[Res::View(&view(src)), Res::View(&view(&fine))],
        );
        let mut acc = texture(gpu, w, h, F::Rgba32Float);
        // Full strength; the map scales the luminance threshold at each pixel.
        let (kl, kc) = (3.0, 4.0 * chroma.clamp(0.0, 100.0) / 100.0);
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
                    Res::View(&view(strength)),
                ],
            );
            acc = next_acc;
            fine = coarse;
        }
        let out = texture(gpu, w, h, F::Rgba16Float);
        self.from_opp.run(
            gpu,
            &mut enc,
            &stab.params(size),
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

    /// Sharpen `src` by the strength map's green at each pixel (`strength`).
    pub fn sharpen(
        &self,
        gpu: &Gpu,
        src: &wgpu::Texture,
        strength: &wgpu::Texture,
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
            &Self::p(size),
            size,
            &[
                Res::View(&view(src)),
                Res::View(&view(&observed)),
                Res::View(&view(&estimate)),
                Res::View(&view(&out)),
                Res::View(&view(strength)),
            ],
        );
        gpu.queue.submit([enc.finish()]);
        out
    }

    /// The strengths over a texture `size` pixels big: luminance noise reduction in red and sharpening in green,
    /// 0…1. 1×1 when no layer changes them (the global settings everywhere); otherwise drawn from the masks
    /// (`detail/strength.wgsl`).
    pub fn strength_map(
        &self,
        gpu: &Gpu,
        s: &DetailSettings,
        masks: Option<&DetailMasks>,
        size: [u32; 2],
    ) -> wgpu::Texture {
        let base = [
            s.denoise_luminance.clamp(0.0, 100.0) / 100.0,
            s.sharpen_amount.clamp(0.0, 100.0) / 100.0,
        ];
        let layered = masks.filter(|_| !s.layers.is_empty());
        let Some(m) = layered else {
            return constant(gpu, [base[0], base[1], 0.0, 1.0]);
        };
        let mut u = DetailLayers {
            frame: m.frame,
            base: [base[0], base[1], 0.0, 0.0],
            ..Default::default()
        };
        for l in s.layers.iter().filter(|l| l.slot < 8) {
            u.layers[l.slot] = [
                l.noise.clamp(-100.0, 100.0) / 100.0,
                l.sharpness.clamp(-100.0, 100.0) / 100.0,
                l.amount.clamp(0.0, 1.0),
                if l.invert { 1.0 } else { 0.0 },
            ];
        }
        let views: Vec<wgpu::TextureView> = (0..8)
            .map(|i| {
                view(
                    m.textures
                        .get(i)
                        .copied()
                        .flatten()
                        .unwrap_or(&self.no_mask),
                )
            })
            .collect();
        let out = texture(gpu, size[0], size[1], F::Rgba32Float);
        let ov = view(&out);
        let mut res = vec![Res::Sampler(&self.sampler)];
        res.extend(views.iter().map(Res::View));
        res.push(Res::View(&ov));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        self.strength.run(gpu, &mut enc, &u, size, &res);
        gpu.queue.submit([enc.finish()]);
        out
    }

    /// Apply denoise then sharpening as configured, through the mask layers when `masks` gives them; returns the
    /// source unchanged when both are off everywhere.
    pub fn apply(
        &self,
        gpu: &Gpu,
        src: wgpu::Texture,
        noise: &Noise,
        s: &DetailSettings,
        masks: Option<&DetailMasks>,
    ) -> wgpu::Texture {
        if s.is_noop() {
            return src;
        }
        let strength = self.strength_map(gpu, s, masks, [src.width(), src.height()]);
        let mut cur = src;
        if s.most_denoise() > 0.0 || s.denoise_chroma > 0.0 {
            let stab = noise.stabilization(s.noise_model);
            cur = self.denoise(gpu, &cur, &stab, &strength, s.denoise_chroma);
        }
        if s.most_sharpen() > 0.0 {
            cur = self.sharpen(gpu, &cur, &strength, s.sharpen_radius, s.sharpen_iterations);
        }
        cur
    }
}

/// A 1×1 RGBA32F texture holding `v`: a strength map that is the same everywhere — `[luminance noise
/// reduction, sharpening, 0, 1]`, each 0…1.
pub fn constant(gpu: &Gpu, v: [f32; 4]) -> wgpu::Texture {
    use wgpu::util::DeviceExt;
    gpu.device.create_texture_with_data(
        &gpu.queue,
        &wgpu::TextureDescriptor {
            label: Some("detail strength"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: F::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(&v),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A layer whose mask covers the left half denoises there and leaves the right half exactly as it was, with
    /// no global noise reduction at all; inverted, it does the other half; at half its amount, half as much.
    #[test]
    fn a_layer_denoises_where_its_mask_is() {
        let Ok(gpu) = crate::test_gpu() else { return };
        let detail = Detail::new(gpu).unwrap();
        let (w, h) = (256u32, 256u32);
        let mut state = 777u64;
        let mut rnd = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % 10_000) as f32 / 10_000.0 - 0.5
        };
        let img = crate::RgbaF32 {
            width: w,
            height: h,
            data: (0..w * h)
                .flat_map(|_| {
                    [
                        0.2 + 0.03 * rnd(),
                        0.2 + 0.03 * rnd(),
                        0.2 + 0.03 * rnd(),
                        0.0,
                    ]
                })
                .collect(),
        };
        let tex = crate::upload_rgba16(gpu, &img);
        let noise = detail.noise(gpu, &tex).unwrap();
        let mask = crate::upload_rgba16(
            gpu,
            &crate::RgbaF32 {
                width: w,
                height: h,
                data: (0..w * h)
                    .flat_map(|i| {
                        let v = if i % w < w / 2 { 1.0 } else { 0.0 };
                        [v, v, v, 1.0]
                    })
                    .collect(),
            },
        );
        let masks = DetailMasks {
            textures: vec![Some(&mask)],
            frame: [0.0, 0.0, w as f32, h as f32],
        };
        let run = |invert: bool, amount: f32| {
            let s = DetailSettings {
                denoise_luminance: 0.0,
                denoise_chroma: 0.0,
                sharpen_amount: 0.0,
                sharpen_radius: 1.0,
                sharpen_iterations: 10,
                noise_model: false,
                layers: vec![LayerDetail {
                    slot: 0,
                    noise: 100.0,
                    sharpness: 0.0,
                    amount,
                    invert,
                }],
            };
            let tex = crate::upload_rgba16(gpu, &img);
            read_rgba16(gpu, &detail.apply(gpu, tex, &noise, &s, Some(&masks))).unwrap()
        };
        // Luminance noise over a column band, inside its 16 px margin from the seam.
        let spread = |img: &crate::RgbaF32, x0: u32, x1: u32| {
            let v: Vec<f32> = (16..h - 16)
                .flat_map(|y| (x0..x1).map(move |x| (y, x)))
                .map(|(y, x)| {
                    let i = ((y * w + x) * 4) as usize;
                    (img.data[i] + img.data[i + 1] + img.data[i + 2]) / 3.0
                })
                .collect();
            let m = v.iter().sum::<f32>() / v.len() as f32;
            (v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
        };
        let before = (spread(&img, 16, 112), spread(&img, 144, 240));
        let left = run(false, 1.0);
        let right = run(true, 1.0);
        let half = run(false, 0.5);
        let after = (spread(&left, 16, 112), spread(&left, 144, 240));
        eprintln!(
            "noise {before:?} → layer {after:?}, inverted ({:.4}, {:.4}), half amount {:.4}",
            spread(&right, 16, 112),
            spread(&right, 144, 240),
            spread(&half, 16, 112)
        );
        assert!(after.0 < 0.6 * before.0, "masked side denoised");
        assert!(
            (after.1 - before.1).abs() < 0.03 * before.1,
            "unmasked side as it was"
        );
        assert!(
            spread(&right, 144, 240) < 0.6 * before.1
                && (spread(&right, 16, 112) - before.0).abs() < 0.03 * before.0
        );
        let h0 = spread(&half, 16, 112);
        assert!(h0 < before.0 && h0 > after.0, "half amount between: {h0}");
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
        let noise = detail.noise(gpu, &tex).unwrap();
        assert!(noise.fixed_sigma > 0.0);
        let stats = |d: &[f32]| {
            let g: Vec<f32> = d.as_chunks::<4>().0.iter().map(|p| p[1]).collect();
            let mean = g.iter().sum::<f32>() / g.len() as f32;
            let var = g.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / g.len() as f32;
            (mean, var.sqrt())
        };
        let (m0, s0) = stats(&img.data);
        // A flat frame is all one level, which can't separate shot from read noise: the fixed transform is
        // what it gets.
        for model in [false, true] {
            let stab = noise.stabilization(model);
            let out = read_rgba16(
                gpu,
                &detail.denoise(
                    gpu,
                    &tex,
                    &stab,
                    &constant(gpu, [100.0 / 100.0, 0.0, 0.0, 1.0]),
                    100.0,
                ),
            )
            .unwrap();
            let (m1, s1) = stats(&out.data);
            assert!((m1 - m0).abs() < 0.005, "{stab:?}: mean {m0} → {m1}");
            assert!(s1 < s0 * 0.6, "{stab:?}: noise {s0} → {s1}");
        }
    }

    /// Poisson-Gaussian noise over a ramp from deep shadow to highlight: the measured model's transform
    /// denoises every level by about the same factor and keeps every level's mean, where the fixed transform
    /// is tuned to one balance of shot and read noise and treats the ends unevenly.
    #[test]
    fn the_noise_model_denoises_shadows_and_highlights_alike() {
        let Ok(gpu) = crate::test_gpu() else { return };
        let detail = Detail::new(gpu).unwrap();
        let (w, h) = (512u32, 512u32);
        let truth = NoiseModel {
            gain: 2e-3,
            read: 4e-5,
        };
        let mut synth = crate::noise::Synth::new(11);
        // Eight flat bands, each a stop apart, 0.004 to 0.5.
        let level = |y: u32| 0.004 * 2f32.powi((y / 64) as i32);
        let img = crate::RgbaF32 {
            width: w,
            height: h,
            data: (0..h)
                .flat_map(|y| (0..w).map(move |_| y))
                .flat_map(|y| {
                    let v = level(y);
                    [
                        synth.observe(&truth, v),
                        synth.observe(&truth, v),
                        synth.observe(&truth, v),
                        0.0,
                    ]
                })
                .collect(),
        };
        let tex = crate::upload_rgba16(gpu, &img);
        let noise = detail.noise(gpu, &tex).unwrap();
        let (m, sigma) = noise.model.expect("eight levels are enough to fit");
        eprintln!(
            "measured {m:?}, stabilized sigma {sigma} (fixed: {})",
            noise.fixed_sigma
        );
        let band = |d: &[f32], b: u32| {
            let g: Vec<f32> = (b * 64 + 8..b * 64 + 56)
                .flat_map(|y| (8..w - 8).map(move |x| ((y * w + x) * 4 + 1) as usize))
                .map(|i| d[i])
                .collect();
            let mean = g.iter().sum::<f32>() / g.len() as f32;
            let var = g.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / g.len() as f32;
            (mean, var.sqrt())
        };
        let fixed = read_rgba16(
            gpu,
            &detail.denoise(
                gpu,
                &tex,
                &noise.stabilization(false),
                &constant(gpu, [60.0 / 100.0, 0.0, 0.0, 1.0]),
                60.0,
            ),
        )
        .unwrap();
        let stab = noise.stabilization(true);
        let out = read_rgba16(
            gpu,
            &detail.denoise(
                gpu,
                &tex,
                &stab,
                &constant(gpu, [60.0 / 100.0, 0.0, 0.0, 1.0]),
                60.0,
            ),
        )
        .unwrap();
        let mut factors = Vec::new();
        for b in 0..8 {
            let ((m0, s0), (m1, s1), (mf, sf)) =
                (band(&img.data, b), band(&out.data, b), band(&fixed.data, b));
            eprintln!(
                "band {b}: level {:.4} mean {m0:.5} → model {m1:.5} fixed {mf:.5}, noise {s0:.5} → model {s1:.5} fixed {sf:.5}",
                level(b * 64)
            );
            // At a few photo-electrons the noise is skewed and soft thresholding of skewed noise is not quite
            // unbiased; what is asked is that no level's mean moves by more than a tenth of its own noise.
            assert!(
                (m1 - m0).abs() < 0.1 * s0,
                "band {b}: the mean is kept ({m0} → {m1}, noise {s0})"
            );
            factors.push(s1 / s0);
        }
        let (lo, hi) = factors
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), &f| (a.min(f), b.max(f)));
        assert!(
            hi / lo < 2.0,
            "every level denoised alike: factors {factors:?}"
        );
    }
}
