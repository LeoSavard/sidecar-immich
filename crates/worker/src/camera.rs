// SPDX-License-Identifier: Apache-2.0
//! The camera's own rendering of a RAW, for the editor to show beside the edit: the largest JPEG the camera
//! embedded in the file (`JpgFromRaw`, `PreviewImage` or `OtherImage`, as ExifTool names them), with the RAW's
//! orientation tag copied onto it so a browser shows it upright. Cached under `camera/`. A JPEG, HEIC or TIFF
//! has none: it is its own rendering.
//!
//! `GET /v1/assets/{key}/camera`  the JPEG, or 404

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use crate::{Worker, unique_tmp};

/// The embedded JPEGs ExifTool can extract, any of which may be the largest.
const TAGS: [&str; 3] = ["JpgFromRaw", "PreviewImage", "OtherImage"];

impl Worker {
    pub async fn camera_jpeg(self: &Arc<Self>, key: &str) -> Result<Option<Vec<u8>>> {
        let target = self.cache.join("camera").join(format!("{key}.jpg"));
        if let Ok(bytes) = tokio::fs::read(&target).await {
            return Ok(Some(bytes));
        }
        let path = self.paths.lock().unwrap().get(key).cloned().context("unknown asset; ingest it first")?;
        if sidecar_raw::decoded::is_supported(&path) {
            return Ok(None);
        }
        tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>> {
            let Some(jpeg) = largest_embedded(&path)? else { return Ok(None) };
            std::fs::create_dir_all(target.parent().expect("in the cache"))?;
            let tmp = unique_tmp(&target);
            std::fs::write(&tmp, &jpeg)?;
            // The RAW's orientation, so the browser turns it as the editor turns the RAW. A file without one
            // leaves the JPEG as it is.
            let _ = Command::new("exiftool").args(["-q", "-q", "-overwrite_original", "-TagsFromFile"]).arg(&path).args(["-Orientation"]).arg(&tmp).status();
            std::fs::rename(&tmp, &target)?;
            Ok(Some(std::fs::read(&target)?))
        })
        .await?
    }
}

/// The largest embedded JPEG, or none.
fn largest_embedded(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut best: Option<Vec<u8>> = None;
    for tag in TAGS {
        let out = Command::new("exiftool").args(["-b", &format!("-{tag}")]).arg(path).output().context("running exiftool — is it installed?")?;
        let bytes = out.stdout;
        if bytes.starts_with(&[0xFF, 0xD8]) && best.as_ref().is_none_or(|b| bytes.len() > b.len()) {
            best = Some(bytes);
        }
    }
    Ok(best)
}
