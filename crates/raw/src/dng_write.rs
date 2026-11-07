// SPDX-License-Identifier: Apache-2.0
//! Writing a linear DNG: what an HDR merge, a panorama, a focus stack or a denoised frame produces, so the result
//! is an original like any other — opened, edited, versioned and exported by the code that already does that
//! (docs/research/multi-frame-and-denoise.md).
//!
//! The image is camera-native linear RGB (`PhotometricInterpretation` LinearRaw), exactly what the RAW path hands
//! the develop pass after its demosaic, stored as 16-bit IEEE floats so a merge keeps whatever range it has —
//! values above 1 are scene light above the source frames' white, not clipping. 256² tiles, Deflate with the
//! floating-point predictor of Adobe's TIFF Technical Note 3, as the DNG specification (1.4 onward) allows for
//! floating-point data. Colour is the source camera's: its `ColorMatrix` calibrations and as-shot neutral, so
//! white balance and the camera matrix work as they did on the frames it came from. The measured noise model
//! goes in DNG's own `NoiseProfile` (σ² = S·x + O per plane), where any DNG reader can find it.
//!
//! Written from the DNG specification 1.7 and TIFF 6.0; `linear_dng` reads it back, and LibRaw — an independent
//! decoder — opens it too.

use std::io::Write;
use std::path::Path;

use sidecar_color::camera::CameraColor;

/// What a linear DNG says about itself beyond its pixels.
pub struct DngMeta<'a> {
    pub make: &'a str,
    pub model: &'a str,
    pub software: &'a str,
    pub color: &'a CameraColor,
    /// TIFF orientation, 1 for none.
    pub orientation: u16,
    /// σ² = gain·x + read, in this file's units (white = 1), per plane alike; `None` to leave it out.
    pub noise: Option<(f64, f64)>,
    /// DNG's `LinearResponseLimit`: the level, in this file's units, above which a sample is no longer a
    /// measurement — where its sensor saturated. 1.0 for a single frame; `None` for a merge in which no sample
    /// reached a source's saturation, whose every value is light.
    pub linear_response_limit: Option<f64>,
}

const TILE: usize = 256;

/// DNG's illuminant code for a calibration's temperature: the nearest of the standard light sources.
fn illuminant_code(temperature_k: f64) -> u16 {
    const CODES: [(u16, f64); 9] = [
        (17, 2856.0),
        (24, 3200.0),
        (15, 3450.0),
        (14, 4150.0),
        (18, 4874.0),
        (23, 5003.0),
        (20, 5503.0),
        (21, 6504.0),
        (22, 7504.0),
    ];
    CODES
        .iter()
        .min_by(|a, b| {
            (a.1 - temperature_k)
                .abs()
                .total_cmp(&(b.1 - temperature_k).abs())
        })
        .map_or(21, |c| c.0)
}

/// One TIFF field, with its value bytes (little-endian) and count.
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    bytes: Vec<u8>,
}

const SHORT: u16 = 3;
const LONG: u16 = 4;
const RATIONAL: u16 = 5;
const SRATIONAL: u16 = 10;
const BYTE: u16 = 1;
const ASCII: u16 = 2;
const DOUBLE: u16 = 12;

fn shorts(tag: u16, v: &[u16]) -> Entry {
    Entry {
        tag,
        kind: SHORT,
        count: v.len() as u32,
        bytes: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}

fn longs(tag: u16, v: &[u32]) -> Entry {
    Entry {
        tag,
        kind: LONG,
        count: v.len() as u32,
        bytes: v.iter().flat_map(|x| x.to_le_bytes()).collect(),
    }
}

fn ascii(tag: u16, s: &str) -> Entry {
    let mut bytes: Vec<u8> = s.bytes().filter(|b| b.is_ascii() && *b != 0).collect();
    bytes.push(0);
    Entry {
        tag,
        kind: ASCII,
        count: bytes.len() as u32,
        bytes,
    }
}

/// Rationals with a fixed denominator of 10⁶: six decimals, the precision matrices are published to.
fn rationals(tag: u16, v: &[f64], signed: bool) -> Entry {
    let bytes = v
        .iter()
        .flat_map(|x| {
            let den = 1_000_000i64;
            let num = (x * den as f64).round() as i64;
            let mut b = Vec::with_capacity(8);
            if signed {
                b.extend((num as i32).to_le_bytes());
                b.extend((den as i32).to_le_bytes());
            } else {
                b.extend((num.max(0) as u32).to_le_bytes());
                b.extend((den as u32).to_le_bytes());
            }
            b
        })
        .collect();
    Entry {
        tag,
        kind: if signed { SRATIONAL } else { RATIONAL },
        count: v.len() as u32,
        bytes,
    }
}

/// TIFF Technical Note 3's floating-point predictor, for one row of `samples` values of `bytes_per` bytes,
/// stored little-endian in `row`: the bytes are regrouped most significant first across the whole row, then
/// differenced byte by byte with a stride of `spp`. In place; `decode_float_row` undoes it.
fn encode_float_row(row: &mut [u8], samples: usize, bytes_per: usize, spp: usize) {
    let mut shuffled = vec![0u8; row.len()];
    for i in 0..samples {
        for b in 0..bytes_per {
            // Byte b of the big-endian value is byte (bytes_per - 1 - b) of the little-endian one.
            shuffled[b * samples + i] = row[i * bytes_per + (bytes_per - 1 - b)];
        }
    }
    for j in (spp..shuffled.len()).rev() {
        shuffled[j] = shuffled[j].wrapping_sub(shuffled[j - spp]);
    }
    row.copy_from_slice(&shuffled);
}

/// The inverse of `encode_float_row`: back to little-endian samples.
pub(crate) fn decode_float_row(row: &mut [u8], samples: usize, bytes_per: usize, spp: usize) {
    for j in spp..row.len() {
        row[j] = row[j].wrapping_add(row[j - spp]);
    }
    let mut out = vec![0u8; row.len()];
    for i in 0..samples {
        for b in 0..bytes_per {
            out[i * bytes_per + (bytes_per - 1 - b)] = row[b * samples + i];
        }
    }
    row.copy_from_slice(&out);
}

/// One tile, compressed: `rgb` is the whole image (3 floats a pixel, row-major), the tile is padded to its full
/// size by repeating the image's last row and column, as TIFF requires every tile to be whole.
fn tile_bytes(rgb: &[f32], w: usize, h: usize, tx: usize, ty: usize) -> std::io::Result<Vec<u8>> {
    let mut raw = vec![0u8; TILE * TILE * 3 * 2];
    for y in 0..TILE {
        let gy = (ty * TILE + y).min(h - 1);
        let row = &mut raw[y * TILE * 6..(y + 1) * TILE * 6];
        for x in 0..TILE {
            let gx = (tx * TILE + x).min(w - 1);
            for c in 0..3 {
                let v = half::f16::from_f32(rgb[(gy * w + gx) * 3 + c]).to_bits();
                row[(x * 3 + c) * 2..(x * 3 + c) * 2 + 2].copy_from_slice(&v.to_le_bytes());
            }
        }
        encode_float_row(row, TILE * 3, 2, 3);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(6));
    z.write_all(&raw)?;
    z.finish()
}

/// Write `rgb` (`w × h` pixels, three floats each, camera-native linear, white = 1) as a linear DNG at `path`.
pub fn write(path: &Path, w: u32, h: u32, rgb: &[f32], meta: &DngMeta) -> std::io::Result<()> {
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 || rgb.len() < w * h * 3 {
        return Err(std::io::Error::other("an image needs pixels"));
    }
    let (across, down) = (w.div_ceil(TILE), h.div_ceil(TILE));
    use rayon::prelude::*;
    let tiles: Vec<Vec<u8>> = (0..across * down)
        .into_par_iter()
        .map(|i| tile_bytes(rgb, w, h, i % across, i / across))
        .collect::<std::io::Result<_>>()?;

    // The directory and its values first, after the 8-byte header, then the pixel data: readers that look for
    // the directory near the start of the file (LibRaw's identification among them) find it there. Offsets
    // depend on the directory's size, which depends only on how many tiles there are, so they are placed in a
    // second step.
    let counts: Vec<u32> = tiles.iter().map(|t| t.len() as u32).collect();
    let offsets = vec![0u32; tiles.len()];
    let c = meta.color;
    let neutral = [
        1.0 / c.as_shot_mul[0].max(1e-9),
        1.0,
        1.0 / c.as_shot_mul[2].max(1e-9),
    ];
    let mut entries = vec![
        longs(254, &[0]),
        longs(256, &[w as u32]),
        longs(257, &[h as u32]),
        shorts(258, &[16, 16, 16]),
        shorts(259, &[8]),
        shorts(262, &[34892]),
        ascii(271, meta.make),
        ascii(272, meta.model),
        shorts(274, &[meta.orientation.clamp(1, 8)]),
        shorts(277, &[3]),
        shorts(284, &[1]),
        ascii(305, meta.software),
        shorts(317, &[3]),
        longs(322, &[TILE as u32]),
        longs(323, &[TILE as u32]),
        longs(324, &offsets),
        longs(325, &counts),
        shorts(339, &[3, 3, 3]),
        Entry {
            tag: 50706,
            kind: BYTE,
            count: 4,
            bytes: vec![1, 4, 0, 0],
        },
        Entry {
            tag: 50707,
            kind: BYTE,
            count: 4,
            bytes: vec![1, 4, 0, 0],
        },
        ascii(50708, &format!("{} {}", meta.make, meta.model)),
        // White is 1.0 — the specification's default for floating-point samples, stated because LibRaw assumes
        // the 16-bit integer white (65535) without it and scales every sample to zero.
        longs(50717, &[1, 1, 1]),
        rationals(50728, &neutral, false),
    ];
    for (i, cal) in c.calibrations.iter().take(2).enumerate() {
        let m: Vec<f64> = cal.xyz_to_camera.iter().flatten().copied().collect();
        entries.push(rationals(50721 + i as u16, &m, true));
        entries.push(shorts(
            50778 + i as u16,
            &[illuminant_code(cal.temperature_k)],
        ));
    }
    if let Some(l) = meta.linear_response_limit {
        entries.push(rationals(50734, &[l], false));
    }
    if let Some(b) = c.baseline_exposure {
        entries.push(rationals(50730, &[b], true));
    }
    if let Some((s, o)) = meta.noise {
        entries.push(Entry {
            tag: 51041,
            kind: DOUBLE,
            count: 2,
            bytes: [s, o].iter().flat_map(|v| v.to_le_bytes()).collect(),
        });
    }
    entries.sort_by_key(|e| e.tag);

    let serialize = |entries: &[Entry]| -> (Vec<u8>, Vec<u8>) {
        let ifd = 8usize;
        let mut values_at = ifd + 2 + entries.len() * 12 + 4;
        let (mut dir, mut values) = (Vec::new(), Vec::new());
        dir.extend((entries.len() as u16).to_le_bytes());
        for e in entries {
            dir.extend(e.tag.to_le_bytes());
            dir.extend(e.kind.to_le_bytes());
            dir.extend(e.count.to_le_bytes());
            if e.bytes.len() <= 4 {
                let mut v = e.bytes.clone();
                v.resize(4, 0);
                dir.extend(v);
            } else {
                dir.extend((values_at as u32).to_le_bytes());
                values.extend(&e.bytes);
                values_at += e.bytes.len();
                if values_at % 2 == 1 {
                    values.push(0);
                    values_at += 1;
                }
            }
        }
        dir.extend(0u32.to_le_bytes());
        (dir, values)
    };
    // Where the pixels start, and so where each tile goes.
    let (dir, values) = serialize(&entries);
    let mut at = 8 + dir.len() + values.len();
    let mut offsets = Vec::with_capacity(tiles.len());
    for t in &tiles {
        at += at % 2;
        offsets.push(at as u32);
        at += t.len();
    }
    if at > u32::MAX as usize {
        return Err(std::io::Error::other(
            "larger than a classic TIFF can address",
        ));
    }
    if let Some(e) = entries.iter_mut().find(|e| e.tag == 324) {
        *e = longs(324, &offsets);
    }
    let (dir, values) = serialize(&entries);
    let mut out: Vec<u8> = Vec::with_capacity(at);
    out.extend(b"II*\0");
    out.extend(8u32.to_le_bytes());
    out.extend(dir);
    out.extend(values);
    for (t, &o) in tiles.iter().zip(&offsets) {
        out.resize(o as usize, 0);
        out.extend(t);
    }
    if out.len() > u32::MAX as usize {
        return Err(std::io::Error::other(
            "larger than a classic TIFF can address",
        ));
    }
    std::fs::write(path, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sidecar_color::camera::Calibration;

    #[test]
    fn the_float_predictor_round_trips() {
        let mut row: Vec<u8> = (0..96u32).map(|i| (i * 37 % 251) as u8).collect();
        let orig = row.clone();
        encode_float_row(&mut row, 48, 2, 3);
        assert_ne!(row, orig);
        decode_float_row(&mut row, 48, 2, 3);
        assert_eq!(row, orig);
    }

    fn color() -> CameraColor {
        CameraColor {
            as_shot_mul: [2.1, 1.0, 1.6],
            calibrations: vec![
                Calibration {
                    temperature_k: 2856.0,
                    xyz_to_camera: [[1.1, -0.4, 0.1], [-0.3, 1.2, 0.1], [0.0, 0.1, 0.7]],
                },
                Calibration {
                    temperature_k: 6504.0,
                    xyz_to_camera: [[0.9, -0.3, -0.05], [-0.35, 1.15, 0.2], [-0.05, 0.15, 0.55]],
                },
            ],
            baseline_exposure: Some(0.35),
            profile: None,
        }
    }

    /// Written and read back through `linear_dng`: the same pixels to half-float precision, including light
    /// far above white, an odd size that leaves partial tiles, and the colour model.
    #[test]
    fn a_written_dng_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("merge.dng");
        let (w, h) = (601u32, 333u32);
        let rgb: Vec<f32> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let v = 0.001 * 1.04f32.powi(((x + y) % 200) as i32);
                [v, 0.5 * v, 2.0 * v]
            })
            .collect();
        assert!(
            rgb.iter().cloned().fold(0.0, f32::max) > 4.0,
            "light well above white"
        );
        let c = color();
        write(
            &path,
            w,
            h,
            &rgb,
            &DngMeta {
                make: "Nikon",
                model: "D5",
                software: "Sidecar test",
                color: &c,
                orientation: 6,
                noise: Some((4e-4, 2.5e-6)),
                linear_response_limit: None,
            },
        )
        .unwrap();
        assert!(crate::linear_dng::is_supported(&path));
        let back = crate::linear_dng::decode(&path, 10_000).unwrap();
        assert_eq!((back.width, back.height), (w, h));
        let worst = back
            .rgba
            .chunks(4)
            .zip(rgb.chunks(3))
            .map(|(a, b)| {
                (0..3)
                    .map(|k| ((a[k] - b[k]) / b[k]).abs())
                    .fold(0.0, f32::max)
            })
            .fold(0.0, f32::max);
        assert!(worst < 1e-3, "relative error {worst}");
        let info = crate::linear_dng::info(&path, &Default::default()).unwrap();
        assert_eq!(
            info.orientation_flip,
            crate::decoded::flip_from_orientation(6)
        );
        let got = &info.color;
        assert!((got.as_shot_mul[0] - 2.1).abs() < 1e-4 && (got.as_shot_mul[2] - 1.6).abs() < 1e-4);
        assert_eq!(got.calibrations.len(), 2);
        assert!((got.calibrations[1].xyz_to_camera[1][2] - 0.2).abs() < 1e-6);
        assert_eq!(got.baseline_exposure, Some(0.35));
    }

    /// LibRaw, an independent DNG decoder, opens what this writes and reads the same picture: its linear output
    /// tracks ours up to one overall scale (LibRaw normalises a float file to its own range). LibRaw takes a
    /// file as tiled only when a tile fits inside the image both ways, so an image under 256 pixels on an edge
    /// is misread there; nothing Sidecar merges is that small.
    #[test]
    fn libraw_reads_it_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("interop.dng");
        let (w, h) = (700u32, 450u32);
        let rgb: Vec<f32> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let v = 0.02 + 0.9 * (x as f32 / w as f32) * (0.3 + 0.7 * y as f32 / h as f32);
                [v, v, v]
            })
            .collect();
        let c = color();
        write(
            &path,
            w,
            h,
            &rgb,
            &DngMeta {
                make: "Nikon",
                model: "D5",
                software: "Sidecar test",
                color: &c,
                orientation: 1,
                noise: None,
                linear_response_limit: Some(1.0),
            },
        )
        .unwrap();
        let mut raw = crate::RawFile::open(&path).expect("LibRaw opens it");
        let info = raw.info();
        assert_eq!((info.width, info.height), (w, h));
        let lin = raw.develop_linear(false).expect("LibRaw decodes it");
        assert_eq!((lin.width, lin.height), (w, h));
        // Green, a bright and a dark pixel: the ratio is the picture's, whatever LibRaw's scale.
        let g = |x: u32, y: u32| lin.rgb[((y * w + x) * 3 + 1) as usize] as f64;
        let ours = |x: u32, y: u32| rgb[((y * w + x) * 3 + 1) as usize] as f64;
        let (a, b) = ((660, 420), (40, 30));
        let (theirs_ratio, ours_ratio) =
            (g(a.0, a.1) / g(b.0, b.1), ours(a.0, a.1) / ours(b.0, b.1));
        eprintln!("LibRaw's bright/dark ratio {theirs_ratio:.4}, ours {ours_ratio:.4}");
        assert!((theirs_ratio / ours_ratio - 1.0).abs() < 0.02);
    }
}
