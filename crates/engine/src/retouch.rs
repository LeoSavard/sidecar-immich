// SPDX-License-Identifier: Apache-2.0
//! Retouch (DESIGN.md §15, "Retouch"): heal, clone and red-eye spots on the photo's camera-native linear light,
//! before the lens pass, in order — a spot can take from what an earlier one healed. Each spot runs
//! `client/retouch.wgsl` over its own box and the box is copied back, so the cost is the spots' area, not the
//! photo's. The browser runs the same pass (`web/src/lib/gpu/retouch.ts`).

use anyhow::Result;
use sidecar_color::params::{Spot, SpotKind};
use sidecar_uniform::Uniform;
use wgpu::TextureFormat as F;

use crate::Gpu;
use crate::compute::{B, Kernel, Res, texture, view};

/// One spot's uniform (64 bytes). Its WGSL declaration, `shaders/uniforms/retouch_params.wgsl`, is generated from
/// this struct (`sidecar-uniform`); the `uniforms` test checks the file and every offset.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable, Uniform)]
pub struct RetouchParams {
    /// The white balance's camera multipliers, for red eye's test; w unused.
    pub wb: [f32; 4],
    /// The spot's centre and its source's, in pixels.
    pub centre: [f32; 2],
    pub take_from: [f32; 2],
    /// Pixels.
    pub radius: f32,
    /// 0…1: the share of the radius the edge fades over.
    pub feather: f32,
    /// 0…1.
    pub opacity: f32,
    /// 0 heal, 1 clone, 2 red eye.
    pub kind: u32,
    /// The box this dispatch covers: its corner and size, in pixels.
    pub origin: [u32; 2],
    pub size: [u32; 2],
}

pub struct RetouchPass {
    spot: Kernel,
    sampler: wgpu::Sampler,
}

/// A spot in a texture's pixels, oriented as that texture is.
#[derive(Clone, Copy, Debug)]
pub struct PixelSpot {
    pub kind: SpotKind,
    pub at: [f32; 2],
    pub from: [f32; 2],
    pub radius: f32,
    pub feather: f32,
    pub opacity: f32,
}

impl PixelSpot {
    /// `s` in a texture `size` pixels across, its positions mapped by `place` (0…1 in the edit's frame → 0…1 in
    /// the texture's; the identity for an oriented texture).
    pub fn new(s: &Spot, size: [u32; 2], place: impl Fn([f64; 2]) -> [f64; 2]) -> Self {
        let px = |p: [f64; 2]| {
            let q = place(p);
            [
                (q[0] * size[0] as f64) as f32,
                (q[1] * size[1] as f64) as f32,
            ]
        };
        Self {
            kind: s.kind,
            at: px(s.at),
            from: px(s.from),
            radius: (s.radius.max(0.0) * size[0].max(size[1]) as f64) as f32,
            feather: (s.feather.clamp(0.0, 100.0) / 100.0) as f32,
            opacity: (s.opacity.clamp(0.0, 100.0) / 100.0) as f32,
        }
    }
}

impl RetouchPass {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        Ok(Self {
            spot: Kernel::new(
                gpu,
                "client/retouch.wgsl",
                &[B::Uniform, B::TexF, B::Samp, B::Store(F::Rgba16Float)],
            )?,
            sampler: gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        })
    }

    /// A copy of `src` (RGBA16F) with `spots` applied in order.
    pub fn apply(
        &self,
        gpu: &Gpu,
        src: &wgpu::Texture,
        spots: &[PixelSpot],
        wb: [f32; 3],
    ) -> wgpu::Texture {
        let (w, h) = (src.width(), src.height());
        // The image as it stands (copied back into after each spot) and the scratch each spot writes.
        let image = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("retouched"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: F::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let scratch = texture(gpu, w, h, F::Rgba16Float);
        let all = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_texture(src.as_image_copy(), image.as_image_copy(), all);
        let (iv, sv) = (view(&image), view(&scratch));
        for s in spots {
            // The box: the spot, and for heal its edge samples' reach (they read outside it, but only read).
            let reach = s.radius * 1.12 + 2.0;
            let x0 = (s.at[0] - reach).floor().clamp(0.0, w as f32) as u32;
            let y0 = (s.at[1] - reach).floor().clamp(0.0, h as f32) as u32;
            let x1 = (s.at[0] + reach).ceil().clamp(0.0, w as f32) as u32;
            let y1 = (s.at[1] + reach).ceil().clamp(0.0, h as f32) as u32;
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let p = RetouchParams {
                wb: [wb[0], wb[1], wb[2], 0.0],
                centre: s.at,
                take_from: s.from,
                radius: s.radius.max(0.5),
                feather: s.feather,
                opacity: s.opacity,
                kind: match s.kind {
                    SpotKind::Heal => 0,
                    SpotKind::Clone => 1,
                    SpotKind::RedEye => 2,
                },
                origin: [x0, y0],
                size: [x1 - x0, y1 - y0],
            };
            self.spot.run(
                gpu,
                &mut enc,
                &p,
                p.size,
                &[Res::View(&iv), Res::Sampler(&self.sampler), Res::View(&sv)],
            );
            let corner = wgpu::Origin3d { x: x0, y: y0, z: 0 };
            enc.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &scratch,
                    mip_level: 0,
                    origin: corner,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &image,
                    mip_level: 0,
                    origin: corner,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: x1 - x0,
                    height: y1 - y0,
                    depth_or_array_layers: 1,
                },
            );
        }
        gpu.queue.submit([enc.finish()]);
        image
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RgbaF32, read_rgba16, upload_rgba16};

    /// A textured surface under light falling off across it, with a dark blemish: healed from a patch lit two
    /// thirds of a stop differently, the spot is nearly the clean surface — far nearer than a clone of the same
    /// patch, whose light is wrong — and nothing outside the spot moves.
    #[test]
    fn heal_matches_the_light_clone_does_not() {
        let Ok(gpu) = crate::test_gpu() else {
            return eprintln!("skipping: no GPU");
        };
        let pass = RetouchPass::new(gpu).unwrap();
        let (w, h) = (256u32, 128u32);
        let texture = |x: u32, y: u32| 1.0 + 0.15 * (((x * 7 + y * 13) % 11) as f32 / 11.0 - 0.5);
        let light = |x: u32| 0.4 * (-(x as f32) / 120.0).exp2();
        let clean = |x: u32, y: u32| light(x) * texture(x, y);
        let blemish =
            |x: u32, y: u32| ((x as f32 - 60.0).hypot(y as f32 - 64.0) < 9.0) as u32 as f32;
        let img = RgbaF32 {
            width: w,
            height: h,
            data: (0..w * h)
                .flat_map(|i| {
                    let (x, y) = (i % w, i / w);
                    let v = clean(x, y) * (1.0 - 0.8 * blemish(x, y));
                    [v * 1.1, v, v * 0.9, 0.0]
                })
                .collect(),
        };
        let spot = |kind| PixelSpot {
            kind,
            at: [60.0, 64.0],
            from: [140.0, 64.0],
            radius: 14.0,
            feather: 0.0,
            opacity: 1.0,
        };
        let run = |kind| {
            read_rgba16(
                gpu,
                &pass.apply(gpu, &upload_rgba16(gpu, &img), &[spot(kind)], [1.0; 3]),
            )
            .unwrap()
        };
        let err = |out: &RgbaF32| {
            let (mut e, mut n) = (0f64, 0f64);
            for y in 52..76 {
                for x in 48..72 {
                    let g = out.data[((y * w + x) * 4 + 1) as usize];
                    e += ((g / clean(x, y)) as f64).log2().abs();
                    n += 1.0;
                }
            }
            e / n
        };
        let (healed, cloned) = (run(SpotKind::Heal), run(SpotKind::Clone));
        eprintln!(
            "mean error in stops over the spot: blemish {:.3}, cloned {:.3}, healed {:.3}",
            err(&img),
            err(&cloned),
            err(&healed)
        );
        assert!(err(&healed) < 0.1, "healed {}", err(&healed));
        assert!(err(&healed) < 0.3 * err(&cloned));
        // Outside the spot, nothing moved (against the photo as uploaded, in half floats).
        let as_uploaded = read_rgba16(gpu, &upload_rgba16(gpu, &img)).unwrap();
        for (x, y) in [(10u32, 10u32), (100, 64), (200, 100)] {
            let i = ((y * w + x) * 4 + 1) as usize;
            assert_eq!(healed.data[i], as_uploaded.data[i]);
        }
    }

    /// Red eye takes a red pupil's red down to its green and blue, and leaves a neutral one alone.
    #[test]
    fn red_eye_takes_the_red_out() {
        let Ok(gpu) = crate::test_gpu() else {
            return eprintln!("skipping: no GPU");
        };
        let pass = RetouchPass::new(gpu).unwrap();
        let (w, h) = (64u32, 32u32);
        let img = RgbaF32 {
            width: w,
            height: h,
            data: (0..w * h)
                .flat_map(|i| {
                    if i % w < 32 {
                        [0.6, 0.1, 0.1, 0.0]
                    } else {
                        [0.2, 0.2, 0.2, 0.0]
                    }
                })
                .collect(),
        };
        let spot = |x: f32| PixelSpot {
            kind: SpotKind::RedEye,
            at: [x, 16.0],
            from: [0.0; 2],
            radius: 8.0,
            feather: 0.0,
            opacity: 1.0,
        };
        let out = read_rgba16(
            gpu,
            &pass.apply(
                gpu,
                &upload_rgba16(gpu, &img),
                &[spot(16.0), spot(48.0)],
                [1.0; 3],
            ),
        )
        .unwrap();
        let px = |x: u32| &out.data[((16 * w + x) * 4) as usize..((16 * w + x) * 4 + 3) as usize];
        assert!((px(16)[0] - 0.1).abs() < 0.01, "red pupil {:?}", px(16));
        assert!(
            (px(48)[0] - 0.2).abs() < 0.01,
            "neutral left alone {:?}",
            px(48)
        );
    }
}
