// SPDX-License-Identifier: Apache-2.0
//! Masks from the models (`sidecar-ml`): computed once per photo and cached beside its proxies, never
//! written next to the original (DESIGN.md §15). They are made from the proxy developed at default settings
//! — a normal-looking photo, which is what the models were trained on — so they don't depend on the edit and
//! never need recomputing when a slider moves.
//!
//! A mask is in the proxy's frame: oriented, before lens correction, the same frame as the focus mask, so the
//! editor and the export align it the way they already align that.
//!
//! `GET /v1/assets/{key}/masks`         `{ available, names, faces, airlight }`, computing them on first use
//! `GET /v1/assets/{key}/masks/{name}`  one mask as an 8-bit greyscale PNG

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

use crate::ml::{Face, PARTS, Picture};
use crate::{DEFAULT_EDGE, Worker, cache_generation, export, unique_tmp};

/// Bump when the models, or how masks are made from them, change: every cached mask is then made again.
pub const MASK_REVISION: u32 = 6;

/// The masks an edit can name, in the order the editor lists them.
pub const NAMES: [&str; 8] = [
    "subject",
    "skin",
    "face-skin",
    "body-skin",
    "hair",
    "clothes",
    "depth",
    "sky",
];

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
    /// The light haze tends to: the farthest part of the photo's colour, camera RGB as the proxy holds it.
    /// What haze by distance adds toward or takes away (`sidecar_color::depth`).
    #[serde(default)]
    pub airlight: Option<[f32; 3]>,
}

/// The depth mask and the airlight, for the depth passes.
pub struct DepthMap {
    pub image: sidecar_engine::RgbaF32,
    pub airlight: [f32; 3],
}

/// The sky mask's long edge: its edges come from the photo (`sidecar_engine::sky`), so it is kept as fine as
/// the subject's.
const SKY_EDGE: u32 = 1024;

/// The sky (`sidecar_engine::sky`): found in the depth map — the far plane, from the top edge, not the subject,
/// light — and drawn by the proxy, box-averaged to `SKY_EDGE`. Empty when there is none.
fn sky(
    proxy: &sidecar_engine::RgbaF32,
    depth: &[u8],
    dw: u32,
    dh: u32,
    subject: (&[u8], u32, u32),
    wb: [f32; 3],
) -> (u32, u32, Vec<u8>) {
    let k = proxy.width.max(proxy.height).div_ceil(SKY_EDGE).max(1);
    let (w, h) = (proxy.width / k, proxy.height / k);
    let photo = sidecar_engine::RgbaF32 {
        width: w,
        height: h,
        data: (0..w * h)
            .flat_map(|i| {
                let (bx, by) = (i % w * k, i / w * k);
                let mut s = [0f32; 3];
                for y in by..by + k {
                    for x in bx..bx + k {
                        let p = ((y * proxy.width + x) * 4) as usize;
                        for (c, v) in s.iter_mut().enumerate() {
                            *v += proxy.data[p + c].max(0.0);
                        }
                    }
                }
                let n = (k * k) as f32;
                [s[0] / n, s[1] / n, s[2] / n, 1.0]
            })
            .collect(),
    };
    // At the depth map's size: the photo's log luminance and the subject.
    let at = |x: u32, y: u32, sw: u32, sh: u32| {
        (
            ((x as f64 + 0.5) / dw as f64 * sw as f64) as u32,
            ((y as f64 + 0.5) / dh as f64 * sh as f64) as u32,
        )
    };
    // White-balanced, so a grey sky is grey and foliage green.
    let rgb: Vec<[f32; 3]> = (0..dw * dh)
        .map(|i| {
            let (x, y) = at(i % dw, i / dw, w, h);
            let p = ((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize;
            [0, 1, 2].map(|c| photo.data[p + c] * wb[c])
        })
        .collect();
    let (sd, sw, sh) = subject;
    let subject: Vec<u8> = (0..dw * dh)
        .map(|i| {
            let (x, y) = at(i % dw, i / dw, sw, sh);
            sd[(y.min(sh - 1) * sw + x.min(sw - 1)) as usize]
        })
        .collect();
    let mask =
        match sidecar_engine::sky::coarse(depth, dw as usize, dh as usize, &rgb, Some(&subject)) {
            Some(c) => sidecar_engine::sky::refine(&photo, &c, dw as usize, dh as usize),
            None => vec![0; (w * h) as usize],
        };
    (w, h, mask)
}

/// The share of the photo, farthest first, whose colour is the airlight.
const AIRLIGHT_SHARE: f64 = 0.03;

/// The mean camera RGB of the proxy over the farthest `AIRLIGHT_SHARE` of the depth map (both in the same
/// frame, at their own sizes).
fn airlight(proxy: &sidecar_engine::RgbaF32, depth: &[u8], dw: u32, dh: u32) -> [f32; 3] {
    let mut sorted = depth.to_vec();
    sorted.sort_unstable();
    let cut = sorted[((sorted.len() - 1) as f64 * AIRLIGHT_SHARE) as usize];
    let (mut sum, mut n) = ([0f64; 3], 0u64);
    for y in 0..dh {
        for x in 0..dw {
            if depth[(y * dw + x) as usize] > cut {
                continue;
            }
            let px = ((x as f64 + 0.5) / dw as f64 * proxy.width as f64) as u32;
            let py = ((y as f64 + 0.5) / dh as f64 * proxy.height as f64) as u32;
            let i =
                ((py.min(proxy.height - 1) * proxy.width + px.min(proxy.width - 1)) * 4) as usize;
            for (c, s) in sum.iter_mut().enumerate() {
                *s += proxy.data[i + c] as f64;
            }
            n += 1;
        }
    }
    sum.map(|v| (v / n.max(1) as f64) as f32)
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
    png::Encoder::with_info(&mut out, info)?
        .write_header()?
        .write_image_data(data)?;
    Ok(out)
}

impl Worker {
    /// `file` is a mask name with `.png`, `depth.json` or `faces.json` (written last: the set is done).
    fn mask_path(&self, key: &str, file: &str) -> PathBuf {
        self.cache.join("masks").join(format!(
            "{key}-{}-m{MASK_REVISION}-{file}",
            cache_generation()
        ))
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
                let mut paths: Vec<(&'static str, PathBuf)> = NAMES
                    .iter()
                    .map(|n| (*n, self.mask_path(key, &format!("{n}.png"))))
                    .collect();
                paths.push(("depth.json", self.mask_path(key, "depth.json")));
                paths.push(("faces.json", self.mask_path(key, "faces.json")));
                let path = self
                    .paths
                    .lock()
                    .unwrap()
                    .get(key)
                    .cloned()
                    .unwrap_or_default();
                tokio::task::spawn_blocking(move || -> Result<()> {
                    let started = std::time::Instant::now();
                    let picture = default_rendering(&kit, &info, w, h, &bytes)?;
                    let subject = ml.subject(&picture, SUBJECT_EDGE)?;
                    let parts = ml.parts(&picture, PARTS_EDGE)?;
                    let depth = ml.depth(&picture, DEPTH_EDGE)?;
                    let air = airlight(
                        &sidecar_engine::RgbaF32::from_f16_bytes(w, h, &bytes),
                        &depth.data,
                        depth.width,
                        depth.height,
                    );
                    let sky = sky(
                        &sidecar_engine::RgbaF32::from_f16_bytes(w, h, &bytes),
                        &depth.data,
                        depth.width,
                        depth.height,
                        (&subject.data, subject.width, subject.height),
                        {
                            let m = info.color.as_shot_mul;
                            [m[0] as f32, 1.0, m[2] as f32]
                        },
                    );
                    let faces = ml.faces(&picture)?;
                    let channel = |name: &str| {
                        let c = PARTS.iter().position(|p| *p == name).expect("a part");
                        parts
                            .data
                            .iter()
                            .skip(c)
                            .step_by(parts.channels as usize)
                            .copied()
                            .collect::<Vec<u8>>()
                    };
                    let (face, body) = (channel("face-skin"), channel("body-skin"));
                    let skin: Vec<u8> = face
                        .iter()
                        .zip(&body)
                        .map(|(a, b)| a.saturating_add(*b))
                        .collect();
                    for (name, target) in &paths {
                        let bytes = match *name {
                            "subject" => grey_png(subject.width, subject.height, &subject.data)?,
                            "skin" => grey_png(parts.width, parts.height, &skin)?,
                            "depth" => grey_png(depth.width, depth.height, &depth.data)?,
                            "sky" => grey_png(sky.0, sky.1, &sky.2)?,
                            "depth.json" => {
                                serde_json::to_vec(&serde_json::json!({ "airlight": air }))?
                            }
                            "faces.json" => serde_json::to_vec(&faces)?,
                            part => grey_png(parts.width, parts.height, &channel(part))?,
                        };
                        write_atomic(target, &bytes)?;
                    }
                    tracing::info!(
                        "masks for {} in {:.0} ms",
                        path.display(),
                        started.elapsed().as_secs_f64() * 1e3
                    );
                    Ok(())
                })
                .await??;
            }
        }
        let faces =
            serde_json::from_slice(&tokio::fs::read(self.mask_path(key, "faces.json")).await?)?;
        let airlight = self.airlight(key).await;
        Ok(MaskSet {
            available: true,
            names: NAMES.iter().map(|n| n.to_string()).collect(),
            faces,
            airlight,
        })
    }

    async fn airlight(&self, key: &str) -> Option<[f32; 3]> {
        let json: serde_json::Value = serde_json::from_slice(
            &tokio::fs::read(self.mask_path(key, "depth.json"))
                .await
                .ok()?,
        )
        .ok()?;
        serde_json::from_value(json["airlight"].clone()).ok()
    }

    /// The depth mask and airlight when `edit` blurs or hazes by distance and this worker has them; `None`
    /// otherwise, and the depth passes don't run.
    pub async fn depth_map(
        self: &Arc<Self>,
        key: &str,
        edit: &sidecar_color::EditParams,
    ) -> Option<DepthMap> {
        sidecar_color::depth::pack(edit, [0.0; 3], [1, 1], [0.0; 2])?;
        let png = self.mask(key, "depth").await.unwrap_or_else(|e| {
            tracing::warn!("depth for {key}: {e:#}");
            None
        })?;
        Some(DepthMap {
            image: decode_grey(&png).ok()?,
            airlight: self.airlight(key).await?,
        })
    }

    /// One mask's PNG, computing the photo's masks first if needed.
    pub async fn mask(self: &Arc<Self>, key: &str, name: &str) -> Result<Option<Vec<u8>>> {
        if !NAMES.contains(&name) || !self.masks(key).await?.available {
            return Ok(None);
        }
        let coarse = tokio::fs::read(self.mask_path(key, &format!("{name}.png")))
            .await
            .context("mask was not written")?;
        if !REFINED.contains(&name) {
            return Ok(Some(coarse));
        }
        // A part mask is served with its edges put where the photo's are, made once and kept beside it.
        let path = self.mask_path(key, &format!("{name}-refined.png"));
        if let Ok(bytes) = tokio::fs::read(&path).await {
            return Ok(Some(bytes));
        }
        let refined = self.refined(key, decode_grey(&coarse)?).await;
        let bytes: Vec<u8> = refined
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| (p[0] * 255.0).round() as u8)
            .collect();
        let png = grey_png(refined.width, refined.height, &bytes)?;
        write_atomic(&path, &png)?;
        Ok(Some(png))
    }
}

impl Worker {
    /// The masks `edit` names, from the cache (computing them first if needed), in its order: `None` for one
    /// this worker can't provide — no models, or a mask this version doesn't render — which then weighs
    /// nothing. Each is the mask's value in every channel, ready for `Rendering::mask_textures`.
    pub async fn mask_images(
        self: &Arc<Self>,
        key: &str,
        edit: &sidecar_color::EditParams,
    ) -> Vec<Option<sidecar_engine::RgbaF32>> {
        let mut out = Vec::new();
        for m in edit.masks.iter().take(sidecar_color::params::MAX_MASKS) {
            if m.mask == sidecar_color::color_range::COLOR && m.renders() {
                out.push(
                    self.color_selection(key, edit, m)
                        .await
                        .unwrap_or_else(|e| {
                            tracing::warn!("colour range for {key}: {e:#}");
                            None
                        }),
                );
                continue;
            }
            let png = if m.renders() {
                self.mask(key, &m.mask).await.unwrap_or_else(|e| {
                    tracing::warn!("mask {} for {key}: {e:#}", m.mask);
                    None
                })
            } else {
                None
            };
            out.push(png.and_then(|p| decode_grey(&p).ok()));
        }
        out
    }
}

/// The masks from the 256² multi-class model, whose edges are refined against the photo: what `mask` serves, to
/// the browser and to every render, for these names.
const REFINED: [&str; 5] = ["hair", "skin", "face-skin", "body-skin", "clothes"];

impl Worker {
    /// `coarse` with its edges put where the photo's are (`sidecar_color::mask_refine`), at the proxy's
    /// `color_range::SIZE`, the colour guide judged as shot — so a refined mask depends on the photo alone. Kept
    /// as it came when the proxy can't be had.
    async fn refined(
        self: &Arc<Self>,
        key: &str,
        coarse: sidecar_engine::RgbaF32,
    ) -> sidecar_engine::RgbaF32 {
        let made = async {
            let (width, height, bytes) = self.proxy(key, sidecar_color::color_range::SIZE).await?;
            let proxy = sidecar_engine::RgbaF32::from_f16_bytes(width, height, &bytes);
            let meta = self.cached_meta(key).await?;
            let packed = crate::export::Rendering::of(&meta.info)
                .pack(&Default::default(), sidecar_color::DisplaySpace::Srgb);
            let develop: sidecar_color::DevelopParams =
                bytemuck::pod_read_unaligned(&packed.uniform);
            let mask: Vec<u8> = coarse
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| (p[0] * 255.0).round() as u8)
                .collect();
            let (mw, mh) = (coarse.width, coarse.height);
            let out = tokio::task::spawn_blocking(move || {
                sidecar_color::mask_refine::refine(
                    &mask,
                    mw,
                    mh,
                    &proxy.data,
                    width,
                    height,
                    &develop,
                )
            })
            .await?;
            anyhow::Ok(sidecar_engine::RgbaF32 {
                width,
                height,
                data: out.iter().flat_map(|&v| [v as f32 / 255.0; 4]).collect(),
            })
        };
        match made.await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("refining a mask for {key}: {e:#}");
                coarse
            }
        }
    }

    /// A colour-range layer's mask (`sidecar_color::color_range::select`): the proxy at the selection's size,
    /// judged through the edit's own white balance and camera matrix, within its model mask when it names one.
    async fn color_selection(
        self: &Arc<Self>,
        key: &str,
        edit: &sidecar_color::EditParams,
        layer: &sidecar_color::params::MaskAdjustment,
    ) -> Result<Option<sidecar_engine::RgbaF32>> {
        use sidecar_color::color_range::{SIZE, select};
        let range = layer.color.clone().unwrap_or_default();
        let (width, height, bytes) = self.proxy(key, SIZE).await?;
        let proxy = sidecar_engine::RgbaF32::from_f16_bytes(width, height, &bytes);
        let meta = self.cached_meta(key).await?;
        let packed =
            crate::export::Rendering::of(&meta.info).pack(edit, sidecar_color::DisplaySpace::Srgb);
        let develop: sidecar_color::DevelopParams = bytemuck::pod_read_unaligned(&packed.uniform);
        let within = match range.within.as_deref() {
            Some(name) => match self.mask(key, name).await? {
                Some(png) => Some(decode_grey(&png)?),
                // Its model mask isn't there (the plain image): nothing is selected, rather than everywhere.
                None => return Ok(None),
            },
            None => None,
        };
        let within_bytes = within.as_ref().map(|m| {
            m.data
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| (p[0] * 255.0).round() as u8)
                .collect::<Vec<u8>>()
        });
        let sel = select(
            &range,
            &develop,
            &proxy.data,
            width,
            height,
            within
                .as_ref()
                .zip(within_bytes.as_deref())
                .map(|(m, b)| (b, m.width, m.height)),
        );
        Ok(Some(sidecar_engine::RgbaF32 {
            width,
            height,
            data: sel.iter().flat_map(|&v| [v as f32 / 255.0; 4]).collect(),
        }))
    }
}

pub(crate) fn decode_grey(png_bytes: &[u8]) -> Result<sidecar_engine::RgbaF32> {
    let mut reader = png::Decoder::new(std::io::Cursor::new(png_bytes)).read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().context("mask too large")?];
    let info = reader.next_frame(&mut buf)?;
    let data = buf[..info.buffer_size()]
        .iter()
        .flat_map(|&v| [v as f32 / 255.0; 4])
        .collect();
    Ok(sidecar_engine::RgbaF32 {
        width: info.width,
        height: info.height,
        data,
    })
}

/// The proxy developed at default settings, as the 8-bit sRGB picture the models read.
fn default_rendering(
    kit: &export::Kit,
    info: &sidecar_raw::RawInfo,
    w: u32,
    h: u32,
    proxy: &[u8],
) -> Result<Picture> {
    let img = sidecar_engine::RgbaF32::from_f16_bytes(w, h, proxy).downsampled(INPUT_EDGE);
    let source = sidecar_engine::upload_rgba16(&kit.gpu, &img);
    let packed =
        export::Rendering::of(info).pack(&Default::default(), sidecar_color::DisplaySpace::Srgb);
    let developed = kit.develop.render(
        &kit.gpu,
        &source,
        &packed.uniform,
        &packed.lut,
        &sidecar_engine::Masks::none(),
    )?;
    let out = sidecar_engine::read_rgba16(&kit.gpu, &developed)?;
    let rgb = out
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [0, 1, 2].map(|c| (export::srgb_encode(p[c]) * 255.0 + 0.5) as u8))
        .collect();
    Ok(Picture {
        width: out.width,
        height: out.height,
        rgb,
    })
}
