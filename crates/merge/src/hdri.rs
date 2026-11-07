// SPDX-License-Identifier: Apache-2.0
//! HDRIs: a full sphere of scene-linear light, for lighting 3D scenes (docs/research/multi-frame-and-denoise.md,
//! "HDRIs"). The work is HDR merge per direction, then a spherical stitch of the merged frames (`hdr`, `pano`);
//! this module holds what is particular to the result: which frames make a bracket, the colour spaces a
//! renderer expects, the absolute scale, and the two file formats renderers read.
//!
//! - **OpenEXR** through the `exr` crate (BSD-3-Clause), with the `chromaticities` attribute saying what the
//!   primaries are and `whiteLuminance` saying how many cd/m² a value of 1 is.
//! - **Radiance RGBE** (`.hdr`), written here: Greg Ward's shared-exponent format ("Real pixels", Graphics Gems
//!   II, 1991) with the run-length encoding of the Radiance file format.
//!
//! The absolute scale follows ISO 12232's saturation-based speed: a sensor at ISO S saturates at a focal-plane
//! exposure of 78 / S lux·s, and that exposure is (π/4)·T·v·cos⁴θ · L · t / N² ≈ 0.65 · L · t / N² (the
//! standard's q). So a value of 1 — saturation — is L = 78 N² / (0.65 S t) ≈ 120 N² / (S t) cd/m². Makers rate
//! ISO by other methods (SOS, REI) that differ from this by a third of a stop or so; the scale is good to that.

use std::io::Write;

/// A colour space a renderer may want the HDRI in: linear, white-balanced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// Linear Rec.709 / sRGB primaries, D65: what most renderers assume of an HDRI.
    Rec709,
    Rec2020,
    /// ACES AP1 primaries, ACES white: ACEScg, the working space of ACES pipelines.
    AcesCg,
}

impl Space {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "rec709" | "srgb" => Self::Rec709,
            "rec2020" => Self::Rec2020,
            "acescg" => Self::AcesCg,
            _ => return None,
        })
    }

    /// Red, green, blue and white chromaticities.
    pub fn chromaticities(self) -> [[f64; 2]; 4] {
        match self {
            Self::Rec709 => [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06], [0.3127, 0.3290]],
            Self::Rec2020 => [
                [0.708, 0.292],
                [0.170, 0.797],
                [0.131, 0.046],
                [0.3127, 0.3290],
            ],
            Self::AcesCg => [
                [0.713, 0.293],
                [0.165, 0.830],
                [0.128, 0.044],
                [0.32168, 0.33767],
            ],
        }
    }
}

/// Luminance, in cd/m², of a value of 1 (the reference frame's saturation) by ISO 12232's saturation-based
/// speed: `aperture` the f-number, `shutter` in seconds.
pub fn white_luminance(aperture: f64, shutter: f64, iso: f64) -> f64 {
    78.0 * aperture * aperture / (0.65 * iso * shutter)
}

/// How many consecutive frames make one bracket, from what each frame let in: the shortest period `p`
/// dividing the count with every frame within a fifth of a stop of the frame `p` before it, and with the
/// exposures inside a period actually differing. 1 when the frames are not bracketed.
pub fn bracket_period(light: &[f64]) -> usize {
    let n = light.len();
    let ev: Vec<f64> = light.iter().map(|l| l.max(1e-30).log2()).collect();
    for p in 2..=n / 2 {
        if !n.is_multiple_of(p) {
            continue;
        }
        let repeats = (p..n).all(|k| (ev[k] - ev[k - p]).abs() < 0.2);
        let spread = ev[..p].iter().cloned().fold(f64::MIN, f64::max)
            - ev[..p].iter().cloned().fold(f64::MAX, f64::min);
        if repeats && spread > 0.5 {
            return p;
        }
    }
    // One bracket of everything, if the exposures differ at all and nothing repeats.
    let spread =
        ev.iter().cloned().fold(f64::MIN, f64::max) - ev.iter().cloned().fold(f64::MAX, f64::min);
    if spread > 0.5 && n > 1 && (1..n).all(|k| (ev[k] - ev[0]).abs() >= 0.2) {
        n
    } else {
        1
    }
}

/// Write an OpenEXR file: three floats a pixel (`half` stores 16-bit floats, whose largest value is 65,504),
/// ZIP-compressed scanlines, with the primaries and the absolute scale in its header.
pub fn write_exr(
    path: &std::path::Path,
    w: usize,
    h: usize,
    rgb: &[f32],
    space: Space,
    white_luminance: Option<f64>,
    half: bool,
) -> std::io::Result<()> {
    use exr::prelude::*;
    let c = space.chromaticities();
    let v = |p: [f64; 2]| Vec2(p[0] as f32, p[1] as f32);
    let attrs = LayerAttributes {
        white_luminance: white_luminance.map(|l| l as f32),
        software_name: Some(Text::from("Sidecar")),
        ..Default::default()
    };
    let err = |e: exr::error::Error| std::io::Error::other(e.to_string());
    let px = |x: usize, y: usize| {
        let i = (y * w + x) * 3;
        (rgb[i], rgb[i + 1], rgb[i + 2])
    };
    let chroma = exr::meta::attribute::Chromaticities {
        red: v(c[0]),
        green: v(c[1]),
        blue: v(c[2]),
        white: v(c[3]),
    };
    if half {
        let ch = SpecificChannels::rgb(|Vec2(x, y)| {
            let (r, g, b) = px(x, y);
            (f16::from_f32(r), f16::from_f32(g), f16::from_f32(b))
        });
        let mut image = Image::from_layer(Layer::new((w, h), attrs, Encoding::SMALL_LOSSLESS, ch));
        image.attributes.chromaticities = Some(chroma);
        image.write().to_file(path).map_err(err)
    } else {
        let ch = SpecificChannels::rgb(|Vec2(x, y)| px(x, y));
        let mut image = Image::from_layer(Layer::new((w, h), attrs, Encoding::SMALL_LOSSLESS, ch));
        image.attributes.chromaticities = Some(chroma);
        image.write().to_file(path).map_err(err)
    }
}

/// One pixel as Ward's RGBE: a shared exponent and three 8-bit mantissas.
pub fn rgbe(p: [f32; 3]) -> [u8; 4] {
    let v = p[0].max(p[1]).max(p[2]);
    if v < 1e-32 {
        return [0; 4];
    }
    // v = m · 2^e with m in [0.5, 1).
    let e = v.log2().floor() as i32 + 1;
    let scale = 256.0 / (e as f32).exp2();
    let q = |c: f32| (c.max(0.0) * scale).min(255.0) as u8;
    [q(p[0]), q(p[1]), q(p[2]), (e + 128).clamp(0, 255) as u8]
}

/// Back to floats (the centre of each mantissa step, as Radiance reads it).
pub fn from_rgbe(b: [u8; 4]) -> [f32; 3] {
    if b[3] == 0 {
        return [0.0; 3];
    }
    let f = (b[3] as i32 - 136) as f32;
    let s = f.exp2();
    [0, 1, 2].map(|k| (b[k] as f32 + 0.5) * s)
}

/// Run-length encode one component of a scanline, Radiance's way: a byte above 128 is a run of (byte − 128)
/// copies of the next byte; otherwise it counts the literal bytes that follow.
fn rle(data: &[u8], out: &mut Vec<u8>) {
    let n = data.len();
    let mut i = 0;
    while i < n {
        // The next run of at least four.
        let mut run_start = i;
        let mut run_len = 0;
        while run_start < n {
            run_len = 1;
            while run_start + run_len < n
                && run_len < 127
                && data[run_start + run_len] == data[run_start]
            {
                run_len += 1;
            }
            if run_len >= 4 {
                break;
            }
            run_start += run_len;
            run_len = 0;
        }
        // Literals before it.
        while i < run_start {
            let k = (run_start - i).min(128);
            out.push(k as u8);
            out.extend_from_slice(&data[i..i + k]);
            i += k;
        }
        if run_len >= 4 {
            out.push(128 + run_len as u8);
            out.push(data[run_start]);
            i = run_start + run_len;
        }
    }
}

/// Write a Radiance `.hdr`: RGBE with run-length-encoded scanlines, and the primaries and the absolute scale
/// in its header (`PRIMARIES=`, and `EXPOSURE=`: a value times it is in watts/sr/m², at 179 lm/W — Radiance's
/// own luminous efficacy for its white).
pub fn write_hdr(
    path: &std::path::Path,
    w: usize,
    h: usize,
    rgb: &[f32],
    space: Space,
    white_luminance: Option<f64>,
) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let c = space.chromaticities();
    writeln!(f, "#?RADIANCE")?;
    writeln!(f, "SOFTWARE=Sidecar")?;
    writeln!(f, "FORMAT=32-bit_rle_rgbe")?;
    writeln!(
        f,
        "PRIMARIES={} {} {} {} {} {} {} {}",
        c[0][0], c[0][1], c[1][0], c[1][1], c[2][0], c[2][1], c[3][0], c[3][1]
    )?;
    if let Some(l) = white_luminance {
        // Radiance divides by EXPOSURE to recover the original values.
        writeln!(f, "EXPOSURE={}", 179.0 / l)?;
    }
    writeln!(f)?;
    writeln!(f, "-Y {h} +X {w}")?;
    let mut line = vec![[0u8; 4]; w];
    let mut out = Vec::with_capacity(w * 4);
    let mut comp = vec![0u8; w];
    for y in 0..h {
        for (x, p) in line.iter_mut().enumerate() {
            let i = (y * w + x) * 3;
            *p = rgbe([rgb[i], rgb[i + 1], rgb[i + 2]]);
        }
        out.clear();
        if (8..32768).contains(&w) {
            out.extend_from_slice(&[2, 2, (w >> 8) as u8, (w & 255) as u8]);
            for k in 0..4 {
                for (c, p) in comp.iter_mut().zip(&line) {
                    *c = p[k];
                }
                rle(&comp, &mut out);
            }
        } else {
            for p in &line {
                out.extend_from_slice(p);
            }
        }
        f.write_all(&out)?;
    }
    f.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read back what `write_hdr` wrote: the new-style run-length scanlines.
    fn read_hdr(bytes: &[u8]) -> (usize, usize, Vec<[f32; 3]>) {
        let text_end = bytes.windows(2).position(|w| w == b"\n\n").unwrap() + 2;
        let line_end = text_end + bytes[text_end..].iter().position(|&b| b == b'\n').unwrap();
        let dims = std::str::from_utf8(&bytes[text_end..line_end]).unwrap();
        let parts: Vec<&str> = dims.split_whitespace().collect();
        let (h, w): (usize, usize) = (parts[1].parse().unwrap(), parts[3].parse().unwrap());
        let mut i = line_end + 1;
        let mut px = vec![];
        for _ in 0..h {
            assert_eq!(&bytes[i..i + 2], &[2, 2]);
            i += 4;
            let mut comps = vec![vec![]; 4];
            for c in comps.iter_mut() {
                while c.len() < w {
                    let b = bytes[i];
                    i += 1;
                    if b > 128 {
                        c.extend(std::iter::repeat_n(bytes[i], (b - 128) as usize));
                        i += 1;
                    } else {
                        c.extend_from_slice(&bytes[i..i + b as usize]);
                        i += b as usize;
                    }
                }
            }
            px.extend(
                (0..w).map(|x| from_rgbe([comps[0][x], comps[1][x], comps[2][x], comps[3][x]])),
            );
        }
        (w, h, px)
    }

    #[test]
    fn radiance_files_round_trip() {
        let (w, h) = (300, 4);
        // Runs, literals, zeros, and twenty stops of range.
        let rgb: Vec<f32> = (0..w * h)
            .flat_map(|k| {
                let x = k % w;
                let v = if x < 50 {
                    0.0
                } else if x < 120 {
                    0.18
                } else {
                    ((x as f32) / 15.0 - 10.0).exp2()
                };
                [v, v * 0.5, v * 0.25]
            })
            .collect();
        let dir = std::env::temp_dir().join(format!("sidecar-hdr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.hdr");
        write_hdr(&path, w, h, &rgb, Space::Rec709, Some(1000.0)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() < w * h * 4, "run-length encoding saved nothing");
        let (rw, rh, px) = read_hdr(&bytes);
        assert_eq!((rw, rh), (w, h));
        for (k, p) in px.iter().enumerate() {
            for c in 0..3 {
                let want = rgb[k * 3 + c];
                // RGBE keeps 8 bits of the largest channel; smaller channels lose the bits below.
                let tol = 2.0 * rgb[k * 3] / 256.0;
                assert!(
                    (p[c] - want).abs() <= tol + 1e-30,
                    "pixel {k} channel {c}: {} against {want}",
                    p[c]
                );
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn exr_files_carry_their_values_and_metadata() {
        let (w, h) = (64, 32);
        let rgb: Vec<f32> = (0..w * h * 3)
            .map(|k| (k as f32 * 0.37).sin().abs() * 5000.0)
            .collect();
        let dir = std::env::temp_dir().join(format!("sidecar-exr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.exr");
        write_exr(&path, w, h, &rgb, Space::AcesCg, Some(812.5), false).unwrap();
        use exr::prelude::*;
        let img = read_first_rgba_layer_from_file(
            &path,
            |res, _| (res.width(), vec![[0f32; 3]; res.width() * res.height()]),
            |px: &mut (usize, Vec<[f32; 3]>), pos, (r, g, b, _a): (f32, f32, f32, f32)| {
                px.1[pos.y() * px.0 + pos.x()] = [r, g, b];
            },
        )
        .unwrap();
        let got = &img.layer_data.channel_data.pixels.1;
        for (k, p) in got.iter().enumerate() {
            assert_eq!(*p, [rgb[k * 3], rgb[k * 3 + 1], rgb[k * 3 + 2]]);
        }
        let meta = MetaData::read_from_file(&path, false).unwrap();
        let header = &meta.headers[0];
        assert_eq!(header.own_attributes.white_luminance, Some(812.5));
        let c = header.shared_attributes.chromaticities.unwrap();
        assert!((c.white.x() - 0.32168).abs() < 1e-6);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn brackets_are_found_by_their_repeating_exposures() {
        let ev = |v: &[f64]| v.iter().map(|e| e.exp2()).collect::<Vec<_>>();
        assert_eq!(
            bracket_period(&ev(&[0.0, -2.0, 2.0, 0.0, -2.0, 2.0, 0.05, -2.0, 2.0])),
            3
        );
        assert_eq!(bracket_period(&ev(&[0.0, 0.0, 0.0, 0.0])), 1);
        assert_eq!(bracket_period(&ev(&[0.0, -1.0, -2.0, -3.0, -4.0])), 5);
        assert_eq!(bracket_period(&ev(&[0.0, -2.0, 0.0, -2.0])), 2);
    }

    #[test]
    fn white_luminance_follows_iso_12232() {
        // f/8, 1/125 s, ISO 100: 78·64 / (0.65·100/125) = 9600 cd/m².
        assert!((white_luminance(8.0, 1.0 / 125.0, 100.0) - 9600.0).abs() < 1e-6);
    }
}
