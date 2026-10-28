// SPDX-License-Identifier: Apache-2.0
//! The camera's own rendering of a RAW, for the editor to show beside the edit: the largest JPEG the camera
//! embedded in the file (`JpgFromRaw`, `PreviewImage` or `OtherImage`, as ExifTool names them), with the RAW's
//! orientation tag copied onto it so a browser shows it upright. Cached under `camera/`. A JPEG, HEIC or TIFF
//! has none: it is its own rendering.
//!
//! A camera set to Adobe RGB writes that JPEG in Adobe RGB and says so **only in its metadata** — the
//! embedded preview itself carries no ICC profile, no `ColorSpace` and no `InteropIndex`. Every viewer then
//! has to guess, and the guess is sRGB, which renders the picture under-saturated. The RAW does say it
//! (`SourceSpace::from_tags`, the review's D11), so the profile is attached on the way out rather than the
//! omission passed on.
//!
//! `GET /v1/assets/{key}/camera`  the JPEG, or 404

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use crate::{CAMERA_GENERATION, Worker, unique_tmp};
use sidecar_color::camera::SourceSpace;

/// Adobe RGB (1998)-compatible, for a camera JPEG that is in that space and never said so in the file
/// itself. Compact-ICC-Profiles (CC0-1.0); the same project the output profiles come from.
const ADOBE_ICC: &[u8] = include_bytes!("../../../assets/icc/AdobeCompat-v4.icc");

/// The embedded JPEGs ExifTool can extract, any of which may be the largest.
const TAGS: [&str; 3] = ["JpgFromRaw", "PreviewImage", "OtherImage"];

impl Worker {
    pub async fn camera_jpeg(self: &Arc<Self>, key: &str) -> Result<Option<Vec<u8>>> {
        let target = self
            .cache
            .join("camera")
            .join(format!("{key}-c{CAMERA_GENERATION}.jpg"));
        if let Ok(bytes) = tokio::fs::read(&target).await {
            return Ok(Some(bytes));
        }
        let path = self
            .paths
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .context("unknown asset; ingest it first")?;
        if sidecar_raw::decoded::is_supported(&path) {
            return Ok(None);
        }
        tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>> {
            let Some(jpeg) = largest_embedded(&path)? else {
                return Ok(None);
            };
            std::fs::create_dir_all(target.parent().expect("in the cache"))?;
            let tmp = unique_tmp(&target);
            std::fs::write(&tmp, &jpeg)?;
            // The RAW's orientation, so the browser turns it as the editor turns the RAW. A file without one
            // leaves the JPEG as it is.
            let _ = Command::new("exiftool")
                .args(["-q", "-q", "-overwrite_original", "-TagsFromFile"])
                .arg(&path)
                .args(["-Orientation"])
                .arg(&tmp)
                .status();
            // What space the camera wrote it in, when the JPEG does not carry a profile saying so itself.
            if !has_icc(&jpeg) && sidecar_raw::decoded::exif(&path).space == SourceSpace::AdobeRgb {
                let icc = unique_tmp(&target.with_extension("icc"));
                if std::fs::write(&icc, ADOBE_ICC).is_ok() {
                    let _ = Command::new("exiftool")
                        .args(["-q", "-q", "-overwrite_original"])
                        .arg(format!("-icc_profile<={}", icc.display()))
                        .arg(&tmp)
                        .status();
                    let _ = std::fs::remove_file(&icc);
                }
            }
            std::fs::rename(&tmp, &target)?;
            Ok(Some(std::fs::read(&target)?))
        })
        .await?
    }
}

/// Does this JPEG already say what space it is in? `ICC_PROFILE\0` is the APP2 segment's identifier, and a
/// profile the camera wrote is the camera's own word — never overwritten by the guess below.
fn has_icc(jpeg: &[u8]) -> bool {
    jpeg.windows(12).any(|w| w == b"ICC_PROFILE\0")
}

/// The largest embedded JPEG, or none.
fn largest_embedded(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut best: Option<Vec<u8>> = None;
    for tag in TAGS {
        let out = Command::new("exiftool")
            .args(["-b", &format!("-{tag}")])
            .arg(path)
            .output()
            .context("running exiftool — is it installed?")?;
        let bytes = out.stdout;
        if bytes.starts_with(&[0xFF, 0xD8]) && best.as_ref().is_none_or(|b| bytes.len() > b.len()) {
            best = Some(bytes);
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The camera's own word wins: a preview that already carries a profile is never relabelled, and the
    /// identifier looked for is the APP2 segment's, not the words appearing anywhere in the file.
    #[test]
    fn a_profile_the_camera_wrote_is_recognised() {
        assert!(has_icc(
            b"\xff\xd8\xff\xe2\x00\x14ICC_PROFILE\x00\x01\x01rest"
        ));
        assert!(!has_icc(
            b"\xff\xd8\xff\xe1 Exif plain jpeg with no profile"
        ));
        // The string without its terminator is not the segment identifier.
        assert!(!has_icc(
            b"a comment mentioning ICC_PROFILE but not the marker"
        ));
    }

    /// What the tagging turns on: the review's D11, that a camera set to Adobe RGB says so in the RAW's
    /// metadata and writes the preview itself with nothing at all.
    #[test]
    fn adobe_rgb_is_what_the_raws_tags_say() {
        assert_eq!(SourceSpace::from_tags("", 2, ""), SourceSpace::AdobeRgb);
        assert_eq!(
            SourceSpace::from_tags("", 0xFFFF, "R03"),
            SourceSpace::AdobeRgb
        );
        assert_eq!(SourceSpace::from_tags("", 1, ""), SourceSpace::Srgb);
        // A profile the file carries outranks the EXIF guess, so such a file is left alone above.
        assert_eq!(
            SourceSpace::from_tags("sRGB IEC61966-2.1", 2, ""),
            SourceSpace::Srgb
        );
    }
}
