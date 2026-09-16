// SPDX-License-Identifier: Apache-2.0
//! Proxy generation: area-filtered downsampling in linear light, with per-pixel clip coverage in alpha.

use crate::LinearImage;

/// Linear RGBA, f32. Alpha = fraction of source pixels under this pixel that reached sensor saturation.
pub struct Proxy {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<f32>,
}

/// Saturation threshold. LibRaw scales the declared white level to 65535, but some cameras saturate below
/// it; when the brightest sample is well up the range, treat anything within 2 % of it as clipped too.
fn saturation_threshold(rgb: &[u16]) -> u16 {
    let max = rgb.iter().copied().max().unwrap_or(0);
    let declared = (65535u32 * 99 / 100) as u16;
    if max >= 32768 { declared.min((max as u32 * 98 / 100) as u16) } else { declared }
}

/// Weights of source samples overlapping each destination sample, for a 1-D area filter.
fn area_weights(src: u32, dst: u32) -> Vec<Vec<(u32, f32)>> {
    let scale = src as f64 / dst as f64;
    (0..dst)
        .map(|d| {
            let (a, b) = (d as f64 * scale, (d as f64 + 1.0) * scale);
            let mut w = Vec::new();
            let mut s = a.floor() as u32;
            while (s as f64) < b && s < src {
                let overlap = (b.min(s as f64 + 1.0) - a.max(s as f64)).max(0.0);
                if overlap > 0.0 {
                    w.push((s, (overlap / scale) as f32));
                }
                s += 1;
            }
            w
        })
        .collect()
}

pub fn build(img: &LinearImage, long_edge: u32) -> Proxy {
    let long = img.width.max(img.height);
    let scale = (long_edge as f64 / long as f64).min(1.0);
    let (dw, dh) = (((img.width as f64 * scale).round() as u32).max(1), ((img.height as f64 * scale).round() as u32).max(1));
    // Source as RGBA f32 with a 0/1 clip flag.
    let saturated = saturation_threshold(&img.rgb);
    let src: Vec<[f32; 4]> = img
        .rgb
        .chunks_exact(3)
        .map(|p| {
            let clip = p.iter().any(|&v| v >= saturated) as u8 as f32;
            [p[0] as f32 / 65535.0, p[1] as f32 / 65535.0, p[2] as f32 / 65535.0, clip]
        })
        .collect();
    let wx = area_weights(img.width, dw);
    let wy = area_weights(img.height, dh);
    // Horizontal pass.
    let mut tmp = vec![[0f32; 4]; (dw * img.height) as usize];
    for y in 0..img.height as usize {
        let row = &src[y * img.width as usize..(y + 1) * img.width as usize];
        for (x, weights) in wx.iter().enumerate() {
            let mut acc = [0f32; 4];
            for &(s, w) in weights {
                for c in 0..4 {
                    acc[c] += row[s as usize][c] * w;
                }
            }
            tmp[y * dw as usize + x] = acc;
        }
    }
    // Vertical pass.
    let mut rgba = Vec::with_capacity((dw * dh * 4) as usize);
    for weights in &wy {
        for x in 0..dw as usize {
            let mut acc = [0f32; 4];
            for &(s, w) in weights {
                let p = tmp[s as usize * dw as usize + x];
                for c in 0..4 {
                    acc[c] += p[c] * w;
                }
            }
            rgba.extend_from_slice(&acc);
        }
    }
    Proxy { width: dw, height: dh, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_filter_preserves_mean_and_marks_clipping() {
        let img = LinearImage { width: 4, height: 2, rgb: [65535u16, 0, 0, 32768, 32768, 32768].repeat(4) };
        let p = build(&img, 2);
        assert_eq!((p.width, p.height), (2, 1));
        assert!((p.rgba[0] - 0.75).abs() < 1e-3, "{:?}", &p.rgba[..4]);
        assert!((p.rgba[3] - 0.5).abs() < 1e-3);
    }
}
