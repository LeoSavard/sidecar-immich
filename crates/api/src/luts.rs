// SPDX-License-Identifier: Apache-2.0
//! 3D LUTs uploaded as looks (DESIGN.md §15, "3D LUTs"). The api keeps them, in `<state>/luts/`, under the first
//! 16 hex digits of the file's SHA-256 — so an edit names the table it was made with, and the same file
//! uploaded twice is one LUT — beside a small record of the name it came under. The worker renders exports
//! through them, so each is handed to it on upload and again whenever the api starts.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::App;
use crate::session::ApiUser;

/// A 65³ table written out in text is about 7 MB; 64 MB leaves room for 129³, the largest accepted.
pub const LUT_LIMIT: usize = 64 << 20;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LutInfo {
    pub id: String,
    /// The file name it was uploaded under, without `.cube`.
    pub name: String,
    /// The file's own TITLE, when it has one.
    pub title: Option<String>,
    pub size: u32,
}

fn dir(app: &App) -> PathBuf {
    app.config.server.state_dir.join("luts")
}

fn valid_id(id: &str) -> bool {
    id.len() == 16
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub fn id_of(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))[..16].to_string()
}

/// `GET /api/luts`: every stored LUT, by name.
pub async fn list(State(app): State<Arc<App>>, ApiUser(_user): ApiUser) -> Response {
    let mut out: Vec<LutInfo> = std::fs::read_dir(dir(&app))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    Json(out).into_response()
}

#[derive(Deserialize)]
pub struct UploadQuery {
    name: String,
}

/// `POST /api/luts?name=…`, the `.cube` file as the body: checked, stored, handed to the worker.
pub async fn upload(
    State(app): State<Arc<App>>,
    ApiUser(_user): ApiUser,
    Query(q): Query<UploadQuery>,
    body: String,
) -> Response {
    let lut = match sidecar_color::lut::parse_cube(&body) {
        Ok(l) => l,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Not a usable .cube file: {e}"),
            )
                .into_response();
        }
    };
    let name: String = q
        .name
        .trim()
        .trim_end_matches(".cube")
        .chars()
        .take(120)
        .collect();
    let info = LutInfo {
        id: id_of(&body),
        name: if name.is_empty() { "LUT".into() } else { name },
        title: lut.title.clone(),
        size: lut.size,
    };
    let stored = (|| -> Result<()> {
        let d = dir(&app);
        std::fs::create_dir_all(&d)?;
        std::fs::write(d.join(format!("{}.cube", info.id)), &body)?;
        std::fs::write(
            d.join(format!("{}.json", info.id)),
            serde_json::to_vec(&info)?,
        )?;
        Ok(())
    })();
    if let Err(e) = stored {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Couldn't store the LUT: {e:#}"),
        )
            .into_response();
    }
    if let Err(e) = push(&app, &info.id, body).await {
        return (
            StatusCode::BAD_GATEWAY,
            format!("Stored, but the worker didn't take it, so exports can't use it yet: {e:#}"),
        )
            .into_response();
    }
    Json(info).into_response()
}

/// `GET /api/luts/{id}`: the table for the browser's own pass — the side as a little-endian u32, the domain's
/// six floats (from, to), then RGBA floats, red fastest. Immutable: the id is the file's hash.
pub async fn table(
    State(app): State<Arc<App>>,
    ApiUser(_user): ApiUser,
    Path(id): Path<String>,
) -> Response {
    if !valid_id(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(text) = std::fs::read_to_string(dir(&app).join(format!("{id}.cube"))) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let lut = match sidecar_color::lut::parse_cube(&text) {
        Ok(l) => l,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    let mut out = Vec::with_capacity(28 + lut.table.len() * 16);
    out.extend_from_slice(&lut.size.to_le_bytes());
    for v in lut
        .domain_min
        .iter()
        .chain(&lut.domain_max)
        .chain(&lut.rgba())
    {
        out.extend_from_slice(&v.to_le_bytes());
    }
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            ),
        ],
        out,
    )
        .into_response()
}

/// `DELETE /api/luts/{id}`. Edits that name it keep its name, and render without it until it is uploaded again.
pub async fn remove(
    State(app): State<Arc<App>>,
    ApiUser(_user): ApiUser,
    Path(id): Path<String>,
) -> Response {
    if !valid_id(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let d = dir(&app);
    let _ = std::fs::remove_file(d.join(format!("{id}.json")));
    if std::fs::remove_file(d.join(format!("{id}.cube"))).is_err() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let _ = app
        .http
        .delete(app.worker(&format!("/v1/luts/{id}")))
        .send()
        .await;
    StatusCode::NO_CONTENT.into_response()
}

async fn push(app: &App, id: &str, text: String) -> Result<()> {
    let resp = app
        .http
        .put(app.worker(&format!("/v1/luts/{id}")))
        .body(text)
        .send()
        .await
        .context("the worker didn't answer")?;
    let status = resp.status();
    anyhow::ensure!(
        status.is_success(),
        "{status}: {}",
        resp.text().await.unwrap_or_default()
    );
    Ok(())
}

impl App {
    /// At start: hand the worker every stored LUT, which it may have lost with its cache.
    pub(crate) fn start_lut_sync(self: &Arc<Self>) {
        let app = self.clone();
        tokio::spawn(async move {
            let files: Vec<PathBuf> = std::fs::read_dir(dir(&app))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "cube"))
                .collect();
            for _attempt in 0..30 {
                let mut failed = 0;
                for p in &files {
                    let (Some(id), Ok(text)) = (
                        p.file_stem().and_then(|s| s.to_str()),
                        std::fs::read_to_string(p),
                    ) else {
                        continue;
                    };
                    if push(&app, id, text).await.is_err() {
                        failed += 1;
                    }
                }
                if failed == 0 {
                    if !files.is_empty() {
                        tracing::info!("luts: handed the worker {} stored LUTs", files.len());
                    }
                    return;
                }
                // The worker may still be starting.
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
            tracing::warn!(
                "luts: the worker wouldn't take the stored LUTs; exports through them will fail"
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_the_files_hash() {
        let id = id_of("LUT_3D_SIZE 2\n");
        assert!(valid_id(&id));
        assert_eq!(id, id_of("LUT_3D_SIZE 2\n"));
        assert_ne!(id, id_of("LUT_3D_SIZE 3\n"));
        assert!(!valid_id("../../etc/passwd"));
        assert!(!valid_id("ABCDEF0123456789"));
    }
}
