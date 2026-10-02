// SPDX-License-Identifier: Apache-2.0
//! The warp pass: crop, straighten, keystone and quarter turns on the worker (`server/warp.wgsl`), with the
//! map `sidecar_color::geometry` computes — the one the editor's view pass shows.

use anyhow::Result;
use sidecar_color::geometry::WarpParams;
use wgpu::TextureFormat as F;

use crate::Gpu;
use crate::compute::{B, Kernel, Res, texture, view};

pub struct WarpPass {
    kernel: Kernel,
}

impl WarpPass {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        Ok(Self { kernel: Kernel::new(gpu, "server/warp.wgsl", &[B::Uniform, B::Tex, B::Store(F::Rgba16Float)])? })
    }

    /// Resample `src` into a new RGBA16F texture of `params.out_size`.
    pub fn apply(&self, gpu: &Gpu, src: &wgpu::Texture, params: &WarpParams) -> wgpu::Texture {
        let out = texture(gpu, params.out_size[0], params.out_size[1], F::Rgba16Float);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        self.kernel.run(gpu, &mut enc, params, params.out_size, &[Res::View(&view(src)), Res::View(&view(&out))]);
        gpu.queue.submit([enc.finish()]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RgbaF32, read_rgba16, upload_rgba16};
    use sidecar_color::geometry::{Crop, Geometry, output_size, warp_params};

    fn ramp(w: u32, h: u32) -> RgbaF32 {
        RgbaF32 { width: w, height: h, data: (0..w * h).flat_map(|i| [(i % w) as f32, (i / w) as f32, 0.5, 0.0]).collect() }
    }

    /// A crop and a quarter turn read exactly the texels the frame says; orientation still pending on the
    /// input (a sensor-frame texture) is applied on the way.
    #[test]
    fn crops_and_turns_texel_for_texel() {
        let Ok(gpu) = Gpu::new_headless() else { return eprintln!("skipping: no GPU") };
        let pass = WarpPass::new(&gpu).unwrap();
        let (w, h) = (40u32, 20u32);
        let src = upload_rgba16(&gpu, &ramp(w, h));
        let g = Geometry { rotate: 1, crop: Crop { left: 0.25, top: 0.0, right: 0.75, bottom: 0.5 }, ..Default::default() };
        let out_size = output_size(&g, [w, h]);
        assert_eq!(out_size, [10, 20]);
        let out = read_rgba16(&gpu, &pass.apply(&gpu, &src, &warp_params(&g, [w, h], 0, out_size, [0.0; 2], out_size, [0.0; 2]))).unwrap();
        let at = |x: u32, y: u32| [out.data[((y * out.width + x) * 4) as usize], out.data[((y * out.width + x) * 4 + 1) as usize]];
        // Turned clockwise: the output's top-right is the crop's top-left, texel (10, 0).
        assert_eq!(at(9, 0), [10.0, 0.0]);
        assert_eq!(at(0, 0), [10.0, 9.0]);
        assert_eq!(at(0, 19), [29.0, 9.0]);

        // The same picture stored in the sensor's orientation (flip 3: rotated 180°) comes out the same.
        let rotated = ramp(w, h).oriented(3);
        let src = upload_rgba16(&gpu, &rotated);
        let out2 = read_rgba16(&gpu, &pass.apply(&gpu, &src, &warp_params(&g, [w, h], 3, out_size, [0.0; 2], out_size, [0.0; 2]))).unwrap();
        let at2 = |x: u32, y: u32| [out2.data[((y * out2.width + x) * 4) as usize], out2.data[((y * out2.width + x) * 4 + 1) as usize]];
        assert_eq!(at2(9, 0), at(9, 0));
        assert_eq!(at2(3, 7), at(3, 7));
    }
}
