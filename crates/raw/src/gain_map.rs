// SPDX-License-Identifier: Apache-2.0
//! Apple's HDR gain map, read from a HEIC's auxiliary image through the same external-decoder boundary as
//! `decoded::decode_heif` and `portrait_depth::read` — nothing LGPL runs in this process. See
//! docs/ROADMAP.md, "HDR originals keep their gain map".
//!
//! Checked against a real iPhone 17 Pro HEIC rather than assumed: the aux image is
//! `urn:com:apple:photo:2020:aux:hdrgainmap`, `heif-convert --with-aux` writes it as
//! `<stem>-urn_com_apple_photo_2020_aux_hdrgainmap.png`, and it is an 8-bit grey image at exactly half the
//! main image's linear size (1512×2016 of 3024×4032; 1512×1512 of a 3024×3024 square crop) — the same
//! resolution convention as the Portrait Effects Matte (`portrait_depth`), not a coincidence: both are
//! Apple's "normal-looking derived image, half the main one's size" shape.
//!
//! `exiftool` carries four numbers alongside it, and only two are clearly the same thing:
//! `XMP-HDRGainMap:HDRGainMapVersion` (131072 = 0.2.0.0 — ExifTool's four-byte reading of it) and
//! `XMP-HDRGainMap:HDRGainMapHeadroom` (e.g. 3.325102) sit in the gain map's own tag group and are named for
//! it. `Apple:HDRHeadroom` (e.g. 1.00999999, on the same photo) and `Apple:HDRGain` (e.g. 1.87978518) are
//! MakerNotes scalars that sound related but are carried here unused: nothing confirms what they mean against
//! `HDRGainMapHeadroom`, and guessing would be worse than saying so.
//!
//! **There is no reconstruction formula here, and that is deliberate.** Apple hasn't published one, and the
//! independent attempts to reverse-engineer it that exist call their own results "known to be incorrect" or
//! "unverified" — even Photos on iPhone and on Mac are reported to render the same file differently. A wrong
//! formula would look plausible and be silently wrong, which is worse than not rendering HDR at all. This
//! module's job stops at extracting and preserving the map and its metadata; applying it (docs/ROADMAP.md
//! stage 2) needs a real HDR display to check against before it can be trusted, not another guess.
//!
//! **A second, newer mechanism exists and this module doesn't read it.** Some files also carry an ICC
//! profile (`Display P3 Primaries; PQ (Adaptive Gain Curve …)`) with a binary `HD Gain Map Info` tag — Apple's
//! iOS 18+ "Adaptive HDR" / ISO 21496-1-adjacent encoding, a different mechanism from the auxiliary-image one
//! above (WWDC24; Apple's own encoding still isn't interchangeable with Adobe/Google's ISO 21496-1 files in
//! practice). Investigating that is separate follow-up work, not done here.

use crate::decoded::DecodeError;
use std::path::Path;

/// Env override for the command that dumps a HEIC's auxiliary images to files beside the main decode,
/// `%i`/`%o` as in `decoded::HEIF_DECODER_ENV` and `portrait_depth::DEPTH_DECODER_ENV` (which this
/// deliberately doesn't share — the three call sites happen to agree today, but nothing requires an operator
/// to point them at the same program).
pub const GAINMAP_DECODER_ENV: &str = "SIDECAR_HEIF_GAINMAP_DECODER";

const GAINMAP_COMMANDS: &[&str] = &["heif-convert --with-aux %i %o"];

/// The raw numbers `exiftool` carries beside the map — see the module docs for which are understood.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GainMapMeta {
    pub version: Option<i64>,
    pub headroom: Option<f32>,
    pub apple_hdr_headroom: Option<f32>,
    pub apple_hdr_gain: Option<f32>,
}

/// One greyscale sample per pixel, 0–1, linear — a stored fraction, not light to run through an sRGB
/// transfer function, and not yet run through any reconstruction formula either (see the module docs).
pub struct GainMap {
    pub width: u32,
    pub height: u32,
    pub values: Vec<f32>,
    pub meta: GainMapMeta,
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

fn gainmap_command() -> Option<String> {
    if let Ok(configured) = std::env::var(GAINMAP_DECODER_ENV)
        && !configured.trim().is_empty()
    {
        return Some(configured);
    }
    GAINMAP_COMMANDS.iter().find(|c| which(c.split_whitespace().next().unwrap_or_default())).map(|c| c.to_string())
}

fn is_gain_map_file(filename: &str) -> bool {
    filename.to_ascii_lowercase().contains("hdrgainmap")
}

/// Read the HDR gain map in `path`, if any. `None`, not an error, when there is no known aux-capable decoder
/// installed or the file carries none: this enriches a photo that can already be opened, and is never why one
/// can't be.
pub fn read(path: &Path) -> Result<Option<GainMap>, DecodeError> {
    let Some(template) = gainmap_command() else { return Ok(None) };
    let dir = tempfile::tempdir().map_err(|e| DecodeError::HeifDecoder(e.to_string()))?;
    let out = dir.path().join("decoded.png");
    run(&template, path, &out)?;

    let found = std::fs::read_dir(dir.path())
        .map_err(|e| DecodeError::HeifDecoder(e.to_string()))?
        .filter_map(Result::ok)
        .find(|entry| is_gain_map_file(&entry.file_name().to_string_lossy()));
    let Some(entry) = found else { return Ok(None) };

    let bytes = std::fs::read(entry.path()).map_err(|e| DecodeError::HeifDecoder(format!("reading {}: {e}", entry.path().display())))?;
    let (width, height, values) = decode_first_channel(&bytes)?;
    Ok(Some(GainMap { width, height, values, meta: read_meta(path) }))
}

/// One `exiftool` run for the four scalars the module docs name — matching `decoded::exif`'s own rule of one
/// run per photo rather than one per tag, even though this one happens alone rather than beside the rest of
/// `decoded::exif`'s tags: it is only ever wanted lazily, when a gain map was actually found.
fn read_meta(path: &Path) -> GainMapMeta {
    let Ok(output) =
        std::process::Command::new("exiftool").args(["-j", "-n", "-HDRGainMapVersion", "-HDRGainMapHeadroom", "-HDRHeadroom", "-HDRGain"]).arg(path).output()
    else {
        return GainMapMeta::default();
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else { return GainMapMeta::default() };
    let Some(tags) = json.as_array().and_then(|a| a.first()) else { return GainMapMeta::default() };
    GainMapMeta {
        version: tags.get("HDRGainMapVersion").and_then(|v| v.as_i64()),
        headroom: tags.get("HDRGainMapHeadroom").and_then(|v| v.as_f64()).map(|v| v as f32),
        apple_hdr_headroom: tags.get("HDRHeadroom").and_then(|v| v.as_f64()).map(|v| v as f32),
        apple_hdr_gain: tags.get("HDRGain").and_then(|v| v.as_f64()).map(|v| v as f32),
    }
}

fn run(template: &str, input: &Path, out: &Path) -> Result<(), DecodeError> {
    let mut parts =
        template.split_whitespace().map(|part| part.replace("%i", &input.to_string_lossy()).replace("%o", &out.to_string_lossy()));
    let program = parts.next().ok_or_else(|| DecodeError::HeifDecoder("empty decoder command".into()))?;
    let result = std::process::Command::new(&program)
        .args(parts)
        .output()
        .map_err(|e| DecodeError::HeifDecoder(format!("running {program}: {e}")))?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(DecodeError::HeifDecoder(format!("{program} exited {}: {}", result.status, stderr.trim())));
    }
    Ok(())
}

/// The first channel of a PNG, normalized to 0–1 by its bit depth. No sRGB transfer: the stored samples are
/// fractions already, not light to decode.
fn decode_first_channel(bytes: &[u8]) -> Result<(u32, u32, Vec<f32>), DecodeError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| DecodeError::Decode(format!("PNG: {e}")))?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or_else(|| DecodeError::Decode("PNG too large".into()))?];
    let frame = reader.next_frame(&mut buf).map_err(|e| DecodeError::Decode(format!("PNG: {e}")))?;
    let channels = frame.color_type.samples();
    let used = &buf[..frame.buffer_size()];
    let info = reader.info();
    let values = match frame.bit_depth {
        png::BitDepth::Sixteen => {
            used.as_chunks::<2>().0.iter().enumerate().filter(|(i, _)| i % channels == 0).map(|(_, b)| u16::from_be_bytes(*b) as f32 / 65535.0).collect()
        }
        _ => used.iter().step_by(channels).map(|&v| v as f32 / 255.0).collect(),
    };
    Ok((info.width, info.height, values))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_hdrgainmap_file_is_picked_whatever_colons_became() {
        assert!(is_gain_map_file("out-urn_com_apple_photo_2020_aux_hdrgainmap.png"));
        assert!(is_gain_map_file("out-urn:com:apple:photo:2020:aux:hdrgainmap.png"));
        assert!(!is_gain_map_file("out-urn_com_apple_photo_2018_aux_portraiteffectsmatte.png"));
        assert!(!is_gain_map_file("out-depth.png"));
        assert!(!is_gain_map_file("out.png"));
    }

    fn png_bytes(w: u32, h: u32, color: png::ColorType, depth: png::BitDepth, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(color);
        enc.set_depth(depth);
        enc.write_header().unwrap().write_image_data(data).unwrap();
        out
    }

    #[test]
    fn the_first_channel_is_read_without_gamma() {
        let grey = png_bytes(3, 1, png::ColorType::Grayscale, png::BitDepth::Eight, &[0, 128, 255]);
        let (w, h, v) = decode_first_channel(&grey).unwrap();
        assert_eq!((w, h), (3, 1));
        assert_eq!(v, vec![0.0, 128.0 / 255.0, 1.0]);
    }

    /// Pins the measurements in the module docs against a real file. Skips, rather than fails, where
    /// `heif-convert` or `exiftool` isn't installed, or the fixture isn't there (not committed — see
    /// `portrait_depth`'s equivalent test for why).
    #[test]
    fn a_real_hdr_photo_has_a_half_size_gain_map_and_its_own_headroom() {
        if !which("heif-convert") || !which("exiftool") {
            return eprintln!("skipping: heif-convert or exiftool not installed");
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture = root.join("testdata/cache/IMG_2367.heic");
        if !fixture.is_file() {
            return eprintln!("skipping: no {} (not a committed fixture)", fixture.display());
        }
        let map = read(&fixture).unwrap().expect("IMG_2367.heic carries an HDR gain map");
        assert_eq!((map.width, map.height), (1512, 1512));
        assert!(map.meta.headroom.is_some_and(|h| h > 1.0), "expected a headroom above 1.0, got {:?}", map.meta.headroom);
        assert!(map.meta.version.is_some());
    }
}
