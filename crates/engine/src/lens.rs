// SPDX-License-Identifier: Apache-2.0
//! The lens pass on the server: the same composed WGSL the browser runs (`client/lens.wgsl`), over a whole
//! image for export or over a crop of one for a loupe tile.

use anyhow::Result;
use sidecar_color::lens::LensParams;
use wgpu::TextureFormat as F;

use crate::Gpu;
use crate::compute::{B, Kernel, Res, texture, view};

pub struct LensPass {
    kernel: Kernel,
}

impl LensPass {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        Ok(Self { kernel: Kernel::new(gpu, "client/lens.wgsl", &[B::Uniform, B::Tex, B::Store(F::Rgba16Float)])? })
    }

    /// Correct `src` into a new RGBA16F texture of `size`. `params` places both textures in the frame (see
    /// `sidecar_color::lens::pack`).
    pub fn apply(&self, gpu: &Gpu, src: &wgpu::Texture, params: &LensParams, size: [u32; 2]) -> wgpu::Texture {
        let out = texture(gpu, size[0], size[1], F::Rgba16Float);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        self.kernel.run(gpu, &mut enc, params, size, &[Res::View(&view(src)), Res::View(&view(&out))]);
        gpu.queue.submit([enc.finish()]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RgbaF32, read_rgba16, upload_rgba16};
    use sidecar_color::lens::{Curve, LensProfile, effective, pack, source_bounds};
    use sidecar_color::params::Lens;

    fn profile() -> LensProfile {
        LensProfile {
            id: "test".into(),
            label: "test".into(),
            center: [0.5, 0.5],
            distortion: Some(Curve(vec![[0.0, 1.0], [1.0, 0.9]])),
            ca_red: Some(Curve(vec![[0.0, 1.0], [1.0, 1.002]])),
            ca_blue: None,
            vignetting: Some(Curve(vec![[0.0, 1.0], [1.0, 2.0]])),
            required: false,
        }
    }

    /// A grid of bright dots on grey: each dot's position after the pass must be where the model says the
    /// point it reads from is, and the vignetting gain must brighten the grey towards the corners.
    #[test]
    fn corrects_on_the_gpu_as_the_model_says() {
        let Ok(gpu) = Gpu::new_headless() else { return eprintln!("skipping: no GPU") };
        let pass = LensPass::new(&gpu).unwrap();
        let (w, h) = (300u32, 200u32);
        let mut data = vec![0.1f32; (w * h * 4) as usize];
        for p in data.chunks_mut(4) {
            p[3] = 0.0;
        }
        let e = effective(&profile(), &Lens::default(), w as f64 / h as f64);
        let params = pack(&e, [0.5, 0.5], [w, h], 0, [0.0; 2], [0.0; 2]);
        let out = read_rgba16(&gpu, &pass.apply(&gpu, &upload_rgba16(&gpu, &RgbaF32 { width: w, height: h, data: data.clone() }), &params, [w, h])).unwrap();
        let at = |img: &RgbaF32, x: u32, y: u32| img.data[((y * img.width + x) * 4) as usize];
        // Centre untouched, corner brightened by the gain the model gives the radius it reads from.
        assert!((at(&out, 150, 100) - 0.1).abs() < 1e-3, "centre {}", at(&out, 150, 100));
        let corner = at(&out, 2, 2);
        assert!(corner > 0.14 && corner < 0.2, "corner gain {}", corner / 0.1);

        // A single bright pixel in the source shows up where the corrected image reads it from.
        let (sx, sy) = (40u32, 30u32);
        let mut dot = vec![0.0f32; (w * h * 4) as usize];
        dot[((sy * w + sx) * 4 + 1) as usize] = 1.0;
        let flat = { let mut p = profile(); p.vignetting = None; p };
        let e = effective(&flat, &Lens::default(), w as f64 / h as f64);
        let params = pack(&e, [0.5, 0.5], [w, h], 0, [0.0; 2], [0.0; 2]);
        let out = read_rgba16(&gpu, &pass.apply(&gpu, &upload_rgba16(&gpu, &RgbaF32 { width: w, height: h, data: dot }), &params, [w, h])).unwrap();
        let (mut best, mut bx, mut by) = (0.0, 0, 0);
        for y in 0..h {
            for x in 0..w {
                let g = out.data[((y * w + x) * 4 + 1) as usize];
                if g > best {
                    (best, bx, by) = (g, x, y);
                }
            }
        }
        // Where does the brightest corrected pixel read from? Within a pixel of the dot.
        let (x0, y0, x1, y1) = source_bounds(&params, (bx, by, 1, 1));
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        assert!((cx - (sx as f64 + 0.5)).abs() < 1.0 && (cy - (sy as f64 + 0.5)).abs() < 1.0, "peak at {bx},{by} reads {cx:.1},{cy:.1}");
    }

    /// A loupe tile corrected from a crop of the source matches the same rectangle of a whole-image pass.
    #[test]
    fn a_tile_from_a_crop_matches_the_whole_image() {
        let Ok(gpu) = Gpu::new_headless() else { return eprintln!("skipping: no GPU") };
        let pass = LensPass::new(&gpu).unwrap();
        let (w, h) = (256u32, 160u32);
        let data: Vec<f32> = (0..w * h).flat_map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            [(x * 0.07).sin() * 0.4 + 0.5, (y * 0.05).cos() * 0.4 + 0.5, ((x + y) * 0.03).sin() * 0.4 + 0.5, 0.0]
        }).collect();
        let whole_src = RgbaF32 { width: w, height: h, data };
        let e = effective(&profile(), &Lens::default(), w as f64 / h as f64);
        let whole = read_rgba16(&gpu, &pass.apply(&gpu, &upload_rgba16(&gpu, &whole_src), &pack(&e, [0.5, 0.5], [w, h], 0, [0.0; 2], [0.0; 2]), [w, h])).unwrap();

        let tile = (10u32, 12u32, 64u32, 48u32);
        let frame = pack(&e, [0.5, 0.5], [w, h], 0, [0.0; 2], [0.0; 2]);
        let (x0, y0, x1, y1) = source_bounds(&frame, tile);
        let (cx, cy) = ((x0.floor() as i64 - 3).max(0) as u32, (y0.floor() as i64 - 3).max(0) as u32);
        let (cw, ch) = (((x1.ceil() as u32 + 3).min(w)) - cx, ((y1.ceil() as u32 + 3).min(h)) - cy);
        let crop: Vec<f32> = (cy..cy + ch).flat_map(|y| whole_src.data[((y * w + cx) * 4) as usize..((y * w + cx + cw) * 4) as usize].to_vec()).collect();
        let params = pack(&e, [0.5, 0.5], [w, h], 0, [tile.0 as f64, tile.1 as f64], [cx as f64, cy as f64]);
        let got = read_rgba16(&gpu, &pass.apply(&gpu, &upload_rgba16(&gpu, &RgbaF32 { width: cw, height: ch, data: crop }), &params, [tile.2, tile.3])).unwrap();
        for y in 0..tile.3 {
            for x in 0..tile.2 {
                for c in 0..3 {
                    let a = whole.data[(((tile.1 + y) * w + tile.0 + x) * 4 + c) as usize];
                    let b = got.data[((y * tile.2 + x) * 4 + c) as usize];
                    assert!((a - b).abs() < 2e-3, "({x},{y}) channel {c}: whole {a}, tile {b}");
                }
            }
        }
    }
}
