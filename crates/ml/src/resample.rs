// SPDX-License-Identifier: Apache-2.0
//! Resizing between a picture and a model's fixed input, and back from a model's output to the picture.

/// Resizes interleaved samples (`channels` per pixel) from `w × h` to `dw × dh` with a tent filter in two
/// separable passes. Shrinking widens the tent to the scale, so every source pixel contributes, as an area
/// filter would; enlarging is bilinear.
pub fn resize(
    src: &[f32],
    (w, h): (usize, usize),
    channels: usize,
    (dw, dh): (usize, usize),
) -> Vec<f32> {
    debug_assert_eq!(src.len(), w * h * channels);
    let columns = taps(w, dw);
    let mut wide = vec![0f32; h * dw * channels];
    for y in 0..h {
        let row = &src[y * w * channels..(y + 1) * w * channels];
        for (x, (start, weights)) in columns.iter().enumerate() {
            let out = &mut wide[(y * dw + x) * channels..][..channels];
            for (i, weight) in weights.iter().enumerate() {
                let px = &row[(start + i) * channels..][..channels];
                out.iter_mut().zip(px).for_each(|(o, p)| *o += p * weight);
            }
        }
    }
    let rows = taps(h, dh);
    let mut out = vec![0f32; dh * dw * channels];
    for (y, (start, weights)) in rows.iter().enumerate() {
        let dst = &mut out[y * dw * channels..(y + 1) * dw * channels];
        for (i, weight) in weights.iter().enumerate() {
            let src = &wide[(start + i) * dw * channels..][..dw * channels];
            dst.iter_mut().zip(src).for_each(|(o, p)| *o += p * weight);
        }
    }
    out
}

/// For each output sample: the first source sample it reads and the normalised weights from there.
fn taps(n: usize, dn: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = n as f32 / dn as f32;
    let radius = scale.max(1.0);
    (0..dn)
        .map(|o| {
            let centre = (o as f32 + 0.5) * scale - 0.5;
            let lo = (centre - radius).ceil().max(0.0) as usize;
            let hi = ((centre + radius).floor().max(0.0) as usize).min(n - 1);
            let mut weights: Vec<f32> = (lo..=hi)
                .map(|i| (1.0 - (i as f32 - centre).abs() / radius).max(0.0))
                .collect();
            let sum: f32 = weights.iter().sum();
            if sum <= 0.0 {
                return (
                    centre.round().clamp(0.0, (n - 1) as f32) as usize,
                    vec![1.0],
                );
            }
            weights.iter_mut().for_each(|w| *w /= sum);
            (lo, weights)
        })
        .collect()
}

/// The size that fits `w × h` inside `long_edge` without enlarging it.
pub fn fit((w, h): (usize, usize), long_edge: usize) -> (usize, usize) {
    let scale = (long_edge as f32 / w.max(h) as f32).min(1.0);
    (
        ((w as f32 * scale).round() as usize).max(1),
        ((h as f32 * scale).round() as usize).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_picture_stays_flat_at_any_size() {
        let src = vec![0.25f32; 37 * 23 * 3];
        for size in [(10, 7), (37, 23), (100, 61), (1, 1)] {
            let out = resize(&src, (37, 23), 3, size);
            assert_eq!(out.len(), size.0 * size.1 * 3);
            assert!(out.iter().all(|v| (v - 0.25).abs() < 1e-5), "{size:?}");
        }
    }

    #[test]
    fn a_ramp_stays_a_ramp() {
        // Away from the edges a tent filter reproduces a linear ramp exactly, at the output's sample centres.
        let ramp: Vec<f32> = (0..16).map(|i| i as f32).collect();
        let half = resize(&ramp, (16, 1), 1, (8, 1));
        for (o, v) in half.iter().enumerate().skip(1).take(6) {
            assert!((v - (2.0 * o as f32 + 0.5)).abs() < 1e-4, "{half:?}");
        }
        let double = resize(&ramp, (16, 1), 1, (32, 1));
        for (o, v) in double.iter().enumerate().skip(1).take(30) {
            assert!((v - (o as f32 * 0.5 - 0.25)).abs() < 1e-4, "{double:?}");
        }
    }

    #[test]
    fn fit_never_enlarges() {
        assert_eq!(fit((4000, 3000), 1024), (1024, 768));
        assert_eq!(fit((500, 300), 1024), (500, 300));
    }
}
