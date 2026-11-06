// SPDX-License-Identifier: Apache-2.0
//! Gaussian and Laplacian pyramids (Burt and Adelson, 1983) on the GPU: the shared primitive under local
//! Laplacian filtering (clarity and texture), multi-band blending (panoramas), exposure fusion and focus
//! stacking. Two WGSL passes, `client/pyr_reduce.wgsl` and `client/pyr_expand.wgsl`, which the browser composes
//! too. Levels are RGBA16F; a level of n texels has ceil(n / 2) above it.
//!
//! A Laplacian pyramid here is the list of band-pass levels, finest first, with the coarsest Gaussian level as
//! its last entry; `collapse` adds them back up. Because expansion is the same pass both ways, collapsing a
//! pyramid returns the image to within half-float rounding.

use anyhow::Result;
use sidecar_uniform::Uniform;
use wgpu::TextureFormat as F;

use crate::Gpu;
use crate::compute::{B, Kernel, Res, texture, view};

/// The pyramid passes' uniform (32 bytes). Its WGSL declaration, `shaders/uniforms/pyramid_params.wgsl`, is
/// generated from this struct (`sidecar-uniform`); the `uniforms` test checks the file and every offset.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable, Uniform)]
pub struct PyramidParams {
    /// The finer level's size, in texels.
    pub fine: [u32; 2],
    /// The coarser level's size, in texels.
    pub coarse: [u32; 2],
    /// Expansion only: 0 write the expansion, 1 the fine level minus it (a Laplacian level), 2 the fine level
    /// plus it (collapse).
    pub mode: u32,
    pub _pad: [u32; 3],
}

/// The size of the level above one of `size` texels.
pub fn coarser(size: [u32; 2]) -> [u32; 2] {
    [size[0].div_ceil(2).max(1), size[1].div_ceil(2).max(1)]
}

/// How many levels a pyramid over `size` can have before its top would be smaller than `min` texels on its
/// short edge, counting the image itself as the first.
pub fn levels_for(size: [u32; 2], min: u32) -> usize {
    let mut n = 1;
    let mut s = size;
    while s[0].min(s[1]) / 2 >= min.max(1) {
        s = coarser(s);
        n += 1;
    }
    n
}

pub struct Pyramid {
    reduce: Kernel,
    expand: Kernel,
}

fn size_of(t: &wgpu::Texture) -> [u32; 2] {
    [t.width(), t.height()]
}

impl Pyramid {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        let store = B::Store(F::Rgba16Float);
        Ok(Self {
            reduce: Kernel::new(gpu, "client/pyr_reduce.wgsl", &[B::Uniform, B::Tex, store])?,
            expand: Kernel::new(
                gpu,
                "client/pyr_expand.wgsl",
                &[B::Uniform, B::Tex, B::Tex, store],
            )?,
        })
    }

    fn reduce_into(
        &self,
        gpu: &Gpu,
        enc: &mut wgpu::CommandEncoder,
        fine: &wgpu::Texture,
    ) -> wgpu::Texture {
        let (f, c) = (size_of(fine), coarser(size_of(fine)));
        let out = texture(gpu, c[0], c[1], F::Rgba16Float);
        let p = PyramidParams {
            fine: f,
            coarse: c,
            ..Default::default()
        };
        self.reduce.run(
            gpu,
            enc,
            &p,
            c,
            &[Res::View(&view(fine)), Res::View(&view(&out))],
        );
        out
    }

    fn expand_into(
        &self,
        gpu: &Gpu,
        enc: &mut wgpu::CommandEncoder,
        coarse: &wgpu::Texture,
        fine: &wgpu::Texture,
        mode: u32,
    ) -> wgpu::Texture {
        let f = size_of(fine);
        let out = texture(gpu, f[0], f[1], F::Rgba16Float);
        let p = PyramidParams {
            fine: f,
            coarse: size_of(coarse),
            mode,
            ..Default::default()
        };
        self.expand.run(
            gpu,
            enc,
            &p,
            f,
            &[
                Res::View(&view(coarse)),
                Res::View(&view(fine)),
                Res::View(&view(&out)),
            ],
        );
        out
    }

    /// The Gaussian pyramid of `image` above its first level: level 0 is `image` itself and is not repeated,
    /// so a pyramid of `levels` levels returns `levels − 1` textures, level 1 first. `levels_for` picks a count.
    pub fn gaussian(&self, gpu: &Gpu, image: &wgpu::Texture, levels: usize) -> Vec<wgpu::Texture> {
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let mut out: Vec<wgpu::Texture> = Vec::new();
        for _ in 1..levels.max(1) {
            let next = self.reduce_into(gpu, &mut enc, out.last().unwrap_or(image));
            out.push(next);
        }
        gpu.queue.submit([enc.finish()]);
        out
    }

    /// The Laplacian pyramid of `image` with `levels` levels: band-pass levels finest first, then the coarsest
    /// Gaussian level, which carries the image's mean.
    pub fn laplacian(&self, gpu: &Gpu, image: &wgpu::Texture, levels: usize) -> Vec<wgpu::Texture> {
        let gauss = self.gaussian(gpu, image, levels);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let mut out = Vec::with_capacity(gauss.len() + 1);
        for (i, coarse) in gauss.iter().enumerate() {
            let fine = if i == 0 { image } else { &gauss[i - 1] };
            out.push(self.expand_into(gpu, &mut enc, coarse, fine, 1));
        }
        gpu.queue.submit([enc.finish()]);
        out.push(
            gauss
                .into_iter()
                .last()
                .expect("a Laplacian pyramid needs at least two levels"),
        );
        out
    }

    /// Add a Laplacian pyramid back up into an image the size of its first level.
    pub fn collapse(&self, gpu: &Gpu, pyramid: &[wgpu::Texture]) -> wgpu::Texture {
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        let mut iter = pyramid.iter().rev();
        let top = iter.next().expect("an empty pyramid");
        let mut acc: Option<wgpu::Texture> = None;
        for band in iter {
            let coarse = acc.as_ref().unwrap_or(top);
            acc = Some(self.expand_into(gpu, &mut enc, coarse, band, 2));
        }
        gpu.queue.submit([enc.finish()]);
        acc.expect("a Laplacian pyramid needs at least two levels")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RgbaF32, read_rgba16, upload_rgba16};

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> f32) -> RgbaF32 {
        RgbaF32 {
            width: w,
            height: h,
            data: (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .flat_map(|(x, y)| {
                    let v = f(x, y);
                    [v, 0.5 * v, 1.0 - v, 1.0]
                })
                .collect(),
        }
    }

    #[test]
    fn level_sizes_halve_rounding_up() {
        assert_eq!(coarser([101, 64]), [51, 32]);
        assert_eq!(coarser([1, 1]), [1, 1]);
        assert_eq!(levels_for([1024, 768], 8), 7);
        assert_eq!(levels_for([16, 16], 16), 1);
    }

    /// Collapsing the Laplacian pyramid gives the image back, at an odd size and with fine detail; each band
    /// level carries the detail of its own scale only, so a flat image's bands are zero.
    #[test]
    fn laplacian_round_trips_and_flat_has_no_bands() {
        let Ok(gpu) = crate::test_gpu() else {
            return eprintln!("skipping: no GPU");
        };
        let p = Pyramid::new(gpu).unwrap();
        let (w, h) = (203u32, 117u32);
        let img = image(w, h, |x, y| {
            0.5 + 0.3 * ((x as f32 * 0.37).sin() * (y as f32 * 0.21).cos())
                + if (x / 9 + y / 7) % 2 == 0 { 0.1 } else { 0.0 }
        });
        let levels = levels_for([w, h], 4);
        let pyr = p.laplacian(gpu, &upload_rgba16(gpu, &img), levels);
        assert_eq!(pyr.len(), levels);
        assert_eq!(size_of(&pyr[1]), [102, 59]);
        let back = read_rgba16(gpu, &p.collapse(gpu, &pyr)).unwrap();
        let worst = back
            .data
            .iter()
            .zip(&img.data)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        eprintln!("pyramid round trip: largest error {worst:.2e} over {levels} levels");
        assert!(
            worst < 3e-3,
            "collapse(laplacian(x)) = x, to half floats: {worst}"
        );

        let flat = image(w, h, |_, _| 0.42);
        let pyr = p.laplacian(gpu, &upload_rgba16(gpu, &flat), levels);
        for band in &pyr[..pyr.len() - 1] {
            let b = read_rgba16(gpu, band).unwrap();
            assert!(
                b.data.iter().all(|v| v.abs() < 1e-3),
                "a flat image has no detail at any scale"
            );
        }
        let top = read_rgba16(gpu, pyr.last().unwrap()).unwrap();
        assert!(
            (top.data[0] - 0.42).abs() < 1e-3,
            "and its top level is its value"
        );
    }
}
