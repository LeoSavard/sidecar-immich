// SPDX-License-Identifier: Apache-2.0
//! Public HTTP surface. The worker is reached over the internal network only.

pub mod config;
pub mod export;
pub mod immich;
mod lensfun;
pub mod resolve;
pub mod session;
pub mod store;

use anyhow::{Context, Result};
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
use sidecar_xmp::{Commit, SidecarDoc};
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
            Some(p) => Some(
                std::fs::read_to_string(p)
                    .map(|s| s.trim().to_string())
                    .map_err(|e| anyhow::anyhow!("reading {}: {e}", p.display()))?,
            ),
            None => None,
        };
        let immich = immich::Immich {
            base: config.immich.url.clone(),
            http: http.clone(),
        };
        Ok(Arc::new(Self {
            config,
            http,
            immich,
            store,
            secret,
            admin_key,
            jobs: export::Jobs::default(),
            discovery: Mutex::new(Discovery::default()),
        }))
    }

    pub(crate) fn worker(&self, path: &str) -> String {
        format!(
            "{}{}",
            self.config.server.worker_url.trim_end_matches('/'),
            path
        )
    }

    /// Hand the worker a photo's path; it answers with the photo's metadata. It forgets paths when it
    /// restarts, so anything that asks it about a photo may need to do this again first. Cheap after the
    /// first time: the worker only hashes a file it hasn't seen.
    pub(crate) async fn ingest(
        &self,
        path: &std::path::Path,
    ) -> reqwest::Result<reqwest::Response> {
        self.http
            .post(self.worker("/v1/ingest"))
            .json(&serde_json::json!({ "path": path }))
            .send()
            .await
    }

    /// Hourly: drop finished export jobs, picker tokens nobody came back to, and sessions past their cookie's
    /// one-year lifetime. None of these were ever removed otherwise.
    fn start_housekeeping(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            loop {
                let jobs = app.jobs.sweep();
                let picks = app
                    .store
                    .sweep("picks", Duration::from_secs(24 * 3600))
                    .await;
                let sessions = app
                    .store
                    .sweep("sessions", Duration::from_secs(366 * 24 * 3600))
                    .await;
                if jobs + picks + sessions > 0 {
                    tracing::info!(
                        "housekeeping: dropped {jobs} finished jobs, {picks} picker tokens, {sessions} expired sessions"
                    );
                }
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        });
    }

    /// Ask Immich to discover a newly created internal-library sidecar: at most one run an hour.
    fn queue_sidecar_discovery(self: &Arc<Self>) {
        let Some(key) = self.admin_key.clone() else {
            return;
        };
        let delay = {
            let mut d = self.discovery.lock().unwrap();
            if d.pending {
                return;
            }
            d.pending = true;
            d.last.map_or(Duration::ZERO, |l| {
                Duration::from_secs(3600).saturating_sub(l.elapsed())
            })
        };
        let app = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            match app.immich.discover_missing_sidecars(&key).await {
                Ok(()) => tracing::info!("started Immich sidecar discovery"),
                Err(e) => tracing::warn!(
                    "Immich sidecar discovery failed — an internal-library sidecar may not move with its original: {e:#}"
                ),
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
        Self {
            output_space: sidecar_color::DisplaySpace::Srgb,
            clip_highlights: false,
            clip_shadows: false,
        }
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
    /// The checksum `/open` was asked for, when it was given one. A photo shared from the phone carries it,
    /// and the share target kept the file under it: with this, the pick page can offer to import that file
    /// when nothing matched. Absent in records made before it existed.
    #[serde(default)]
    sha1: Option<String>,
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
    let resp = app.ingest(&c.path).await?;
    if !resp.status().is_success() {
        anyhow::bail!("{}", resp.text().await.unwrap_or_default());
    }
    let meta: serde_json::Value = resp.json().await?;
    let key = meta["assetKey"].as_str().unwrap_or_default().to_string();
    let rec: AssetRecord = app
        .store
        .update("assets", &key, |found: Option<AssetRecord>| {
            let mut rec = found.unwrap_or(AssetRecord {
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
            rec
        })
        .await?;
    let item = RecentItem {
        asset_key: key.clone(),
        file_name: rec.file_name,
        immich_asset_id: rec.immich_asset_id,
        opened_at: sidecar_xmp::utc_now(),
    };
    app.store
        .update("users", &user.id, |found: Option<UserRecord>| {
            let mut u = found.unwrap_or_default();
            u.recent.retain(|r| r.asset_key != key);
            u.recent.insert(0, item);
            u.recent.truncate(RECENT_KEPT);
            u
        })
        .await?;
    Ok(key)
}

/// How many photographs stay in Recent. It is a way back to what you were working on, not a library view:
/// past this the list is scrolling rather than remembering, and every row costs a thumbnail to render.
const RECENT_KEPT: usize = 40;

/// The largest photo `POST /api/intake` will take. A 100-megapixel 16-bit TIFF is around 600 MB; this leaves
/// room for one and refuses a video someone dropped on the page by mistake before it is read into memory.
const INTAKE_LIMIT: usize = 1 << 30;

/// How long to wait for Immich to put an uploaded photo where its storage template says it goes. Upload
/// returns as soon as the bytes land, and metadata extraction — which is what reads the capture date the
/// template is keyed on — runs after. Until it has, the asset's `originalPath` points somewhere Sidecar
/// either can't see or would open only to have the file move out from under it.
const INTAKE_SETTLE: Duration = Duration::from_secs(20);

/// Where `POST /api/intake` puts the photo. Absent means the library, which is what it always did.
#[derive(Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
enum IntakeTo {
    #[default]
    Immich,
    /// The editing folder: a configured root Immich knows nothing about (DESIGN.md §3).
    Folder,
}

#[derive(Deserialize, Default)]
struct IntakeQuery {
    #[serde(default)]
    to: IntakeTo,
}

/// `POST /api/intake` (multipart) — a photo chosen on the device being edited on, rather than one already in
/// the library. The edit belongs in an XMP beside an original, so the original has to become a file in a root
/// before there is anywhere to put it; `?to=` says which root. `immich` uploads it to the library and opens
/// what Immich filed, `folder` writes it into the editing folder and Immich never sees it. Either way nothing
/// about the editor downstream knows the photo arrived this way.
async fn intake(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Query(q): Query<IntakeQuery>,
    mut form: axum::extract::Multipart,
) -> Response {
    let folder = app.config.intake_root();
    if q.to == IntakeTo::Immich && !app.immich.enabled() {
        let detail = if folder.is_some() {
            "This Sidecar has no Immich library. Import the photo into the editing folder instead."
        } else {
            "This Sidecar has no Immich library to put a photo into, so it can only open photos already on the server."
        };
        return (StatusCode::NOT_IMPLEMENTED, detail).into_response();
    }
    if q.to == IntakeTo::Folder && folder.is_none() {
        return (StatusCode::NOT_IMPLEMENTED, "This Sidecar has no editing folder configured, so an imported photo has nowhere to go that isn't the library.").into_response();
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
                    Err(e) => {
                        return (StatusCode::BAD_REQUEST, format!("reading the upload: {e}"))
                            .into_response();
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, format!("reading the upload: {e}"))
                    .into_response();
            }
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

    if q.to == IntakeTo::Folder {
        let root = folder.expect("checked above");
        return match into_folder(&app, &user, root, &file_name, bytes).await {
            Ok(key) => Json(serde_json::json!({ "assetKey": key })).into_response(),
            Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
        };
    }

    let id = match app
        .immich
        .upload(&user.api_key, &file_name, bytes, &sidecar_xmp::utc_now())
        .await
    {
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
                        Err(e) => {
                            (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response()
                        }
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
        format!(
            "Immich put it at {last_path}, which isn't inside a library Sidecar is configured for."
        )
    };
    (StatusCode::BAD_GATEWAY, format!("{file_name} is in your library, but Sidecar can't open it yet. {detail} Try opening it from Immich in a moment.")).into_response()
}

/// One photograph in the editing folder, for the list the home page shows and the room check the import
/// makes. `exported` is what decides the order things are removed in: a render of it exists somewhere else,
/// so the folder is no longer the only copy of that work.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderItem {
    asset_key: String,
    file_name: String,
    /// Relative to the root, so the list can show where in the folder it sits.
    at: String,
    bytes: u64,
    /// Newest first in the listing; also what breaks a tie between two exported photographs.
    modified_unix: i64,
    exported: bool,
    edited: bool,
}

/// What the editing folder holds, newest first. Every original under the root, the staging folder aside.
///
/// Each file is hashed to get its asset key, which is what the editor opens it by. That is only affordable
/// because the folder is small by construction — `intake_limit` is the reason this can be a plain walk with
/// no index behind it.
async fn folder_items(root: &config::Root) -> Result<Vec<FolderItem>> {
    let mut found = Vec::new();
    collect_originals(&root.mount, &root.mount.join(STAGING), &mut found).await?;
    let mut items = Vec::new();
    for path in found {
        let Some(file_name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let meta = tokio::fs::metadata(&path).await?;
        let modified_unix = meta
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        let sidecar = sidecar_xmp::sidecar_path_for(&path);
        let (exported, edited) = match SidecarDoc::load(&sidecar) {
            Ok(doc) => (!doc.exports().is_empty(), doc.source_sha1().is_some()),
            Err(_) => (false, false),
        };
        let Some(asset_key) = resolve::sha1_of(path.clone()).await else {
            continue;
        };
        items.push(FolderItem {
            asset_key,
            file_name,
            at: path
                .strip_prefix(&root.mount)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned(),
            bytes: meta.len(),
            modified_unix,
            exported,
            edited,
        });
    }
    items.sort_by_key(|i| std::cmp::Reverse(i.modified_unix));
    Ok(items)
}

/// Every editable original under `dir`, skipping `staging` — a half-written import is not in the folder yet.
async fn collect_originals(
    dir: &std::path::Path,
    staging: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) -> Result<()> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&at).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path == staging {
                continue;
            }
            match entry.file_type().await {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(t) if t.is_file() && sidecar_template::is_editable(&path) => out.push(path),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Which photograph goes when the folder is full: the oldest one that has been exported. A photograph that
/// has never been rendered anywhere else is never chosen — the folder is the only copy of that work, and
/// losing it to make room for the next import would be the editor deleting someone's negative. When nothing
/// has been exported the import is refused instead, and the person decides what to let go of.
fn evictable(items: &[FolderItem]) -> Option<&FolderItem> {
    items
        .iter()
        .filter(|i| i.exported)
        .min_by_key(|i| i.modified_unix)
}

/// `GET /api/intake/folder` — what the editing folder holds and how much room is left.
async fn folder(State(app): State<Arc<App>>, ApiUser(_): ApiUser) -> Response {
    let Some(root) = app.config.intake_root() else {
        return Json(serde_json::json!({ "configured": false })).into_response();
    };
    match folder_items(root).await {
        Ok(items) => {
            let next_out = evictable(&items).map(|i| i.file_name.clone());
            Json(serde_json::json!({
                "configured": true,
                "name": root.name,
                "limit": root.intake_limit,
                "full": items.len() >= root.intake_limit,
                "nextOut": next_out,
                "items": items,
            }))
            .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// `DELETE /api/intake/folder/{key}` — take one photograph out of the editing folder: the original, its
/// sidecar, and the record that it was ever opened. This is the one place Sidecar deletes a photograph, so
/// it is confined to the intake root by `remove_original`, and the asset key has to match a file actually
/// in it — a key is not a path, so nothing outside the folder is nameable here.
async fn forget(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
) -> Response {
    let Some(root) = app.config.intake_root() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "This Sidecar has no editing folder.",
        )
            .into_response();
    };
    let items = match folder_items(root).await {
        Ok(i) => i,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    };
    let Some(item) = items.iter().find(|i| i.asset_key == key) else {
        return (
            StatusCode::NOT_FOUND,
            "That photo isn't in the editing folder.",
        )
            .into_response();
    };
    let path = root.mount.join(&item.at);
    if let Err(e) = remove_original(&app, root, &path).await {
        return (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response();
    }
    app.store.delete("assets", &key).await;
    let _ = app
        .store
        .update("users", &user.id, |found: Option<UserRecord>| {
            let mut u = found.unwrap_or_default();
            u.recent.retain(|r| r.asset_key != key);
            u
        })
        .await;
    Json(serde_json::json!({ "removed": item.file_name })).into_response()
}

/// Delete an original and its sidecar, having first proved the file really is inside the editing folder.
/// Both sidecar spellings go: `DSC_1234.NEF.xmp` and `DSC_1234.xmp` (`sidecar_path_for`).
async fn remove_original(
    app: &Arc<App>,
    root: &config::Root,
    path: &std::path::Path,
) -> Result<()> {
    let mount = root.mount.canonicalize()?;
    let real = path.canonicalize()?;
    if !real.starts_with(&mount) || !sidecar_template::is_editable(&real) {
        anyhow::bail!(
            "{} is not a photo inside the editing folder",
            real.display()
        );
    }
    // Not the intake of some other root: only the folder this Sidecar imports into may be deleted from.
    if !app
        .config
        .intake_root()
        .is_some_and(|r| r.name == root.name)
    {
        anyhow::bail!("{} is not the editing folder", root.name);
    }
    for sidecar in [
        std::path::PathBuf::from(format!("{}.xmp", real.display())),
        real.with_extension("xmp"),
    ] {
        if sidecar.is_file() {
            tokio::fs::remove_file(&sidecar).await.ok();
        }
    }
    tokio::fs::remove_file(&real)
        .await
        .with_context(|| format!("removing {}", real.display()))?;
    prune_empty_dirs(&mount, real.parent()).await;
    Ok(())
}

/// Walk back up from a deleted photo's folder, removing directories the template made that now hold
/// nothing. Stops at the root itself, which stays whether or not it is empty.
async fn prune_empty_dirs(mount: &std::path::Path, from: Option<&std::path::Path>) {
    let mut at = from.map(std::path::Path::to_path_buf);
    while let Some(dir) = at {
        if dir == mount || !dir.starts_with(mount) {
            return;
        }
        match tokio::fs::read_dir(&dir).await {
            Ok(mut entries) => match entries.next_entry().await {
                Ok(None) => {}
                _ => return,
            },
            Err(_) => return,
        }
        if tokio::fs::remove_dir(&dir).await.is_err() {
            return;
        }
        at = dir.parent().map(std::path::Path::to_path_buf);
    }
}

/// Where a photo lands while its capture date is being read: inside the root, so the move into place is a
/// rename on the same filesystem rather than a copy, and hidden so a folder listing doesn't show half-imports.
const STAGING: &str = ".incoming";

/// A photo chosen on the editing device, written into the editing folder and opened from there — no upload,
/// no Immich, nothing to wait for a library to file. Immich's own intake is what settles a photo's path, and
/// there is none here, so the path is settled the same way a library would: by the capture date, through the
/// root's storage template. That date is in the file, which only the worker can read, so the bytes are
/// written to a staging name first, ingested to learn the date, and then renamed into place.
async fn into_folder(
    app: &Arc<App>,
    user: &User,
    root: &config::Root,
    file_name: &str,
    bytes: Vec<u8>,
) -> Result<String> {
    let staging = root.mount.join(STAGING);
    tokio::fs::create_dir_all(&staging)
        .await
        .with_context(|| format!("making {}", staging.display()))?;
    let temp = staging.join(format!("{}-{file_name}", random_hex(8)));
    tokio::fs::write(&temp, &bytes)
        .await
        .with_context(|| format!("writing {}", temp.display()))?;
    // Anything from here on leaves a file in the staging folder if it fails; take it away on the way out.
    let placed = place(app, root, &temp, file_name, &bytes).await;
    if placed.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
    }
    let path = placed?;
    admit(
        app,
        user,
        Candidate {
            path,
            file_name: file_name.to_string(),
            immich_asset_id: None,
            taken: None,
        },
    )
    .await
}

/// The staged file's home: the root's template rendered for the date in the file, and a name nothing else in
/// that folder has. Returns where it now is.
async fn place(
    app: &Arc<App>,
    root: &config::Root,
    temp: &std::path::Path,
    file_name: &str,
    bytes: &[u8],
) -> Result<std::path::PathBuf> {
    let resp = app.ingest(temp).await?;
    if !resp.status().is_success() {
        anyhow::bail!("{}", resp.text().await.unwrap_or_default());
    }
    let meta: serde_json::Value = resp.json().await?;
    // 0 is "no DateTimeOriginal"; a photo with no date is filed under the day it was imported, as a library
    // with nothing else to go on would file it.
    let captured = meta["info"]["capturedUnix"].as_i64().unwrap_or_default();
    let date = sidecar_template::Date::from_unix(if captured > 0 {
        captured
    } else {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    });
    let dir = root.mount.join(sidecar_template::render_dir(
        &root.template,
        date,
        &root.month_locale,
    ));
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("making {}", dir.display()))?;
    let dest = free_name(&dir, file_name, bytes).await?;
    // `free_name` only hands back a path that exists when the file there holds exactly these bytes: the same
    // photo imported twice is one file, as it would be in a library, and the staged copy goes away. Nothing
    // is being added, so the folder's limit doesn't come into it.
    if dest.exists() {
        tokio::fs::remove_file(temp).await.ok();
        return Ok(dest);
    }
    // Only now, with a genuinely new photograph to file, is room needed.
    make_room(app, root).await?;
    tokio::fs::rename(temp, &dest)
        .await
        .with_context(|| format!("moving the import into {}", dir.display()))?;
    Ok(dest)
}

/// `dir/file_name`, or the same name with `-1`, `-2` … before the extension if something is already there.
/// A file of that name holding exactly these bytes is the same photo: its path comes back as it stands, and
/// the caller keeps that one instead of writing a second copy.
/// Make room for one more photograph in the editing folder. Removes the oldest exported one if the folder
/// is at its limit; refuses when everything in it is the only copy of itself. The home page asks the same
/// question before it opens the file picker, so this is normally a formality — but it is the rule, and a
/// client that skipped the question still meets it here.
async fn make_room(app: &Arc<App>, root: &config::Root) -> Result<()> {
    let items = folder_items(root).await?;
    if items.len() < root.intake_limit {
        return Ok(());
    }
    let Some(out) = evictable(&items) else {
        anyhow::bail!(
            "The editing folder is full ({} of {}), and nothing in it has been exported yet. Export what \
             you want to keep, or remove a photo from the folder, and then import this one.",
            items.len(),
            root.intake_limit
        );
    };
    let (name, at) = (out.file_name.clone(), out.at.clone());
    let path = root.mount.join(&at);
    remove_original(app, root, &path).await?;
    let key = out.asset_key.clone();
    app.store.delete("assets", &key).await;
    tracing::info!("editing folder was full: removed {name}, which had been exported");
    Ok(())
}

async fn free_name(
    dir: &std::path::Path,
    file_name: &str,
    bytes: &[u8],
) -> Result<std::path::PathBuf> {
    let stem = sidecar_template::stem_of(file_name);
    let ext = std::path::Path::new(file_name)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 0..1000 {
        let candidate = dir.join(if n == 0 {
            file_name.to_string()
        } else {
            format!("{stem}-{n}{ext}")
        });
        match tokio::fs::read(&candidate).await {
            Err(_) => return Ok(candidate),
            Ok(there) if there == bytes => return Ok(candidate),
            Ok(_) => continue,
        }
    }
    anyhow::bail!(
        "a thousand files in {} are already called {file_name}",
        dir.display()
    )
}

async fn new_pick(
    app: &App,
    user: &User,
    query: String,
    candidates: Vec<Candidate>,
    sha1: Option<String>,
) -> Result<String> {
    let token = random_hex(16);
    app.store
        .put(
            "picks",
            &token,
            &PickRecord {
                user_id: user.id.clone(),
                query,
                candidates,
                sha1,
            },
        )
        .await?;
    Ok(token)
}

/// `GET /open?sha1=…&name=…&taken=…&size=…` (the iOS Shortcut), or `?path=…` where allowed.
async fn open(
    State(app): State<Arc<App>>,
    PageUser(user): PageUser,
    Query(q): Query<OpenQuery>,
) -> Response {
    let outcome = match resolve::resolve(&app, &user, &q).await {
        Ok(r) => r,
        Err(e) => return error_page(StatusCode::BAD_GATEWAY, &format!("{e:#}")),
    };
    let query = q.name.clone().unwrap_or_default();
    let sha1 = shared_sha1(q.sha1.as_deref());
    match outcome {
        Resolved::Found(c) if !sidecar_template::is_editable(&c.path) => {
            error_page(StatusCode::UNPROCESSABLE_ENTITY, &cant_open(&c.file_name))
        }
        Resolved::Found(c) => match admit(&app, &user, c).await {
            Ok(key) => Redirect::to(&format!("/edit/{key}")).into_response(),
            Err(e) => error_page(StatusCode::UNPROCESSABLE_ENTITY, &format!("{e:#}")),
        },
        Resolved::Choose(list) => match new_pick(&app, &user, query, list, sha1).await {
            Ok(t) => Redirect::to(&format!("/pick/{t}")).into_response(),
            Err(e) => internal(e),
        },
        Resolved::NotFound => match new_pick(&app, &user, query, Vec::new(), sha1).await {
            Ok(t) => Redirect::to(&format!("/pick/{t}")).into_response(),
            Err(e) => internal(e),
        },
    }
}

/// A checksum as the share target writes it: forty hex digits, lower-cased so the page finds the file the
/// service worker kept under it. Anything else is not one, and the pick page then offers no import.
fn shared_sha1(sha1: Option<&str>) -> Option<String> {
    sha1.filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
}

#[derive(Deserialize)]
struct PickQuery {
    q: Option<String>,
}

async fn get_pick(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(token): Path<String>,
    Query(pq): Query<PickQuery>,
) -> Response {
    let Some(mut rec) = app
        .store
        .get::<PickRecord>("picks", &token)
        .await
        .filter(|r| r.user_id == user.id)
    else {
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
    Json(serde_json::json!({ "query": rec.query, "candidates": rec.candidates, "sha1": rec.sha1 }))
        .into_response()
}

#[derive(Deserialize)]
struct PickChoice {
    index: usize,
}

async fn choose_pick(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(token): Path<String>,
    Json(choice): Json<PickChoice>,
) -> Response {
    let Some(rec) = app
        .store
        .get::<PickRecord>("picks", &token)
        .await
        .filter(|r| r.user_id == user.id)
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(c) = rec.candidates.get(choice.index).cloned() else {
        return StatusCode::BAD_REQUEST.into_response();
    };
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

async fn put_settings(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Json(settings): Json<Settings>,
) -> Response {
    let saved = app
        .store
        .update("users", &user.id, |found: Option<UserRecord>| UserRecord {
            settings: settings.clone(),
            ..found.unwrap_or_default()
        })
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

async fn thumbnail(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(id): Path<String>,
) -> Response {
    if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return StatusCode::NOT_FOUND.into_response();
    }
    match app.immich.thumbnail(&user.api_key, &id).await {
        Ok(r) => {
            let ct = r
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("image/jpeg")
                .to_string();
            (
                [
                    (header::CONTENT_TYPE, ct),
                    (header::CACHE_CONTROL, "private, max-age=86400".into()),
                ],
                Body::from_stream(r.bytes_stream()),
            )
                .into_response()
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
    let rec: AssetRecord = app
        .store
        .get("assets", key)
        .await
        .filter(|r: &AssetRecord| r.users.contains(&user.id))?;
    if rec.path.is_file() {
        return Some(rec);
    }
    let asset = app
        .immich
        .asset(&user.api_key, rec.immich_asset_id.as_deref()?)
        .await
        .ok()?;
    let found = resolve::from_asset(app, &asset)?;
    tracing::info!(
        "{} is gone; Immich has it at {}",
        rec.path.display(),
        found.path.display()
    );
    let fixed = AssetRecord {
        path: found.path,
        file_name: found.file_name,
        ..rec
    };
    // Through `update`, so a user added by a concurrent `admit` isn't written back out of the record.
    let (path, file_name) = (fixed.path.clone(), fixed.file_name.clone());
    let _ = app
        .store
        .update("assets", key, |now: Option<AssetRecord>| AssetRecord {
            path,
            file_name,
            ..now.unwrap_or(fixed.clone())
        })
        .await;
    Some(fixed)
}

pub(crate) async fn relay(resp: reqwest::Result<reqwest::Response>) -> Response {
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("The worker didn't answer: {e}"),
            )
                .into_response();
        }
    };
    let mut out = Response::builder().status(resp.status().as_u16());
    for name in ["content-type", "x-width", "x-height", "x-process-version"] {
        if let Some(v) = resp.headers().get(name) {
            out = out.header(name, v.as_bytes());
        }
    }
    out.body(Body::from_stream(resp.bytes_stream())).unwrap()
}

async fn meta(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let _ = app.ingest(&rec.path).await;
    relay(
        app.http
            .get(app.worker(&format!("/v1/assets/{key}/meta")))
            .send()
            .await,
    )
    .await
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
    let Some(rec) = asset_for(app, user, key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let fetch = || async { relay(app.http.get(app.worker(&worker_path)).send().await).await };
    let mut resp = fetch().await;
    if !resp.status().is_success() {
        let ingested = app.ingest(&rec.path).await;
        if ingested.is_ok_and(|r| r.status().is_success()) {
            tracing::info!(
                "re-ingested {} after the worker forgot it",
                rec.path.display()
            );
            resp = fetch().await;
        }
    }
    if resp.status().is_success() {
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("private, max-age=86400"),
        );
    }
    resp
}

async fn proxy(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
    Query(q): Query<ProxyQuery>,
) -> Response {
    let edge = q.long_edge.unwrap_or(2560);
    pixels(
        &app,
        &user,
        &key,
        format!("/v1/assets/{key}/proxy?long_edge={edge}"),
    )
    .await
}

/// How wide a thumbnail is rendered. Twice the grid's cell, so it still looks right on a dense screen.
const THUMB_EDGE: u32 = 480;

/// `GET /api/assets/{key}/thumb` — the photo as the edit currently renders it, small, as JPEG.
///
/// Immich serves thumbnails for everything it holds, and that stays the fast path; this is for the photos it
/// cannot — one imported into the editing folder, which Immich has never seen, and any file whose thumbnail
/// Immich declines to make. It costs a develop pass over the cached proxy at this size, not a decode, so the
/// expensive part (reading the RAW) is paid once by whatever opened the photo first. Cached for an hour by
/// the browser, keyed on the edit's etag so a re-edit shows through.
async fn thumb(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let path = rec.path.clone();
    let Ok(edit) = tokio::task::spawn_blocking(move || {
        SidecarDoc::load(&path)
            .map(|d| d.edit())
            .unwrap_or_default()
    })
    .await
    else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let req = serde_json::json!({
        "key": key,
        "path": rec.path,
        "edit": edit,
        "longEdge": THUMB_EDGE,
    });
    let mut out = relay(
        app.http
            .post(app.worker("/v1/preview"))
            .json(&req)
            .send()
            .await,
    )
    .await;
    if out.status().is_success() {
        out.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("private, max-age=3600"),
        );
    }
    out
}

#[derive(Deserialize)]
struct FocusQuery {
    edge: Option<u32>,
}

/// Focus-peaking mask: PNG bytes at the same edge as a proxy, so the client can pair them up.
async fn focus(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
    Query(q): Query<FocusQuery>,
) -> Response {
    let edge = q.edge.unwrap_or(2560);
    pixels(
        &app,
        &user,
        &key,
        format!("/v1/assets/{key}/focus?edge={edge}"),
    )
    .await
}

/// Which spectral colour stocks this deployment renders. Not cached by the browser: "none" must not outlive
/// the operator adding their copies of the standards.
async fn film_stocks(State(app): State<Arc<App>>, ApiUser(_user): ApiUser) -> Response {
    relay(app.http.get(app.worker("/v1/film/stocks")).send().await).await
}

/// One stock's profile, relayed from the worker: the slide's colours over its layers' log H, which is what the
/// browser needs to render the stock itself. The ISO tables it was measured through stay on the server.
async fn film_stock(
    State(app): State<Arc<App>>,
    ApiUser(_user): ApiUser,
    Path(id): Path<String>,
) -> Response {
    if !id
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    relay(
        app.http
            .get(app.worker(&format!("/v1/film/stocks/{id}")))
            .send()
            .await,
    )
    .await
}

/// Which masks this photo has, and its faces: `{ available, names, faces }`, computed on first use. Not cached
/// by the browser: "unavailable" must not outlive a switch to the `-ml` image.
async fn masks(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let _ = app.ingest(&rec.path).await;
    relay(
        app.http
            .get(app.worker(&format!("/v1/assets/{key}/masks")))
            .send()
            .await,
    )
    .await
}

/// One mask as a greyscale PNG, in the proxy's frame. Keyed by content like a proxy, so cached like one.
/// `GET /api/assets/{key}/camera` — the JPEG the camera embedded in the RAW, upright: its own rendering.
async fn camera(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
) -> Response {
    pixels(&app, &user, &key, format!("/v1/assets/{key}/camera")).await
}

async fn mask(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path((key, name)): Path<(String, String)>,
) -> Response {
    if !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
        return StatusCode::NOT_FOUND.into_response();
    }
    pixels(&app, &user, &key, format!("/v1/assets/{key}/masks/{name}")).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EditState {
    edit: EditParams,
    etag: String,
    /// The history node `edit` is: what the next save names as its base.
    head: String,
    read_only: bool,
    /// False when the photo's folder can't take a sidecar, so the editor can say so before the edit is made
    /// rather than at the first save. Advisory — see `sidecar_xmp::writable_beside`.
    writable: bool,
    exports: Vec<serde_json::Value>,
    /// This save branched: another device had saved since the version it was made from. Both are kept.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    forked: bool,
    /// The SHA-1 this sidecar was saved for, when it isn't this photo's: the file beside it was renamed into
    /// place or rewritten since (DESIGN.md §4, `sc:SourceSHA1`). The edit is still shown — a DNG rewritten by
    /// another program is the same picture — and the next save ties the sidecar to this file.
    #[serde(skip_serializing_if = "Option::is_none")]
    saved_for: Option<String>,
}

impl EditState {
    fn of(doc: &SidecarDoc, key: &str, writable: bool, forked: bool) -> Self {
        let saved_for = doc
            .source_sha1()
            .filter(|s| !s.eq_ignore_ascii_case(key))
            .map(String::from);
        Self {
            edit: doc.edit(),
            etag: doc.etag(),
            head: doc.history().head,
            read_only: doc.is_read_only(),
            writable,
            exports: doc.exports(),
            forked,
            saved_for,
        }
    }

    fn into_response(self) -> Response {
        ([(header::ETAG, format!("\"{}\"", self.etag))], Json(self)).into_response()
    }
}

async fn get_edit(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let loaded = tokio::task::spawn_blocking(move || {
        let writable = sidecar_xmp::writable_beside(&rec.path);
        SidecarDoc::load(&rec.path).map(|d| EditState::of(&d, &key, writable, false))
    })
    .await;
    match loaded {
        Ok(Ok(state)) => state.into_response(),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e.into()),
    }
}

/// A client that predates history sent an edit whose `If-Match` is no longer current.
#[derive(Debug, thiserror::Error)]
#[error("the edit changed since it was loaded")]
struct Stale;

/// Change a photo's sidecar through the guarded writer, off the async runtime, and answer with the edit state
/// it leaves. `change` returns whether the save forked. The failures a person can act on carry their own
/// message and status; anything else is a bug here and goes to the log as one.
async fn write_sidecar(
    app: &Arc<App>,
    key: &str,
    rec: &AssetRecord,
    mut change: impl FnMut(&mut SidecarDoc) -> anyhow::Result<bool> + Send + 'static,
) -> Response {
    let roots = app.config.canonical_mounts();
    let moves_files = app
        .config
        .root_of(&rec.path)
        .is_some_and(|(r, _)| r.moves_files);
    let path = rec.path.clone();
    let key = key.to_string();
    let result = tokio::task::spawn_blocking(move || {
        let created = !sidecar_xmp::sidecar_path_for(&path).exists();
        let writable = sidecar_xmp::writable_beside(&path);
        let mut state = None;
        let saved = sidecar_xmp::update(&path, &roots, |doc| {
            let forked = change(doc)?;
            state = Some(EditState::of(doc, &key, writable, forked));
            Ok(())
        });
        let stale = || SidecarDoc::load(&path).map(|d| EditState::of(&d, &key, writable, false));
        match saved {
            Ok(_) => Ok((state.expect("set by a successful change"), created)),
            Err(e) if e.is::<Stale>() => Err(Box::new(stale().map_or_else(internal, |s| {
                (
                    StatusCode::PRECONDITION_FAILED,
                    [(header::ETAG, format!("\"{}\"", s.etag))],
                    Json(s),
                )
                    .into_response()
            }))),
            Err(e) => Err(Box::new(
                match e.downcast_ref::<sidecar_xmp::SidecarError>() {
                    Some(sidecar_xmp::SidecarError::NewerProcessVersion { .. }) => {
                        (StatusCode::CONFLICT, format!("{e}")).into_response()
                    }
                    Some(sidecar_xmp::SidecarError::UnknownVersion { .. }) => {
                        (StatusCode::NOT_FOUND, format!("{e}")).into_response()
                    }
                    Some(sidecar_xmp::SidecarError::NotWritable { .. }) => {
                        tracing::error!("{e:#}");
                        (StatusCode::FORBIDDEN, format!("{e}")).into_response()
                    }
                    _ => internal(e),
                },
            )),
        }
    })
    .await;
    match result {
        Ok(Ok((state, created))) => {
            if created && moves_files {
                app.queue_sidecar_discovery();
            }
            state.into_response()
        }
        Ok(Err(resp)) => *resp,
        Err(e) => internal(e.into()),
    }
}

/// `PUT /api/assets/{key}/edit`. The editor sends the history node it edited from as `base`: a save from a
/// node that is no longer the head forks instead of failing, and a `checkpoint` (sent after a pause) folds
/// into the checkpoint before it. A bare edit, from a client that predates history, still needs `If-Match`.
#[derive(Deserialize)]
#[serde(untagged)]
enum SaveBody {
    Versioned(Save),
    Bare(EditParams),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Save {
    edit: EditParams,
    base: String,
    #[serde(default)]
    checkpoint: bool,
    #[serde(default)]
    session: Option<String>,
}

async fn put_edit(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SaveBody>,
) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let if_match = headers
        .get(header::IF_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_matches('"').to_string());
    let save = match body {
        SaveBody::Versioned(save) => save,
        SaveBody::Bare(edit) => {
            let Some(if_match) = if_match else {
                return (
                    StatusCode::PRECONDITION_REQUIRED,
                    "Send If-Match with the ETag you edited from.",
                )
                    .into_response();
            };
            // Without a base the save can only go on top of the head, and only if the client saw the head.
            return write_sidecar(&app, &key.clone(), &rec, move |doc| {
                if doc.etag() != if_match {
                    anyhow::bail!(Stale);
                }
                let base = doc.history().head;
                Ok(doc
                    .commit(
                        &Commit {
                            edit: &edit,
                            base: &base,
                            checkpoint: false,
                            session: None,
                            user: None,
                        },
                        &key,
                    )?
                    .forked)
            })
            .await;
        }
    };
    let name = user.name.clone();
    write_sidecar(&app, &key.clone(), &rec, move |doc| {
        let c = Commit {
            edit: &save.edit,
            base: &save.base,
            checkpoint: save.checkpoint,
            session: save.session.as_deref(),
            user: Some(&name),
        };
        Ok(doc.commit(&c, &key)?.forked)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HistoryNode {
    #[serde(flatten)]
    node: sidecar_xmp::Node,
    /// The full edit this node stands for, so the client can compare and render any two without a round trip.
    edit: EditParams,
    /// `sc:Exports` entries made from this node's edit.
    exports: Vec<serde_json::Value>,
}

/// `GET /api/assets/{key}/history` — every version of this photo's edit, oldest first.
async fn get_history(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let built = tokio::task::spawn_blocking(move || -> anyhow::Result<serde_json::Value> {
        let doc = SidecarDoc::load(&rec.path)?;
        let (history, exports) = (doc.history(), doc.exports());
        let nodes = history
            .nodes
            .iter()
            .map(|n| {
                let edit = history.version_of(&n.id)?;
                let exports = exports
                    .iter()
                    .filter(|e| e["editEtag"] == n.etag)
                    .cloned()
                    .collect();
                Ok(HistoryNode {
                    node: n.clone(),
                    edit,
                    exports,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(serde_json::json!({ "head": history.head, "nodes": nodes }))
    })
    .await;
    match built {
        Ok(Ok(v)) => Json(v).into_response(),
        Ok(Err(e)) => internal(e),
        Err(e) => internal(e.into()),
    }
}

#[derive(Deserialize)]
struct Checkout {
    node: String,
}

/// `POST /api/assets/{key}/history/checkout { node }` — make that version (or `original`) the current edit.
async fn checkout(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
    Json(body): Json<Checkout>,
) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    write_sidecar(&app, &key.clone(), &rec, move |doc| {
        doc.checkout(&body.node, &key).map(|()| false)
    })
    .await
}

#[derive(Deserialize)]
struct Label {
    label: Option<String>,
}

/// `PUT /api/assets/{key}/history/{node}/label { label }` — name a version, or clear its name with null.
async fn label(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path((key, node)): Path<(String, String)>,
    Json(body): Json<Label>,
) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    write_sidecar(&app, &key, &rec, move |doc| {
        doc.label(&node, body.label.clone()).map(|()| false)
    })
    .await
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
async fn loupe(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
    Json(body): Json<LoupeBody>,
) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let req = serde_json::json!({ "key": key, "path": rec.path, "center": body.center, "size": body.size, "edit": body.edit, "colorSpace": body.color_space });
    relay(
        app.http
            .post(app.worker("/v1/loupe"))
            .json(&req)
            .send()
            .await,
    )
    .await
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
async fn preview(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
    Json(body): Json<PreviewBody>,
) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let req = serde_json::json!({ "key": key, "path": rec.path, "edit": body.edit, "colorSpace": body.color_space, "longEdge": body.long_edge });
    relay(
        app.http
            .post(app.worker("/v1/preview"))
            .json(&req)
            .send()
            .await,
    )
    .await
}

pub fn router(app: Arc<App>) -> Router {
    app.start_housekeeping();
    app.start_lensfun_sync();
    let web = &app.config.server.web_root;
    let spa = ServeDir::new(web).fallback(ServeFile::new(web.join("index.html")));
    Router::new()
        .route("/open", get(open))
        .route("/api/health", get(|| async { "ok" }))
        .route(
            "/api/session",
            get(session::session)
                .post(session::sign_in)
                .delete(session::sign_out),
        )
        .route("/api/recent", get(recent))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/picks/{token}", get(get_pick).post(choose_pick))
        .route(
            "/api/intake",
            axum::routing::post(intake).layer(axum::extract::DefaultBodyLimit::max(INTAKE_LIMIT)),
        )
        .route("/api/intake/folder", get(folder))
        .route("/api/intake/folder/{key}", axum::routing::delete(forget))
        .route("/api/immich/assets/{id}/thumbnail", get(thumbnail))
        .route("/api/film/stocks", get(film_stocks))
        .route("/api/film/stocks/{id}", get(film_stock))
        .route("/api/assets/{key}/meta", get(meta))
        .route("/api/assets/{key}/proxy", get(proxy))
        .route("/api/assets/{key}/thumb", get(thumb))
        .route("/api/assets/{key}/focus", get(focus))
        .route("/api/assets/{key}/masks", get(masks))
        .route("/api/assets/{key}/masks/{name}", get(mask))
        .route("/api/assets/{key}/camera", get(camera))
        .route("/api/assets/{key}/edit", get(get_edit).put(put_edit))
        .route("/api/assets/{key}/history", get(get_history))
        .route(
            "/api/assets/{key}/history/checkout",
            axum::routing::post(checkout),
        )
        .route(
            "/api/assets/{key}/history/{node}/label",
            axum::routing::put(label),
        )
        .route("/api/assets/{key}/loupe", axum::routing::post(loupe))
        .route("/api/assets/{key}/preview", axum::routing::post(preview))
        .route("/api/exports", axum::routing::post(export::create))
        .route("/api/exports/{id}", get(export::status))
        .route("/api/exports/{id}/events", get(export::events))
        .route("/api/exports/{id}/file", get(export::file))
        .fallback_service(spa)
        // Proxies compress with brotli for Safari and zstd for Chromium.
        .layer(
            CompressionLayer::new()
                .br(true)
                .zstd(true)
                .gzip(true)
                .compress_when(
                    tower_http::compression::predicate::SizeAbove::new(1024).and(
                        tower_http::compression::predicate::NotForContentType::new(
                            "text/event-stream",
                        ),
                    ),
                ),
        )
        .with_state(app)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field of an edit has a default, so a versioned save would also parse as a (default) bare edit if
    /// the two were tried the other way round — and quietly save nothing.
    #[test]
    fn a_versioned_save_is_never_read_as_a_bare_edit() {
        let mut edit = EditParams::default();
        edit.exposure.ev = 0.7;
        let versioned = serde_json::json!({ "edit": edit, "base": "original", "checkpoint": true, "session": "tab" });
        let SaveBody::Versioned(s) = serde_json::from_value(versioned).unwrap() else {
            panic!("read as bare")
        };
        assert_eq!(
            (
                s.edit.exposure.ev,
                s.base.as_str(),
                s.checkpoint,
                s.session.as_deref()
            ),
            (0.7, "original", true, Some("tab"))
        );

        let SaveBody::Bare(e) =
            serde_json::from_value(serde_json::to_value(&edit).unwrap()).unwrap()
        else {
            panic!("read as versioned")
        };
        assert_eq!(e, edit, "a client from before history still saves");
    }

    /// A sidecar saved for other bytes says so, whatever the hex's case; the file's own doesn't, and neither
    /// does a photo with no sidecar yet.
    #[test]
    fn a_sidecar_saved_for_another_file_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        let original = dir.join("DSC_1.NEF");
        std::fs::write(&original, b"raw").unwrap();
        sidecar_xmp::update(&original, std::slice::from_ref(&dir), |d| {
            d.set_edit(&EditParams::default(), "aaaa")
        })
        .unwrap();
        let doc = SidecarDoc::load(&original).unwrap();
        assert_eq!(
            EditState::of(&doc, "bbbb", true, false)
                .saved_for
                .as_deref(),
            Some("aaaa")
        );
        assert_eq!(EditState::of(&doc, "AAAA", true, false).saved_for, None);
        let other = dir.join("DSC_2.NEF");
        std::fs::write(&other, b"raw2").unwrap();
        assert_eq!(
            EditState::of(&SidecarDoc::load(&other).unwrap(), "bbbb", true, false).saved_for,
            None
        );
    }

    /// A name nothing else holds. The import keeps the photographer's file name where it can.
    #[tokio::test]
    async fn an_import_keeps_its_name_when_the_folder_is_free() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            free_name(tmp.path(), "IMG_1768.CR2", b"raw").await.unwrap(),
            tmp.path().join("IMG_1768.CR2")
        );
    }

    /// A different photo that happens to share a name doesn't overwrite the one already there.
    #[tokio::test]
    async fn a_name_collision_numbers_the_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("IMG_1768.CR2"), b"first").unwrap();
        assert_eq!(
            free_name(tmp.path(), "IMG_1768.CR2", b"second")
                .await
                .unwrap(),
            tmp.path().join("IMG_1768-1.CR2")
        );
        std::fs::write(tmp.path().join("IMG_1768-1.CR2"), b"second").unwrap();
        // And the one after that steps past both.
        assert_eq!(
            free_name(tmp.path(), "IMG_1768.CR2", b"third")
                .await
                .unwrap(),
            tmp.path().join("IMG_1768-2.CR2")
        );
    }

    /// The same photo imported twice is one file: the path that already holds those bytes comes back, and
    /// the caller keeps it rather than writing a copy beside it.
    #[tokio::test]
    async fn the_same_photo_twice_is_the_file_already_there() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("IMG_1768.CR2"), b"raw").unwrap();
        let again = free_name(tmp.path(), "IMG_1768.CR2", b"raw").await.unwrap();
        assert_eq!(again, tmp.path().join("IMG_1768.CR2"));
        assert!(
            again.exists(),
            "the caller keeps this one instead of writing another"
        );
    }

    fn item(name: &str, modified_unix: i64, exported: bool) -> FolderItem {
        FolderItem {
            asset_key: name.to_string(),
            file_name: name.to_string(),
            at: name.to_string(),
            bytes: 1,
            modified_unix,
            exported,
            edited: false,
        }
    }

    /// Room is made from what has been rendered somewhere else, oldest first.
    #[test]
    fn the_oldest_exported_photo_is_the_one_that_goes() {
        let items = [
            item("new-exported.CR2", 300, true),
            item("old-exported.CR2", 100, true),
            item("middle.CR2", 200, true),
        ];
        assert_eq!(
            evictable(&items).map(|i| i.file_name.as_str()),
            Some("old-exported.CR2")
        );
    }

    /// A photograph nobody has exported is the only copy of that work: it is never chosen, however old it
    /// is, and an import that would need it refuses instead.
    #[test]
    fn an_unexported_photo_is_never_evicted() {
        let items = [
            item("ancient-unexported.CR2", 1, false),
            item("recent-exported.CR2", 900, true),
        ];
        assert_eq!(
            evictable(&items).map(|i| i.file_name.as_str()),
            Some("recent-exported.CR2"),
            "the exported one goes even though it is much newer"
        );
    }

    /// Nothing exported at all: there is nothing to take, and the caller has to say so rather than guess.
    #[test]
    fn a_folder_of_unexported_photos_offers_nothing() {
        let items = [item("a.CR2", 1, false), item("b.CR2", 2, false)];
        assert!(evictable(&items).is_none());
    }

    /// Only a real checksum reaches the pick page, in the case the share target stored it under.
    #[test]
    fn a_shared_checksum_is_forty_hex_digits_lower_cased() {
        let sha1 = "DA39A3EE5E6B4B0D3255BFEF95601890AFD80709";
        assert_eq!(
            shared_sha1(Some(sha1)).as_deref(),
            Some(&*sha1.to_ascii_lowercase())
        );
        assert_eq!(shared_sha1(Some("da39a3ee")), None, "too short");
        assert_eq!(shared_sha1(Some(&"g".repeat(40))), None, "not hex");
        assert_eq!(shared_sha1(None), None);
    }

    /// A pick saved before the field existed still loads, with no checksum.
    #[test]
    fn a_pick_from_before_the_checksum_still_reads() {
        let rec: PickRecord =
            serde_json::from_str(r#"{"userId":"u","query":"DSC_1.NEF","candidates":[]}"#).unwrap();
        assert_eq!((rec.query.as_str(), rec.sha1), ("DSC_1.NEF", None));
    }

    /// The limit defaults to four, and it is the editing folder's alone.
    #[test]
    fn the_editing_folder_holds_four_by_default() {
        let c: config::Config = toml::from_str(
            r#"
            [[roots]]
            name = "Editing folder"
            mount = "/photos"
            template = "{{y}}/{{filename}}"
            intake = true
            "#,
        )
        .unwrap();
        assert_eq!(c.intake_root().unwrap().intake_limit, 4);
    }
}
