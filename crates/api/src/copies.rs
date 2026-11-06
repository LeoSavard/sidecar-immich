// SPDX-License-Identifier: Apache-2.0
//! Virtual copies (DESIGN.md §15, "Virtual copies"): further entries for one photo, each a head of its own in
//! the photo's history (`sidecar_xmp::VirtualCopy`). A copy's key is `<key>~<copy id>`; the edit routes take it
//! as they take the photo's own, and everything about the file itself — proxy, masks, metadata — stays keyed on
//! the photo's.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::session::ApiUser;
use crate::{App, RecentItem, UserRecord, asset_for, valid_key, write_sidecar};

#[derive(Deserialize)]
pub struct NewCopy {
    name: String,
    /// The version it starts from; the photo's current one when absent.
    #[serde(default)]
    from: Option<String>,
}

#[derive(Deserialize)]
pub struct Rename {
    name: String,
}

fn copy_name(name: &str) -> Option<String> {
    let n: String = name.trim().chars().take(60).collect();
    (!n.is_empty()).then_some(n)
}

/// Recent lists a copy as its own entry, by its own key, so it opens as itself from the home page.
async fn remember(
    app: &App,
    user_id: &str,
    key: &str,
    file_name: &str,
    copy: &sidecar_xmp::VirtualCopy,
) {
    let item = RecentItem {
        asset_key: format!("{key}~{}", copy.id),
        file_name: format!("{file_name} — {}", copy.name),
        immich_asset_id: None,
        opened_at: sidecar_xmp::utc_now(),
    };
    let _ = app
        .store
        .update("users", user_id, |found: Option<UserRecord>| {
            let mut u = found.unwrap_or_default();
            u.recent.retain(|r| r.asset_key != item.asset_key);
            u.recent.insert(0, item.clone());
            u.recent.truncate(crate::RECENT_KEPT);
            u
        })
        .await;
}

/// `POST /api/assets/{key}/copies { name, from? }` — a new virtual copy. Answers with the photo's edit state,
/// whose `copies` ends with the new one.
pub async fn create(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(key): Path<String>,
    Json(body): Json<NewCopy>,
) -> Response {
    if !valid_key(&key) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(name) = copy_name(&body.name) else {
        return (StatusCode::BAD_REQUEST, "A virtual copy needs a name.").into_response();
    };
    let Some(rec) = asset_for(&app, &user, &key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let made = Arc::new(std::sync::Mutex::new(None));
    let (into, n) = (made.clone(), name.clone());
    let resp = write_sidecar(&app, &key, None, &rec, move |doc| {
        *into.lock().unwrap() = Some(doc.add_copy(&n, body.from.as_deref())?);
        Ok(false)
    })
    .await;
    let created = made.lock().unwrap().clone();
    if let Some(copy) = created
        && resp.status().is_success()
    {
        remember(&app, &user.id, &key, &rec.file_name, &copy).await;
    }
    resp
}

/// `PUT /api/assets/{key}/copies/{id} { name }`.
pub async fn rename(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path((key, id)): Path<(String, String)>,
    Json(body): Json<Rename>,
) -> Response {
    let Some(name) = copy_name(&body.name) else {
        return (StatusCode::BAD_REQUEST, "A virtual copy needs a name.").into_response();
    };
    let Some(rec) = asset_for(&app, &user, &key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let n = name.clone();
    let i = id.clone();
    let resp = write_sidecar(&app, &key, None, &rec, move |doc| {
        doc.rename_copy(&i, &n).map(|()| false)
    })
    .await;
    if resp.status().is_success() {
        let copy = sidecar_xmp::VirtualCopy {
            id,
            name,
            head: String::new(),
        };
        remember(&app, &user.id, &key, &rec.file_name, &copy).await;
    }
    resp
}

/// `DELETE /api/assets/{key}/copies/{id}` — the copy goes; its versions stay in the photo's history.
pub async fn remove(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path((key, id)): Path<(String, String)>,
) -> Response {
    let Some(rec) = asset_for(&app, &user, &key).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let i = id.clone();
    let resp = write_sidecar(&app, &key, None, &rec, move |doc| {
        doc.remove_copy(&i).map(|()| false)
    })
    .await;
    if resp.status().is_success() {
        let gone = format!("{key}~{id}");
        let _ = app
            .store
            .update("users", &user.id, |found: Option<UserRecord>| {
                let mut u = found.unwrap_or_default();
                u.recent.retain(|r| r.asset_key != gone);
                u
            })
            .await;
    }
    resp
}
