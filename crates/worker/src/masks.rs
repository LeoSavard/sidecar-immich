// SPDX-License-Identifier: Apache-2.0
//! Masks from the models (`sidecar-ml`): computed once per photo and cached beside its proxies, never
//! written next to the original (DESIGN.md §15). They are made from the proxy developed at default settings
//! — a normal-looking photo, which is what the models were trained on — so they don't depend on the edit and
//! never need recomputing when a slider moves.
//!
//! A mask is in the proxy's frame: oriented, before lens correction, the same frame as the focus mask, so the
//! editor and the export align it the way they already align that.
//!
//! `GET /v1/assets/{key}/masks`         `{ available, names, faces }`, computing them on first use
//! `GET /v1/assets/{key}/masks/{name}`  one mask as an 8-bit greyscale PNG

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

use crate::ml::{Face, PARTS, Picture};
use crate::{DEFAULT_EDGE, Worker, cache_generation, export, unique_tmp};

/// Bump when the models, or how masks are made from them, change: every cached mask is then made again.
pub const MASK_REVISION: u32 = 2;

/// The masks an edit can name, in the order the editor lists them.
pub const NAMES: [&str; 7] = ["subject", "skin", "face-skin", "body-skin", "hair", "clothes", "depth"];

/// The size the models look at: past BiRefNet's 1024² input nothing more is seen.
const INPUT_EDGE: u32 = 1024;
/// The subject mask keeps BiRefNet's resolution; the parts come from a 256² model, so 512 already holds all
/// they know.
const SUBJECT_EDGE: u32 = 1024;
const PARTS_EDGE: u32 = 512;
/// Depth comes from a 518² model, and a layer on it is a smooth band of distance.
const DEPTH_EDGE: u32 = 518;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MaskSet {
    pub available: bool,
    pub names: Vec<String>,
    pub faces: Vec<Face>,
}

fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let tmp = unique_tmp(path);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn grey_png(width: u32, height: u32, data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut info = png::Info::with_size(width, height);
    info.color_type = png::ColorType::Grayscale;
    info.bit_depth = png::BitDepth::Eight;
    png::Encoder::with_info(&mut out, info)?.write_header()?.write_image_data(data)?;
    Ok(out)
}

impl Worker {
    /// `file` is a mask name with `.png`, or `faces.json`.
    fn mask_path(&self, key: &str, file: &str) -> PathBuf {
        self.cache.join("masks").join(format!("{key}-{}-m{MASK_REVISION}-{file}", cache_generation()))
    }

    fn masks_done(&self, key: &str) -> bool {
        self.mask_path(key, "faces.json").is_file()
    }

    /// Every mask of this photo and its faces, computed on first use. Unavailable (not an error) without the
    /// models or a GPU.
    pub async fn masks(self: &Arc<Self>, key: &str) -> Result<MaskSet> {
        let (Some(ml), Some(kit)) = (self.ml.clone(), self.gpu.clone()) else {
            return Ok(MaskSet::default());
        };
        if !self.masks_done(key) {
            // The proxy first: it takes this photo's decode turn itself, so it can't be held around it.
            let (w, h, bytes) = self.proxy(key, DEFAULT_EDGE).await?;
            // One computation per photo at a time, under a lock of its own: under the decode turn, every proxy
            // the editor asks for while the models run (seconds; minutes on a new install while they tune)
            // waited for them.
            let _turn = self.decode_guard(&format!("masks:{key}")).await;
            if !self.masks_done(key) {
                let info = self.cached_meta(key).await?.info;
                let mut paths: Vec<(&'static str, PathBuf)> = NAMES.iter().map(|n| (*n, self.mask_path(key, &format!("{n}.png")))).collect();
                paths.push(("faces.json", self.mask_path(key, "faces.json")));
                let path = self.paths.lock().unwrap().get(key).cloned().unwrap_or_default();
                tokio::task::spawn_blocking(move || -> Result<()> {
                    let started = std::time::Instant::now();
                    let picture = default_rendering(&kit, &info, w, h, &bytes)?;
                    let subject = ml.subject(&picture, SUBJECT_EDGE)?;
                    let parts = ml.parts(&picture, PARTS_EDGE)?;
                    let depth = ml.depth(&picture, DEPTH_EDGE)?;
                    let faces = ml.faces(&picture)?;
                    let channel = |name: &str| {
                        let c = PARTS.iter().position(|p| *p == name).expect("a part");
                        parts.data.iter().skip(c).step_by(parts.channels as usize).copied().collect::<Vec<u8>>()
                    };
                    let (face, body) = (channel("face-skin"), channel("body-skin"));
                    let skin: Vec<u8> = face.iter().zip(&body).map(|(a, b)| a.saturating_add(*b)).collect();
                    for (name, target) in &paths {
                        let bytes = match *name {
                            "subject" => grey_png(subject.width, subject.height, &subject.data)?,
                            "skin" => grey_png(parts.width, parts.height, &skin)?,
                            "depth" => grey_png(depth.width, depth.height, &depth.data)?,
                            "faces.json" => serde_json::to_vec(&faces)?,
                            part => grey_png(parts.width, parts.height, &channel(part))?,
                        };
                        write_atomic(target, &bytes)?;
                    }
                    tracing::info!("masks for {} in {:.0} ms", path.display(), started.elapsed().as_secs_f64() * 1e3);
                    Ok(())
                })
                .await??;
            }
        }
        let faces = serde_json::from_slice(&tokio::fs::read(self.mask_path(key, "faces.json")).await?)?;
        Ok(MaskSet { available: true, names: NAMES.iter().map(|n| n.to_string()).collect(), faces })
    }

    /// One mask's PNG, computing the photo's masks first if needed.
    pub async fn mask(self: &Arc<Self>, key: &str, name: &str) -> Result<Option<Vec<u8>>> {
        if !NAMES.contains(&name) || !self.masks(key).await?.available {
            return Ok(None);
        }
        Ok(Some(tokio::fs::read(self.mask_path(key, &format!("{name}.png"))).await.context("mask was not written")?))
    }
}

impl Worker {
    /// The masks `edit` names, from the cache (computing them first if needed), in its order: `None` for one
    /// this worker can't provide — no models, or a mask this version doesn't render — which then weighs
    /// nothing. Each is the mask's value in every channel, ready for `Rendering::mask_textures`.
    pub async fn mask_images(self: &Arc<Self>, key: &str, edit: &sidecar_color::EditParams) -> Vec<Option<sidecar_engine::RgbaF32>> {
        let mut out = Vec::new();
        for m in edit.masks.iter().take(sidecar_color::params::MAX_MASKS) {
            let png = if m.renders() { self.mask(key, &m.mask).await.unwrap_or_else(|e| {
                tracing::warn!("mask {} for {key}: {e:#}", m.mask);
                None
            }) } else { None };
            out.push(png.and_then(|p| decode_grey(&p).ok()));
        }
        out
    }
}

fn decode_grey(png_bytes: &[u8]) -> Result<sidecar_engine::RgbaF32> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes)).read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().context("mask too large")?];
    let info = reader.next_frame(&mut buf)?;
    let data = buf[..info.buffer_size()].iter().flat_map(|&v| [v as f32 / 255.0; 4]).collect();
    Ok(sidecar_engine::RgbaF32 { width: info.width, height: info.height, data })
}

/// The proxy developed at default settings, as the 8-bit sRGB picture the models read.
fn default_rendering(kit: &export::Kit, info: &sidecar_raw::RawInfo, w: u32, h: u32, proxy: &[u8]) -> Result<Picture> {
    let img = sidecar_engine::RgbaF32::from_f16_bytes(w, h, proxy).downsampled(INPUT_EDGE);
    let source = sidecar_engine::upload_rgba16(&kit.gpu, &img);
    let packed = export::Rendering::of(info).pack(&Default::default(), sidecar_color::DisplaySpace::Srgb);
    let developed = kit.develop.render(&kit.gpu, &source, &packed.uniform, &packed.lut, &sidecar_engine::Masks::none())?;
    let out = sidecar_engine::read_rgba16(&kit.gpu, &developed)?;
    let rgb = out.data.as_chunks::<4>().0.iter().flat_map(|p| [0, 1, 2].map(|c| (export::srgb_encode(p[c]) * 255.0 + 0.5) as u8)).collect();
    Ok(Picture { width: out.width, height: out.height, rgb })
}
