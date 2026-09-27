// SPDX-License-Identifier: Apache-2.0
//! DNGs LibRaw can't open: linear (already demosaiced) images stored as JPEG XL tiles (DNG 1.7, compression
//! 52546). Lightroom writes its panorama merges this way since 2023 — six of the library's merges — and LibRaw
//! 0.22 reports them as unsupported. Decoded here with `jxl-oxide` (MIT OR Apache-2.0).
//!
//! The pixels are camera-native linear RGB, the same thing the RAW path hands the develop pass after its
//! demosaic, so they join the pipeline at that point and develop as a RAW does: white balance from
//! `AsShotNeutral`, colour from `ColorMatrix1/2`. Only what that needs is read from the TIFF structure; the
//! rest of the EXIF comes from ExifTool, as for any file LibRaw doesn't handle.
//!
//! A merge can be far larger than any GPU texture (a 23,474 × 13,049 panorama is 306 MP), so decoding takes a
//! size bound and averages tiles down into it as they are decoded: the whole image is never held at full size.

use std::path::Path;

use sidecar_color::camera::{Calibration, CameraColor};

use crate::{RawInfo, SensorKind, SourceKind};

#[derive(Debug, thiserror::Error)]
pub enum LinearDngError {
    #[error("not a JPEG XL linear DNG")]
    NotThisKind,
    #[error("DNG: {0}")]
    Malformed(String),
    #[error("JPEG XL tile: {0}")]
    Tile(String),
}

fn bad(s: impl Into<String>) -> LinearDngError {
    LinearDngError::Malformed(s.into())
}

/// A TIFF value, widened: integers and rationals both read as f64 lists (text is not needed here).
#[derive(Clone, Debug)]
struct Field {
    values: Vec<f64>,
}

struct Tiff<'a> {
    d: &'a [u8],
    le: bool,
}

impl<'a> Tiff<'a> {
    fn u16(&self, o: usize) -> Option<u16> {
        let b: [u8; 2] = self.d.get(o..o + 2)?.try_into().ok()?;
        Some(if self.le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    }
    fn u32(&self, o: usize) -> Option<u32> {
        let b: [u8; 4] = self.d.get(o..o + 4)?.try_into().ok()?;
        Some(if self.le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    }

    /// One IFD's tags, and the offset of the next IFD.
    fn ifd(&self, off: usize) -> Option<(std::collections::HashMap<u16, Field>, usize)> {
        let n = self.u16(off)? as usize;
        let mut tags = std::collections::HashMap::new();
        for i in 0..n.min(4096) {
            let e = off + 2 + 12 * i;
            let (tag, typ, count) = (self.u16(e)?, self.u16(e + 2)?, self.u32(e + 4)? as usize);
            let size = match typ {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 => 8,
                _ => continue,
            };
            let total = size * count;
            let at = if total <= 4 { e + 8 } else { self.u32(e + 8)? as usize };
            if self.d.len() < at + total || count > 1 << 24 {
                continue;
            }
            let value = |k: usize| -> f64 {
                let o = at + k * size;
                match typ {
                    1 | 7 => self.d[o] as f64,
                    6 => self.d[o] as i8 as f64,
                    3 => self.u16(o).unwrap_or(0) as f64,
                    8 => self.u16(o).unwrap_or(0) as i16 as f64,
                    4 | 13 => self.u32(o).unwrap_or(0) as f64,
                    9 => self.u32(o).unwrap_or(0) as i32 as f64,
                    5 => self.u32(o).unwrap_or(0) as f64 / self.u32(o + 4).unwrap_or(1).max(1) as f64,
                    10 => self.u32(o).unwrap_or(0) as i32 as f64 / (self.u32(o + 4).unwrap_or(1) as i32).max(1) as f64,
                    11 => f32::from_bits(self.u32(o).unwrap_or(0)) as f64,
                    _ => 0.0,
                }
            };
            let values = if typ == 2 { Vec::new() } else { (0..count).map(value).collect() };
            tags.insert(tag, Field { values });
        }
        Some((tags, self.u32(off + 2 + 12 * n)? as usize))
    }
}

type Tags = std::collections::HashMap<u16, Field>;

/// The DNG's first IFD (colour, orientation) and the IFD holding the full-resolution image.
struct Layout {
    ifd0: Tags,
    main: Tags,
}

fn layout(d: &[u8]) -> Option<Layout> {
    let le = match d.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let t = Tiff { d, le };
    if t.u16(2)? != 42 {
        return None;
    }
    let (ifd0, _) = t.ifd(t.u32(4)? as usize)?;
    // Full resolution: NewSubfileType 0, in IFD0 itself or one of its SubIFDs (tag 330).
    let full = |tags: &Tags| tags.get(&254).and_then(|f| f.values.first()).copied().unwrap_or(0.0) == 0.0;
    if full(&ifd0) && ifd0.contains_key(&324) {
        return Some(Layout { main: ifd0.clone(), ifd0 });
    }
    let subs: Vec<usize> = ifd0.get(&330)?.values.iter().map(|&v| v as usize).collect();
    let main = subs.into_iter().filter_map(|o| t.ifd(o).map(|x| x.0)).find(full)?;
    Some(Layout { ifd0, main })
}

fn first(tags: &Tags, tag: u16) -> Option<f64> {
    tags.get(&tag)?.values.first().copied()
}

/// True for a DNG whose full-resolution image is linear RGB in JPEG XL tiles — what this module decodes.
pub fn is_supported(path: &Path) -> bool {
    if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dng")) {
        return false;
    }
    // The IFDs sit near the start in every DNG seen; the tile data is what's large.
    let Ok(head) = read_head(path, 1 << 20) else { return false };
    layout(&head).is_some_and(|l| first(&l.main, 259) == Some(52546.0) && first(&l.main, 262) == Some(34892.0))
}

fn read_head(path: &Path, n: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut buf = Vec::with_capacity(n);
    std::fs::File::open(path)?.take(n as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// EXIF LightSource → correlated colour temperature, as the RAW path reads DNG calibration illuminants.
fn illuminant(code: f64) -> Option<f64> {
    crate::illuminant_temperature(code as u16)
}

fn matrix(tags: &Tags, tag: u16) -> Option<[[f64; 3]; 3]> {
    let v = &tags.get(&tag)?.values;
    (v.len() >= 9).then(|| [[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]])
}

/// The colour model a linear DNG declares: its calibrations and as-shot white.
fn color(ifd0: &Tags) -> CameraColor {
    let mut calibrations = Vec::new();
    for (m, i) in [(50721, 50778), (50722, 50779)] {
        if let (Some(m), Some(t)) = (matrix(ifd0, m), first(ifd0, i).and_then(illuminant)) {
            calibrations.push(Calibration { temperature_k: t, xyz_to_camera: m });
        }
    }
    if calibrations.is_empty()
        && let Some(m) = matrix(ifd0, 50721)
    {
        calibrations.push(Calibration { temperature_k: 6504.0, xyz_to_camera: m });
    }
    let neutral = ifd0.get(&50728).map(|f| f.values.clone()).filter(|v| v.len() == 3 && v.iter().all(|&x| x > 0.0));
    let as_shot_mul = neutral.map_or([1.0, 1.0, 1.0], |n| [n[1] / n[0], 1.0, n[1] / n[2]]);
    CameraColor { as_shot_mul, calibrations }
}

/// Metadata for the develop pass and the editor, shaped like the RAW path's. `exif` supplies what a TIFF walk
/// here doesn't: camera, exposure, capture time.
pub fn info(path: &Path, exif: &crate::decoded::Exif) -> Result<RawInfo, LinearDngError> {
    let head = read_head(path, 1 << 20).map_err(|e| bad(e.to_string()))?;
    let l = layout(&head).ok_or(LinearDngError::NotThisKind)?;
    let (w, h) = (first(&l.main, 256).unwrap_or(0.0) as u32, first(&l.main, 257).unwrap_or(0.0) as u32);
    let flip = crate::decoded::flip_from_orientation(first(&l.ifd0, 274).unwrap_or(1.0) as i64);
    let rotated = flip & 4 != 0;
    Ok(RawInfo {
        make: exif.make.clone(),
        model: exif.model.clone(),
        lens: exif.lens.clone(),
        width: if rotated { h } else { w },
        height: if rotated { w } else { h },
        orientation_flip: flip,
        sensor: SensorKind::Other,
        iso: exif.iso,
        shutter_s: exif.shutter_s,
        aperture: exif.aperture,
        focal_length_mm: exif.focal_length_mm,
        captured_unix: exif.captured_unix,
        color: color(&l.ifd0),
        af_point: None,
        lens_profile: None,
        lens_identity: None,
        source_kind: SourceKind::Raw,
        source_space: None,
    })
}

/// Linear camera RGB, alpha = 1 where a channel reached the white level. Unoriented.
pub struct Linear {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<f32>,
}

/// Decode the full-resolution image, averaged down by a whole factor so its long edge is at most
/// `max_edge`. Tiles are decoded one at a time and folded straight into the smaller image.
pub fn decode(path: &Path, max_edge: u32) -> Result<Linear, LinearDngError> {
    let data = std::fs::read(path).map_err(|e| bad(e.to_string()))?;
    let l = layout(&data).ok_or(LinearDngError::NotThisKind)?;
    let m = &l.main;
    if first(m, 259) != Some(52546.0) || first(m, 262) != Some(34892.0) {
        return Err(LinearDngError::NotThisKind);
    }
    let (w, h) = (first(m, 256).ok_or_else(|| bad("no width"))? as usize, first(m, 257).ok_or_else(|| bad("no height"))? as usize);
    let (tw, th) = (first(m, 322).ok_or_else(|| bad("not tiled"))? as usize, first(m, 323).ok_or_else(|| bad("not tiled"))? as usize);
    let offsets = &m.get(&324).ok_or_else(|| bad("no tile offsets"))?.values;
    let counts = &m.get(&325).ok_or_else(|| bad("no tile sizes"))?.values;
    let (across, down) = (w.div_ceil(tw), h.div_ceil(th));
    if offsets.len() < across * down || counts.len() < across * down {
        return Err(bad("tile table shorter than the image"));
    }
    let black = first(m, 50714).unwrap_or(0.0);
    let white = first(m, 50717).unwrap_or(65535.0);
    let bits = first(m, 258).unwrap_or(16.0);
    let full = (1u64 << bits as u32) as f64 - 1.0;
    let range = (white - black).max(1.0);

    let factor = w.max(h).div_ceil(max_edge.max(1) as usize).max(1);
    let (ow, oh) = (w.div_ceil(factor), h.div_ceil(factor));
    let mut sum = vec![0f32; ow * oh * 4];
    let mut n = vec![0u32; ow * oh];
    // One tile: its rendered width, height, channel count and samples.
    let tile = |i: usize| -> Result<(usize, usize, usize, Vec<f32>), LinearDngError> {
        let (o, c) = (offsets[i] as usize, counts[i] as usize);
        let bytes = data.get(o..o + c).ok_or_else(|| bad("tile past the end of the file"))?;
        let mut img = jxl_oxide::JxlImage::builder().read(std::io::Cursor::new(bytes)).map_err(|e| LinearDngError::Tile(e.to_string()))?;
        // Lightroom tags each tile as BT.2100 primaries with the sRGB transfer curve so JPEG XL's perceptual
        // coding spends its bits sensibly; the samples themselves are the DNG's linear values. Rendering in that
        // same encoding with a linear transfer hands them back unchanged in colour — the default render
        // returned them gamma-encoded, which developed as a flat, magenta picture.
        if let jxl_oxide::color::ColourEncoding::Enum(e) = &img.image_header().metadata.colour_encoding {
            let mut linear = e.clone();
            linear.tf = jxl_oxide::color::TransferFunction::Linear;
            img.request_color_encoding(linear);
        }
        let render = img.render_frame(0).map_err(|e| LinearDngError::Tile(e.to_string()))?;
        let fb = render.image_all_channels();
        if fb.channels() < 3 {
            return Err(LinearDngError::Tile(format!("{} channels", fb.channels())));
        }
        Ok((fb.width(), fb.height(), fb.channels(), fb.buf().to_vec()))
    };
    // A row of tiles at a time, decoded in parallel, then folded into the smaller image in order: memory stays
    // at one row of full-resolution tiles.
    use rayon::prelude::*;
    for ty in 0..down {
        let row: Vec<_> = (0..across).into_par_iter().map(|tx| tile(ty * across + tx)).collect::<Result<_, _>>()?;
        for (tx, (fw, fh, ch, buf)) in row.into_iter().enumerate() {
            for y in 0..fh.min(th) {
                let gy = ty * th + y;
                if gy >= h {
                    break;
                }
                for x in 0..fw.min(tw) {
                    let gx = tx * tw + x;
                    if gx >= w {
                        break;
                    }
                    let s = &buf[(y * fw + x) * ch..(y * fw + x) * ch + 3];
                    let v = [s[0], s[1], s[2]].map(|v| ((v as f64 * full - black) / range) as f32);
                    let clip = v.iter().any(|&c| c >= 0.99) as u8 as f32;
                    let d = (gy / factor) * ow + gx / factor;
                    for (k, c) in v.iter().enumerate() {
                        sum[d * 4 + k] += c.max(0.0);
                    }
                    sum[d * 4 + 3] += clip;
                    n[d] += 1;
                }
            }
        }
    }
    for (p, &k) in sum.chunks_mut(4).zip(&n) {
        if k > 0 {
            p.iter_mut().for_each(|v| *v /= k as f32);
        }
    }
    Ok(Linear { width: ow as u32, height: oh as u32, rgba: sum })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian DNG skeleton: IFD0 with colour tags and one SubIFD with a JPEG XL linear image.
    #[test]
    fn finds_the_full_resolution_image_and_its_colour() {
        let mut d = Vec::new();
        d.extend(b"II");
        d.extend(42u16.to_le_bytes());
        d.extend(8u32.to_le_bytes());
        // IFD0 at 8: NewSubfileType 1, Orientation 6, SubIFDs → 200, AsShotNeutral → 300, CalibrationIlluminant1 21, ColorMatrix1 → 330.
        let entries: Vec<(u16, u16, u32, u32)> = vec![(254, 4, 1, 1), (274, 3, 1, 6), (330, 4, 1, 200), (50721, 10, 9, 330), (50728, 5, 3, 300), (50778, 3, 1, 21)];
        d.extend((entries.len() as u16).to_le_bytes());
        for (tag, typ, count, value) in &entries {
            d.extend(tag.to_le_bytes());
            d.extend(typ.to_le_bytes());
            d.extend(count.to_le_bytes());
            d.extend(value.to_le_bytes());
        }
        d.extend(0u32.to_le_bytes());
        d.resize(200, 0);
        // SubIFD at 200: NewSubfileType 0, 1000 × 500, compression 52546, photometric 34892.
        let sub: Vec<(u16, u16, u32, u32)> = vec![(254, 4, 1, 0), (256, 4, 1, 1000), (257, 4, 1, 500), (259, 4, 1, 52546), (262, 4, 1, 34892)];
        d.extend((sub.len() as u16).to_le_bytes());
        for (tag, typ, count, value) in &sub {
            d.extend(tag.to_le_bytes());
            d.extend(typ.to_le_bytes());
            d.extend(count.to_le_bytes());
            d.extend(value.to_le_bytes());
        }
        d.extend(0u32.to_le_bytes());
        d.resize(300, 0);
        for (num, den) in [(532225u32, 1000000u32), (1, 1), (778116, 1000000)] {
            d.extend(num.to_le_bytes());
            d.extend(den.to_le_bytes());
        }
        d.resize(330, 0);
        for v in [9336i32, -3405, 14, -7321, 14779, 2764, -914, 1171, 8248] {
            d.extend(v.to_le_bytes());
            d.extend(10000i32.to_le_bytes());
        }
        let l = layout(&d).expect("layout");
        assert_eq!(first(&l.main, 256), Some(1000.0));
        assert_eq!(first(&l.main, 259), Some(52546.0));
        let c = color(&l.ifd0);
        assert!((c.as_shot_mul[0] - 1.0 / 0.532225).abs() < 1e-6 && (c.as_shot_mul[2] - 1.0 / 0.778116).abs() < 1e-6, "{:?} from {:?}", c.as_shot_mul, l.ifd0.get(&50728));
        assert_eq!(c.calibrations.len(), 1);
        assert_eq!(c.calibrations[0].temperature_k, 6504.0);
        assert!((c.calibrations[0].xyz_to_camera[1][1] - 1.4779).abs() < 1e-9);
        assert_eq!(crate::decoded::flip_from_orientation(first(&l.ifd0, 274).unwrap() as i64), 6);
    }
}
