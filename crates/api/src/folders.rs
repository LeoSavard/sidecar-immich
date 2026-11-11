// SPDX-License-Identifier: Apache-2.0
//! Browsing folders (DESIGN.md §3, "Browsing folders"): a file browser over the folders Immich doesn't stand in
//! front of — the editing folder, and a library Sidecar is given without Immich — so a photo there can be
//! opened by finding it rather than by knowing its path. A browser, not a catalogue: each request lists one
//! folder as it is on disk, nothing is indexed or kept, and looking never writes anything.
//!
//! A library Immich serves is never browsed here: Immich decides who may see which of its photos, and listing
//! its folders would step round that.

use std::path::{Component, Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::config::Root;
use crate::resolve::Candidate;
use crate::session::ApiUser;
use crate::{App, admit};

/// The most entries one listing returns; a folder with more says it was cut short.
const MAX_ENTRIES: usize = 5_000;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderRoot {
    index: usize,
    name: String,
    intake: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    name: String,
    size: u64,
    /// Unix seconds.
    modified: u64,
    /// A sidecar sits beside it: it has been edited, here or elsewhere.
    edited: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    root: String,
    /// The folder listed, relative to the root, `/`-separated; empty for the root itself.
    dir: String,
    folders: Vec<String>,
    files: Vec<FileEntry>,
    truncated: bool,
}

fn browsable(app: &App, root: &Root) -> bool {
    root.immich_prefix.is_none() || app.immich.is_none()
}

/// `rel` under the root's folder for this user, or `None` if it would leave it: only plain names, and the
/// result, links resolved, still inside.
fn inside(root: &Root, user: &crate::session::User, rel: &str) -> Option<PathBuf> {
    let base = crate::resolve::mount_for(root, user).canonicalize().ok()?;
    let rel = FsPath::new(rel.trim_matches('/'));
    if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let real = base.join(rel).canonicalize().ok()?;
    real.starts_with(&base).then_some(real)
}

/// `GET /api/folders` — the folders this person may browse.
pub async fn roots(State(app): State<Arc<App>>, ApiUser(_user): ApiUser) -> Response {
    let out: Vec<FolderRoot> = app
        .config
        .roots
        .iter()
        .enumerate()
        .filter(|(_, r)| browsable(&app, r))
        .map(|(index, r)| FolderRoot {
            index,
            name: r.name.clone(),
            intake: r.intake,
        })
        .collect();
    Json(out).into_response()
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    dir: String,
}

/// `GET /api/folders/{root}/list?dir=…` — one folder: its subfolders, and the photos in it.
pub async fn list(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(index): Path<usize>,
    Query(q): Query<ListQuery>,
) -> Response {
    let Some(root) = app.config.roots.get(index).filter(|r| browsable(&app, r)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(dir) = inside(root, &user, &q.dir).filter(|d| d.is_dir()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let name = root.name.clone();
    let rel = q.dir.trim_matches('/').to_string();
    let listed = tokio::task::spawn_blocking(move || -> std::io::Result<Listing> {
        let (mut folders, mut files, mut truncated) = (Vec::new(), Vec::new(), false);
        for entry in std::fs::read_dir(&dir)?.flatten() {
            if folders.len() + files.len() >= MAX_ENTRIES {
                truncated = true;
                break;
            }
            let Some(fname) = entry.file_name().to_str().map(String::from) else {
                continue;
            };
            if fname.starts_with('.') {
                continue;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                folders.push(fname);
            } else if ft.is_file() && sidecar_template::is_editable(&entry.path()) {
                let md = entry.metadata()?;
                files.push(FileEntry {
                    edited: sidecar_xmp::sidecar_path_for(&entry.path()).exists(),
                    name: fname,
                    size: md.len(),
                    modified: md
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_secs()),
                });
            }
        }
        // Dated folders (2026, 2026-11-06) newest first, as a photo library is read; named ones alphabetically.
        let dated = |f: &str| f.starts_with(|c: char| c.is_ascii_digit());
        folders.sort_by(|a, b| match (dated(a), dated(b)) {
            (true, true) => b.cmp(a),
            (false, false) => a.to_lowercase().cmp(&b.to_lowercase()),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
        });
        files.sort_by_key(|f| f.name.to_lowercase());
        Ok(Listing {
            root: name,
            dir: rel,
            folders,
            files,
            truncated,
        })
    })
    .await;
    match listed {
        Ok(Ok(l)) => Json(l).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response(),
    }
}

#[derive(Deserialize)]
pub struct OpenBody {
    path: String,
}

/// `POST /api/folders/{root}/open { path }` — open a photo found by browsing: admitted as `/open` admits one,
/// and answered with its key.
pub async fn open(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(index): Path<usize>,
    Json(body): Json<OpenBody>,
) -> Response {
    let Some(root) = app.config.roots.get(index).filter(|r| browsable(&app, r)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(path) = inside(root, &user, &body.path).filter(|p| p.is_file()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let file_name = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !sidecar_template::is_editable(&path) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            crate::cant_open(&file_name),
        )
            .into_response();
    }
    let c = Candidate {
        path,
        file_name,
        immich_asset_id: None,
        taken: None,
    };
    match admit(&app, &user, c).await {
        Ok(key) => Json(serde_json::json!({ "key": key })).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, format!("{e:#}")).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only plain names below the root, links resolved: `..`, an absolute path and a link out of the root are
    /// all refused, so browsing can't reach a file the root doesn't hold.
    #[test]
    fn paths_stay_inside_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("lib");
        std::fs::create_dir_all(lib.join("2019/Holidays")).unwrap();
        std::fs::write(tmp.path().join("secret.NEF"), b"raw").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path(), lib.join("out")).unwrap();
        let root: Root = toml::from_str(&format!(
            "name = \"Lib\"\nmount = \"{}\"\ntemplate = \"{{{{filename}}}}\"",
            lib.display()
        ))
        .unwrap();
        let user = crate::session::User {
            id: "u".into(),
            name: "U".into(),
            email: String::new(),
            api_key: String::new(),
            storage_label: None,
        };
        assert!(inside(&root, &user, "").is_some());
        assert!(inside(&root, &user, "2019/Holidays").is_some());
        assert!(
            inside(&root, &user, "/2019/").is_some(),
            "slashes around a path are trimmed"
        );
        assert!(inside(&root, &user, "../secret.NEF").is_none());
        assert!(inside(&root, &user, "2019/../../secret.NEF").is_none());
        assert!(
            inside(&root, &user, "out/secret.NEF").is_none(),
            "a link out of the root"
        );
        assert!(inside(&root, &user, "missing").is_none());
    }
}
