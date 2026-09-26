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

    /// Hourly: drop finished export jobs, picker tokens nobody came back to, and sessions past their cookie's
    /// one-year lifetime. None of these were ever removed otherwise.
    fn start_housekeeping(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            loop {
                let jobs = app.jobs.sweep();
                let picks = app.store.sweep("picks", Duration::from_secs(24 * 3600)).await;
                let sessions = app.store.sweep("sessions", Duration::from_secs(366 * 24 * 3600)).await;
                if jobs + picks + sessions > 0 {
                    tracing::info!("housekeeping: dropped {jobs} finished jobs, {picks} picker tokens, {sessions} expired sessions");
                }
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        });
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

fn cant_open(file_name: &str) -> String {
    format!("Sidecar can't open {file_name} — it edits RAW files, JPEG, HEIC, PNG and TIFF.")
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
    let rec: AssetRecord = app
        .store
        .update("assets", &key, |found: Option<AssetRecord>| {
            let mut rec = found.unwrap_or(AssetRecord { path: c.path.clone(), file_name: c.file_name.clone(), immich_asset_id: None, users: Vec::new() });
            rec.path = c.path;
            rec.immich_asset_id = c.immich_asset_id.or(rec.immich_asset_id);
            if !rec.users.contains(&user.id) {
                rec.users.push(user.id.clone());
            }
            rec
        })
        .await?;
    let item = RecentItem { asset_key: key.clone(), file_name: rec.file_name, immich_asset_id: rec.immich_asset_id, opened_at: sidecar_xmp::utc_now() };
    app.store
        .update("users", &user.id, |found: Option<UserRecord>| {
            let mut u = found.unwrap_or_default();
            u.recent.retain(|r| r.asset_key != key);
            u.recent.insert(0, item);
            u.recent.truncate(50);
            u
        })
        .await?;
    Ok(key)
}

/// The largest photo `POST /api/intake` will take. A 100-megapixel 16-bit TIFF is around 600 MB; this leaves
/// room for one and refuses a video someone dropped on the page by mistake before it is read into memory.
const INTAKE_LIMIT: usize = 1 << 30;

/// How long to wait for Immich to put an uploaded photo where its storage template says it goes. Upload
/// returns as soon as the bytes land, and metadata extraction — which is what reads the capture date the
/// template is keyed on — runs after. Until it has, the asset's `originalPath` points somewhere Sidecar
/// either can't see or would open only to have the file move out from under it.
const INTAKE_SETTLE: Duration = Duration::from_secs(20);

/// `POST /api/intake` (multipart) — a photo chosen on the device being edited on, rather than one already in
/// the library. It goes into Immich first and is then opened exactly like any other asset: the edit belongs
/// in an XMP beside an original, so the original has to become a file in a library root before there is
/// anywhere to put it. Nothing about the editor downstream knows the photo arrived this way.
async fn intake(State(app): State<Arc<App>>, ApiUser(user): ApiUser, mut form: axum::extract::Multipart) -> Response {
    if !app.immich.enabled() {
        return (StatusCode::NOT_IMPLEMENTED, "This Sidecar has no Immich library to put a photo into, so it can only open photos already on the server.").into_response();
    }
    let mut file: Option<(String, Vec<u8>)> = None;
    loop {
        match form.next_field().await {
            Ok(Some(field)) => {
                if field.name() != Some("file") {
                    continue;
                }
                let name = field.file_name().map(str::to_string).unwrap_or_default();
                match field.bytes().await {
                    Ok(b) => file = Some((name, b.to_vec())),
                    Err(e) => return (StatusCode::BAD_REQUEST, format!("reading the upload: {e}")).into_response(),
                }
            }
            Ok(None) => break,
            Err(e) => return (StatusCode::BAD_REQUEST, format!("reading the upload: {e}")).into_response(),
        }
    }
    let Some((file_name, bytes)) = file.filter(|(n, b)| !n.is_empty() && !b.is_empty()) else {
        return (StatusCode::BAD_REQUEST, "No photo in the request.").into_response();
    };
    // The same check `/open` makes, before anything is uploaded: a file Sidecar can't edit shouldn't be put
    // into someone's library by an editor that then refuses to open it.
    if !sidecar_template::is_editable(std::path::Path::new(&file_name)) {
        return (StatusCode::UNPROCESSABLE_ENTITY, cant_open(&file_name)).into_response();
    }

    let id = match app.immich.upload(&user.api_key, &file_name, bytes, &sidecar_xmp::utc_now()).await {
        Ok(id) => id,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
    };

    // Poll rather than sleep once: a duplicate is already filed and resolves on the first try, while a large
    // RAW on a busy server can take several seconds to be moved into place.
    let deadline = Instant::now() + INTAKE_SETTLE;
    let mut last_path;
    loop {
        match app.immich.asset(&user.api_key, &id).await {
            Ok(asset) => {
                last_path = asset.original_path.clone();
                if let Some(c) = resolve::from_asset(&app, &asset) {
                    return match admit(&app, &user, c).await {
                        Ok(key) => Json(serde_json::json!({ "assetKey": key })).into_response(),
                        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
                    };
                }
            }
            Err(e) => return (StatusCode::BAD_GATEWAY, format!("{e:#}")).into_response(),
        }
        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    // Uploaded and safe, just not openable from here — say which, since the two causes need different fixes.
    let detail = if last_path.is_empty() {
        "Immich hasn't finished filing it yet.".to_string()
    } else {
        format!("Immich put it at {last_path}, which isn't inside a library Sidecar is configured for.")
    };
    (StatusCode::BAD_GATEWAY, format!("{file_name} is in your library, but Sidecar can't open it yet. {detail} Try opening it from Immich in a moment.")).into_response()
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
        Resolved::Found(c) if !sidecar_template::is_editable(&c.path) => error_page(StatusCode::UNPROCESSABLE_ENTITY, &cant_open(&c.file_name)),
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
    let saved = app
        .store
        .update("users", &user.id, |found: Option<UserRecord>| UserRecord { settings: settings.clone(), ..found.unwrap_or_default() })
        .await;
    match saved {
        Ok(_) => Json(settings).into_response(),
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
///
/// The stored path can stop being true while the record stays valid: Immich moves a file in its internal
/// library whenever the storage template's inputs change, and a record written before `immich_prefix` was
/// understood holds an Immich path that was never mapped to Sidecar's mount at all (five of those are still
/// in this server's state directory). The asset id finds the photo either way, so a path that no longer
/// exists is re-resolved through Immich once and written back, rather than every route downstream failing
/// with "No such file or directory" until the photo is opened from Immich again.
pub(crate) async fn asset_for(app: &App, user: &User, key: &str) -> Option<AssetRecord> {
    if !valid_key(key) {
        return None;
    }
    let rec: AssetRecord = app.store.get("assets", key).await.filter(|r: &AssetRecord| r.users.contains(&user.id))?;
    if rec.path.is_file() {
        return Some(rec);
    }
    let asset = app.immich.asset(&user.api_key, rec.immich_asset_id.as_deref()?).await.ok()?;
    let found = resolve::from_asset(app, &asset)?;
    tracing::info!("{} is gone; Immich has it at {}", rec.path.display(), found.path.display());
    let fixed = AssetRecord { path: found.path, file_name: found.file_name, ..rec };
    // Through `update`, so a user added by a concurrent `admit` isn't written back out of the record.
    let (path, file_name) = (fixed.path.clone(), fixed.file_name.clone());
    let _ = app.store.update("assets", key, |now: Option<AssetRecord>| AssetRecord { path, file_name, ..now.unwrap_or(fixed.clone()) }).await;
    Some(fixed)
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
    // The worker forgets paths on restart. Re-ingesting is cheap: it re-hashes only a file it hasn't seen.
    let _ = app.http.post(app.worker("/v1/ingest")).json(&serde_json::json!({ "path": rec.path })).send().await;
    relay(app.http.get(app.worker(&format!("/v1/assets/{key}/meta"))).send().await).await
}

#[derive(Deserialize)]
struct ProxyQuery {
    long_edge: Option<u32>,
}

/// A proxy or focus mask from the worker, for a photo this user has opened.
///
/// The worker forgets which path an asset key came from when it restarts, and answers "unknown asset; ingest
/// it first" until something re-ingests it — which an open editor would see as a broken photo. So a failure
/// is retried once after re-ingesting. `meta` re-ingests up front instead; both are cheap once the worker has
/// hashed the file, and cost one hash after a restart.
///
/// Both are keyed by content hash, so a success is cached for a day. A failure is not: the browser would
/// go on showing a broken photo long after the worker recovered.
async fn pixels(app: &Arc<App>, user: &User, key: &str, worker_path: String) -> Response {
    let Some(rec) = asset_for(app, user, key).await else { return StatusCode::NOT_FOUND.into_response() };
    let fetch = || async { relay(app.http.get(app.worker(&worker_path)).send().await).await };
    let mut resp = fetch().await;
    if !resp.status().is_success() {
        let ingested = app.http.post(app.worker("/v1/ingest")).json(&serde_json::json!({ "path": rec.path })).send().await;
        if ingested.is_ok_and(|r| r.status().is_success()) {
            tracing::info!("re-ingested {} after the worker forgot it", rec.path.display());
            resp = fetch().await;
        }
    }
    if resp.status().is_success() {
        resp.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static("private, max-age=86400"));
    }
    resp
}

async fn proxy(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(key): Path<String>, Query(q): Query<ProxyQuery>) -> Response {
    let edge = q.long_edge.unwrap_or(2560);
    pixels(&app, &user, &key, format!("/v1/assets/{key}/proxy?long_edge={edge}")).await
}

#[derive(Deserialize)]
struct FocusQuery {
    edge: Option<u32>,
}

/// Focus-peaking mask: PNG bytes at the same edge as a proxy, so the client can pair them up.
async fn focus(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(key): Path<String>, Query(q): Query<FocusQuery>) -> Response {
    let edge = q.edge.unwrap_or(2560);
    pixels(&app, &user, &key, format!("/v1/assets/{key}/focus?edge={edge}")).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EditState {
    edit: EditParams,
    etag: String,
    read_only: bool,
    /// False when the photo's folder can't take a sidecar, so the editor can say so before the edit is made
    /// rather than at the first save. Advisory — see `sidecar_xmp::writable_beside`.
    writable: bool,
    exports: Vec<serde_json::Value>,
}

async fn get_edit(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(key): Path<String>) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else { return StatusCode::NOT_FOUND.into_response() };
    let loaded = tokio::task::spawn_blocking(move || {
        let writable = sidecar_xmp::writable_beside(&rec.path);
        sidecar_xmp::SidecarDoc::load(&rec.path).map(|d| (d, writable))
    })
    .await;
    match loaded {
        Ok(Ok((doc, writable))) => {
            let state = EditState { edit: doc.edit(), etag: doc.etag(), read_only: doc.is_read_only(), writable, exports: doc.exports() };
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
        let writable = sidecar_xmp::writable_beside(&path);
        let mut conflict = None;
        let saved = sidecar_xmp::update(&path, &roots, |doc| {
            if doc.etag() != if_match {
                conflict = Some(EditState { edit: doc.edit(), etag: doc.etag(), read_only: doc.is_read_only(), writable, exports: doc.exports() });
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
        // The two failures the person can act on carry their own message through; anything else is a bug
        // here and goes to the log as one.
        Ok((Err(e), None, _)) => match e.downcast_ref::<sidecar_xmp::SidecarError>() {
            Some(sidecar_xmp::SidecarError::NewerProcessVersion { .. }) => (StatusCode::CONFLICT, format!("{e}")).into_response(),
            Some(sidecar_xmp::SidecarError::NotWritable { .. }) => {
                tracing::error!("{e:#}");
                (StatusCode::FORBIDDEN, format!("{e}")).into_response()
            }
            _ => internal(e),
        },
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewBody {
    edit: EditParams,
    #[serde(default)]
    color_space: sidecar_color::DisplaySpace,
    long_edge: Option<u32>,
}

/// `POST /api/assets/{key}/preview` — the whole photo developed on the server, as JPEG, for browsers that
/// can't run the develop pass themselves.
async fn preview(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(key): Path<String>, Json(body): Json<PreviewBody>) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else { return StatusCode::NOT_FOUND.into_response() };
    let req = serde_json::json!({ "key": key, "path": rec.path, "edit": body.edit, "colorSpace": body.color_space, "longEdge": body.long_edge });
    relay(app.http.post(app.worker("/v1/preview")).json(&req).send().await).await
}

pub fn router(app: Arc<App>) -> Router {
    app.start_housekeeping();
    let web = &app.config.server.web_root;
    let spa = ServeDir::new(web).fallback(ServeFile::new(web.join("index.html")));
    Router::new()
        .route("/open", get(open))
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/session", get(session::session).post(session::sign_in).delete(session::sign_out))
        .route("/api/recent", get(recent))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/picks/{token}", get(get_pick).post(choose_pick))
        .route("/api/intake", axum::routing::post(intake).layer(axum::extract::DefaultBodyLimit::max(INTAKE_LIMIT)))
        .route("/api/immich/assets/{id}/thumbnail", get(thumbnail))
        .route("/api/assets/{key}/meta", get(meta))
        .route("/api/assets/{key}/proxy", get(proxy))
        .route("/api/assets/{key}/focus", get(focus))
        .route("/api/assets/{key}/edit", get(get_edit).put(put_edit))
        .route("/api/assets/{key}/loupe", axum::routing::post(loupe))
        .route("/api/assets/{key}/preview", axum::routing::post(preview))
        .route("/api/exports", axum::routing::post(export::create))
        .route("/api/exports/{id}", get(export::status))
        .route("/api/exports/{id}/events", get(export::events))
        .route("/api/exports/{id}/file", get(export::file))
        .fallback_service(spa)
        // Proxies compress with brotli for Safari and zstd for Chromium.
        .layer(CompressionLayer::new().br(true).zstd(true).gzip(true).compress_when(tower_http::compression::predicate::SizeAbove::new(1024).and(tower_http::compression::predicate::NotForContentType::new("text/event-stream"))))
        .with_state(app)
}
