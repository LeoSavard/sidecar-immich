// SPDX-License-Identifier: Apache-2.0
//! Export jobs. `POST /api/exports` creates a job (or returns an identical earlier version); the render runs in
//! the worker whether or not anyone is watching; `GET /api/exports/{id}/events` streams progress as SSE with
//! replay from `Last-Event-ID`. Finished files are uploaded to Immich and stacked with the RAW, newest JPEG first.

use crate::{App, AssetRecord, session::ApiUser, store::random_hex};
use anyhow::{Context, Result, bail};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sidecar_color::{DisplaySpace, EditParams};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where a finished render goes. `Immich` is the default and what every earlier client asks for by saying
/// nothing: upload, stack with the original, drop the local copy. The other two keep the rendered file in the
/// worker's outbox so the browser can fetch it — the phone or laptop doing the editing is then a destination
/// in its own right, not only a remote control for the library.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Destination {
    #[default]
    Immich,
    Device,
    Both,
}

impl Destination {
    /// Does the browser need to be able to download the rendered file afterwards?
    fn to_device(self) -> bool {
        matches!(self, Self::Device | Self::Both)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub asset_key: String,
    pub edit_etag: String,
    #[serde(default = "jpeg")]
    pub format: String,
    #[serde(default)]
    pub color_space: DisplaySpace,
    #[serde(default = "quality")]
    pub quality: u8,
    #[serde(default)]
    pub long_edge: Option<u32>,
    #[serde(default)]
    pub destination: Destination,
}

fn jpeg() -> String {
    "jpeg".into()
}
fn quality() -> u8 {
    92
}

#[derive(Clone, Debug, Serialize)]
struct JobEvent {
    id: u64,
    kind: String,
    data: Value,
}

pub struct Job {
    pub id: String,
    user_id: String,
    created: std::time::Instant,
    events: Mutex<Vec<JobEvent>>,
    notify: tokio::sync::Notify,
}

impl Job {
    fn push(&self, kind: &str, data: Value) {
        let mut ev = self.events.lock().unwrap();
        let id = ev.len() as u64 + 1;
        ev.push(JobEvent { id, kind: kind.into(), data });
        drop(ev);
        self.notify.notify_waiters();
    }

    fn finished(&self) -> bool {
        self.events.lock().unwrap().last().is_some_and(|e| e.kind == "done" || e.kind == "failed")
    }
}

#[derive(Default)]
pub struct Jobs {
    by_id: Mutex<HashMap<String, Arc<Job>>>,
    by_idempotency: Mutex<HashMap<String, String>>,
    /// Asset keys with an export being numbered, so two taps can't claim the same version.
    numbering: Mutex<HashSet<String>>,
}

/// How long a finished job stays answerable — the worker's outbox keeps its file this long too, so the
/// download link and the job that names it expire together.
const JOB_TTL: Duration = Duration::from_secs(24 * 3600);

impl Jobs {
    /// Forget finished jobs older than `JOB_TTL`. Nothing else ever removed one, so every export since the
    /// api started stayed in memory with its whole event log.
    pub fn sweep(&self) -> usize {
        let mut by_id = self.by_id.lock().unwrap();
        let before = by_id.len();
        by_id.retain(|_, j| !(j.finished() && j.created.elapsed() > JOB_TTL));
        self.by_idempotency.lock().unwrap().retain(|_, id| by_id.contains_key(id));
        before - by_id.len()
    }
}

fn file_stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

fn same_settings(entry: &Value, req: &ExportRequest) -> bool {
    entry["editEtag"] == req.edit_etag
        && entry["format"] == req.format
        && entry["colorSpace"] == serde_json::to_value(req.color_space).unwrap()
        && entry["quality"].as_u64() == Some(req.quality as u64)
        && entry["longEdge"].as_u64() == req.long_edge.map(u64::from)
}

pub async fn create(State(app): State<Arc<App>>, ApiUser(user): ApiUser, headers: HeaderMap, Json(req): Json<ExportRequest>) -> Response {
    let Some(rec) = crate::asset_for(&app, &user, &req.asset_key).await else { return StatusCode::NOT_FOUND.into_response() };
    if !matches!(req.format.as_str(), "jpeg" | "tiff") {
        return (StatusCode::BAD_REQUEST, "format must be jpeg or tiff").into_response();
    }
    let idem = headers.get("idempotency-key").and_then(|v| v.to_str().ok()).map(|v| format!("{}:{v}", user.id));
    if let Some(k) = &idem
        && let Some(id) = app.jobs.by_idempotency.lock().unwrap().get(k).cloned()
    {
        return (StatusCode::ACCEPTED, Json(json!({ "jobId": id }))).into_response();
    }

    let path = rec.path.clone();
    let doc = match tokio::task::spawn_blocking(move || sidecar_xmp::SidecarDoc::load(&path)).await {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => return crate::internal(e),
        Err(e) => return crate::internal(e.into()),
    };
    if doc.etag() != req.edit_etag {
        return (StatusCode::CONFLICT, "Save your edit before exporting; the sidecar has changed.").into_response();
    }
    // An identical export already in `sc:Exports` normally answers the request on its own — nothing renders
    // twice. It can't when the file is wanted on this device, though: that copy was uploaded and deleted, so
    // there are no bytes left to hand back. Render it again under its existing version number, and let `run`
    // know not to upload or record a second time.
    let exports = doc.exports();
    let already = exports.iter().find(|e| same_settings(e, &req)).cloned();
    if let Some(existing) = &already
        && !req.destination.to_device()
    {
        return (StatusCode::OK, Json(json!({ "existing": existing }))).into_response();
    }

    if !app.jobs.numbering.lock().unwrap().insert(req.asset_key.clone()) {
        return (StatusCode::CONFLICT, "An export of this photo is being prepared; try again in a moment.").into_response();
    }
    let version = exports.iter().find(|e| e["editEtag"] == req.edit_etag).and_then(|e| e["version"].as_u64()).unwrap_or_else(|| exports.iter().filter_map(|e| e["version"].as_u64()).max().unwrap_or(0) + 1);
    let ext = if req.format == "tiff" { "tif" } else { "jpg" };
    let size_suffix = req.long_edge.map(|e| format!("-{e}px")).unwrap_or_default();
    let file_name = format!("{}-v{version}{size_suffix}.{ext}", file_stem(&rec.file_name));

    let job = Arc::new(Job { id: random_hex(16), user_id: user.id.clone(), created: std::time::Instant::now(), events: Mutex::new(Vec::new()), notify: tokio::sync::Notify::new() });
    app.jobs.by_id.lock().unwrap().insert(job.id.clone(), job.clone());
    if let Some(k) = idem {
        app.jobs.by_idempotency.lock().unwrap().insert(k, job.id.clone());
    }
    job.push("queued", json!({ "version": version, "file": file_name }));
    let (app2, job2, edit, name2) = (app.clone(), job.clone(), doc.edit(), file_name.clone());
    tokio::spawn(async move {
        let outcome = run(&app2, &job2, &user, &rec, &req, edit, version, &name2, already).await;
        app2.jobs.numbering.lock().unwrap().remove(&req.asset_key);
        if let Err(e) = outcome {
            tracing::error!("export {}: {e:#}", job2.id);
            job2.push("failed", json!({ "message": format!("{e:#}"), "retryable": true }));
        }
    });
    (StatusCode::ACCEPTED, Json(json!({ "jobId": job.id, "version": version, "file": file_name }))).into_response()
}

#[allow(clippy::too_many_arguments)]
async fn run(app: &Arc<App>, job: &Job, user: &crate::session::User, rec: &AssetRecord, req: &ExportRequest, edit: EditParams, version: u64, file_name: &str, recorded: Option<Value>) -> Result<()> {
    let sidecar = json!({
        "processVersion": sidecar_color::params::PROCESS_VERSION,
        "sourceSha1": req.asset_key,
        "version": version,
        "editEtag": req.edit_etag,
        "edit": edit,
    });
    let body = json!({
        "jobId": job.id, "path": rec.path, "fileName": file_name, "edit": edit, "format": req.format,
        "colorSpace": req.color_space, "quality": req.quality, "longEdge": req.long_edge, "sidecar": sidecar,
    });
    let resp = app.http.post(app.worker("/v1/render")).json(&body).send().await.context("reaching the worker")?;
    if !resp.status().is_success() {
        bail!("{}", resp.text().await.unwrap_or_default());
    }
    // Relay worker progress (0–85 % of the job).
    let mut stream = resp.bytes_stream();
    let mut buf = Vec::new();
    let mut done: Option<Value> = None;
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk?);
        while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=nl).collect();
            let v: Value = serde_json::from_slice(&line)?;
            match v["event"].as_str() {
                Some("stage") => job.push("progress", json!({ "stage": v["stage"], "overall": v["overall"].as_f64().unwrap_or(0.0) * 0.85 })),
                Some("done") => done = Some(v),
                Some("failed") => bail!("{}", v["message"].as_str().unwrap_or("render failed")),
                _ => {}
            }
        }
    }
    let done = done.context("the worker stopped before finishing")?;

    // `recorded` means this exact render is already in Immich under this version number: it is being made
    // again only so the browser can download it, so uploading it a second time would ask Immich to dedupe
    // something it has no reason to see twice.
    let mut immich_asset_id: Option<String> = None;
    if app.immich.enabled() && req.destination != Destination::Device && recorded.is_none() {
        job.push("progress", json!({ "stage": "upload", "overall": 0.88 }));
        let file = app.http.get(app.worker(&format!("/v1/outbox/{}/{}", job.id, file_name))).send().await?.error_for_status()?.bytes().await?;
        let meta: Value = app.http.get(app.worker(&format!("/v1/assets/{}/meta", req.asset_key))).send().await?.json().await?;
        let captured = meta["info"]["capturedUnix"].as_i64().filter(|&t| t > 0).map(sidecar_xmp::rfc3339).unwrap_or_else(sidecar_xmp::utc_now);
        let id = app.immich.upload(&user.api_key, file_name, file.to_vec(), &captured).await?;
        job.push("progress", json!({ "stage": "stack", "overall": 0.95 }));
        let raw_id = match &rec.immich_asset_id {
            Some(id) => Some(id.clone()),
            None => crate::resolve::immich_asset_at(app, user, &rec.path).await?.map(|a| a.id),
        };
        if let Some(raw_id) = raw_id {
            app.immich.stack_export(&user.api_key, &raw_id, &id, req.format == "jpeg").await?;
        }
        immich_asset_id = Some(id);
    }

    if recorded.is_none() {
        let entry = json!({
            "version": version, "editEtag": req.edit_etag, "file": file_name, "format": req.format,
            "colorSpace": req.color_space, "quality": req.quality, "longEdge": req.long_edge,
            "fileSha1": done["sha1"], "exported": sidecar_xmp::utc_now(), "immichAssetId": immich_asset_id,
        });
        let roots = app.config.canonical_mounts();
        let path = rec.path.clone();
        // An export that reached Immich is not lost when the sidecar can't be written, so this one failure
        // is reported and stepped over rather than failing the job: the photo's version history misses an
        // entry, and the file the person asked for still arrives.
        let saved = tokio::task::spawn_blocking(move || sidecar_xmp::update(&path, &roots, |d| {
            d.append_export(entry.clone());
            Ok(())
        }))
        .await?;
        if let Err(e) = saved {
            tracing::error!("recording export v{version} of {}: {e:#}", rec.path.display());
            job.push("warning", json!({ "message": format!("{e}") }));
        }
    }

    // The outbox copy is what `GET /api/exports/{id}/file` serves, so it only goes when nothing wants it:
    // the render reached Immich and this device isn't asking for it too. What stays is swept by the worker.
    // A re-render of an export that is already in Immich still belongs to that asset, so the finished event
    // names it rather than reporting the photo as never uploaded.
    let immich_id = immich_asset_id.clone().or_else(|| recorded.as_ref().and_then(|e| e["immichAssetId"].as_str().map(String::from)));
    let download = (req.destination.to_device() || immich_id.is_none()).then(|| format!("/api/exports/{}/file", job.id));
    if download.is_none() {
        let _ = app.http.delete(app.worker(&format!("/v1/outbox/{}", job.id))).send().await;
    }
    job.push("done", json!({ "version": version, "file": file_name, "bytes": done["bytes"], "width": done["width"], "height": done["height"], "immichAssetId": immich_id, "download": download }));
    Ok(())
}

fn job_for(app: &App, user: &crate::session::User, id: &str) -> Option<Arc<Job>> {
    app.jobs.by_id.lock().unwrap().get(id).filter(|j| j.user_id == user.id).cloned()
}

pub async fn status(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(id): Path<String>) -> Response {
    let Some(job) = job_for(&app, &user, &id) else { return StatusCode::NOT_FOUND.into_response() };
    let events = job.events.lock().unwrap().clone();
    Json(json!({ "jobId": id, "last": events.last(), "finished": job.finished() })).into_response()
}

pub async fn events(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(id): Path<String>, headers: HeaderMap) -> Response {
    let Some(job) = job_for(&app, &user, &id) else { return StatusCode::NOT_FOUND.into_response() };
    let after = headers.get("last-event-id").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let stream = futures_util::stream::unfold((job, after, false), |(job, mut sent, ended)| async move {
        if ended {
            return None;
        }
        loop {
            let waiter = job.clone();
            let notified = waiter.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let next = job.events.lock().unwrap().iter().find(|e| e.id > sent).cloned();
            if let Some(e) = next {
                sent = e.id;
                let terminal = e.kind == "done" || e.kind == "failed";
                let event = Event::default().id(e.id.to_string()).event(e.kind).json_data(e.data).unwrap();
                return Some((Ok::<_, std::convert::Infallible>(event), (job, sent, terminal)));
            }
            notified.await;
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)).text("keepalive")).into_response()
}

/// Download a finished export that wasn't uploaded (no Immich configured).
pub async fn file(State(app): State<Arc<App>>, ApiUser(user): ApiUser, Path(id): Path<String>) -> Response {
    let Some(job) = job_for(&app, &user, &id) else { return StatusCode::NOT_FOUND.into_response() };
    let done = job.events.lock().unwrap().iter().find(|e| e.kind == "done").map(|e| e.data.clone());
    let Some(name) = done.as_ref().and_then(|d| d["file"].as_str()).map(String::from) else { return StatusCode::NOT_FOUND.into_response() };
    let mut resp = crate::relay(app.http.get(app.worker(&format!("/v1/outbox/{id}/{name}"))).send().await).await;
    let ct = if name.ends_with(".tif") { "image/tiff" } else { "image/jpeg" };
    resp.headers_mut().insert("content-type", ct.parse().unwrap());
    resp.headers_mut().insert("content-disposition", attachment(&name));
    resp
}

/// `Content-Disposition` for a download named after the original. The quoted `filename` alone mangled any
/// non-ASCII name (browsers read it as Latin-1) and broke on a `"`; RFC 6266 has browsers prefer the UTF-8
/// `filename*`, and the plain one stays as an ASCII fallback.
fn attachment(name: &str) -> axum::http::HeaderValue {
    let fallback: String = name.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' }).collect();
    let encoded: String = name
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    axum::http::HeaderValue::from_str(&format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")).expect("ASCII only")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_jobs_expire_and_running_ones_stay() {
        let jobs = Jobs::default();
        let job = |id: &str, age: Duration, kind: &str| {
            let j = Arc::new(Job { id: id.into(), user_id: "u".into(), created: std::time::Instant::now() - age, events: Mutex::default(), notify: Default::default() });
            j.push(kind, json!({}));
            jobs.by_id.lock().unwrap().insert(id.into(), j);
            jobs.by_idempotency.lock().unwrap().insert(format!("u:{id}"), id.into());
        };
        let day = Duration::from_secs(25 * 3600);
        job("old-done", day, "done");
        job("old-running", day, "progress");
        job("new-done", Duration::ZERO, "done");
        assert_eq!(jobs.sweep(), 1);
        assert!(!jobs.by_id.lock().unwrap().contains_key("old-done"));
        assert_eq!(jobs.by_idempotency.lock().unwrap().len(), 2);
    }

    #[test]
    fn download_names_survive_accents_and_quotes() {
        assert_eq!(attachment("IMG_1-v2.jpg").to_str().unwrap(), "attachment; filename=\"IMG_1-v2.jpg\"; filename*=UTF-8''IMG_1-v2.jpg");
        assert_eq!(attachment("Été \"1\".jpg").to_str().unwrap(), "attachment; filename=\"_t_ _1_.jpg\"; filename*=UTF-8''%C3%89t%C3%A9%20%221%22.jpg");
    }

    /// A client that predates the destination picker sends no `destination` at all, and has to keep getting
    /// what it has always got: uploaded to Immich, stacked, local copy dropped.
    #[test]
    fn a_request_without_a_destination_still_means_immich() {
        let req: ExportRequest = serde_json::from_str(r#"{"assetKey":"a","editEtag":"b"}"#).unwrap();
        assert_eq!(req.destination, Destination::Immich);
        assert!(!req.destination.to_device());
        assert_eq!(req.format, "jpeg");
    }

    #[test]
    fn both_destinations_keep_the_file_for_the_browser() {
        for (json, want, to_device) in [
            (r#"{"assetKey":"a","editEtag":"b","destination":"device"}"#, Destination::Device, true),
            (r#"{"assetKey":"a","editEtag":"b","destination":"both"}"#, Destination::Both, true),
            (r#"{"assetKey":"a","editEtag":"b","destination":"immich"}"#, Destination::Immich, false),
        ] {
            let req: ExportRequest = serde_json::from_str(json).unwrap();
            assert_eq!(req.destination, want);
            assert_eq!(req.destination.to_device(), to_device, "{json}");
        }
    }

    /// `same_settings` decides whether a render is skipped entirely, so it must not start matching on the
    /// destination: the same file asked for twice, once for Immich and once for this phone, is one version.
    #[test]
    fn destination_is_not_part_of_what_makes_two_exports_the_same() {
        let entry = json!({ "editEtag": "b", "format": "jpeg", "colorSpace": "srgb", "quality": 92, "longEdge": null });
        let to_immich: ExportRequest = serde_json::from_str(r#"{"assetKey":"a","editEtag":"b"}"#).unwrap();
        let to_device: ExportRequest = serde_json::from_str(r#"{"assetKey":"a","editEtag":"b","destination":"device"}"#).unwrap();
        assert!(same_settings(&entry, &to_immich));
        assert!(same_settings(&entry, &to_device));
    }
}
