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
    if let Some(k) = &idem {
        if let Some(id) = app.jobs.by_idempotency.lock().unwrap().get(k).cloned() {
            return (StatusCode::ACCEPTED, Json(json!({ "jobId": id }))).into_response();
        }
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
    let exports = doc.exports();
    if let Some(existing) = exports.iter().find(|e| same_settings(e, &req)) {
        return (StatusCode::OK, Json(json!({ "existing": existing }))).into_response();
    }

    if !app.jobs.numbering.lock().unwrap().insert(req.asset_key.clone()) {
        return (StatusCode::CONFLICT, "An export of this photo is being prepared; try again in a moment.").into_response();
    }
    let version = exports.iter().find(|e| e["editEtag"] == req.edit_etag).and_then(|e| e["version"].as_u64()).unwrap_or_else(|| exports.iter().filter_map(|e| e["version"].as_u64()).max().unwrap_or(0) + 1);
    let ext = if req.format == "tiff" { "tif" } else { "jpg" };
    let size_suffix = req.long_edge.map(|e| format!("-{e}px")).unwrap_or_default();
    let file_name = format!("{}-v{version}{size_suffix}.{ext}", file_stem(&rec.file_name));

    let job = Arc::new(Job { id: random_hex(16), user_id: user.id.clone(), events: Mutex::new(Vec::new()), notify: tokio::sync::Notify::new() });
    app.jobs.by_id.lock().unwrap().insert(job.id.clone(), job.clone());
    if let Some(k) = idem {
        app.jobs.by_idempotency.lock().unwrap().insert(k, job.id.clone());
    }
    job.push("queued", json!({ "version": version, "file": file_name }));
    let (app2, job2, edit, name2) = (app.clone(), job.clone(), doc.edit(), file_name.clone());
    tokio::spawn(async move {
        let outcome = run(&app2, &job2, &user, &rec, &req, edit, version, &name2).await;
        app2.jobs.numbering.lock().unwrap().remove(&req.asset_key);
        if let Err(e) = outcome {
            tracing::error!("export {}: {e:#}", job2.id);
            job2.push("failed", json!({ "message": format!("{e:#}"), "retryable": true }));
        }
    });
    (StatusCode::ACCEPTED, Json(json!({ "jobId": job.id, "version": version, "file": file_name }))).into_response()
}

#[allow(clippy::too_many_arguments)]
async fn run(app: &Arc<App>, job: &Job, user: &crate::session::User, rec: &AssetRecord, req: &ExportRequest, edit: EditParams, version: u64, file_name: &str) -> Result<()> {
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

    let mut immich_asset_id: Option<String> = None;
    if app.immich.enabled() {
        job.push("progress", json!({ "stage": "upload", "overall": 0.88 }));
        let file = app.http.get(app.worker(&format!("/v1/outbox/{}/{}", job.id, file_name))).send().await?.error_for_status()?.bytes().await?;
        let meta: Value = app.http.get(app.worker(&format!("/v1/assets/{}/meta", req.asset_key))).send().await?.json().await?;
        let captured = meta["info"]["capturedUnix"].as_i64().filter(|&t| t > 0).map(unix_to_rfc3339).unwrap_or_else(sidecar_xmp::utc_now);
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

    let entry = json!({
        "version": version, "editEtag": req.edit_etag, "file": file_name, "format": req.format,
        "colorSpace": req.color_space, "quality": req.quality, "longEdge": req.long_edge,
        "fileSha1": done["sha1"], "exported": sidecar_xmp::utc_now(), "immichAssetId": immich_asset_id,
    });
    let roots = app.config.canonical_mounts();
    let path = rec.path.clone();
    let entry2 = entry.clone();
    tokio::task::spawn_blocking(move || sidecar_xmp::update(&path, &roots, |d| {
        d.append_export(entry2.clone());
        Ok(())
    }))
    .await??;

    if immich_asset_id.is_some() {
        let _ = app.http.delete(app.worker(&format!("/v1/outbox/{}", job.id))).send().await;
    }
    job.push("done", json!({ "version": version, "file": file_name, "bytes": done["bytes"], "width": done["width"], "height": done["height"], "immichAssetId": immich_asset_id, "download": immich_asset_id.is_none().then(|| format!("/api/exports/{}/file", job.id)) }));
    Ok(())
}

fn unix_to_rfc3339(t: i64) -> String {
    let days = t.div_euclid(86400);
    let rem = t.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z", rem / 3600, rem / 60 % 60, rem % 60)
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
    resp.headers_mut().insert("content-disposition", format!("attachment; filename=\"{name}\"").parse().unwrap());
    resp
}
