// SPDX-License-Identifier: Apache-2.0
//! Full-resolution sharpness measure for focus peaking: high-frequency energy normalized by local contrast,
//! so a crisp edge outscores a blurred one of the same amplitude. Runs on the demosaiced luminance, before
//! any develop settings apply, so the result doesn't change when a slider moves. See docs/ROADMAP.md,
//! "Focus peaking measures the wrong thing."

use crate::{Mono, RgbaF32};

const LUMA: [f64; 3] = [0.2126, 0.7152, 0.0722];
/// Local-contrast window, one edge: WINDOW×WINDOW samples, "roughly sixteen" per the roadmap.
const WINDOW: i64 = 4;

/// Discrete Laplacian energy over luminance, divided by the local standard deviation over a small window —
/// a soft high-contrast edge has a large gradient but also a large local deviation, so it scores low; crisp
/// fine texture has a large gradient against a locally flat surround, so it scores high.
pub fn focus_energy(img: &RgbaF32) -> Mono {
    let (w, h) = (img.width as usize, img.height as usize);
    let luma: Vec<f64> = img.data.as_chunks::<4>().0.iter().map(|p| LUMA[0] * p[0] as f64 + LUMA[1] * p[1] as f64 + LUMA[2] * p[2] as f64).collect();
    let (sum, sumsq) = summed_area_tables(&luma, w, h);

    let at = |x: i64, y: i64| luma[y.clamp(0, h as i64 - 1) as usize * w + x.clamp(0, w as i64 - 1) as usize];
    let mut data = vec![0f32; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let laplacian = (4.0 * at(x, y) - at(x - 1, y) - at(x + 1, y) - at(x, y - 1) - at(x, y + 1)).abs();
            let variance = window_variance(&sum, &sumsq, w, h, x, y);
            let std = variance.max(0.0).sqrt();
            let raw = laplacian / (std + 1e-3);
            data[y as usize * w + x as usize] = (raw / (raw + 1.0)) as f32; // compress (0, inf) to (0, 1)
        }
    }
    Mono { width: img.width, height: img.height, data }
}

/// Padded (w+1)×(h+1) prefix sums of the values and their squares, for O(1) windowed mean/variance.
fn summed_area_tables(values: &[f64], w: usize, h: usize) -> (Vec<f64>, Vec<f64>) {
    let stride = w + 1;
    let mut sum = vec![0f64; stride * (h + 1)];
    let mut sumsq = vec![0f64; stride * (h + 1)];
    for y in 0..h {
        for x in 0..w {
            let v = values[y * w + x];
            sum[(y + 1) * stride + x + 1] = v + sum[y * stride + x + 1] + sum[(y + 1) * stride + x] - sum[y * stride + x];
            sumsq[(y + 1) * stride + x + 1] = v * v + sumsq[y * stride + x + 1] + sumsq[(y + 1) * stride + x] - sumsq[y * stride + x];
        }
    }
    (sum, sumsq)
}

fn rect_sum(table: &[f64], stride: usize, x0: usize, y0: usize, x1: usize, y1: usize) -> f64 {
    table[y1 * stride + x1] - table[y0 * stride + x1] - table[y1 * stride + x0] + table[y0 * stride + x0]
}

fn window_variance(sum: &[f64], sumsq: &[f64], w: usize, h: usize, x: i64, y: i64) -> f64 {
    let stride = w + 1;
    let half = WINDOW / 2;
    let x0 = (x - half).clamp(0, w as i64) as usize;
    let y0 = (y - half).clamp(0, h as i64) as usize;
    let x1 = (x - half + WINDOW).clamp(0, w as i64) as usize;
    let y1 = (y - half + WINDOW).clamp(0, h as i64) as usize;
    let n = ((x1 - x0) * (y1 - y0)).max(1) as f64;
    let s = rect_sum(sum, stride, x0, y0, x1, y1);
    let sq = rect_sum(sumsq, stride, x0, y0, x1, y1);
    let mean = s / n;
    sq / n - mean * mean
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, v: f32) -> RgbaF32 {
        RgbaF32 { width: w, height: h, data: (0..(w * h) as usize).flat_map(|_| [v, v, v, 1.0]).collect() }
    }

    /// A hard step edge (contrast in one pixel) against a soft ramp of the same total contrast spread over
    /// many pixels: the roadmap's exact claim ("a soft high-contrast edge keeps a large gradient across two
    /// texels while crisp fine texture has no signal left" — but the sharp one has to win).
    #[test]
    fn a_sharp_edge_outscores_a_blurred_edge_of_the_same_contrast() {
        let n = 40;
        let mut sharp = flat(n, 8, 0.0);
        let mut blurred = flat(n, 8, 0.0);
        for y in 0..8usize {
            for x in 0..n as usize {
                let s = if x >= n as usize / 2 { 1.0 } else { 0.0 };
                let b = (x as f32 / n as f32).clamp(0.0, 1.0); // ramps 0..1 over the whole width
                for img in [(&mut sharp, s), (&mut blurred, b)] {
                    let (img, v) = img;
                    let i = (y * n as usize + x) * 4;
                    img.data[i] = v;
                    img.data[i + 1] = v;
                    img.data[i + 2] = v;
                }
            }
        }
        let sharp_peak = focus_energy(&sharp).data.iter().cloned().fold(0f32, f32::max);
        let blurred_peak = focus_energy(&blurred).data.iter().cloned().fold(0f32, f32::max);
        assert!(sharp_peak > blurred_peak, "sharp {sharp_peak} should beat blurred {blurred_peak}");
        assert!(sharp_peak > 0.5, "a hard step should score high: {sharp_peak}");
    }

    #[test]
    fn a_flat_field_scores_zero() {
        let img = flat(16, 16, 0.4);
        assert!(focus_energy(&img).data.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn max_pooling_keeps_a_single_sharp_pixel_a_uniform_downsample_would_average_away() {
        let mut mono = Mono { width: 8, height: 8, data: vec![0.0; 64] };
        mono.data[3 * 8 + 3] = 1.0; // one bright pixel in an otherwise black 8x8
        let pooled = mono.max_pooled(2);
        assert!(pooled.data.contains(&1.0), "{:?}", pooled.data);
    }

    #[test]
    fn max_pooled_never_upscales() {
        let mono = Mono { width: 4, height: 4, data: vec![0.0; 16] };
        let pooled = mono.max_pooled(64);
        assert_eq!((pooled.width, pooled.height), (4, 4));
    }
}
