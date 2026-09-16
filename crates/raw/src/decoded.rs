// SPDX-License-Identifier: Apache-2.0
//! Non-RAW sources: JPEG, PNG and TIFF decoded in-process, HEIC through an external decoder.
//!
//! These arrive already demosaiced, white-balanced, tone-mapped and gamma-encoded, so they skip the ingest
//! stages that assume a mosaic (hot pixels, demosaic, highlight reconstruction) and join the pipeline where
//! the RAW path does: linear light, in the source's own primaries, orientation not yet applied.
//!
//! The colour model needs no special case downstream. `SourceSpace::camera_color` hands the develop pass a
//! `CameraColor` whose "camera" space *is* sRGB or Display P3, with unity as-shot multipliers — so
//! `camera::resolve` returns unity multipliers and the correct primaries matrix, and temperature and tint
//! become relative shifts around D65 rather than an illuminant estimate. That is what they can mean here:
//! the white balance was baked in by whatever wrote the file, and there is no sensor data to re-derive it
//! from. See docs/ROADMAP.md, "Editing non-RAW images".
//!
//! **HEIC decoding is deliberately not built in.** libheif is LGPL-3.0-or-later, which this Apache-2.0 image
//! cannot link (CLAUDE.md), and HEVC carries patent-pool exposure separate from any copyright licence. So
//! HEIC is decoded by running a separate program, if the operator installed one — the same boundary the
//! image already documents for ExifTool. The published image ships no HEIF decoder and reports HEIC as
//! unsupported; a deployment opts in by installing one.

use crate::{RawInfo, SensorKind};
use serde::{Deserialize, Serialize};
use sidecar_color::camera::{Calibration, CameraColor};
use sidecar_color::mat;
use std::path::Path;

/// Environment override for the HEIC decode command, with `%i` and `%o` standing for the input and output
/// paths, e.g. `magick %i png48:%o`. Unset means "look for a decoder we know how to drive".
pub const HEIF_DECODER_ENV: &str = "SIDECAR_HEIF_DECODER";

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("{0} is not an image Sidecar can open")]
    Unsupported(String),
    #[error("HEIC needs an external decoder, and none is installed. Set {HEIF_DECODER_ENV}, or install one \
             (for example libheif-examples or ImageMagick) — the published image ships none, because the \
             HEIF decoders available are LGPL and HEVC is patent-encumbered.")]
    NoHeifDecoder,
    #[error("the HEIC decoder failed: {0}")]
    HeifDecoder(String),
    #[error("{0}")]
    Decode(String),
}

/// The colour space a decoded non-RAW image already lives in. Both use the sRGB transfer function and a D65
/// white; they differ only in primaries.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SourceSpace {
    #[default]
    Srgb,
    DisplayP3,
}

impl SourceSpace {
    /// Guess from an ICC profile description. Anything unrecognised is treated as sRGB, which is what an
    /// untagged JPEG almost always is.
    pub fn from_profile(description: &str) -> Self {
        if description.contains("P3") { Self::DisplayP3 } else { Self::Srgb }
    }

    /// XYZ → this space, derived from the working-space constants rather than written out again, so there is
    /// one source of truth for the primaries.
    fn xyz_to_rgb(self) -> mat::M3 {
        let from_rec2020 = match self {
            Self::Srgb => &mat::REC2020_TO_SRGB,
            Self::DisplayP3 => &mat::REC2020_TO_P3,
        };
        mat::mul(from_rec2020, &mat::XYZ_TO_REC2020)
    }

    /// A `CameraColor` that makes the existing colour pipeline treat this space as the camera's native one.
    pub fn camera_color(self) -> CameraColor {
        CameraColor {
            as_shot_mul: [1.0, 1.0, 1.0],
            calibrations: vec![Calibration { temperature_k: 6504.0, xyz_to_camera: self.xyz_to_rgb() }],
        }
    }
}

/// Linear RGBA in the source's own primaries, orientation not yet applied — the same shape the RAW path
/// hands the worker, so downsampling, the focus measure and the develop pass are all unchanged.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<f32>,
    /// Rotation *still owed* to these pixels, in LibRaw's numbering — zero when the decoder already applied
    /// it. Distinct from `Exif::orientation_flip`, which describes the source however it was decoded; mixing
    /// the two is how an image ends up rotated twice or not at all.
    pub pending_flip: i32,
    pub space: SourceSpace,
}

/// Extensions handled here. RAW extensions are LibRaw's business; the list lives in `sidecar_template` so
/// the api can answer "will this open?" without depending on LibRaw.
pub fn is_supported(path: &Path) -> bool {
    sidecar_template::is_image(path)
}

fn extension(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase()
}

/// sRGB EOTF over a lookup table — exact for integer input, and much faster than a `powf` per sample on a
/// 36-megapixel frame.
fn linear_lut(levels: usize) -> Vec<f32> {
    (0..levels)
        .map(|i| {
            let v = i as f32 / (levels - 1) as f32;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        })
        .collect()
}

/// Decoded samples, kept in their own width so an 8-bit frame doesn't pay for a widening copy — at 36
/// megapixels that copy alone is a couple of hundred megabytes.
enum Samples {
    Eight(Vec<u8>),
    Sixteen(Vec<u16>),
}

/// Interleaved integer samples → linear RGBA f32. `channels` is 1, 3 or 4.
///
/// Alpha is **zero**, not one, and any alpha the source carried is dropped. Downstream, alpha is not opacity:
/// it is the fraction of the pixel clipped at the sensor, and `highlight_blend` pulls those pixels toward
/// neutral to kill the magenta cast white-balance multipliers leave on reconstructed highlights. None of
/// that applies to an already-developed file — its blown highlights are flat white already, and its
/// multipliers are unity — so the honest value is zero. Writing one here quietly desaturates the whole
/// frame by the highlight-recovery strength.
fn to_linear_rgba(samples: &Samples, channels: usize) -> Vec<f32> {
    fn convert<T: Copy>(samples: &[T], channels: usize, levels: usize, index: impl Fn(T) -> usize) -> Vec<f32> {
        let lut = linear_lut(levels);
        let mut out = Vec::with_capacity(samples.len() / channels * 4);
        for p in samples.chunks_exact(channels) {
            let (r, g, b) = if channels == 1 { (p[0], p[0], p[0]) } else { (p[0], p[1], p[2]) };
            out.push(lut[index(r)]);
            out.push(lut[index(g)]);
            out.push(lut[index(b)]);
            out.push(0.0);
        }
        out
    }
    match samples {
        Samples::Eight(v) => convert(v, channels, 256, |s| s as usize),
        Samples::Sixteen(v) => convert(v, channels, 65536, |s| s as usize),
    }
}

/// Binary PPM/PGM (P5, P6). This is the fast road out of an external decoder: compressing a 36-megapixel
/// frame to PNG costs about twenty seconds of deflate, and writing it raw costs two.
fn decode_pnm(bytes: &[u8]) -> Result<(u32, u32, Vec<f32>), DecodeError> {
    let bad = |what: &str| DecodeError::Decode(format!("PNM: {what}"));
    let mut fields: Vec<u64> = Vec::new();
    let mut magic = None;
    let mut at = 0usize;
    // Header tokens are whitespace-separated, and `#` comments may appear wherever whitespace may.
    while at < bytes.len() && fields.len() < 3 {
        match bytes[at] {
            b'#' => while at < bytes.len() && bytes[at] != b'\n' {
                at += 1;
            },
            c if c.is_ascii_whitespace() => at += 1,
            _ => {
                let start = at;
                while at < bytes.len() && !bytes[at].is_ascii_whitespace() {
                    at += 1;
                }
                let token = std::str::from_utf8(&bytes[start..at]).map_err(|_| bad("unreadable header"))?;
                match magic {
                    None => magic = Some(token.to_string()),
                    Some(_) => fields.push(token.parse().map_err(|_| bad("non-numeric header field"))?),
                }
            }
        }
    }
    let channels = match magic.as_deref() {
        Some("P6") => 3,
        Some("P5") => 1,
        other => return Err(bad(&format!("unsupported magic {other:?}; only binary P5 and P6 are read"))),
    };
    let [width, height, maxval] = fields[..] else { return Err(bad("truncated header")) };
    // Exactly one whitespace byte separates the header from the samples.
    let data = bytes.get(at + 1..).ok_or_else(|| bad("no sample data"))?;
    let count = (width as usize) * (height as usize) * channels;
    let samples = if maxval > 255 {
        let wanted = count * 2;
        if data.len() < wanted {
            return Err(bad("sample data is short"));
        }
        Samples::Sixteen(data[..wanted].as_chunks::<2>().0.iter().map(|b| u16::from_be_bytes(*b)).collect())
    } else {
        if data.len() < count {
            return Err(bad("sample data is short"));
        }
        Samples::Eight(data[..count].to_vec())
    };
    Ok((width as u32, height as u32, to_linear_rgba(&samples, channels)))
}

fn decode_jpeg(bytes: &[u8]) -> Result<(u32, u32, Vec<f32>), DecodeError> {
    let mut decoder = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(bytes));
    let pixels = decoder.decode().map_err(|e| DecodeError::Decode(format!("JPEG: {e}")))?;
    let info = decoder.info().ok_or_else(|| DecodeError::Decode("JPEG has no frame header".into()))?;
    let channels = decoder.output_colorspace().map(|c| c.num_components()).unwrap_or(3);
    Ok((info.width as u32, info.height as u32, to_linear_rgba(&Samples::Eight(pixels), channels)))
}

fn decode_png(bytes: &[u8]) -> Result<(u32, u32, Vec<f32>), DecodeError> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().map_err(|e| DecodeError::Decode(format!("PNG: {e}")))?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or_else(|| DecodeError::Decode("PNG too large".into()))?];
    let frame = reader.next_frame(&mut buf).map_err(|e| DecodeError::Decode(format!("PNG: {e}")))?;
    let info = reader.info();
    let channels = frame.color_type.samples();
    let used = &buf[..frame.buffer_size()];
    let samples = match frame.bit_depth {
        png::BitDepth::Sixteen => Samples::Sixteen(used.as_chunks::<2>().0.iter().map(|b| u16::from_be_bytes(*b)).collect()),
        _ => Samples::Eight(used.to_vec()),
    };
    Ok((info.width, info.height, to_linear_rgba(&samples, channels)))
}

fn decode_tiff(bytes: &[u8]) -> Result<(u32, u32, Vec<f32>), DecodeError> {
    use tiff::decoder::DecodingResult;
    let mut decoder = tiff::decoder::Decoder::new(std::io::Cursor::new(bytes)).map_err(|e| DecodeError::Decode(format!("TIFF: {e}")))?;
    let (width, height) = decoder.dimensions().map_err(|e| DecodeError::Decode(format!("TIFF: {e}")))?;
    let colour = decoder.colortype().map_err(|e| DecodeError::Decode(format!("TIFF: {e}")))?;
    let channels = match colour {
        tiff::ColorType::Gray(_) => 1,
        tiff::ColorType::RGB(_) => 3,
        tiff::ColorType::RGBA(_) => 4,
        other => return Err(DecodeError::Decode(format!("TIFF colour type {other:?} isn't supported"))),
    };
    let samples = match decoder.read_image().map_err(|e| DecodeError::Decode(format!("TIFF: {e}")))? {
        DecodingResult::U8(v) => Samples::Eight(v),
        DecodingResult::U16(v) => Samples::Sixteen(v),
        _ => return Err(DecodeError::Decode("TIFF sample format isn't supported".into())),
    };
    Ok((width, height, to_linear_rgba(&samples, channels)))
}

/// Commands we know how to drive, tried in order when the environment doesn't name one. ImageMagick is asked
/// for 16-bit PPM: uncompressed, so a 36-megapixel frame costs about two seconds instead of the twenty PNG's
/// deflate would, and 16 bits keeps whatever precision a 10-bit HDR HEIC had. `heif-convert` only writes PNG
/// or JPEG, so it pays the deflate.
const HEIF_COMMANDS: &[&str] = &["magick %i -depth 16 ppm:%o", "convert %i -depth 16 ppm:%o", "heif-convert %i %o"];

fn heif_command() -> Option<String> {
    if let Ok(configured) = std::env::var(HEIF_DECODER_ENV)
        && !configured.trim().is_empty()
    {
        return Some(configured);
    }
    HEIF_COMMANDS.iter().find(|c| which(c.split_whitespace().next().unwrap_or_default())).map(|c| c.to_string())
}

/// What the command will write, read back off the template so a custom `SIDECAR_HEIF_DECODER` naming `ppm:`
/// or `.tif` is handled without a second setting to keep in step.
fn heif_output_extension(template: &str) -> &'static str {
    let t = template.to_ascii_lowercase();
    if t.contains("ppm") || t.contains("pnm") {
        "ppm"
    } else if t.contains("tif") {
        "tif"
    } else {
        "png"
    }
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

/// Run an external decoder over `path` and read back what it wrote. Nothing licensed runs in this process:
/// this is a `Command`, not a link.
fn decode_heif(path: &Path) -> Result<(u32, u32, Vec<f32>), DecodeError> {
    let template = heif_command().ok_or(DecodeError::NoHeifDecoder)?;
    let dir = tempfile::tempdir().map_err(|e| DecodeError::HeifDecoder(e.to_string()))?;
    let out = dir.path().join(format!("decoded.{}", heif_output_extension(&template)));

    let mut parts = template.split_whitespace().map(|part| {
        part.replace("%i", &path.to_string_lossy()).replace("%o", &out.to_string_lossy())
    });
    let program = parts.next().ok_or_else(|| DecodeError::HeifDecoder("empty decoder command".into()))?;
    let result = std::process::Command::new(&program)
        .args(parts)
        .output()
        .map_err(|e| DecodeError::HeifDecoder(format!("running {program}: {e}")))?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(DecodeError::HeifDecoder(format!("{program} exited {}: {}", result.status, stderr.trim())));
    }
    let bytes = std::fs::read(&out).map_err(|e| DecodeError::HeifDecoder(format!("reading {}: {e}", out.display())))?;
    match heif_output_extension(&template) {
        "ppm" => decode_pnm(&bytes),
        "tif" => decode_tiff(&bytes),
        _ => decode_png(&bytes),
    }
}

/// Decode a non-RAW image, given the EXIF already read for it — the orientation (see `crate::af`'s notes on
/// the numbering) and the colour space its ICC profile claims. When the decoder has already applied the
/// rotation, the returned `flip` is zero so the worker doesn't rotate twice.
pub fn decode(path: &Path, exif: &Exif) -> Result<Decoded, DecodeError> {
    let orientation_flip = exif.orientation_flip;
    let ext = extension(path);
    let (width, height, rgba) = match ext.as_str() {
        "heic" | "heif" => decode_heif(path)?,
        _ => {
            let bytes = std::fs::read(path).map_err(|e| DecodeError::Decode(format!("reading {}: {e}", path.display())))?;
            match ext.as_str() {
                "jpg" | "jpeg" => decode_jpeg(&bytes)?,
                "png" => decode_png(&bytes)?,
                "tif" | "tiff" => decode_tiff(&bytes)?,
                _ => return Err(DecodeError::Unsupported(path.display().to_string())),
            }
        }
    };

    // A decoder that rotated the pixels itself leaves the frame transposed against what EXIF describes; in
    // that case the rotation is already spent. Checking the pixels beats trusting each tool's flags.
    let rotates = matches!(orientation_flip, 5 | 6);
    let pending_flip = if rotates && width < height { 0 } else { orientation_flip };

    Ok(Decoded { width, height, rgba, pending_flip, space: exif.space })
}

/// Metadata for a non-RAW source, shaped like the RAW path's so the api and the editor need no special case.
/// Read from EXIF alone — decoding a 36-megapixel HEIC just to learn its dimensions would cost seconds on
/// every open. `width` and `height` are the visible size *after* orientation, matching `RawFile::info`.
pub fn info(path: &Path, exif: &Exif) -> RawInfo {
    let rotated = matches!(exif.orientation_flip, 5 | 6);
    let (w, h) = (exif.width, exif.height);
    RawInfo {
        make: exif.make.clone(),
        model: exif.model.clone(),
        lens: exif.lens.clone(),
        width: if rotated { h } else { w },
        height: if rotated { w } else { h },
        orientation_flip: exif.orientation_flip,
        sensor: SensorKind::Other,
        iso: exif.iso,
        shutter_s: exif.shutter_s,
        aperture: exif.aperture,
        focal_length_mm: exif.focal_length_mm,
        captured_unix: exif.captured_unix,
        color: exif.space.camera_color(),
        af_point: crate::af::af_point(path, exif.orientation_flip),
        source_kind: crate::SourceKind::Developed,
    }
}

/// The EXIF a non-RAW file carries, read with ExifTool — LibRaw won't open these, and ExifTool is already in
/// the image for metadata copying.
#[derive(Clone, Debug, Default)]
pub struct Exif {
    pub make: String,
    pub model: String,
    pub lens: String,
    /// Stored size, before orientation.
    pub width: u32,
    pub height: u32,
    pub iso: f32,
    pub shutter_s: f32,
    pub aperture: f32,
    pub focal_length_mm: f32,
    pub captured_unix: i64,
    /// How the source says it should be displayed, whoever ends up doing the rotating.
    pub orientation_flip: i32,
    pub space: SourceSpace,
}

/// EXIF orientation (1…8) → LibRaw's flip numbering, the same mapping `RgbaF32::oriented` expects.
fn flip_from_orientation(orientation: i64) -> i32 {
    match orientation {
        3 => 3,
        6 => 6,
        8 => 5,
        _ => 0,
    }
}

pub fn exif(path: &Path) -> Exif {
    let Ok(output) = std::process::Command::new("exiftool")
        .args([
            "-j", "-n", "-Make", "-Model", "-LensModel", "-ImageWidth", "-ImageHeight", "-ISO",
            "-ExposureTime", "-FNumber", "-FocalLength", "-DateTimeOriginal", "-Orientation", "-ProfileDescription",
        ])
        .arg(path)
        .output()
    else {
        return Exif::default();
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else { return Exif::default() };
    let Some(tags) = json.as_array().and_then(|a| a.first()) else { return Exif::default() };
    let text = |key: &str| tags.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let number = |key: &str| tags.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0);

    Exif {
        make: text("Make"),
        model: text("Model"),
        lens: text("LensModel"),
        width: number("ImageWidth") as u32,
        height: number("ImageHeight") as u32,
        iso: number("ISO") as f32,
        shutter_s: number("ExposureTime") as f32,
        aperture: number("FNumber") as f32,
        focal_length_mm: number("FocalLength") as f32,
        captured_unix: captured_unix(&text("DateTimeOriginal")),
        orientation_flip: flip_from_orientation(number("Orientation") as i64),
        space: SourceSpace::from_profile(&text("ProfileDescription")),
    }
}

/// `YYYY:MM:DD HH:MM:SS` in the camera's own local time, which is all EXIF records — read as if it were UTC,
/// matching what LibRaw hands back for a RAW so the two paths sort together.
fn captured_unix(stamp: &str) -> i64 {
    let n: Vec<i64> = stamp.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
    let Some([year, month, day, hour, minute, second]) = n.get(..6).and_then(|s| <[i64; 6]>::try_from(s).ok()) else {
        return 0;
    };
    // Days from civil (H. Hinnant), the inverse of the formatting in sidecar_xmp::utc_now.
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    days * 86400 + hour * 3600 + minute * 60 + second
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the colour model: unity multipliers, and a matrix that lands the source's own
    /// white on Rec.2020 neutral. If this holds, temperature and tint behave as relative shifts and nothing
    /// downstream needs a non-RAW special case.
    #[test]
    fn a_source_space_resolves_to_neutral_at_d65() {
        for space in [SourceSpace::Srgb, SourceSpace::DisplayP3] {
            let camera = space.camera_color();
            let xy = camera.as_shot_xy();
            let wb = sidecar_color::camera::resolve(&camera, xy);
            assert!(wb.mul.iter().all(|m| (m - 1.0).abs() < 1e-3), "{space:?} multipliers {:?}", wb.mul);
            let white = mat::apply(&wb.cam_to_rec2020, &[1.0, 1.0, 1.0]);
            assert!(white.iter().all(|v| (v - 1.0).abs() < 1e-9), "{space:?} white {white:?}");
            assert!((wb.temperature_k - 6504.0).abs() < 200.0, "{space:?} temperature {}", wb.temperature_k);
        }
    }

    /// Display P3 is wider, so the same encoded triplet has to reach further once expressed in Rec.2020.
    #[test]
    fn display_p3_is_wider_than_srgb() {
        let red = |space: SourceSpace| {
            let camera = space.camera_color();
            let wb = sidecar_color::camera::resolve(&camera, camera.as_shot_xy());
            mat::apply(&wb.cam_to_rec2020, &[1.0, 0.0, 0.0])
        };
        assert!(red(SourceSpace::DisplayP3)[0] > red(SourceSpace::Srgb)[0]);
    }

    #[test]
    fn the_srgb_curve_is_inverted_exactly_at_the_ends() {
        let lut = linear_lut(256);
        assert_eq!(lut[0], 0.0);
        assert!((lut[255] - 1.0).abs() < 1e-6);
        // Mid grey, 128/255 encoded, is about 21.6 % of linear light.
        assert!((lut[128] - 0.2158).abs() < 1e-3, "{}", lut[128]);
    }

    #[test]
    fn orientation_maps_onto_libraws_numbering() {
        assert_eq!(flip_from_orientation(1), 0);
        assert_eq!(flip_from_orientation(3), 3);
        assert_eq!(flip_from_orientation(6), 6);
        assert_eq!(flip_from_orientation(8), 5);
    }

    /// Caught by reading the develop shader, not by looking at a render: alpha feeds `highlight_blend` as a
    /// sensor-clip fraction, so a constant 1.0 desaturates every pixel by the recovery strength — about half
    /// the colour out of the whole frame, uniformly enough that it looks like a plausible image.
    #[test]
    fn alpha_is_zero_because_nothing_here_is_sensor_clipped() {
        let rgba = to_linear_rgba(&Samples::Eight(vec![255, 128, 0, 10, 20, 30]), 3);
        assert_eq!(rgba.len(), 8);
        assert_eq!(rgba[3], 0.0);
        assert_eq!(rgba[7], 0.0);
    }

    #[test]
    fn a_grey_ramp_round_trips_through_the_transfer_function() {
        // Encoded mid grey stays mid grey in linear light, and white stays white.
        let rgba = to_linear_rgba(&Samples::Eight(vec![0, 0, 0, 128, 128, 128, 255, 255, 255]), 3);
        assert_eq!(rgba[0], 0.0);
        assert!((rgba[4] - 0.2158).abs() < 1e-3);
        assert!((rgba[8] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn exif_timestamps_convert_to_unix_seconds() {
        // 1970-01-01 00:00:00 is the epoch, and 2026-07-29 18:30:00 is a date this library actually holds.
        assert_eq!(captured_unix("1970:01:01 00:00:00"), 0);
        assert_eq!(captured_unix("2000:03:01 00:00:00"), 951868800);
        assert_eq!(captured_unix("2026:07:29 18:30:00"), 1785349800);
        assert_eq!(captured_unix(""), 0);
        assert_eq!(captured_unix("nonsense"), 0);
    }

    #[test]
    fn supported_extensions_are_recognised_case_insensitively() {
        assert!(is_supported(Path::new("a.HEIC")));
        assert!(is_supported(Path::new("a.jpg")));
        assert!(!is_supported(Path::new("a.NEF")));
    }
}
