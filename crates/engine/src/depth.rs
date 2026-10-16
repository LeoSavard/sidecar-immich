// SPDX-License-Identifier: Apache-2.0
//! Lens blur and haze by distance on the server: the same composed WGSL the browser runs
//! (`client/depth_down.wgsl`, `client/depth_gather.wgsl`, `client/depth_compose.wgsl`), over a whole image for
//! export or a tile with its margin for the loupe. Runs on camera-native linear light after the lens pass and
//! before develop.

use anyhow::Result;
use sidecar_color::depth::DepthParams;
use wgpu::TextureFormat as F;

use crate::Gpu;
use crate::compute::{B, Kernel, Res, texture, view};

pub struct DepthPass {
    refine: Kernel,
    down: Kernel,
    gather: Kernel,
    compose: Kernel,
    sampler: wgpu::Sampler,
}

impl DepthPass {
    pub fn new(gpu: &Gpu) -> Result<Self> {
        let store = B::Store(F::Rgba16Float);
        Ok(Self {
            refine: Kernel::new(gpu, "client/depth_refine.wgsl", &[B::Uniform, B::TexF, B::TexF, B::Samp, store])?,
            down: Kernel::new(gpu, "client/depth_down.wgsl", &[B::Uniform, B::Tex, B::TexF, B::Samp, store])?,
            gather: Kernel::new(gpu, "client/depth_gather.wgsl", &[B::Uniform, B::TexF, B::TexF, B::Samp, store])?,
            compose: Kernel::new(gpu, "client/depth_compose.wgsl", &[B::Uniform, B::Tex, B::TexF, B::TexF, B::Samp, store])?,
            sampler: gpu.device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        })
    }

    /// The depth map with the photo's own edges, as a new RGBA16F texture `out` pixels big
    /// (`client/depth_refine.wgsl`). Both `depth` and `photo` are sampled across their whole frame, so neither
    /// has to be that size — the model's map is 518², and this is what upsamples it. `None` when `out` is no
    /// larger than the model's own output, where there is nothing to repair and the caller keeps the map it
    /// has. `sidecar_color::depth::refined_size` is the size to ask for.
    ///
    /// This depends only on the photo and the map, not on any slider, so a caller renders it once and keeps
    /// it — it costs 25 taps a texel.
    pub fn refined(&self, gpu: &Gpu, photo: &wgpu::Texture, depth: &wgpu::Texture, out: [u32; 2]) -> Option<wgpu::Texture> {
        let params = sidecar_color::depth::DepthRefineParams::for_frame(out);
        if !params.worth_it() {
            return None;
        }
        let (w, h) = (out[0].max(1), out[1].max(1));
        let out = texture(gpu, w, h, F::Rgba16Float);
        let (pv, dv, ov) = (view(photo), view(depth), view(&out));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        self.refine.run(gpu, &mut enc, &params, [w, h], &[Res::View(&pv), Res::View(&dv), Res::Sampler(&self.sampler), Res::View(&ov)]);
        gpu.queue.submit([enc.finish()]);
        Some(out)
    }

    /// `src` with the haze and lens blur `params` describe, as a new RGBA16F texture of its size. `depth` is the
    /// depth map (nearness in red) covering the frame `params` places `src` in, lens-corrected as `src` is.
    pub fn apply(&self, gpu: &Gpu, src: &wgpu::Texture, depth: &wgpu::Texture, params: &DepthParams) -> wgpu::Texture {
        let (w, h) = (src.width(), src.height());
        let out = texture(gpu, w, h, F::Rgba16Float);
        let (sv, dv, ov) = (view(src), view(depth), view(&out));
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        // Without blur, compose reads nothing from the low-resolution side: bind the source there.
        let blurred = params.blurs().then(|| {
            let k = params.scale();
            let (sw, sh) = (w.div_ceil(k), h.div_ceil(k));
            let small = texture(gpu, sw, sh, F::Rgba16Float);
            let blurred = texture(gpu, sw, sh, F::Rgba16Float);
            let (smv, bv) = (view(&small), view(&blurred));
            self.down.run(gpu, &mut enc, params, [sw, sh], &[Res::View(&sv), Res::View(&dv), Res::Sampler(&self.sampler), Res::View(&smv)]);
            self.gather.run(gpu, &mut enc, params, [sw, sh], &[Res::View(&smv), Res::View(&dv), Res::Sampler(&self.sampler), Res::View(&bv)]);
            (small, blurred)
        });
        let bv = blurred.as_ref().map(|(_, b)| view(b));
        self.compose.run(gpu, &mut enc, params, [w, h], &[Res::View(&sv), Res::View(bv.as_ref().unwrap_or(&sv)), Res::View(&dv), Res::Sampler(&self.sampler), Res::View(&ov)]);
        gpu.queue.submit([enc.finish()]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RgbaF32, read_rgba16, upload_rgba16};
    use sidecar_color::depth::pack;
    use sidecar_color::params::{EditParams, LensBlur};

    /// A scene `w` × `h`: the left half near (nearness 1), the right half far (0).
    fn halves(w: u32, h: u32) -> RgbaF32 {
        RgbaF32 { width: w, height: h, data: (0..h).flat_map(|_| (0..w).flat_map(move |x| [if x < w / 2 { 1.0 } else { 0.0 }; 4])).collect() }
    }

    fn at(img: &RgbaF32, x: u32, y: u32) -> [f32; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        img.data[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn haze_moves_the_far_half_to_the_airlight_and_leaves_the_near_one() {
        let Ok(gpu) = crate::test_gpu() else { return eprintln!("skipping: no GPU") };
        let pass = DepthPass::new(gpu).unwrap();
        let (w, h) = (64u32, 16u32);
        let src = RgbaF32 { width: w, height: h, data: [0.2, 0.2, 0.2, 0.0].repeat((w * h) as usize) };
        let depth = upload_rgba16(gpu, &halves(w, h));
        let edit = EditParams { haze: 100.0, ..Default::default() };
        let params = pack(&edit, [1.0, 0.5, 0.0], [w, h], [0.0; 2]).unwrap();
        let out = read_rgba16(gpu, &pass.apply(gpu, &upload_rgba16(gpu, &src), &depth, &params)).unwrap();
        let near = at(&out, 4, 8);
        assert!((near[0] - 0.2).abs() < 1e-3 && (near[2] - 0.2).abs() < 1e-3, "near: {near:?}");
        let t = (-3.0f32).exp();
        let far = at(&out, 60, 8);
        for (c, a) in [(0, 1.0), (1, 0.5), (2, 0.0)] {
            assert!((far[c] - (0.2 * t + a * (1.0 - t))).abs() < 2e-3, "far: {far:?}");
        }
        let removed = pack(&EditParams { haze: -100.0, ..Default::default() }, [0.1, 0.1, 0.1], [w, h], [0.0; 2]).unwrap();
        let out = read_rgba16(gpu, &pass.apply(gpu, &upload_rgba16(gpu, &src), &depth, &removed)).unwrap();
        let far = at(&out, 60, 8)[0];
        assert!((far - (0.2 - 0.1 * 0.8) / 0.2).abs() < 2e-3, "removing haze, the transmittance floored at 0.2: {far}");
    }

    /// Focus on the near half: a bright point in the far half becomes a disc holding about the light it had,
    /// the near half is untouched, and the bright near half doesn't bleed into the blurred far side.
    #[test]
    fn the_blur_spreads_far_points_into_discs_and_keeps_the_focus_sharp() {
        let Ok(gpu) = crate::test_gpu() else { return eprintln!("skipping: no GPU") };
        let pass = DepthPass::new(gpu).unwrap();
        let (w, h) = (800u32, 400u32);
        let mut src = RgbaF32 { width: w, height: h, data: vec![0.0; (w * h * 4) as usize] };
        for y in 0..h {
            for x in 0..w / 2 {
                let i = ((y * w + x) * 4) as usize;
                src.data[i..i + 3].copy_from_slice(&[1.0; 3]);
            }
        }
        let dot = ((200 * w + 600) * 4) as usize;
        src.data[dot..dot + 3].copy_from_slice(&[400.0; 3]);
        let depth = upload_rgba16(gpu, &halves(w, h));
        let edit = EditParams { lens_blur: Some(LensBlur { amount: 100.0, focus: 100.0, range: 0.0 }), ..Default::default() };
        let params = pack(&edit, [0.0; 3], [w, h], [0.0; 2]).unwrap();
        assert_eq!((params.blur[0], params.scale()), (20.0, 2));
        let out = read_rgba16(gpu, &pass.apply(gpu, &upload_rgba16(gpu, &src), &depth, &params)).unwrap();
        assert_eq!(at(&out, 100, 100)[0], 1.0, "in focus, untouched");
        assert_eq!(at(&out, 399, 200)[0], 1.0, "in focus right up to the edge");
        assert!(at(&out, 600, 200)[0] < 20.0, "the point is spread out: {:?}", at(&out, 600, 200));
        assert!(at(&out, 610, 200)[0] > 0.05 && at(&out, 600, 190)[0] > 0.05, "into a disc: {:?} {:?}", at(&out, 610, 200), at(&out, 600, 190));
        assert!(at(&out, 640, 200)[0] < 0.01, "and no farther than its radius: {:?}", at(&out, 640, 200));
        let light: f32 = (560..640).flat_map(|x| (160..240).map(move |y| (x, y))).map(|(x, y)| at(&out, x, y)[0]).sum();
        assert!((light - 400.0).abs() < 400.0 * 0.25, "about the light it had: {light}");
        assert!(at(&out, 412, 300)[0] < 0.05, "the sharp bright half doesn't bleed into the blurred far side: {:?}", at(&out, 412, 300));
    }

    /// A depth edge that has been smeared by stretching the model's map snaps back to where the photo's own
    /// edge is, and a flat region keeps its depth rather than being chopped up by the photo's texture.
    #[test]
    fn refinement_puts_the_depth_edge_where_the_photo_has_one() {
        let Ok(gpu) = crate::test_gpu() else { return eprintln!("skipping: no GPU") };
        let pass = DepthPass::new(gpu).unwrap();
        // A frame ten times the model's own edge, so one of its texels is ten pixels here — the scale at which
        // the stretched map is wrong, and the scale the taps are spaced at.
        let (w, h) = (5180u32, 200u32);
        let texel = w as f32 / 518.0;
        let edge = (w / 2) as f32;
        // The photo: a hard edge at the middle, dark on the near side. Plus fine texture on the far side, which
        // the refinement must not mistake for depth.
        let photo = RgbaF32 {
            width: w,
            height: h,
            data: (0..h)
                .flat_map(|y| {
                    (0..w).flat_map(move |x| {
                        let far = x as f32 >= edge;
                        let v = if far { 0.8 + if (x + y) % 2 == 0 { 0.02 } else { -0.02 } } else { 0.1 };
                        [v, v, v, 1.0]
                    })
                })
                .collect(),
        };
        // The depth map as stretching the model's output leaves it: the step smeared over two of its texels,
        // so it disagrees with the photo for a texel either side.
        let smear = 2.0 * texel;
        let coarse = RgbaF32 {
            width: w,
            height: h,
            data: (0..h)
                .flat_map(|y| {
                    (0..w).flat_map(move |x| {
                        let _ = y;
                        let t = (((x as f32 - edge) / smear) + 0.5).clamp(0.0, 1.0);
                        let n = 1.0 - t; // near (1) on the left, far (0) on the right
                        [n, n, n, 1.0]
                    })
                })
                .collect(),
        };
        let refined = pass
            .refined(gpu, &upload_rgba16(gpu, &photo), &upload_rgba16(gpu, &coarse), [w, h])
            .expect("a frame this much larger than the model's map is worth refining");
        let out = read_rgba16(gpu, &refined).unwrap();
        let n = |x: u32| at(&out, x, 100)[0];
        let before = |x: u32| at(&coarse, x, 100)[0];
        let (inside, outside) = (edge as u32 - texel as u32 / 2, edge as u32 + texel as u32 / 2);
        // Half a texel inside the photo's near side, the smeared map had already given away a quarter of the
        // step; the refinement takes it back, and the same on the far side.
        // The centre tap is itself a texel of the stretched map, so it carries its own error into the mean: a
        // single pass cannot snap the step completely, and it is not supposed to. What it must do is take most
        // of the error out — here, more than half of it — and put the step where the photo's is.
        assert!(before(inside) < 0.8, "the map as it stands is smeared here: {}", before(inside));
        let (was, now) = (1.0 - before(inside), 1.0 - n(inside));
        assert!(now < was / 2.0, "near, at the photo's edge: {} (was {}), error {now} against {was}", n(inside), before(inside));
        assert!(n(outside) < before(outside) / 2.0, "far, right after it: {} (was {})", n(outside), before(outside));
        // The step is now the photo's: most of it happens across the two pixels the photo's edge falls between,
        // where the stretched map spread it over two of its texels.
        let (step_was, step_now) = (before(edge as u32 - 1) - before(edge as u32 + 1), n(edge as u32 - 1) - n(edge as u32 + 1));
        assert!(step_now > 4.0 * step_was, "the step moved to the photo's edge: {step_now} across two pixels, against {step_was}");
        assert!(step_now > 0.5, "and it is most of the whole step: {step_now}");
        // Deep in each region the value is unchanged, and the far side's texture has not broken it up.
        assert!(n(500) > 0.95 && n(w - 500) < 0.05, "interiors keep their depth: {} {}", n(500), n(w - 500));
        let far: Vec<f32> = (w - 600..w - 500).map(n).collect();
        let far_spread = far.iter().cloned().fold(0.0f32, f32::max) - far.iter().cloned().fold(1.0f32, f32::min);
        assert!(far_spread < 0.02, "fine texture is not depth: spread {far_spread}");
    }

    /// A tile with its margin, placed in the frame, comes out as the same part of the whole picture does.
    #[test]
    fn a_tile_with_its_margin_matches_the_whole() {
        let Ok(gpu) = crate::test_gpu() else { return eprintln!("skipping: no GPU") };
        let pass = DepthPass::new(gpu).unwrap();
        let (w, h) = (320u32, 160u32);
        let src = RgbaF32 { width: w, height: h, data: (0..w * h).flat_map(|i| { let v = ((i % w) as f32 * 0.37).sin().abs() * 2.0 + ((i / w) as f32 * 0.21).cos().abs(); [v, v * 0.5, v * 0.25, 0.0] }).collect() };
        let depth = upload_rgba16(gpu, &RgbaF32 { width: w, height: h, data: (0..w * h).flat_map(|i| [(i % w) as f32 / w as f32; 4]).collect() });
        let edit = EditParams { lens_blur: Some(LensBlur { amount: 60.0, focus: 90.0, range: 5.0 }), haze: 40.0, ..Default::default() };
        let params = pack(&edit, [0.3, 0.3, 0.4], [w, h], [0.0; 2]).unwrap();
        let whole = read_rgba16(gpu, &pass.apply(gpu, &upload_rgba16(gpu, &src), &depth, &params)).unwrap();
        // A tile at (128, 48), 64 × 64, cut with the margin the blur needs, rounded to the low-resolution grid.
        let (k, m) = (params.scale(), params.reach().next_multiple_of(params.scale()));
        let (x0, y0) = (128 - m, 48u32.saturating_sub(m) / k * k);
        let (cw, ch) = ((128 + 64 + m).min(w) - x0, (48 + 64 + m).min(h) - y0);
        let src_ref = &src;
        let cut = RgbaF32 { width: cw, height: ch, data: (y0..y0 + ch).flat_map(|y| (x0..x0 + cw).flat_map(move |x| at(src_ref, x, y))).collect() };
        let tile = read_rgba16(gpu, &pass.apply(gpu, &upload_rgba16(gpu, &cut), &depth, &params.at([x0 as f32, y0 as f32]))).unwrap();
        let worst = (48..112).flat_map(|y| (128..192).map(move |x| (x, y))).map(|(x, y)| (at(&tile, x - x0, y - y0)[0] - at(&whole, x, y)[0]).abs()).fold(0.0, f32::max);
        assert!(worst < 0.01, "worst difference {worst}");
    }
}
