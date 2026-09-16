// SPDX-License-Identifier: Apache-2.0
//! Worker HTTP interface (internal network only; the api is its only client).
//!
//! `POST /v1/ingest { path }`                 hash, read metadata, cache it, pre-build proxies
//! `GET  /v1/assets/{key}/meta`               cached metadata
//! `GET  /v1/assets/{key}/proxy?long_edge=N`  RGBA16F proxy bytes; `X-Width`, `X-Height` headers

pub mod export;

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sidecar_color::params::PROCESS_VERSION;
use sidecar_raw::{RawFile, RawInfo, proxy};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

/// Proxy sizes built together from one decode: a quick first paint, and the phone-sized proxy.
pub const PREVIEW_EDGE: u32 = 768;
pub const DEFAULT_EDGE: u32 = 2560;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub asset_key: String,
    pub file_name: String,
    pub process_version: u32,
    pub info: RawInfo,
}

#[derive(Deserialize)]
pub struct IngestRequest {
    pub path: PathBuf,
}

pub struct Worker {
    cache: PathBuf,
    paths: Mutex<HashMap<String, PathBuf>>,
    decode_slots: Semaphore,
    /// GPU ingest and develop (Vulkan). `None` when no usable GPU: proxies then come from LibRaw on the CPU,
    /// and exports are refused.
    gpu: Option<Arc<export::Kit>>,
    /// One full-resolution render at a time.
    export_slot: Semaphore,
    /// Full-resolution linear buffers of recently opened photos, for loupe tiles (most recent first).
    sessions: Mutex<Vec<Session>>,
}

pub fn sha1_file(path: &std::path::Path) -> Result<String> {
    use sha1::Digest;
    let mut f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut h = sha1::Sha1::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

fn internal(e: anyhow::Error) -> Response {
    tracing::error!("{e:#}");
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response()
}

impl Worker {
    pub fn new(cache: PathBuf) -> Result<Arc<Self>> {
        std::fs::create_dir_all(cache.join("meta"))?;
        std::fs::create_dir_all(cache.join("proxies"))?;
        std::fs::create_dir_all(cache.join("outbox"))?;
        let kit = sidecar_engine::Gpu::new_headless().and_then(|gpu| {
            let ingest = sidecar_engine::Ingest::new(&gpu)?;
            let develop = sidecar_engine::DevelopPass::new(&gpu)?;
            let detail = sidecar_engine::detail::Detail::new(&gpu)?;
            Ok(export::Kit { gpu, ingest, develop, detail })
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
        Ok(Arc::new(Self { cache, paths: Mutex::new(HashMap::new()), decode_slots: Semaphore::new(2), gpu, export_slot: Semaphore::new(1), sessions: Mutex::new(Vec::new()) }))
    }

    fn meta_path(&self, key: &str) -> PathBuf {
        self.cache.join("meta").join(format!("{key}.json"))
    }

    fn proxy_path(&self, key: &str, edge: u32) -> PathBuf {
        self.cache.join("proxies").join(format!("{key}-pv{PROCESS_VERSION}-{edge}.f16"))
    }

    pub async fn ingest(self: &Arc<Self>, path: PathBuf) -> Result<Meta> {
        let key = {
            let p = path.clone();
            tokio::task::spawn_blocking(move || sha1_file(&p)).await??
        };
        self.paths.lock().unwrap().insert(key.clone(), path.clone());
        let meta_path = self.meta_path(&key);
        if let Ok(bytes) = tokio::fs::read(&meta_path).await
            && let Ok(meta) = serde_json::from_slice::<Meta>(&bytes)
        {
            return Ok(meta);
        }
        let _slot = self.decode_slots.acquire().await?;
        let (k, p) = (key.clone(), path.clone());
        let meta = tokio::task::spawn_blocking(move || -> Result<Meta> {
            let raw = RawFile::open(&p)?;
            Ok(Meta {
                asset_key: k,
                file_name: p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
                process_version: PROCESS_VERSION,
                info: raw.info(),
            })
        })
        .await??;
        tokio::fs::write(&meta_path, serde_json::to_vec_pretty(&meta)?).await?;
        Ok(meta)
    }

    /// Build (or reuse) the proxy at `edge`, building the preview size from the same decode.
    pub async fn proxy(self: &Arc<Self>, key: &str, edge: u32) -> Result<(u32, u32, Vec<u8>)> {
        let target = self.proxy_path(key, edge);
        if let Some(found) = read_proxy(&target).await {
            return Ok(found);
        }
        let _slot = self.decode_slots.acquire().await?;
        if let Some(found) = read_proxy(&target).await {
            return Ok(found);
        }
        let path = self.paths.lock().unwrap().get(key).cloned().context("unknown asset; ingest it first")?;
        let edges: Vec<(u32, PathBuf)> = [edge, PREVIEW_EDGE].into_iter().map(|e| (e, self.proxy_path(key, e))).collect();
        let gpu = self.gpu.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let started = std::time::Instant::now();
            let mut raw = RawFile::open(&path)?;
            let largest = edges.iter().map(|(e, _)| *e).max().unwrap_or(edge);
            let full = match (gpu.as_deref(), raw.mosaic()) {
                (Some(export::Kit { gpu: g, ingest, .. }), Ok(m)) => {
                    let mul = raw.info().color.as_shot_mul;
                    let input = sidecar_engine::MosaicInput { width: m.width, height: m.height, data: &m.data, black: m.black, white: m.white, pattern: m.pattern, period: m.period };
                    ingest.proxy(g, &input, [mul[0] as f32, 1.0, mul[2] as f32], largest)?.oriented(m.flip)
                }
                _ => {
                    let linear = raw.develop_linear(true)?;
                    let p = proxy::build(&linear, largest);
                    sidecar_engine::RgbaF32 { width: p.width, height: p.height, data: p.rgba }
                }
            };
            for (e, out) in edges {
                let p = full.downsampled(e);
                let mut bytes = Vec::with_capacity(8 + p.data.len() * 2);
                bytes.extend_from_slice(&p.width.to_le_bytes());
                bytes.extend_from_slice(&p.height.to_le_bytes());
                bytes.extend_from_slice(&p.to_f16_bytes());
                let tmp = out.with_extension("tmp");
                std::fs::write(&tmp, &bytes)?;
                std::fs::rename(&tmp, &out)?;
            }
            tracing::info!("proxy for {} in {:.0} ms", path.display(), started.elapsed().as_secs_f64() * 1e3);
            Ok(())
        })
        .await??;
        read_proxy(&target).await.context("proxy was not written")
    }
}

async fn read_proxy(path: &std::path::Path) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = tokio::fs::read(path).await.ok()?;
    let w = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?);
    let h = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?);
    Some((w, h, bytes[8..].to_vec()))
}

pub struct Session {
    key: String,
    source: Arc<wgpu::Texture>,
    flip: i32,
    sigma: f32,
    color: sidecar_color::camera::CameraColor,
}

const SESSION_LIMIT: usize = 2;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoupeRequest {
    pub key: String,
    pub path: PathBuf,
    /// Centre in the oriented image, 0–1.
    pub center: [f32; 2],
    pub size: [u32; 2],
    pub edit: sidecar_color::EditParams,
    #[serde(default)]
    pub color_space: sidecar_color::DisplaySpace,
}

fn loupe_png(w: &Worker, kit: &export::Kit, req: &LoupeRequest) -> Result<Vec<u8>> {
    let session = {
        let mut sessions = w.sessions.lock().unwrap();
        sessions.iter().position(|s| s.key == req.key).map(|i| {
            let s = sessions.remove(i);
            let out = (s.source.clone(), s.flip, s.sigma, s.color.clone());
            sessions.insert(0, s);
            out
        })
    };
    let (source, flip, sigma, color) = match session {
        Some(s) => s,
        None => {
            let mut raw = RawFile::open(&req.path)?;
            let color = raw.info().color;
            let (tex, flip) = export::full_source(kit, &mut raw)?;
            let sigma = kit.detail.noise_sigma(&kit.gpu, &tex)?;
            let tex = Arc::new(tex);
            let mut sessions = w.sessions.lock().unwrap();
            sessions.insert(0, Session { key: req.key.clone(), source: tex.clone(), flip, sigma, color: color.clone() });
            sessions.truncate(SESSION_LIMIT);
            (tex, flip, sigma, color)
        }
    };
    let (sw, sh) = (source.width(), source.height());
    let rotated = matches!(flip, 5 | 6);
    let (ow, oh) = if rotated { (sh, sw) } else { (sw, sh) };
    let (tw, th) = (req.size[0].clamp(64, 1024).min(ow), req.size[1].clamp(64, 1024).min(oh));
    // Tile rectangle in oriented pixels, kept inside the image.
    let ox = ((req.center[0].clamp(0.0, 1.0) * ow as f32) as i64 - tw as i64 / 2).clamp(0, (ow - tw) as i64) as u32;
    let oy = ((req.center[1].clamp(0.0, 1.0) * oh as f32) as i64 - th as i64 / 2).clamp(0, (oh - th) as i64) as u32;
    // Same rectangle in sensor (unoriented) pixels.
    let (ux, uy, uw, uh) = match flip {
        3 => (sw - ox - tw, sh - oy - th, tw, th),
        6 => (oy, sh - ox - tw, th, tw),
        5 => (sw - oy - th, ox, th, tw),
        _ => (ox, oy, tw, th),
    };
    let settings = export::detail_settings(&req.edit);
    let m = settings.margin();
    let (cx, cy) = (ux.saturating_sub(m), uy.saturating_sub(m));
    let (cw, ch) = ((ux + uw + m).min(sw) - cx, (uy + uh + m).min(sh) - cy);
    let tile = kit.detail.crop(&kit.gpu, &source, cx, cy, cw, ch);
    let tile = kit.detail.apply(&kit.gpu, tile, sigma, &settings);
    let packed = sidecar_color::pack(&req.edit, &color, req.color_space);
    let developed = kit.develop.render(&kit.gpu, &tile, &packed.uniform, &packed.lut)?;
    let img = sidecar_engine::read_rgba16(&kit.gpu, &developed)?;
    // Cut the margin, then orient.
    let (mx, my) = (ux - cx, uy - cy);
    let mut data = Vec::with_capacity((uw * uh * 4) as usize);
    for y in 0..uh {
        let row = ((my + y) * cw + mx) as usize * 4;
        data.extend_from_slice(&img.data[row..row + uw as usize * 4]);
    }
    let img = sidecar_engine::RgbaF32 { width: uw, height: uh, data }.oriented(flip);
    let mut rgb = Vec::with_capacity((img.width * img.height * 3) as usize);
    for p in img.data.as_chunks::<4>().0.iter() {
        for v in &p[..3] {
            let v = v.clamp(0.0, 1.0);
            let e = if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
            rgb.push((e * 255.0 + 0.5) as u8);
        }
    }
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
    match tokio::task::spawn_blocking(move || loupe_png(&w2, &kit, &req)).await {
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
    let (tx, rx) = std::sync::mpsc::channel::<export::Progress>();
    let (atx, arx) = tokio::sync::mpsc::unbounded_channel::<Result<String, std::io::Error>>();
    std::thread::spawn(move || {
        for p in rx {
            if atx.send(Ok(serde_json::to_string(&p).unwrap() + "\n")).is_err() {
                break;
            }
        }
    });
    let w2 = w.clone();
    tokio::spawn(async move {
        let _slot = w2.export_slot.acquire().await;
        let outbox = w2.cache.join("outbox");
        let tx2 = tx.clone();
        let result = tokio::task::spawn_blocking(move || export::render(&kit, &req, &outbox, &tx2)).await;
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
    ([("content-type", "application/x-ndjson")], axum::body::Body::from_stream(tokio_stream::wrappers::UnboundedReceiverStream::new(arx))).into_response()
}

async fn outbox_file(State(w): State<Arc<Worker>>, Path((job, file)): Path<(String, String)>) -> Response {
    let Some(dir) = outbox_dir(&w, &job) else { return StatusCode::NOT_FOUND.into_response() };
    if file.contains('/') || file.starts_with('.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::File::open(dir.join(&file)).await {
        Ok(f) => ([("content-type", "application/octet-stream")], axum::body::Body::from_stream(tokio_util_stream(f))).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn tokio_util_stream(f: tokio::fs::File) -> impl futures_util::Stream<Item = std::io::Result<Vec<u8>>> {
    futures_util::stream::unfold(f, |mut f| async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 1 << 20];
        match f.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(buf), f))
            }
            Err(e) => Some((Err(e), f)),
        }
    })
}

async fn outbox_delete(State(w): State<Arc<Worker>>, Path(job): Path<String>) -> Response {
    let Some(dir) = outbox_dir(&w, &job) else { return StatusCode::NOT_FOUND.into_response() };
    let _ = tokio::fs::remove_dir_all(dir).await;
    StatusCode::NO_CONTENT.into_response()
}

pub fn router(worker: Arc<Worker>) -> Router {
    Router::new()
        .route("/v1/health", get(|| async { "ok" }))
        .route("/v1/render", post(render))
        .route("/v1/loupe", post(loupe))
        .route("/v1/outbox/{job}/{file}", get(outbox_file))
        .route("/v1/outbox/{job}", axum::routing::delete(outbox_delete))
        .route("/v1/ingest", post(ingest))
        .route("/v1/assets/{key}/meta", get(meta))
        .route("/v1/assets/{key}/proxy", get(proxy_handler))
        .with_state(worker)
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
