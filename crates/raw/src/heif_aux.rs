// SPDX-License-Identifier: Apache-2.0
//! A HEIC's auxiliary images — Apple's depth, mattes and HDR gain map — through the same external-decoder
//! boundary as `decoded::decode_heif`: nothing LGPL runs in this process. One decoder run writes every
//! auxiliary image to a temporary directory; `portrait_depth` and `gain_map` each pick theirs from the same
//! extraction, so a photo is decoded once however many of them are wanted.

use crate::decoded::DecodeError;
use std::path::{Path, PathBuf};

/// Env override for the command that writes a HEIC's auxiliary images beside its main decode, `%i`/`%o` as in
/// `decoded::HEIF_DECODER_ENV`. ImageMagick's HEIF delegate has no equivalent, so `heif-convert --with-aux`
/// (checked against its own `--help`) is the only built-in command.
pub const AUX_DECODER_ENV: &str = "SIDECAR_HEIF_AUX_DECODER";

const AUX_COMMANDS: &[&str] = &["heif-convert --with-aux %i %o"];

/// One greyscale sample per pixel, 0–1, linear: auxiliary images store fractions (a matte weight, a
/// normalized disparity, a gain-map value), not light to run through an sRGB transfer function.
pub struct Plane {
    pub width: u32,
    pub height: u32,
    pub values: Vec<f32>,
}

/// Every auxiliary image one decoder run wrote, alive until this is dropped.
pub struct AuxImages {
    _dir: tempfile::TempDir,
    files: Vec<(String, PathBuf)>,
}

impl AuxImages {
    /// `None`, not an error, when no aux-capable decoder is installed: these images enrich a photo that can
    /// already be opened, and are never why one can't be.
    pub fn extract(path: &Path) -> Result<Option<Self>, DecodeError> {
        let Some(template) = command() else { return Ok(None) };
        let dir = tempfile::tempdir().map_err(|e| DecodeError::HeifDecoder(e.to_string()))?;
        run(&template, path, &dir.path().join("decoded.png"))?;
        let files = std::fs::read_dir(dir.path())
            .map_err(|e| DecodeError::HeifDecoder(e.to_string()))?
            .filter_map(Result::ok)
            .map(|e| (e.file_name().to_string_lossy().to_ascii_lowercase(), e.path()))
            .collect();
        Ok(Some(Self { _dir: dir, files }))
    }

    /// The lowercased file names `heif-convert` gave the images (each named after its auxiliary type).
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(|(n, _)| n.as_str())
    }

    /// Read the first image whose lowercased file name `matches` accepts.
    pub fn read(&self, matches: impl Fn(&str) -> bool) -> Result<Option<Plane>, DecodeError> {
        let Some((_, file)) = self.files.iter().find(|(n, _)| matches(n)) else { return Ok(None) };
        let bytes = std::fs::read(file).map_err(|e| DecodeError::HeifDecoder(format!("reading {}: {e}", file.display())))?;
        decode_first_channel(&bytes).map(Some)
    }
}

/// Whether `program` is on `PATH` (tests that need a real decoder skip without one).
pub(crate) fn which(program: &str) -> bool {
    std::env::var_os("PATH").map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file())).unwrap_or(false)
}

fn command() -> Option<String> {
    if let Ok(configured) = std::env::var(AUX_DECODER_ENV)
        && !configured.trim().is_empty()
    {
        return Some(configured);
    }
    AUX_COMMANDS.iter().find(|c| which(c.split_whitespace().next().unwrap_or_default())).map(|c| c.to_string())
}

fn run(template: &str, input: &Path, out: &Path) -> Result<(), DecodeError> {
    let mut parts = template.split_whitespace().map(|part| part.replace("%i", &input.to_string_lossy()).replace("%o", &out.to_string_lossy()));
    let program = parts.next().ok_or_else(|| DecodeError::HeifDecoder("empty decoder command".into()))?;
    let result =
        std::process::Command::new(&program).args(parts).output().map_err(|e| DecodeError::HeifDecoder(format!("running {program}: {e}")))?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(DecodeError::HeifDecoder(format!("{program} exited {}: {}", result.status, stderr.trim())));
    }
    Ok(())
}

/// The first channel of a PNG, normalized to 0–1 by its bit depth, with no transfer function applied.
pub fn decode_first_channel(bytes: &[u8]) -> Result<Plane, DecodeError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| DecodeError::Decode(format!("PNG: {e}")))?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or_else(|| DecodeError::Decode("PNG too large".into()))?];
    let frame = reader.next_frame(&mut buf).map_err(|e| DecodeError::Decode(format!("PNG: {e}")))?;
    let channels = frame.color_type.samples();
    let used = &buf[..frame.buffer_size()];
    let values = match frame.bit_depth {
        png::BitDepth::Sixteen => {
            used.as_chunks::<2>().0.iter().enumerate().filter(|(i, _)| i % channels == 0).map(|(_, b)| u16::from_be_bytes(*b) as f32 / 65535.0).collect()
        }
        _ => used.iter().step_by(channels).map(|&v| v as f32 / 255.0).collect(),
    };
    Ok(Plane { width: frame.width, height: frame.height, values })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let grey = decode_first_channel(&png_bytes(3, 1, png::ColorType::Grayscale, png::BitDepth::Eight, &[0, 128, 255])).unwrap();
        assert_eq!((grey.width, grey.height), (3, 1));
        assert_eq!(grey.values, vec![0.0, 128.0 / 255.0, 1.0]);
        // RGB with R != G != B: only the first channel is read.
        let rgb = decode_first_channel(&png_bytes(1, 1, png::ColorType::Rgb, png::BitDepth::Eight, &[64, 10, 20])).unwrap();
        assert_eq!(rgb.values, vec![64.0 / 255.0]);
        // 16-bit grey: big-endian samples, normalized by the 16-bit range, not 8-bit's.
        let grey16 = decode_first_channel(&png_bytes(2, 1, png::ColorType::Grayscale, png::BitDepth::Sixteen, &[0, 0, 0xFF, 0xFF])).unwrap();
        assert_eq!(grey16.values, vec![0.0, 1.0]);
    }
}
