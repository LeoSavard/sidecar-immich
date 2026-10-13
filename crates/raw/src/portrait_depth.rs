// SPDX-License-Identifier: Apache-2.0
//! Apple's portrait depth signal, read from a HEIC's auxiliary image through the same external-decoder
//! boundary as `decoded::decode_heif` — nothing LGPL runs in this process; see that module's docs and
//! docs/ROADMAP.md, "Portrait depth maps".
//!
//! Not every Portrait photo carries the same aux image, checked against a stratified sample of this user's
//! own library (205 HEICs out of 7,588, `heif-convert --with-aux` + `exiftool`, iPhone 14 Pro through 17 Pro)
//! rather than assumed. In that sample: 91% carry an HDR gain map (near-universal, unrelated to depth); ~16%
//! (33/205) carry a literal raw disparity auxiliary image (`urn:mpeg:hevc:2015:auxid:2`, described further by
//! Apple's own `XMP-apdi:AuxiliaryImageType = disparity`); ~16% (32/205) carry the derived Portrait Effects
//! Matte (`urn:com:apple:photo:2018:aux:portraiteffectsmatte`) — the two overlap but aren't the same set: 18
//! of the 33 disparity-bearing photos have *no* effects matte at all, so the raw map is the more universal of
//! the two (segmentation apparently needs a recognized subject; the stereo disparity capture doesn't).
//!
//! The two are not interchangeable. The effects matte is a continuous 8-bit grey image at exactly half the
//! main image's linear dimensions (e.g. 1512×2016 of 3024×4032) — plenty sharp, but it is Photos' own
//! blur-strength weight, not a distance. The raw disparity image is a real geometric signal, with its own
//! `XMP-depthData` camera-calibration block (focal length, principal point, lens distortion), but at a
//! **fixed** low resolution regardless of the main photo's size — measured 576×768 on both a 3024×4032 and a
//! 4284×5712 capture from the same iPhone 17 Pro, so it is roughly a twentieth to a thirtieth of the main
//! image's pixel count, always. Every example found reports `XMP-depthData:Accuracy = relative`, never
//! `absolute`, even on these LiDAR-equipped Pro models — Apple's photo pipeline evidently publishes the
//! dual/triple-camera stereo disparity into the still photo, not the LiDAR's metric range, so *true* metric
//! depth, while representable in Apple's own format (`AVDepthData.DepthDataAccuracy.absolute`), is not what
//! ordinary Portrait captures on this hardware actually carry. The raw 8-bit samples are quantized per shot:
//! `XMP-apdi:IntMinValue`/`IntMaxValue` (always 0/255 so far) rescale linearly onto that photo's own
//! `FloatMinValue`/`FloatMaxValue` (e.g. 0.0616–0.571, consistent with inverse metres) — reading those two XMP
//! tags to undo the rescale is the next step, not done here yet, so `PortraitDepth::values` for `Disparity`
//! today are the raw 0–1 samples, not Apple's own disparity units.
//!
//! DNG (ProRAW) is a different, unhandled case: it carries the *derived* mattes too (same URNs, seen directly
//! in this library's DNGs), but as extra TIFF SubIFDs tagged `Semantic Name` (DNG 1.6's semantic-mask
//! extension) rather than a HEIF auxiliary image box — `heif-convert` has no reason to find anything in a
//! DNG, and none of the DNGs sampled here carried the raw disparity array or its calibration block at all.
//! Reading a DNG's depth-ish data needs its own TIFF-SubIFD reader; nothing here attempts it.

use crate::decoded::DecodeError;
use std::path::Path;

/// Env override for the command that dumps a HEIC's auxiliary images to files beside the main decode,
/// `%i`/`%o` as in `decoded::HEIF_DECODER_ENV`. Unlike that one, ImageMagick's HEIF delegate has no equivalent
/// flag — only `heif-convert` is known to expose auxiliary images at all (`--with-aux`, checked against its
/// own `--help`), so there is only one built-in command to try.
pub const DEPTH_DECODER_ENV: &str = "SIDECAR_HEIF_DEPTH_DECODER";

const DEPTH_COMMANDS: &[&str] = &["heif-convert --with-aux %i %o"];

/// What kind of depth signal was found, most to least literal — `read` prefers the first that's present.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DepthKind {
    /// Apple's AVDepthData, continuous, real but relative — see the module docs for what's confirmed (fixed
    /// 576×768 resolution, `Accuracy: relative`, a per-shot linear quantization this type doesn't undo yet).
    Disparity,
    /// `urn:com:apple:photo:2018:aux:portraiteffectsmatte` — how much background blur Photos already baked
    /// in, not a distance.
    EffectsMatte,
}

/// One greyscale sample per pixel, 0–1, linear — these are fractions already (a matte weight, or Apple's
/// normalized disparity), not light to run through an sRGB transfer function.
pub struct PortraitDepth {
    pub width: u32,
    pub height: u32,
    pub values: Vec<f32>,
    pub kind: DepthKind,
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

fn depth_command() -> Option<String> {
    if let Ok(configured) = std::env::var(DEPTH_DECODER_ENV)
        && !configured.trim().is_empty()
    {
        return Some(configured);
    }
    DEPTH_COMMANDS.iter().find(|c| which(c.split_whitespace().next().unwrap_or_default())).map(|c| c.to_string())
}

/// `heif-convert` names each auxiliary file after its aux type, but not uniformly: Apple's own URNs come
/// through as `<output stem>-<urn>.png` (colons sometimes kept, sometimes turned into '_' depending on
/// version and `--no-colons`), while the raw disparity image — tagged with the generic ISO base media type
/// `urn:mpeg:hevc:2015:auxid:2` rather than one of Apple's — comes out named `<output stem>-depth.png`
/// instead (checked against real Portrait photos, not assumed). Matching on the bare word covers both, and
/// none of the other aux types seen in this library (`portraiteffectsmatte`, `semanticskymatte`,
/// `semanticskinmatte`, `semantichairmatte`, `semanticteethmatte`, `semanticglassesmatte`, `hdrgainmap`,
/// `linearthumbnail`, `styledeltamap`) contain "depth" as a substring.
const AUX_NEEDLES: &[(&str, DepthKind)] = &[("disparity", DepthKind::Disparity), ("depth", DepthKind::Disparity), ("portraiteffectsmatte", DepthKind::EffectsMatte)];

fn classify(filename: &str) -> Option<DepthKind> {
    let name = filename.to_ascii_lowercase();
    AUX_NEEDLES.iter().find(|(needle, _)| name.contains(needle)).map(|(_, kind)| *kind)
}

/// Read the most useful depth-like auxiliary image in `path`, if any. `None`, not an error, when there is no
/// known aux-capable decoder installed or the file carries none of `AUX_NEEDLES`: this enriches a photo that
/// can already be opened, and is never why one can't be.
pub fn read(path: &Path) -> Result<Option<PortraitDepth>, DecodeError> {
    let Some(template) = depth_command() else { return Ok(None) };
    let dir = tempfile::tempdir().map_err(|e| DecodeError::HeifDecoder(e.to_string()))?;
    let out = dir.path().join("decoded.png");
    run(&template, path, &out)?;

    let found = std::fs::read_dir(dir.path())
        .map_err(|e| DecodeError::HeifDecoder(e.to_string()))?
        .filter_map(Result::ok)
        .filter_map(|entry| classify(&entry.file_name().to_string_lossy()).map(|kind| (kind, entry.path())))
        .min_by_key(|(kind, _)| *kind);
    let Some((kind, file)) = found else { return Ok(None) };

    let bytes = std::fs::read(&file).map_err(|e| DecodeError::HeifDecoder(format!("reading {}: {e}", file.display())))?;
    let (width, height, values) = decode_first_channel(&bytes)?;
    Ok(Some(PortraitDepth { width, height, values, kind }))
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

/// The first channel of a PNG, normalized to 0–1 by its bit depth. No sRGB transfer: depth and matte values
/// are fractions already, not light to decode.
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
    fn disparity_and_literal_depth_outrank_the_effects_matte() {
        assert_eq!(classify("out-urn_com_apple_photo_2020_aux_disparity.png"), Some(DepthKind::Disparity));
        assert_eq!(classify("out-urn:com:apple:photo:2020:aux:depth.png"), Some(DepthKind::Disparity));
        // The real name `heif-convert` gives the raw disparity image on an actual Portrait photo (its aux
        // type is the generic `urn:mpeg:hevc:2015:auxid:2`, not one of Apple's own URNs) — not assumed.
        assert_eq!(classify("out-depth.png"), Some(DepthKind::Disparity));
        assert_eq!(classify("out-urn_com_apple_photo_2018_aux_portraiteffectsmatte.png"), Some(DepthKind::EffectsMatte));
        assert_eq!(classify("out-urn_com_apple_photo_2020_aux_hdrgainmap.png"), None);
        assert_eq!(classify("out-urn_com_apple_photo_2020_aux_semanticskymatte.png"), None);
        assert_eq!(classify("out.png"), None);

        let found = [(DepthKind::EffectsMatte, std::path::PathBuf::from("a")), (DepthKind::Disparity, std::path::PathBuf::from("b"))];
        assert_eq!(found.into_iter().min_by_key(|(kind, _)| *kind).map(|(k, _)| k), Some(DepthKind::Disparity));
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
    fn the_first_channel_is_read_without_gamma_and_other_channels_are_ignored() {
        // Grey, 8-bit: a matte's own shape (full range, not just the two ends).
        let grey = png_bytes(3, 1, png::ColorType::Grayscale, png::BitDepth::Eight, &[0, 128, 255]);
        let (w, h, v) = decode_first_channel(&grey).unwrap();
        assert_eq!((w, h), (3, 1));
        assert_eq!(v, vec![0.0, 128.0 / 255.0, 1.0]);

        // RGB with R != G != B: only the first channel is read, unlike a light-decoding path that would need
        // all three.
        let rgb = png_bytes(1, 1, png::ColorType::Rgb, png::BitDepth::Eight, &[64, 10, 20]);
        let (_, _, v) = decode_first_channel(&rgb).unwrap();
        assert_eq!(v, vec![64.0 / 255.0]);

        // 16-bit grey: big-endian samples, normalized by the 16-bit range, not 8-bit's.
        let grey16 = png_bytes(2, 1, png::ColorType::Grayscale, png::BitDepth::Sixteen, &[0x00, 0x00, 0xFF, 0xFF]);
        let (_, _, v) = decode_first_channel(&grey16).unwrap();
        assert_eq!(v, vec![0.0, 1.0]);
    }

    /// Pins the measurement in the module docs against the repo's own fixture. Skips, rather than fails, where
    /// `heif-convert` isn't installed — the published image doesn't ship one either (docs/ROADMAP.md) — or the
    /// fixture isn't there: `testdata/cache` is gitignored test-run output (DESIGN.md, "Development"), not a
    /// committed asset, so a clean checkout or worktree never has it.
    #[test]
    fn img_3042_has_an_effects_matte_at_half_the_main_images_size() {
        if !which("heif-convert") {
            return eprintln!("skipping: heif-convert not installed");
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture = root.join("testdata/cache/IMG_3042.heic");
        if !fixture.is_file() {
            return eprintln!("skipping: no {} (not a committed fixture)", fixture.display());
        }
        let depth = read(&fixture).unwrap().expect("IMG_3042.heic carries a portrait effects matte");
        assert_eq!(depth.kind, DepthKind::EffectsMatte);
        assert_eq!((depth.width, depth.height), (1512, 2016));
        let (min, max) = depth.values.iter().fold((1.0f32, 0.0f32), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(min < 0.01 && max > 0.99, "expected the full 0..1 range, got {min}..{max}");
    }
}
