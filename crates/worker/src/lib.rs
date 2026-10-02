// SPDX-License-Identifier: Apache-2.0
//! Worker HTTP interface (internal network only; the api is its only client).
//!
//! `POST /v1/ingest { path }`                 hash, read metadata, cache it, pre-build proxies
//! `GET  /v1/assets/{key}/meta`               cached metadata
//! `GET  /v1/assets/{key}/proxy?long_edge=N`  RGBA16F proxy bytes; `X-Width`, `X-Height` headers
//! `GET  /v1/assets/{key}/masks[/{name}]`     masks from the models (`masks.rs`)

pub mod export;
pub mod lensfun;
pub mod masks;
pub mod ml;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sidecar_color::params::PROCESS_VERSION;
use sidecar_raw::{RawFile, RawInfo};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

/// Proxy sizes built together from one decode: a quick first paint, and the phone-sized proxy.
pub const PREVIEW_EDGE: u32 = 768;
pub const DEFAULT_EDGE: u32 = 2560;

/// What the cached proxy and focus mask were *decoded* by. Bump this whenever a change alters the pixels
/// ingest produces from the same original — demosaic, hot pixels, highlight reconstruction, orientation,
/// the downsample — so the cache stops answering with output the current code would not have made.
///
/// `ProcessVersion` alone used to key these files, and it can't do this job: it versions the develop
/// parameters written into a sidecar, and raising it marks every sidecar as read-only to an older Sidecar.
/// The two move for unrelated reasons. 0.2.1 fixed a HEIC at EXIF orientation 3 being rotated 180° twice —
/// libheif had already applied the file's own `irot` — and left every proxy decoded before it in place and
/// upside down, because nothing in the file name had changed. Generation 2 is that fix; 3 orients the
/// mirrored EXIF orientations (2, 4, 5, 7), which every earlier generation left upright.
pub const INGEST_GENERATION: u32 = 3;

/// What cached metadata was *read* by, apart from the pixels: bump when `Meta` gains something a re-read of
/// the same file would now fill in, so it is read again without rebuilding a single proxy. 1 adds the lens
/// correction the file carries and the lens's identity.
pub const META_REVISION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub asset_key: String,
    pub file_name: String,
    pub process_version: u32,
    /// `INGEST_GENERATION` when this was read. Metadata carries orientation-derived fields — the flip, the
    /// oriented size, the AF point — so a generation that changes orientation has to re-read it too; files
    /// cached before this field existed read as 0 and are re-read once.
    #[serde(default)]
    pub generation: u32,
    /// `META_REVISION` when this was read; 0 in metadata cached before it existed.
    #[serde(default)]
    pub revision: u32,
    /// When the lensfun database this was matched against arrived (`Lensfun::updated`); `None` if there was
    /// none. A newer database re-matches a photo whose file carries no correction.
    #[serde(default)]
    pub lensfun: Option<u64>,
    pub info: RawInfo,
}

#[derive(Deserialize)]
pub struct IngestRequest {
    pub path: PathBuf,
}

pub struct Worker {
    cache: PathBuf,
    paths: Mutex<HashMap<String, PathBuf>>,
    /// Asset key of each path already hashed, with the size and mtime it had then. The api re-ingests on every
    /// `meta` request and after a worker restart; hashing the whole original each time was most of what an
    /// open cost after the first.
    hashed: Mutex<HashMap<PathBuf, (u64, std::time::SystemTime, String)>>,
    decode_slots: Semaphore,
    /// One decode at a time per asset — see `decode_guard`.
    decoding: Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    /// GPU ingest and develop (Vulkan). `None` when no usable GPU: proxies then come from LibRaw on the CPU,
    /// and exports are refused.
    gpu: Option<Arc<export::Kit>>,
    /// One full-resolution render at a time.
    export_slot: Semaphore,
    /// Full-resolution linear buffers of recently opened photos, for loupe tiles (most recent first).
    sessions: Mutex<Vec<Session>>,
    lensfun: Arc<lensfun::Lensfun>,
    /// `sidecar-ml`, in the `-ml` image only; masks are unavailable without it.
    ml: Option<Arc<ml::Ml>>,
}

/// The marker every cached proxy and focus mask carries in its name. Both numbers are in it because they
/// invalidate for different reasons: `ProcessVersion` decides whether a cached file still means what the
/// client will ask for, and `INGEST_GENERATION` whether this code would have decoded it that way.
fn cache_generation() -> String {
    format!("pv{PROCESS_VERSION}-g{INGEST_GENERATION}")
}

/// Drop proxies and focus masks from an earlier generation. Nothing will ever ask for those file names
/// again, so they are unreadable rather than stale — without this, a bump would leave them on disk for good.
/// Cached metadata is left alone: it comes from EXIF, which no ingest change rewrites.
fn sweep_stale_cache(cache: &std::path::Path) {
    let marker = format!("-{}-", cache_generation());
    let (mut removed, mut bytes) = (0u64, 0u64);
    for dir in ["proxies", "focus", "masks"] {
        let Ok(entries) = std::fs::read_dir(cache.join(dir)) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_str().is_some_and(|n| n.contains(&marker)) {
                continue;
            }
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            if std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
                bytes += size;
            }
        }
    }
    if removed > 0 {
        tracing::info!("dropped {removed} cached files from an earlier ingest generation ({} MB); they rebuild on demand", bytes >> 20);
    }
}

fn internal(e: anyhow::Error) -> Response {
    tracing::error!("{e:#}");
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response()
}

impl Worker {
    pub fn new(cache: PathBuf) -> Result<Arc<Self>> {
        std::fs::create_dir_all(cache.join("meta"))?;
        std::fs::create_dir_all(cache.join("proxies"))?;
        std::fs::create_dir_all(cache.join("focus"))?;
        std::fs::create_dir_all(cache.join("outbox"))?;
        std::fs::create_dir_all(cache.join("masks"))?;
        sweep_stale_cache(&cache);
        let lensfun = Arc::new(lensfun::Lensfun::open(cache.join("lensfun")));
        let kit = sidecar_engine::Gpu::new_headless().and_then(|gpu| {
            let ingest = sidecar_engine::Ingest::new(&gpu)?;
            let develop = sidecar_engine::DevelopPass::new(&gpu)?;
            let detail = sidecar_engine::detail::Detail::new(&gpu)?;
            let lens = sidecar_engine::lens::LensPass::new(&gpu)?;
            let warp = sidecar_engine::warp::WarpPass::new(&gpu)?;
            let brush = sidecar_engine::brush::Brush::new(&gpu)?;
            Ok(export::Kit { gpu, ingest, develop, detail, lens, warp, brush, lensfun: lensfun.clone() })
        });
        let gpu = match kit {
            Ok(k) => {
                tracing::info!("GPU ingest on {} ({:?})", k.gpu.adapter_info.name, k.gpu.adapter_info.backend);
                Some(Arc::new(k))
            }
            Err(e) => {
                tracing::warn!("no GPU ingest, using LibRaw on the CPU: {e:#}");
                None
            }
        };
        let ml = ml::Ml::discover(&cache.join("ml")).map(Arc::new);
        Ok(Arc::new(Self {
            cache,
            paths: Mutex::new(HashMap::new()),
            hashed: Mutex::new(HashMap::new()),
            decode_slots: Semaphore::new(2),
            decoding: Mutex::new(HashMap::new()),
            gpu,
            export_slot: Semaphore::new(1),
            sessions: Mutex::new(Vec::new()),
            lensfun,
            ml,
        }))
    }

    fn meta_path(&self, key: &str) -> PathBuf {
        self.cache.join("meta").join(format!("{key}.json"))
    }

    fn proxy_path(&self, key: &str, edge: u32) -> PathBuf {
        self.cache.join("proxies").join(format!("{key}-{}-{edge}.f16", cache_generation()))
    }

    /// Focus-peaking mask at `edge`: max-pooled from a full-resolution measure, so it caches (and expires)
    /// alongside the proxy it overlays, keyed the same way. See docs/ROADMAP.md, "Focus peaking...".
    fn focus_path(&self, key: &str, edge: u32) -> PathBuf {
        self.cache.join("focus").join(format!("{key}-{}-{edge}.png", cache_generation()))
    }

    pub async fn ingest(self: &Arc<Self>, path: PathBuf) -> Result<Meta> {
        let stamp = tokio::fs::metadata(&path).await.ok().and_then(|m| Some((m.len(), m.modified().ok()?)));
        let known = stamp.and_then(|s| self.hashed.lock().unwrap().get(&path).filter(|(len, at, _)| (*len, *at) == s).map(|(_, _, key)| key.clone()));
        let key = match known {
            Some(key) => key,
            None => {
                let p = path.clone();
                let key = tokio::task::spawn_blocking(move || sidecar_xmp::sha1_file(&p)).await??;
                if let Some((len, at)) = stamp {
                    self.hashed.lock().unwrap().insert(path.clone(), (len, at, key.clone()));
                }
                key
            }
        };
        self.paths.lock().unwrap().insert(key.clone(), path.clone());
        let meta_path = self.meta_path(&key);
        if let Ok(bytes) = tokio::fs::read(&meta_path).await
            && let Ok(mut meta) = serde_json::from_slice::<Meta>(&bytes)
            && meta.generation == INGEST_GENERATION
            && meta.revision == META_REVISION
        {
            // A lensfun database that arrived since, for a photo whose file carries no correction: match again.
            // No ExifTool run needed — the lens's identity is already here.
            let from_lensfun = meta.info.lens_profile.as_ref().is_none_or(|p| p.id.starts_with("lensfun:"));
            let raw = meta.info.source_kind == sidecar_raw::SourceKind::Raw;
            if raw && from_lensfun && meta.lensfun != self.lensfun.updated() {
                meta.info.lens_profile = None;
                meta.info.lens_profile = self.lensfun.profile_for(&meta.info);
                meta.lensfun = self.lensfun.updated();
                let tmp = unique_tmp(&meta_path);
                tokio::fs::write(&tmp, serde_json::to_vec_pretty(&meta)?).await?;
                tokio::fs::rename(&tmp, &meta_path).await?;
            }
            return Ok(meta);
        }
        let _slot = self.decode_slots.acquire().await?;
        let (k, p, lf) = (key.clone(), path.clone(), self.lensfun.clone());
        let meta = tokio::task::spawn_blocking(move || -> Result<Meta> {
            let info = if sidecar_raw::decoded::is_supported(&p) {
                // Non-RAW: EXIF alone, without decoding the pixels — see `decoded::info`.
                sidecar_raw::decoded::info(&sidecar_raw::decoded::exif(&p))
            } else if sidecar_raw::linear_dng::is_supported(&p) {
                // A JPEG XL linear DNG LibRaw can't open: colour from its own tags, the rest from ExifTool.
                lf.described(&p, sidecar_raw::linear_dng::info(&p, &sidecar_raw::decoded::exif(&p))?)
            } else {
                lf.described(&p, RawFile::metadata(&p)?)
            };
            Ok(Meta {
                asset_key: k,
                file_name: p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
                process_version: PROCESS_VERSION,
                generation: INGEST_GENERATION,
                revision: META_REVISION,
                lensfun: lf.updated(),
                info,
            })
        })
        .await??;
        // Temp-then-rename: `GET …/meta` reads this file directly, and a concurrent ingest of the same photo
        // must not let it see half a JSON document.
        let tmp = unique_tmp(&meta_path);
        tokio::fs::write(&tmp, serde_json::to_vec_pretty(&meta)?).await?;
        tokio::fs::rename(&tmp, &meta_path).await?;
        Ok(meta)
    }

    /// Serialize decodes of one asset. Opening a photo fans out into four requests that all want the same
    /// decode: `/open`'s pre-build, the `meta` route's re-ingest pre-build, and the client's own proxy and
    /// focus fetches. Each used to decode the file itself, and `decode_slots` let two of them run at once —
    /// every photo in this server's logs was decoded exactly twice, ~5 s apiece for a 12 Mpx HEIC. Holding
    /// this first means the later callers re-check the cache and find what the first one wrote.
    ///
    /// Keyed on the asset, not on `key + edge`, because one decode writes every edge it was asked for: a
    /// caller waiting on a different edge of the same photo still wants that decode to happen only once.
    async fn decode_guard(&self, key: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let mut map = self.decoding.lock().unwrap();
            match map.get(key).and_then(std::sync::Weak::upgrade) {
                Some(l) => l,
                None => {
                    // Nobody is decoding this asset, so every remaining weak entry with no owner is a
                    // finished decode: drop them rather than grow one entry per photo ever opened.
                    map.retain(|_, l| l.strong_count() > 0);
                    let l = Arc::new(tokio::sync::Mutex::new(()));
                    map.insert(key.to_string(), Arc::downgrade(&l));
                    l
                }
            }
        };
        lock.lock_owned().await
    }

    /// The proxy at `edge`, if it and its focus mask are both cached. Both must exist: a proxy built before
    /// focus masks existed (or raced ahead of a concurrent focus request for the same edge) must not
    /// short-circuit past building the mask.
    async fn cached_proxy(&self, key: &str, edge: u32) -> Option<(u32, u32, Bytes)> {
        let found = read_proxy(&self.proxy_path(key, edge)).await?;
        tokio::fs::try_exists(self.focus_path(key, edge)).await.unwrap_or(false).then_some(found)
    }

    /// Build (or reuse) the proxy at `edge`, building the preview size from the same decode.
    pub async fn proxy(self: &Arc<Self>, key: &str, edge: u32) -> Result<(u32, u32, Bytes)> {
        if let Some(found) = self.cached_proxy(key, edge).await {
            return Ok(found);
        }
        // Before the global slot, so waiting for this photo's turn never occupies one of the two decoders.
        let _decoding = self.decode_guard(key).await;
        if let Some(found) = self.cached_proxy(key, edge).await {
            return Ok(found);
        }
        let _slot = self.decode_slots.acquire().await?;
        let path = self.paths.lock().unwrap().get(key).cloned().context("unknown asset; ingest it first")?;
        // Orientation and colour space for a developed file, as ingest already read them — rather than
        // running ExifTool again for every proxy.
        let info = self.cached_meta(key).await.ok().map(|m| m.info);
        // `edge` is the preview size when the client asks for the first paint: build it once, not twice.
        let mut wanted = vec![edge];
        if edge != PREVIEW_EDGE {
            wanted.push(PREVIEW_EDGE);
        }
        let edges: Vec<(u32, PathBuf, PathBuf)> = wanted.into_iter().map(|e| (e, self.proxy_path(key, e), self.focus_path(key, e))).collect();
        let gpu = self.gpu.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let started = std::time::Instant::now();
            let largest = edges.iter().map(|(e, _, _)| *e).max().unwrap_or(edge);
            let full = decode_full(&path, info.as_ref(), gpu.as_deref(), largest)?;
            // Full-resolution, on the demosaiced image, before any develop setting applies (§ ingest, not §
            // develop) — so it caches like the proxy and never has to be recomputed when a slider moves.
            let focus = sidecar_engine::focus_energy(&full);
            for (e, out, focus_out) in edges {
                let p = full.downsampled(e);
                let mut bytes = Vec::with_capacity(8 + p.data.len() * 2);
                bytes.extend_from_slice(&p.width.to_le_bytes());
                bytes.extend_from_slice(&p.height.to_le_bytes());
                bytes.extend_from_slice(&p.to_f16_bytes());
                let tmp = unique_tmp(&out);
                std::fs::write(&tmp, &bytes)?;
                std::fs::rename(&tmp, &out)?;

                let pooled = focus.max_pooled(e);
                write_focus_png(&focus_out, &pooled)?;
            }
            tracing::info!("proxy for {} in {:.0} ms", path.display(), started.elapsed().as_secs_f64() * 1e3);
            Ok(())
        })
        .await??;
        read_proxy(&self.proxy_path(key, edge)).await.context("proxy was not written")
    }

    async fn cached_meta(&self, key: &str) -> Result<Meta> {
        let bytes = tokio::fs::read(self.meta_path(key)).await.context("no cached metadata; ingest it first")?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    /// The focus-peaking mask at `edge`: PNG bytes, built (and cached) alongside the proxy at the same edge.
    pub async fn focus(self: &Arc<Self>, key: &str, edge: u32) -> Result<Vec<u8>> {
        let target = self.focus_path(key, edge);
        if let Ok(bytes) = tokio::fs::read(&target).await {
            return Ok(bytes);
        }
        self.proxy(key, edge).await?;
        tokio::fs::read(&target).await.context("focus mask was not written")
    }
}

/// Full-resolution linear RGBA, orientation applied, from whichever decoder this file's format needs. `info`
/// is the photo's cached metadata, when there is some.
///
/// RAW goes through LibRaw and the GPU ingest pass (normalize, hot pixels, demosaic, highlight
/// reconstruction). A JPEG, HEIC or TIFF has had all of that done to it already by whatever wrote it, so it
/// is decoded straight to linear light in its own primaries — the same shape, joining the pipeline at the
/// same point. `largest` bounds every path's work, same as the RAW GPU path's own downsample: without it, a
/// 48-megapixel HEIC would pay full-resolution cost in `focus_energy` and the downsample loop for a proxy
/// that only ever needs at most 2,560 pixels on the long edge — measured at several seconds on this server's
/// CPU for a single 12-megapixel photo, against a sub-second HEIC decode itself.
fn decode_full(path: &std::path::Path, info: Option<&RawInfo>, gpu: Option<&export::Kit>, largest: u32) -> Result<sidecar_engine::RgbaF32> {
    if sidecar_raw::decoded::is_supported(path) {
        let (flip, space) = match info.and_then(|i| Some((i.orientation_flip, i.source_space?))) {
            Some(known) => known,
            None => {
                let exif = sidecar_raw::decoded::exif(path);
                (exif.orientation_flip, exif.space)
            }
        };
        let d = sidecar_raw::decoded::decode(path, flip, space)?;
        let full = sidecar_engine::RgbaF32 { width: d.width, height: d.height, data: d.rgba };
        return Ok(full.downsampled(largest).oriented(d.pending_flip));
    }
    if sidecar_raw::linear_dng::is_supported(path) {
        let flip = match info {
            Some(i) => i.orientation_flip,
            None => sidecar_raw::linear_dng::info(path, &sidecar_raw::decoded::exif(path))?.orientation_flip,
        };
        let d = sidecar_raw::linear_dng::decode(path, largest)?;
        return Ok(sidecar_engine::RgbaF32 { width: d.width, height: d.height, data: d.rgba }.downsampled(largest).oriented(flip));
    }
    let mut raw = RawFile::open(path)?;
    Ok(match (gpu, raw.mosaic()) {
        (Some(export::Kit { gpu: g, ingest, .. }), Ok(m)) => {
            let mul = raw.info().color.as_shot_mul;
            let input = sidecar_engine::MosaicInput { width: m.width, height: m.height, data: &m.data, black: m.black, white: m.white, pattern: m.pattern, period: m.period };
            ingest.proxy(g, &input, [mul[0] as f32, 1.0, mul[2] as f32], largest)?.oriented(m.flip)
        }
        _ => {
            let linear = raw.develop_linear(true)?;
            sidecar_engine::RgbaF32 { width: linear.width, height: linear.height, data: linear.rgba() }.downsampled(largest)
        }
    })
}

/// The cached proxy's size and pixels. The pixels are a view into the file's bytes past the 8-byte header,
/// not a copy of them — 35 MB at 2,560 px, on every request.
async fn read_proxy(path: &std::path::Path) -> Option<(u32, u32, Bytes)> {
    let bytes = Bytes::from(tokio::fs::read(path).await.ok()?);
    let w = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?);
    let h = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?);
    Some((w, h, bytes.slice(8..)))
}

/// A tmp path next to `target` that won't collide with a concurrent writer of the same target: `/open`'s
/// background pre-build and the client's own immediate request race on identical key+edge cache entries
/// often enough to hit this in practice, and a shared `.tmp` name meant the second writer's rename found the
/// first writer's tmp file already gone. The process id and a counter make every name distinct.
fn unique_tmp(target: &std::path::Path) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    target.with_extension(format!("tmp-{}-{}", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)))
}

/// Atomic write, mirroring the proxy cache's temp-then-rename pattern.
fn write_focus_png(path: &std::path::Path, mono: &sidecar_engine::Mono) -> Result<()> {
    let mut out = Vec::new();
    {
        let mut info = png::Info::with_size(mono.width, mono.height);
        info.color_type = png::ColorType::Grayscale;
        info.bit_depth = png::BitDepth::Eight;
        png::Encoder::with_info(&mut out, info)?.write_header()?.write_image_data(&mono.to_r8_bytes())?;
    }
    let tmp = unique_tmp(path);
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub struct Session {
    key: String,
    source: Arc<wgpu::Texture>,
    flip: i32,
    sigma: f32,
    rendering: export::Rendering,
}

const SESSION_LIMIT: usize = 2;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewRequest {
    pub key: String,
    pub path: PathBuf,
    pub edit: sidecar_color::EditParams,
    #[serde(default)]
    pub color_space: sidecar_color::DisplaySpace,
    /// Long edge of the returned JPEG. The cached proxy at this size is developed — not the original — so
    /// this costs a develop pass over a small image, not a decode.
    pub long_edge: Option<u32>,
}

/// The develop pass the browser would have run, run here instead, over the same cached proxy and with the
/// same parameters and shaders — so a browser without WebGPU sees the same picture, just later. See
/// docs/ROADMAP.md, "Browsers without WebGPU".
fn preview_jpeg(kit: &export::Kit, rendering: &export::Rendering, req: &PreviewRequest, proxy: (u32, u32, Bytes), masks: &[Option<sidecar_engine::RgbaF32>]) -> Result<Vec<u8>> {
    let (width, height, bytes) = proxy;
    let source = sidecar_engine::upload_rgba16(&kit.gpu, &sidecar_engine::RgbaF32::from_f16_bytes(width, height, &bytes));
    let source = rendering.correct(kit, &req.edit, source);
    let packed = rendering.pack(&req.edit, req.color_space);
    // The proxy is oriented already, as the masks are.
    let mask_textures = rendering.mask_textures(kit, &req.edit, masks, 0, [width, height]);
    let developed = kit.develop.render(&kit.gpu, &source, &packed.uniform, &packed.lut, &export::masks_for(&packed, &mask_textures, [0.0; 4]))?;
    let developed = rendering.warp(kit, &req.edit, &developed, 0).unwrap_or(developed);
    let img = sidecar_engine::read_rgba16(&kit.gpu, &developed)?;
    export::jpeg_bytes(&img, 88, req.color_space)
}

async fn preview(State(w): State<Arc<Worker>>, Json(req): Json<PreviewRequest>) -> Response {
    let Some(kit) = w.gpu.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "This worker has no GPU; the fallback preview needs one.").into_response();
    };
    w.paths.lock().unwrap().insert(req.key.clone(), req.path.clone());
    let mut edge = req.long_edge.unwrap_or(1600).clamp(256, 4096);
    // A crop keeps part of the proxy: develop a larger one so what is kept still fills the screen. Rounded up
    // to a few sizes, since each new size is a decode of its own.
    if let Some(g) = req.edit.geometry.as_ref().filter(|g| !g.is_identity()) {
        let kept = (g.crop.right - g.crop.left).max(g.crop.bottom - g.crop.top).clamp(0.1, 1.0);
        let wanted = (edge as f64 / kept) as u32;
        edge = [edge, 2048, 2560, 3072, 4096].into_iter().find(|&e| e >= wanted).unwrap_or(4096);
    }
    let proxy = match w.proxy(&req.key, edge).await {
        Ok(p) => p,
        Err(e) => return internal(e),
    };
    let rendering = match w.cached_meta(&req.key).await {
        Ok(m) => export::Rendering::of(&m.info),
        Err(e) => return internal(e),
    };
    let masks = w.mask_images(&req.key, &req.edit).await;
    match tokio::task::spawn_blocking(move || preview_jpeg(&kit, &rendering, &req, proxy, &masks)).await {
        Ok(Ok(jpeg)) => ([("content-type", "image/jpeg"), ("cache-control", "no-store")], jpeg).into_response(),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e.into()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoupeRequest {
    pub key: String,
    pub path: PathBuf,
    /// Centre in the oriented image, 0–1 — in the output, cropped and turned, when the edit has geometry.
    pub center: [f32; 2],
    pub size: [u32; 2],
    pub edit: sidecar_color::EditParams,
    #[serde(default)]
    pub color_space: sidecar_color::DisplaySpace,
}

fn loupe_png(w: &Worker, kit: &export::Kit, req: &LoupeRequest, masks: &[Option<sidecar_engine::RgbaF32>]) -> Result<Vec<u8>> {
    let session = {
        let mut sessions = w.sessions.lock().unwrap();
        sessions.iter().position(|s| s.key == req.key).map(|i| {
            let s = sessions.remove(i);
            let out = (s.source.clone(), s.flip, s.sigma, s.rendering.clone());
            sessions.insert(0, s);
            out
        })
    };
    let (source, flip, sigma, rendering) = match session {
        Some(s) => s,
        None => {
            let (tex, flip, rendering) = export::full_source_for(kit, &req.path)?;
            let sigma = kit.detail.noise_sigma(&kit.gpu, &tex)?;
            let tex = Arc::new(tex);
            let mut sessions = w.sessions.lock().unwrap();
            // Two loupe requests on a cold photo both decode it; keep one, or the pair fills every slot.
            sessions.retain(|s| s.key != req.key);
            sessions.insert(0, Session { key: req.key.clone(), source: tex.clone(), flip, sigma, rendering: rendering.clone() });
            sessions.truncate(SESSION_LIMIT);
            (tex, flip, sigma, rendering)
        }
    };
    let (sw, sh) = (source.width(), source.height());
    let (ow, oh) = if flip & 4 != 0 { (sh, sw) } else { (sw, sh) };
    let settings = export::detail_settings(&req.edit);
    let m = settings.margin();
    let packed = rendering.pack(&req.edit, req.color_space);
    // Masks cover the whole frame; each develop below says where its tile sits in it.
    let mask_textures = rendering.mask_textures(kit, &req.edit, masks, flip, [sw, sh]);
    let masks_at = |x: u32, y: u32| export::masks_for(&packed, &mask_textures, [x as f32, y as f32, sw as f32, sh as f32]);
    // A rectangle of the picture (oriented pixels), developed as export develops it: denoised, sharpened,
    // lens-corrected, through the masks.
    let region = |ox: u32, oy: u32, tw: u32, th: u32| -> Result<sidecar_engine::RgbaF32> {
        // Same rectangle in sensor (unoriented) pixels.
        let (ux, uy, uw, uh) = sidecar_engine::source_rect(flip, sw, sh, (ox, oy, tw, th));
        let data = match rendering.lens_params(&req.edit, [sw, sh], [ux as f64, uy as f64], [0.0; 2]) {
            // The tile reads a region of the source the lens decides: find it, cut it with the detail filters'
            // margin and the bicubic read's reach around it, denoise and sharpen that, then correct it into the
            // tile — the order export uses, on less of the image.
            Some(frame) => {
                let (x0, y0, x1, y1) = sidecar_color::lens::source_bounds(&frame, (ux, uy, uw, uh));
                let reach = m as f64 + 3.0;
                let cx = (x0.floor() - reach).clamp(0.0, (sw - 1) as f64) as u32;
                let cy = (y0.floor() - reach).clamp(0.0, (sh - 1) as f64) as u32;
                let cw = ((x1.ceil() + reach).clamp(1.0, sw as f64) as u32).max(cx + 1) - cx;
                let ch = ((y1.ceil() + reach).clamp(1.0, sh as f64) as u32).max(cy + 1) - cy;
                let region = kit.detail.crop(&kit.gpu, &source, cx, cy, cw, ch);
                let region = kit.detail.apply(&kit.gpu, region, sigma, &settings);
                let params = rendering.lens_params(&req.edit, [sw, sh], [ux as f64, uy as f64], [cx as f64, cy as f64]).unwrap_or(frame);
                let tile = kit.lens.apply(&kit.gpu, &region, &params, [uw, uh]);
                let developed = kit.develop.render(&kit.gpu, &tile, &packed.uniform, &packed.lut, &masks_at(ux, uy))?;
                sidecar_engine::read_rgba16(&kit.gpu, &developed)?.data
            }
            None => {
                let (cx, cy) = (ux.saturating_sub(m), uy.saturating_sub(m));
                let (cw, ch) = ((ux + uw + m).min(sw) - cx, (uy + uh + m).min(sh) - cy);
                let tile = kit.detail.crop(&kit.gpu, &source, cx, cy, cw, ch);
                let tile = kit.detail.apply(&kit.gpu, tile, sigma, &settings);
                let developed = kit.develop.render(&kit.gpu, &tile, &packed.uniform, &packed.lut, &masks_at(cx, cy))?;
                let img = sidecar_engine::read_rgba16(&kit.gpu, &developed)?;
                // Cut the margin.
                let (mx, my) = (ux - cx, uy - cy);
                let mut data = Vec::with_capacity((uw * uh * 4) as usize);
                for y in 0..uh {
                    let row = ((my + y) * cw + mx) as usize * 4;
                    data.extend_from_slice(&img.data[row..row + uw as usize * 4]);
                }
                data
            }
        };
        Ok(sidecar_engine::RgbaF32 { width: uw, height: uh, data }.oriented(flip))
    };
    // A tile `size` big about `center` (0…1) in an image `frame` pixels big, kept inside it.
    let tile_at = |frame: [u32; 2]| {
        let (tw, th) = (req.size[0].clamp(64, 1024).min(frame[0]), req.size[1].clamp(64, 1024).min(frame[1]));
        let x = ((req.center[0].clamp(0.0, 1.0) * frame[0] as f32) as i64 - tw as i64 / 2).clamp(0, (frame[0] - tw) as i64) as u32;
        let y = ((req.center[1].clamp(0.0, 1.0) * frame[1] as f32) as i64 - th as i64 / 2).clamp(0, (frame[1] - th) as i64) as u32;
        (x, y, tw, th)
    };
    let img = match req.edit.geometry.as_ref().filter(|g| !g.is_identity()) {
        // The centre is in the output, as the editor shows it: develop the part of the picture the tile reads,
        // then warp it into the tile as export warps the whole.
        Some(g) => {
            let frame_out = sidecar_color::geometry::output_size(g, [ow, oh]);
            let tile = tile_at(frame_out);
            let (x0, y0, x1, y1) = sidecar_color::geometry::warp_bounds(g, [ow, oh], 0, frame_out, tile);
            let rx = (x0.floor() - 2.0).clamp(0.0, (ow - 1) as f64) as u32;
            let ry = (y0.floor() - 2.0).clamp(0.0, (oh - 1) as f64) as u32;
            let rw = ((x1.ceil() + 2.0).clamp(1.0, ow as f64) as u32).max(rx + 1) - rx;
            let rh = ((y1.ceil() + 2.0).clamp(1.0, oh as f64) as u32).max(ry + 1) - ry;
            let picture = sidecar_engine::upload_rgba16(&kit.gpu, &region(rx, ry, rw, rh)?);
            let params = sidecar_color::geometry::warp_params(g, [ow, oh], 0, frame_out, [tile.0 as f64, tile.1 as f64], [tile.2, tile.3], [rx as f64, ry as f64]);
            sidecar_engine::read_rgba16(&kit.gpu, &kit.warp.apply(&kit.gpu, &picture, &params))?
        }
        None => {
            let (ox, oy, tw, th) = tile_at([ow, oh]);
            region(ox, oy, tw, th)?
        }
    };
    let rgb: Vec<u8> = img.data.as_chunks::<4>().0.iter().flat_map(|p| [0, 1, 2].map(|c| (export::srgb_encode(p[c]) * 255.0 + 0.5) as u8)).collect();
    let mut out = Vec::new();
    {
        let mut info = png::Info::with_size(img.width, img.height);
        info.color_type = png::ColorType::Rgb;
        info.bit_depth = png::BitDepth::Eight;
        if req.color_space == sidecar_color::DisplaySpace::DisplayP3 {
            info.icc_profile = Some(std::borrow::Cow::Borrowed(include_bytes!("../../../assets/icc/DisplayP3-v4.icc")));
        }
        png::Encoder::with_info(&mut out, info)?.write_header()?.write_image_data(&rgb)?;
    }
    Ok(out)
}

async fn loupe(State(w): State<Arc<Worker>>, Json(req): Json<LoupeRequest>) -> Response {
    let Some(kit) = w.gpu.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "This worker has no GPU; the loupe needs one.").into_response();
    };
    let w2 = w.clone();
    let masks = w.mask_images(&req.key, &req.edit).await;
    match tokio::task::spawn_blocking(move || loupe_png(&w2, &kit, &req, &masks)).await {
        Ok(Ok(png)) => ([("content-type", "image/png"), ("cache-control", "no-store")], png).into_response(),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e.into()),
    }
}

fn outbox_dir(w: &Worker, job: &str) -> Option<PathBuf> {
    (job.len() == 32 && job.bytes().all(|b| b.is_ascii_hexdigit())).then(|| w.cache.join("outbox").join(job))
}

/// `POST /v1/render` — streams NDJSON `Progress` lines; the last is `done` or `failed`.
async fn render(State(w): State<Arc<Worker>>, Json(req): Json<export::RenderRequest>) -> Response {
    let Some(kit) = w.gpu.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "This worker has no GPU; exports need one.").into_response();
    };
    if outbox_dir(&w, &req.job_id).is_none() || req.file_name.contains('/') {
        return StatusCode::BAD_REQUEST.into_response();
    }
    // The render is blocking code; an unbounded tokio sender is safe to call from it, so progress goes
    // straight onto the response with no thread in between.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<export::Progress>();
    let w2 = w.clone();
    tokio::spawn(async move {
        let _slot = w2.export_slot.acquire().await;
        let outbox = w2.cache.join("outbox");
        let tx2 = tx.clone();
        let masks = w2.mask_images(&req.key, &req.edit).await;
        let result = tokio::task::spawn_blocking(move || export::render(&kit, &req, &masks, &outbox, &tx2)).await;
        let last = match result {
            Ok(Ok(done)) => done,
            Ok(Err(e)) => export::Progress::Failed { message: format!("{e:#}") },
            Err(e) => export::Progress::Failed { message: format!("render crashed: {e}") },
        };
        if let export::Progress::Failed { message } = &last {
            tracing::error!("export failed: {message}");
        }
        let _ = tx.send(last);
    });
    use futures_util::StreamExt;
    let lines = tokio_stream::wrappers::UnboundedReceiverStream::new(rx).map(|p| Ok::<_, std::convert::Infallible>(serde_json::to_string(&p).unwrap() + "\n"));
    ([("content-type", "application/x-ndjson")], axum::body::Body::from_stream(lines)).into_response()
}

/// How long a rendered file stays fetchable after its job finishes. It used to be deleted the moment Immich
/// took it, which is still what happens when Immich is the only destination — but an export the browser is
/// going to download has to outlive the job, and a phone that locks mid-download gets to come back to it.
const OUTBOX_TTL: Duration = Duration::from_secs(24 * 3600);

/// Delete outbox entries older than `OUTBOX_TTL`, hourly. Without this, keeping a copy for the device to
/// download would grow the cache by one full-resolution file per export, forever.
fn sweep_outbox(cache: PathBuf) {
    tokio::spawn(async move {
        let outbox = cache.join("outbox");
        loop {
            let mut swept = 0usize;
            if let Ok(mut entries) = tokio::fs::read_dir(&outbox).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let old = match entry.metadata().await.and_then(|m| m.modified()) {
                        Ok(t) => t.elapsed().map(|age| age > OUTBOX_TTL).unwrap_or(false),
                        Err(_) => false,
                    };
                    if old && tokio::fs::remove_dir_all(entry.path()).await.is_ok() {
                        swept += 1;
                    }
                }
            }
            if swept > 0 {
                tracing::info!("swept {swept} finished export{} from the outbox", if swept == 1 { "" } else { "s" });
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

async fn outbox_file(State(w): State<Arc<Worker>>, Path((job, file)): Path<(String, String)>) -> Response {
    let Some(dir) = outbox_dir(&w, &job) else { return StatusCode::NOT_FOUND.into_response() };
    if file.contains('/') || file.starts_with('.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::File::open(dir.join(&file)).await {
        Ok(f) => ([("content-type", "application/octet-stream")], axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(f))).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn outbox_delete(State(w): State<Arc<Worker>>, Path(job): Path<String>) -> Response {
    let Some(dir) = outbox_dir(&w, &job) else { return StatusCode::NOT_FOUND.into_response() };
    let _ = tokio::fs::remove_dir_all(dir).await;
    StatusCode::NO_CONTENT.into_response()
}

pub fn router(worker: Arc<Worker>) -> Router {
    sweep_outbox(worker.cache.clone());
    Router::new()
        .route("/v1/health", get(|| async { "ok" }))
        .route("/v1/render", post(render))
        .route("/v1/loupe", post(loupe))
        .route("/v1/preview", post(preview))
        .route("/v1/outbox/{job}/{file}", get(outbox_file))
        .route("/v1/outbox/{job}", axum::routing::delete(outbox_delete))
        .route("/v1/ingest", post(ingest))
        .route("/v1/assets/{key}/meta", get(meta))
        .route("/v1/assets/{key}/proxy", get(proxy_handler))
        .route("/v1/assets/{key}/focus", get(focus_handler))
        .route("/v1/assets/{key}/masks", get(masks_handler))
        .route("/v1/assets/{key}/masks/{name}", get(mask_handler))
        .route("/v1/lensfun", get(lensfun_status))
        .route("/v1/lensfun/done", post(lensfun_done))
        .route("/v1/lensfun/{name}", axum::routing::put(lensfun_put))
        .with_state(worker)
}

async fn lensfun_status(State(w): State<Arc<Worker>>) -> Response {
    Json(w.lensfun.status()).into_response()
}

async fn lensfun_put(State(w): State<Arc<Worker>>, Path(name): Path<String>, body: String) -> Response {
    let lf = w.lensfun.clone();
    match tokio::task::spawn_blocking(move || lf.put(&name, &body)).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
        Err(e) => internal(e.into()),
    }
}

async fn lensfun_done(State(w): State<Arc<Worker>>) -> Response {
    let lf = w.lensfun.clone();
    match tokio::task::spawn_blocking(move || lf.finish()).await {
        Ok(Ok(())) => Json(w.lensfun.status()).into_response(),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e.into()),
    }
}

async fn ingest(State(w): State<Arc<Worker>>, Json(req): Json<IngestRequest>) -> Response {
    match w.ingest(req.path).await {
        Ok(meta) => {
            let (w2, key) = (w.clone(), meta.asset_key.clone());
            tokio::spawn(async move {
                if let Err(e) = w2.proxy(&key, DEFAULT_EDGE).await {
                    tracing::warn!("pre-building proxy for {key}: {e:#}");
                }
            });
            Json(meta).into_response()
        }
        Err(e) => internal(e),
    }
}

async fn meta(State(w): State<Arc<Worker>>, Path(key): Path<String>) -> Response {
    match tokio::fs::read(w.meta_path(&key)).await {
        Ok(bytes) => ([("content-type", "application/json")], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct ProxyQuery {
    long_edge: Option<u32>,
}

async fn proxy_handler(State(w): State<Arc<Worker>>, Path(key): Path<String>, Query(q): Query<ProxyQuery>) -> Response {
    let edge = q.long_edge.unwrap_or(DEFAULT_EDGE).clamp(256, 8192);
    match w.proxy(&key, edge).await {
        Ok((width, height, bytes)) => {
            let mut h = HeaderMap::new();
            h.insert("content-type", HeaderValue::from_static("application/x-sidecar-rgba16f"));
            h.insert("x-width", width.into());
            h.insert("x-height", height.into());
            h.insert("x-process-version", PROCESS_VERSION.into());
            (h, bytes).into_response()
        }
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
struct FocusQuery {
    edge: Option<u32>,
}

async fn masks_handler(State(w): State<Arc<Worker>>, Path(key): Path<String>) -> Response {
    match w.masks(&key).await {
        Ok(set) => Json(set).into_response(),
        Err(e) => internal(e),
    }
}

async fn mask_handler(State(w): State<Arc<Worker>>, Path((key, name)): Path<(String, String)>) -> Response {
    match w.mask(&key, &name).await {
        Ok(Some(png)) => ([("content-type", "image/png")], png).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no such mask here").into_response(),
        Err(e) => internal(e),
    }
}

async fn focus_handler(State(w): State<Arc<Worker>>, Path(key): Path<String>, Query(q): Query<FocusQuery>) -> Response {
    let edge = q.edge.unwrap_or(DEFAULT_EDGE).clamp(256, 8192);
    match w.focus(&key, edge).await {
        Ok(bytes) => ([("content-type", "image/png")], bytes).into_response(),
        Err(e) => internal(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Develop a display-linear Display P3 test card as a developed file, in Display P3.
    /// One test at a time holds a GPU device. Creating and destroying several at once in one process
    /// crashes NVIDIA's Vulkan driver now and then (a segfault in about a third of parallel runs, none in
    /// serial ones); the worker itself only ever makes one.
    static GPU_TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn develop_developed(edit: &sidecar_color::EditParams, card: &sidecar_engine::RgbaF32) -> Option<sidecar_engine::RgbaF32> {
        let _turn = GPU_TURN.blocking_lock();
        let gpu = sidecar_engine::Gpu::new_headless().ok()?;
        let pass = sidecar_engine::DevelopPass::new(&gpu).unwrap();
        let rendering = export::Rendering::developed(sidecar_raw::SourceSpace::DisplayP3.camera_color());
        let packed = rendering.pack(edit, sidecar_color::DisplaySpace::DisplayP3);
        Some(pass.run(&gpu, card, &packed.uniform, &packed.lut).unwrap())
    }

    /// Greys from black to white, then saturated and muted colours, all inside the file's own gamut.
    fn test_card() -> sidecar_engine::RgbaF32 {
        let mut data = Vec::new();
        for i in 0..64 {
            let v = i as f32 / 63.0;
            data.extend([v, v, v, 0.0]);
        }
        for p in [[1.0, 0.0, 0.0], [0.9, 0.1, 0.05], [0.2, 0.6, 0.9], [0.05, 0.05, 0.9], [0.5, 0.45, 0.4], [1.0, 1.0, 0.0], [0.02, 0.3, 0.02], [0.95, 0.6, 0.5]] {
            data.extend([p[0], p[1], p[2], 0.0]);
        }
        sidecar_engine::RgbaF32 { width: 72, height: 1, data }
    }

    fn encode(v: f32) -> f32 {
        export::srgb_encode(v)
    }

    /// A HEIC or JPEG at default settings is shown as the file holds it: no second tone curve, no gamut
    /// compression. The RAW path rendered its white at 82 % and desaturated its reds.
    #[test]
    fn a_developed_file_is_shown_as_it_is_at_default_settings() {
        let card = test_card();
        let Some(out) = develop_developed(&sidecar_color::EditParams::default(), &card) else { return eprintln!("skipping: no GPU") };
        for (i, (a, b)) in card.data.chunks(4).zip(out.data.chunks(4)).enumerate() {
            for c in 0..3 {
                let d = (encode(a[c]) - encode(b[c])).abs() * 255.0;
                assert!(d < 0.6, "pixel {i} channel {c}: {} → {} ({d:.2}/255)", a[c], b[c]);
            }
        }
    }

    /// Each tone control does what it says on a developed file, and none of them breaks monotonicity.
    #[test]
    fn developed_tone_controls_behave() {
        let card = test_card();
        let grey = |img: &sidecar_engine::RgbaF32| -> Vec<f32> { img.data.chunks(4).take(64).map(|p| encode(p[1])).collect() };
        let run = |f: &dyn Fn(&mut sidecar_color::EditParams)| {
            let mut e = sidecar_color::EditParams::default();
            f(&mut e);
            develop_developed(&e, &card).map(|o| grey(&o))
        };
        let Some(base) = run(&|_| {}) else { return eprintln!("skipping: no GPU") };
        let monotone = |g: &[f32]| g.windows(2).all(|w| w[1] >= w[0] - 1e-3);

        let brighter = run(&|e| e.exposure.ev = 1.0).unwrap();
        assert!(monotone(&brighter) && brighter[32] > base[32] + 0.1, "exposure +1 brightens mid grey");
        assert!(brighter[63] <= 1.0 && brighter[62] < brighter[63], "and rolls off rather than clipping flat");

        let whites = run(&|e| e.tone.white_ev = 2.5).unwrap();
        assert!(monotone(&whites) && (whites[55] - 1.0).abs() < 2e-3, "Whites +100 takes the top fifth to white");
        let dim = run(&|e| e.tone.white_ev = 6.5).unwrap();
        assert!((dim[63] - 0.8).abs() < 2e-3, "Whites −100 dims white to 80 %");

        let lifted = run(&|e| e.tone.black_ev = -11.0).unwrap();
        assert!((lifted[0] - 0.1).abs() < 2e-3, "Blacks +100 lifts black to 10 %");
        let crushed = run(&|e| e.tone.black_ev = -5.0).unwrap();
        for (b, c) in base.iter().zip(&crushed) {
            assert!(if *b <= 0.1 { *c < 1e-3 } else { *c > 0.0 }, "Blacks −100 crushes the bottom tenth, and only it: {b} → {c}");
        }

        let contrast = run(&|e| e.tone.contrast = 1.5).unwrap();
        assert!(monotone(&contrast) && contrast[10] < base[10] && contrast[55] > base[55], "Contrast + is an S around grey");
        assert!((contrast[0] - base[0]).abs() < 2e-3 && (contrast[63] - base[63]).abs() < 2e-3, "with its ends fixed");
    }

    /// The waste this was written for: opening one photo fans out into four requests that all want the same
    /// decode, and two of them used to run it at once — every photo in this server's logs was decoded twice,
    /// ~5 s apiece for a 12 Mpx HEIC. Different photos still decode in parallel.
    #[tokio::test]
    async fn one_decode_per_asset_at_a_time() {
        let tmp = tempfile::tempdir().unwrap();
        let _turn = GPU_TURN.lock().await;
        let w = Worker::new(tmp.path().to_path_buf()).unwrap();
        let wait = Duration::from_millis(100);

        let held = w.decode_guard("aaa").await;
        assert!(tokio::time::timeout(wait, w.decode_guard("bbb")).await.is_ok(), "a different photo shouldn't wait");
        assert!(tokio::time::timeout(wait, w.decode_guard("aaa")).await.is_err(), "the same photo should wait");
        drop(held);
        assert!(tokio::time::timeout(wait, w.decode_guard("aaa")).await.is_ok(), "and go ahead once the first decode is done");
    }

    /// What went wrong on the real server: 0.2.1 stopped rotating a HEIC twice, and every proxy decoded
    /// before it stayed on disk and upside down, because the file name only carried `ProcessVersion` — which
    /// that fix had no reason to change. A cached file from an earlier generation must be unreachable, and
    /// then swept, rather than answering for pixels this code would not have produced.
    #[test]
    fn an_earlier_ingest_generation_is_unreachable_and_swept() {
        let tmp = tempfile::tempdir().unwrap();
        let key = "a".repeat(40);
        let (proxies, focus) = (tmp.path().join("proxies"), tmp.path().join("focus"));
        std::fs::create_dir_all(&proxies).unwrap();
        std::fs::create_dir_all(&focus).unwrap();
        // How the files were named before ingest had a generation of its own.
        let old = proxies.join(format!("{key}-pv{PROCESS_VERSION}-2560.f16"));
        let older = focus.join(format!("{key}-pv{PROCESS_VERSION}-g1-768.png"));
        std::fs::write(&old, b"upside down").unwrap();
        std::fs::write(&older, b"upside down").unwrap();

        let _turn = GPU_TURN.blocking_lock();
        let w = Worker::new(tmp.path().to_path_buf()).unwrap();
        assert_ne!(w.proxy_path(&key, 2560), old, "the current code must not look at the old name");
        assert_ne!(w.focus_path(&key, 768), older);
        assert!(!old.exists() && !older.exists(), "and Worker::new should have swept both");
    }

    /// Re-ingesting an unchanged file must not hash it again, and a changed one must. Proved by swapping in
    /// different bytes of the same length with the old mtime put back: the cached key still answers, which
    /// only happens if the file wasn't read. Touching the mtime then brings the new key.
    #[tokio::test]
    async fn an_unchanged_file_is_not_hashed_again() {
        let tmp = tempfile::tempdir().unwrap();
        let _turn = GPU_TURN.lock().await;
        let w = Worker::new(tmp.path().join("cache")).unwrap();
        let photo = tmp.path().join("a.png");
        let png = |grey: u8| {
            let mut out = Vec::new();
            let mut enc = png::Encoder::new(&mut out, 1, 1);
            enc.set_color(png::ColorType::Grayscale);
            enc.write_header().unwrap().write_image_data(&[grey]).unwrap();
            out
        };
        std::fs::write(&photo, png(10)).unwrap();
        let first = w.ingest(photo.clone()).await.unwrap().asset_key;
        assert_eq!(first, sidecar_xmp::sha1_file(&photo).unwrap());

        let modified = std::fs::metadata(&photo).unwrap().modified().unwrap();
        std::fs::write(&photo, png(200)).unwrap();
        std::fs::File::options().write(true).open(&photo).unwrap().set_modified(modified).unwrap();
        assert_eq!(w.ingest(photo.clone()).await.unwrap().asset_key, first, "same size and mtime: not re-hashed");

        std::fs::File::options().write(true).open(&photo).unwrap().set_modified(modified + Duration::from_secs(1)).unwrap();
        let second = w.ingest(photo.clone()).await.unwrap().asset_key;
        assert_ne!(second, first, "a new mtime is a new file");
        assert_eq!(second, sidecar_xmp::sha1_file(&photo).unwrap());
    }

    /// The map is keyed by asset, so it would otherwise hold one entry per photo ever opened.
    #[tokio::test]
    async fn finished_decodes_leave_nothing_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let _turn = GPU_TURN.lock().await;
        let w = Worker::new(tmp.path().to_path_buf()).unwrap();
        for i in 0..32 {
            drop(w.decode_guard(&format!("key{i}")).await);
        }
        assert!(w.decoding.lock().unwrap().len() <= 1, "dead entries should be swept as new ones are made");
    }

    #[test]
    fn focus_png_round_trips_and_leaves_no_temp_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("f.png");
        let mono = sidecar_engine::Mono { width: 3, height: 2, data: vec![0.0, 0.5, 1.0, 1.0, 0.5, 0.0] };
        write_focus_png(&path, &mono).unwrap();
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1, "only f.png should remain, no leftover tmp file");

        let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
        let mut reader = decoder.read_info().unwrap();
        assert_eq!((reader.info().width, reader.info().height), (3, 2));
        assert_eq!(reader.info().color_type, png::ColorType::Grayscale);
        let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut buf).unwrap();
        assert_eq!(buf, mono.to_r8_bytes());
    }

    /// Caught live: `/open`'s background pre-build and the client's own immediate request race on the same
    /// key+edge often enough to hit this in practice. A shared `.tmp` name meant the second writer's rename
    /// found the first writer's tmp file already gone — "No such file or directory" on real photos.
    #[test]
    fn concurrent_writes_to_the_same_target_never_collide_on_the_tmp_path() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("shared.png");
        let mono = sidecar_engine::Mono { width: 2, height: 2, data: vec![0.0, 0.5, 1.0, 0.25] };
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let target = target.clone();
                let mono = mono.clone();
                std::thread::spawn(move || write_focus_png(&target, &mono))
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "only the final target should remain");
    }
}
