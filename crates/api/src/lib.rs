// SPDX-License-Identifier: Apache-2.0
//! Public HTTP surface. The worker is reached over the internal network only.

pub mod config;
pub mod export;
pub mod immich;
pub mod resolve;
pub mod session;
pub mod store;

use anyhow::Result;
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use config::Config;
use resolve::{Candidate, OpenQuery, Resolved};
use serde::{Deserialize, Serialize};
use session::{ApiUser, PageUser, User};
use sidecar_color::EditParams;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use store::{Store, random_hex};
use tower_http::compression::Predicate;
use tower_http::{
    compression::CompressionLayer,
    services::{ServeDir, ServeFile},
};

pub struct App {
    pub config: Config,
    pub http: reqwest::Client,
    pub immich: immich::Immich,
    pub store: Store,
    pub secret: [u8; 32],
    pub admin_key: Option<String>,
    pub jobs: export::Jobs,
    discovery: Mutex<Discovery>,
}

#[derive(Default)]
struct Discovery {
    last: Option<Instant>,
    pending: bool,
}

impl App {
    pub fn new(config: Config) -> Result<Arc<Self>> {
        let store = Store::new(config.server.state_dir.clone())?;
        let secret = session::load_secret(&config.server.state_dir)?;
        let http = reqwest::Client::new();
        let admin_key = match &config.immich.admin_api_key_file {
            Some(p) => Some(std::fs::read_to_string(p).map(|s| s.trim().to_string()).map_err(|e| anyhow::anyhow!("reading {}: {e}", p.display()))?),
            None => None,
        };
        let immich = immich::Immich { base: config.immich.url.clone(), http: http.clone() };
        Ok(Arc::new(Self { config, http, immich, store, secret, admin_key, jobs: export::Jobs::default(), discovery: Mutex::new(Discovery::default()) }))
    }

    pub(crate) fn worker(&self, path: &str) -> String {
        format!("{}{}", self.config.server.worker_url.trim_end_matches('/'), path)
    }

    /// Ask Immich to discover a newly created internal-library sidecar: at most one run an hour.
    fn queue_sidecar_discovery(self: &Arc<Self>) {
        let Some(key) = self.admin_key.clone() else { return };
        let delay = {
            let mut d = self.discovery.lock().unwrap();
            if d.pending {
                return;
            }
            d.pending = true;
            d.last.map_or(Duration::ZERO, |l| Duration::from_secs(3600).saturating_sub(l.elapsed()))
        };
        let app = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            match app.immich.discover_missing_sidecars(&key).await {
                Ok(()) => tracing::info!("started Immich sidecar discovery"),
                Err(e) => tracing::warn!("Immich sidecar discovery failed — an internal-library sidecar may not move with its original: {e:#}"),
            }
            let mut d = app.discovery.lock().unwrap();
            d.pending = false;
            d.last = Some(Instant::now());
        });
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetRecord {
    pub path: std::path::PathBuf,
    pub file_name: String,
    pub immich_asset_id: Option<String>,
    pub users: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserRecord {
    recent: Vec<RecentItem>,
    #[serde(default)]
    settings: Settings,
}

/// What the viewport renders in, so that what you judge on screen is what the file will hold. The export
/// sheet starts from the same choice and can still be overridden for one export.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub output_space: sidecar_color::DisplaySpace,
    pub clip_highlights: bool,
    pub clip_shadows: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { output_space: sidecar_color::DisplaySpace::Srgb, clip_highlights: false, clip_shadows: false }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecentItem {
    asset_key: String,
    file_name: String,
    immich_asset_id: Option<String>,
    opened_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PickRecord {
    user_id: String,
    query: String,
    candidates: Vec<Candidate>,
}

fn error_page(status: StatusCode, message: &str) -> Response {
    let escaped = message.replace('&', "&amp;").replace('<', "&lt;");
    (
        status,
        Html(format!(
            "<!doctype html><meta name=viewport content='width=device-width'><title>Sidecar</title>\
             <body style='background:#161616;color:#f4f4f4;font:15px system-ui;padding:24px'>\
             <h1 style='font-size:18px'>Sidecar couldn't open that photo</h1><p style='color:#c6c6c6'>{escaped}</p>"
        )),
    )
        .into_response()
}

pub(crate) fn internal(e: anyhow::Error) -> Response {
    tracing::error!("{e:#}");
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response()
}

/// Ingest a resolved original, remember that this user may open it, add it to Recent.
async fn admit(app: &Arc<App>, user: &User, c: Candidate) -> Result<String> {
    let resp = app.http.post(app.worker("/v1/ingest")).json(&serde_json::json!({ "path": c.path })).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("{}", resp.text().await.unwrap_or_default());
    }
    let meta: serde_json::Value = resp.json().await?;
    let key = meta["assetKey"].as_str().unwrap_or_default().to_string();
    let mut rec: AssetRecord = app.store.get("assets", &key).await.unwrap_or(AssetRecord {
        path: c.path.clone(),
        file_name: c.file_name.clone(),
        immich_asset_id: None,
        users: Vec::new(),
    });
    rec.path = c.path;
    rec.immich_asset_id = c.immich_asset_id.or(rec.immich_asset_id);
    if !rec.users.contains(&user.id) {
        rec.users.push(user.id.clone());
    }
    app.store.put("assets", &key, &rec).await?;
    let mut u: UserRecord = app.store.get("users", &user.id).await.unwrap_or_default();
    u.recent.retain(|r| r.asset_key != key);
    u.recent.insert(0, RecentItem { asset_key: key.clone(), file_name: rec.file_name, immich_asset_id: rec.immich_asset_id, opened_at: sidecar_xmp::utc_now() });
    u.recent.truncate(50);
    app.store.put("users", &user.id, &u).await?;
    Ok(key)
}

async fn new_pick(app: &App, user: &User, query: String, candidates: Vec<Candidate>) -> Result<String> {
    let token = random_hex(16);
    app.store.put("picks", &token, &PickRecord { user_id: user.id.clone(), query, candidates }).await?;
    Ok(token)
}

/// `GET /open?sha1=…&name=…&taken=…&size=…` (the iOS Shortcut), or `?path=…` where allowed.
async fn open(State(app): State<Arc<App>>, PageUser(user): PageUser, Query(q): Query<OpenQuery>) -> Response {
    let outcome = match resolve::resolve(&app, &user, &q).await {
        Ok(r) => r,
        Err(e) => return error_page(StatusCode::BAD_GATEWAY, &format!("{e:#}")),
    };
    let query = q.name.clone().unwrap_or_default();
    match outcome {
        Resolved::Found(c) if !sidecar_template::is_raw(&c.path) && !c.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("dng")) => error_page(
            StatusCode::UNPROCESSABLE_ENTITY,
            &format!("Sidecar edits RAW files. {} isn't one — edit it in Immich instead.", c.file_name),
        ),
        Resolved::Found(c) => match admit(&app, &user, c).await {
            Ok(key) => Redirect::to(&format!("/edit/{key}")).into_response(),
            Err(e) => error_page(StatusCode::UNPROCESSABLE_ENTITY, &format!("{e:#}")),
        },
        Resolved::Choose(list) => match new_pick(&app, &user, query, list).await {
            Ok(t) => Redirect::to(&format!("/pick/{t}")).into_response(),
            Err(e) => internal(e),
        },
        Resolved::NotFound => match new_pick(&app, &user, query, Vec::new()).await {
            Ok(t) => Redirect::to(&format!("/pick/{t}")).into_response(),
            Err(e) => internal(e),
        },
    }
}

#[derive(Deserialize)]
struct PickQuery {
    q: Option<String>,
}

async fn get_pick(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(token): Path<String>, Query(pq): Query<PickQuery>) -> Response {
    let Some(mut rec) = app.store.get::<PickRecord>("picks", &token).await.filter(|r| r.user_id == user.id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if let Some(q) = pq.q.filter(|q| !q.trim().is_empty()) {
        match resolve::search_by_name(&app, &user, &q).await {
            Ok(list) => {
                rec.query = q;
                rec.candidates = list;
                if let Err(e) = app.store.put("picks", &token, &rec).await {
                    return internal(e);
                }
            }
            Err(e) => return (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
        }
    }
    Json(serde_json::json!({ "query": rec.query, "candidates": rec.candidates })).into_response()
}

#[derive(Deserialize)]
struct PickChoice {
    index: usize,
}

async fn choose_pick(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(token): Path<String>, Json(choice): Json<PickChoice>) -> Response {
    let Some(rec) = app.store.get::<PickRecord>("picks", &token).await.filter(|r| r.user_id == user.id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(c) = rec.candidates.get(choice.index).cloned() else { return StatusCode::BAD_REQUEST.into_response() };
    match admit(&app, &user, c).await {
        Ok(key) => {
            app.store.delete("picks", &token).await;
            Json(serde_json::json!({ "assetKey": key })).into_response()
        }
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
    }
}

async fn get_settings(State(app): State<Arc<App>>, ApiUser(user): ApiUser) -> Response {
    let u: UserRecord = app.store.get("users", &user.id).await.unwrap_or_default();
    Json(u.settings).into_response()
}

async fn put_settings(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Json(settings): Json<Settings>) -> Response {
    let mut u: UserRecord = app.store.get("users", &user.id).await.unwrap_or_default();
    u.settings = settings.clone();
    match app.store.put("users", &user.id, &u).await {
        Ok(()) => Json(settings).into_response(),
        Err(e) => internal(e),
    }
}

async fn recent(State(app): State<Arc<App>>, ApiUser(user): ApiUser) -> Response {
    let u: UserRecord = app.store.get("users", &user.id).await.unwrap_or_default();
    Json(u.recent).into_response()
}

async fn thumbnail(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(id): Path<String>) -> Response {
    if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return StatusCode::NOT_FOUND.into_response();
    }
    match app.immich.thumbnail(&user.api_key, &id).await {
        Ok(r) => {
            let ct = r.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("image/jpeg").to_string();
            ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, "private, max-age=86400".into())], Body::from_stream(r.bytes_stream())).into_response()
        }
        Err(e) => (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    }
}

fn valid_key(key: &str) -> bool {
    key.len() == 40 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The asset record, if this user has opened the photo.
pub(crate) async fn asset_for(app: &App, user: &User, key: &str) -> Option<AssetRecord> {
    if !valid_key(key) {
        return None;
    }
    app.store.get::<AssetRecord>("assets", key).await.filter(|r| r.users.contains(&user.id))
}

pub(crate) async fn relay(resp: reqwest::Result<reqwest::Response>) -> Response {
    let resp = match resp {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("The worker didn't answer: {e}")).into_response(),
    };
    let mut out = Response::builder().status(resp.status().as_u16());
    for name in ["content-type", "x-width", "x-height", "x-process-version"] {
        if let Some(v) = resp.headers().get(name) {
            out = out.header(name, v.as_bytes());
        }
    }
    out.body(Body::from_stream(resp.bytes_stream())).unwrap()
}

async fn meta(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(key): Path<String>) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else { return StatusCode::NOT_FOUND.into_response() };
    // The worker forgets paths on restart; re-ingesting is cheap when the metadata is cached.
    let _ = app.http.post(app.worker("/v1/ingest")).json(&serde_json::json!({ "path": rec.path })).send().await;
    relay(app.http.get(app.worker(&format!("/v1/assets/{key}/meta"))).send().await).await
}

#[derive(Deserialize)]
struct ProxyQuery {
    long_edge: Option<u32>,
}

async fn proxy(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(key): Path<String>, Query(q): Query<ProxyQuery>) -> Response {
    if asset_for(&app, &user, &key).await.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let edge = q.long_edge.unwrap_or(2560);
    let mut resp = relay(app.http.get(app.worker(&format!("/v1/assets/{key}/proxy?long_edge={edge}"))).send().await).await;
    resp.headers_mut().insert(header::CACHE_CONTROL, "private, max-age=86400".parse().unwrap());
    resp
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EditState {
    edit: EditParams,
    etag: String,
    read_only: bool,
    exports: Vec<serde_json::Value>,
}

async fn get_edit(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(key): Path<String>) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else { return StatusCode::NOT_FOUND.into_response() };
    let loaded = tokio::task::spawn_blocking(move || sidecar_xmp::SidecarDoc::load(&rec.path)).await;
    match loaded {
        Ok(Ok(doc)) => {
            let state = EditState { edit: doc.edit(), etag: doc.etag(), read_only: doc.is_read_only(), exports: doc.exports() };
            ([(header::ETAG, format!("\"{}\"", state.etag))], Json(state)).into_response()
        }
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e.into()),
    }
}

async fn put_edit(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(key): Path<String>, headers: HeaderMap, Json(edit): Json<EditParams>) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else { return StatusCode::NOT_FOUND.into_response() };
    let Some(if_match) = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()).map(|v| v.trim_matches('"').to_string()) else {
        return (StatusCode::PRECONDITION_REQUIRED, "Send If-Match with the ETag you edited from.").into_response();
    };
    let roots = app.config.canonical_mounts();
    let moves_files = app.config.root_of(&rec.path).is_some_and(|(r, _)| r.moves_files);
    let path = rec.path.clone();
    let result = tokio::task::spawn_blocking(move || {
        let created = !sidecar_xmp::sidecar_path_for(&path).exists();
        let mut conflict = None;
        let saved = sidecar_xmp::update(&path, &roots, |doc| {
            if doc.etag() != if_match {
                conflict = Some(EditState { edit: doc.edit(), etag: doc.etag(), read_only: doc.is_read_only(), exports: doc.exports() });
                anyhow::bail!("conflict");
            }
            doc.set_edit(&edit, &key)
        });
        (saved, conflict, created)
    })
    .await;
    match result {
        Ok((_, Some(current), _)) => (StatusCode::PRECONDITION_FAILED, [(header::ETAG, format!("\"{}\"", current.etag))], Json(current)).into_response(),
        Ok((Ok(etag), None, created)) => {
            if created && moves_files {
                app.queue_sidecar_discovery();
            }
            ([(header::ETAG, format!("\"{etag}\""))], Json(serde_json::json!({ "etag": etag }))).into_response()
        }
        Ok((Err(e), None, _)) => {
            let readonly = e.downcast_ref::<sidecar_xmp::SidecarError>().is_some_and(|s| matches!(s, sidecar_xmp::SidecarError::NewerProcessVersion { .. }));
            if readonly { (StatusCode::CONFLICT, format!("{e}")).into_response() } else { internal(e) }
        }
        Err(e) => internal(e.into()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoupeBody {
    center: [f32; 2],
    size: [u32; 2],
    edit: EditParams,
    #[serde(default)]
    color_space: sidecar_color::DisplaySpace,
}

/// `POST /api/assets/{key}/loupe` — a 1:1 tile with denoise and sharpening applied, as PNG.
async fn loupe(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(key): Path<String>, Json(body): Json<LoupeBody>) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else { return StatusCode::NOT_FOUND.into_response() };
    let req = serde_json::json!({ "key": key, "path": rec.path, "center": body.center, "size": body.size, "edit": body.edit, "colorSpace": body.color_space });
    relay(app.http.post(app.worker("/v1/loupe")).json(&req).send().await).await
}

pub fn router(app: Arc<App>) -> Router {
    let web = &app.config.server.web_root;
    let spa = ServeDir::new(web).fallback(ServeFile::new(web.join("index.html")));
    Router::new()
        .route("/open", get(open))
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/session", get(session::session).post(session::sign_in).delete(session::sign_out))
        .route("/api/recent", get(recent))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/picks/{token}", get(get_pick).post(choose_pick))
        .route("/api/immich/assets/{id}/thumbnail", get(thumbnail))
        .route("/api/assets/{key}/meta", get(meta))
        .route("/api/assets/{key}/proxy", get(proxy))
        .route("/api/assets/{key}/edit", get(get_edit).put(put_edit))
        .route("/api/assets/{key}/loupe", axum::routing::post(loupe))
        .route("/api/exports", axum::routing::post(export::create))
        .route("/api/exports/{id}", get(export::status))
        .route("/api/exports/{id}/events", get(export::events))
        .route("/api/exports/{id}/file", get(export::file))
        .fallback_service(spa)
        // Proxies compress with brotli for Safari and zstd for Chromium.
        .layer(CompressionLayer::new().br(true).zstd(true).gzip(true).compress_when(tower_http::compression::predicate::SizeAbove::new(1024).and(tower_http::compression::predicate::NotForContentType::new("text/event-stream"))))
        .with_state(app)
}
