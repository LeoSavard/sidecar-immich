// SPDX-License-Identifier: Apache-2.0
//! Mask edges from the photo (DESIGN.md §15, "Hair-accurate edges"): the part masks — hair, skin, clothes — come
//! from a 256² model, so they are right about *what* is hair and blocky about *where* it ends. Stretched to the
//! frame, their edges are 256th-of-the-frame wide. This puts them where the photo's edges are: joint bilateral
//! upsampling (Kopf, Cohen, Lischinski and Uyttendaele, 2007), implemented from the paper, as the depth map's
//! refinement does — each output pixel a weighted mean of the mask at nearby coarse texels, weighted by distance
//! and by how alike the photo's colour is there and here. Colour rather than luminance alone, measured in OKLab:
//! hair against a sky of the same brightness differs in colour, and that is the edge that matters.
//!
//! The worker and the browser (through wasm) run it on the same proxy at `color_range::SIZE`, as they do the
//! colour selection.

use crate::color_range::oklch;
use crate::mat::{self, M3};
use crate::params::DevelopParams;

/// Taps per side: 5 × 5, two coarse texels either way.
const RADIUS: i32 = 2;
/// The colour difference, OKLab ΔE, at which a tap's weight has fallen to e^(−1/2).
const SIGMA_COLOUR: f32 = 0.04;

/// OKLab (L, a, b) of every pixel of a camera-RGB picture, through the edit's white balance and matrix.
fn lab_of(develop: &DevelopParams, rgba: &[f32]) -> Vec<[f32; 3]> {
    let cam: M3 =
        std::array::from_fn(|r| std::array::from_fn(|c| develop.cam_to_work[c][r] as f64));
    let m = mat::mul(&mat::REC2020_TO_SRGB, &cam);
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let c = [0, 1, 2].map(|k| (p[k] * develop.wb_mul[k] * develop.exposure) as f64);
            let [l, ch, h] = oklch(mat::apply(&m, &c).map(|v| v.max(0.0) as f32));
            let r = h.to_radians();
            [l, ch * r.cos(), ch * r.sin()]
        })
        .collect()
}

/// The mask read bilinearly at a point given in the photo's pixels.
fn sample(mask: &[u8], mw: u32, mh: u32, x: f32, y: f32, w: u32, h: u32) -> f32 {
    let u = ((x + 0.5) * mw as f32 / w as f32 - 0.5).clamp(0.0, (mw - 1) as f32);
    let v = ((y + 0.5) * mh as f32 / h as f32 - 0.5).clamp(0.0, (mh - 1) as f32);
    let (x0, y0) = (u.floor() as u32, v.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(mw - 1), (y0 + 1).min(mh - 1));
    let (fx, fy) = (u - x0 as f32, v - y0 as f32);
    let at = |x: u32, y: u32| mask[(y * mw + x) as usize] as f32;
    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
    let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
    (top * (1.0 - fy) + bottom * fy) / 255.0
}

/// `mask` (one byte a pixel, `mw` × `mh`) refined against the photo (camera-RGB RGBA, `w` × `h`): one byte a pixel
/// at the photo's size. As the paper has it, the taps are the coarse texels themselves — their exact values, at
/// their own centres — weighted by their distance in coarse texels and by the photo's colour at their centres
/// against the colour here.
pub fn refine(
    mask: &[u8],
    mw: u32,
    mh: u32,
    rgba: &[f32],
    w: u32,
    h: u32,
    develop: &DevelopParams,
) -> Vec<u8> {
    refine_with(mask, mw, mh, rgba, w, h, develop, SIGMA_COLOUR)
}

/// `refine` with the colour sigma given, for measuring (`sidecar-dev maskedge`).
#[allow(clippy::too_many_arguments)]
pub fn refine_with(
    mask: &[u8],
    mw: u32,
    mh: u32,
    rgba: &[f32],
    w: u32,
    h: u32,
    develop: &DevelopParams,
    sigma_colour: f32,
) -> Vec<u8> {
    let lab = lab_of(develop, rgba);
    let (sx, sy) = (w as f32 / mw as f32, h as f32 / mh as f32);
    // The photo's colour at each coarse texel's centre.
    let centre: Vec<[f32; 3]> = (0..mw * mh)
        .map(|i| {
            let px = (((i % mw) as f32 + 0.5) * sx).floor().min((w - 1) as f32) as u32;
            let py = (((i / mw) as f32 + 0.5) * sy).floor().min((h - 1) as f32) as u32;
            lab[(py * w + px) as usize]
        })
        .collect();
    const SIGMA_SPACE: f32 = 1.0; // coarse texels
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let v = (y as f32 + 0.5) / sy - 0.5;
        for x in 0..w {
            let u = (x as f32 + 0.5) / sx - 0.5;
            let here = lab[(y * w + x) as usize];
            let (mut sum, mut total) = (0f32, 0f32);
            for dy in -RADIUS..=RADIUS {
                let cy = v.round() as i64 + dy as i64;
                if cy < 0 || cy >= mh as i64 {
                    continue;
                }
                for dx in -RADIUS..=RADIUS {
                    let cx = u.round() as i64 + dx as i64;
                    if cx < 0 || cx >= mw as i64 {
                        continue;
                    }
                    let i = (cy as u32 * mw + cx as u32) as usize;
                    let there = centre[i];
                    let dc = (0..3).map(|k| (here[k] - there[k]).powi(2)).sum::<f32>();
                    let ds = (cx as f32 - u).powi(2) + (cy as f32 - v).powi(2);
                    let wgt = (-ds / (2.0 * SIGMA_SPACE * SIGMA_SPACE)
                        - dc / (2.0 * sigma_colour * sigma_colour))
                        .exp();
                    sum += wgt * mask[i] as f32 / 255.0;
                    total += wgt;
                }
            }
            let val = if total > 1e-6 {
                sum / total
            } else {
                sample(mask, mw, mh, x as f32, y as f32, w, h)
            };
            out.push((val.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> DevelopParams {
        let mut d: DevelopParams = bytemuck::Zeroable::zeroed();
        d.wb_mul = [1.0; 3];
        d.exposure = 1.0;
        let to2020 = mat::inverse(&mat::REC2020_TO_SRGB);
        d.cam_to_work = [0, 1, 2].map(|j| {
            [
                to2020[0][j] as f32,
                to2020[1][j] as f32,
                to2020[2][j] as f32,
                0.0,
            ]
        });
        d
    }

    /// Where the mask's edge lands: the first column, along the middle row, whose value falls below one half.
    fn edge(m: &[u8], w: u32, h: u32) -> u32 {
        let row = &m[((h / 2) * w) as usize..((h / 2 + 1) * w) as usize];
        row.iter().position(|&v| v < 128).unwrap_or(w as usize) as u32
    }

    /// A photo with an edge between two colours of the same lightness (a red and a green), and a mask of it 10×
    /// coarser whose edge is under a third of a coarse texel off, as a model's is: refined, the mask steps where the photo does, which luminance
    /// alone could not have found.
    #[test]
    fn moves_the_edge_to_the_photos_even_at_equal_lightness() {
        let (w, h) = (200u32, 40u32);
        let (red, green) = ([0.45f32, 0.12, 0.10], [0.12f32, 0.30, 0.10]);
        let (lr, lg) = (oklch(red)[0], oklch(green)[0]);
        assert!(
            (lr - lg).abs() < 0.05,
            "the two are alike in lightness: {lr} {lg}"
        );
        let photo: Vec<f32> = (0..w * h)
            .flat_map(|i| {
                let c = if i % w < 103 { red } else { green };
                [c[0], c[1], c[2], 0.0]
            })
            .collect();
        // The coarse mask: 20 × 4, hair (white) on the left up to coarse column 10, so stretched its edge is at
        // 100 where the photo's is at 103.
        let (mw, mh) = (20u32, 4u32);
        let mask: Vec<u8> = (0..mw * mh)
            .map(|i| if i % mw < 10 { 255 } else { 0 })
            .collect();
        let stretched: Vec<u8> = (0..w * h)
            .map(|i| {
                (sample(&mask, mw, mh, (i % w) as f32, (i / w) as f32, w, h) * 255.0).round() as u8
            })
            .collect();
        let refined = refine(&mask, mw, mh, &photo, w, h, &identity());
        let (before, after) = (edge(&stretched, w, h), edge(&refined, w, h));
        println!("edge: stretched {before}, refined {after}, photo 103");
        assert!((after as i32 - 103).abs() <= 1, "refined edge {after}");
        assert!(
            (before as i32 - 103).abs() >= 3,
            "the stretched one was off: {before}"
        );
        // And it is sharp: no more than a few pixels between 10 % and 90 %.
        let row = &refined[((h / 2) * w) as usize..((h / 2 + 1) * w) as usize];
        let soft = row.iter().filter(|&&v| v > 25 && v < 230).count();
        assert!(soft <= 4, "{soft} pixels of transition");
    }

    /// Within a flat area nothing changes: the refinement only moves edges.
    #[test]
    fn leaves_flat_areas_alone() {
        let (w, h) = (60u32, 60u32);
        let photo = [0.3f32, 0.3, 0.3, 0.0].repeat((w * h) as usize);
        let mask = vec![200u8; 36];
        let refined = refine(&mask, 6, 6, &photo, w, h, &identity());
        assert!(refined.iter().all(|&v| v == 200));
    }
}
