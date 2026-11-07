// SPDX-License-Identifier: Apache-2.0
//! The fallback preview's histogram and overlays (DESIGN.md §11, "Without WebGPU"): what the browser's view and
//! histogram passes do (`shaders/client/view.wgsl`, `shaders/client/histogram.wgsl`), done here over the image
//! the preview is about to encode — the same bins, thresholds and colours, so a browser without WebGPU reads the
//! same numbers and sees the same warnings, only when a slider is let go rather than while it moves.

use sidecar_engine::RgbaF32;

/// R, G, B and luma, 256 bins each, then the pixels at white and at black: `histogram.wgsl`'s layout.
pub const HIST_BINS: usize = 1026;

pub const CLIP_HIGHS: u32 = 1;
pub const CLIP_LOWS: u32 = 2;
pub const PEAKING: u32 = 4;

/// `view.wgsl`'s focus peaking colour, sRGB-encoded, and the share of the focus mask that counts as in focus
/// (`PEAK_THRESHOLD` in `view.ts`).
const PEAK_COLOUR: [f32; 3] = [0.98, 0.86, 0.15];
const PEAK_THRESHOLD: f32 = 0.5;

fn encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn decode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// The display-linear values the view treats as at white and at black: half a code value from either end.
fn thresholds() -> (f32, f32) {
    (decode(254.5 / 255.0), decode(0.5 / 255.0))
}

fn bin(v: f32) -> usize {
    ((v.max(0.0) * 256.0) as usize).min(255)
}

/// The histogram of `img`, display-linear, as the screen shows it.
pub fn histogram(img: &RgbaF32) -> Vec<u32> {
    let (hi, lo) = thresholds();
    let mut bins = vec![0u32; HIST_BINS];
    for p in img.data.as_chunks::<4>().0 {
        let e = [encode(p[0]), encode(p[1]), encode(p[2])];
        bins[bin(e[0])] += 1;
        bins[256 + bin(e[1])] += 1;
        bins[512 + bin(e[2])] += 1;
        bins[768 + bin(0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2])] += 1;
        let m = p[0].max(p[1]).max(p[2]);
        if m >= hi {
            bins[1024] += 1;
        }
        if m <= lo {
            bins[1025] += 1;
        }
    }
    bins
}

/// Paint the overlays `flags` asks for into `img`, in the view's order: highlights at white red, shadows at black
/// blue, then what `focus` (the focus mask, mapped to `img`'s pixels) marks in focus.
pub fn paint(img: &mut RgbaF32, flags: u32, focus: Option<&RgbaF32>) {
    let (hi, lo) = thresholds();
    let peak = PEAK_COLOUR.map(decode);
    let focus =
        focus.filter(|f| flags & PEAKING != 0 && f.width == img.width && f.height == img.height);
    for (i, p) in img.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let m = p[0].max(p[1]).max(p[2]);
        let colour = if flags & CLIP_HIGHS != 0 && m >= hi {
            Some([1.0, 0.0, 0.0])
        } else if flags & CLIP_LOWS != 0 && m <= lo {
            Some([0.0, 0.0, 1.0])
        } else if focus.is_some_and(|f| f.data[i * 4] >= PEAK_THRESHOLD) {
            Some(peak)
        } else {
            None
        };
        if let Some(c) = colour {
            p[..3].copy_from_slice(&c);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(pixels: &[[f32; 3]]) -> RgbaF32 {
        RgbaF32 {
            width: pixels.len() as u32,
            height: 1,
            data: pixels
                .iter()
                .flat_map(|p| [p[0], p[1], p[2], 1.0])
                .collect(),
        }
    }

    /// The bins and counts `histogram.wgsl` gives: encoded values, luma over the encoded channels, and white and
    /// black judged on the brightest channel half a code value from either end.
    #[test]
    fn bins_as_the_gpu_does() {
        let h = histogram(&img(&[
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            [0.2158, 0.2158, 0.2158],
            [1.2, 0.1, 0.0],
        ]));
        assert_eq!(h.len(), HIST_BINS);
        assert_eq!(h[255], 2, "white, and the over-range red, in red's top bin");
        assert_eq!(h[0], 1);
        assert_eq!(h[128], 1, "linear 0.2158 encodes to 0.502");
        assert_eq!(
            h[1024], 2,
            "at white: the white pixel and the over-range red"
        );
        assert_eq!(h[1025], 1, "at black");
        assert_eq!(h[768..1024].iter().sum::<u32>(), 4);
    }

    #[test]
    fn paints_in_the_views_order() {
        let mut i = img(&[
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            [0.3, 0.3, 0.3],
            [0.3, 0.3, 0.3],
        ]);
        let focus = RgbaF32 {
            width: 4,
            height: 1,
            data: [0.0, 0.0, 0.9, 0.1].iter().flat_map(|&v| [v; 4]).collect(),
        };
        paint(&mut i, CLIP_HIGHS | CLIP_LOWS | PEAKING, Some(&focus));
        assert_eq!(&i.data[0..3], &[1.0, 0.0, 0.0]);
        assert_eq!(&i.data[4..7], &[0.0, 0.0, 1.0]);
        assert!(
            (encode(i.data[8]) - 0.98).abs() < 1e-4 && (encode(i.data[10]) - 0.15).abs() < 1e-4,
            "in focus"
        );
        assert_eq!(
            &i.data[12..15],
            &[0.3, 0.3, 0.3],
            "out of focus, mid grey: untouched"
        );
        let mut j = img(&[[1.0, 1.0, 1.0]]);
        paint(&mut j, 0, None);
        assert_eq!(&j.data[0..3], &[1.0, 1.0, 1.0], "no flags, no paint");
    }
}
