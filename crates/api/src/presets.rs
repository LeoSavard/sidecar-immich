// SPDX-License-Identifier: Apache-2.0
//! Presets (DESIGN.md §15, "Presets, copy and paste, sync"): a named set of the editor's sections and the values
//! they hold, kept per person. Applying one takes those sections into a photo's edit and leaves the rest, as
//! pasting a copied look does; which fields a section carries is the web app's (`PICKS` in `edit-view.ts`), so a
//! control added to a section travels with its presets without this file changing.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sidecar_color::EditParams;

use crate::App;
use crate::session::ApiUser;
use crate::store::random_hex;

/// A person's presets: few, so one record each.
const MAX_PRESETS: usize = 500;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub id: String,
    pub name: String,
    /// The sections it carries, by the editor's ids (`wb`, `light`, `film`…).
    pub sections: Vec<String>,
    /// An edit holding those sections' values; its other sections are defaults and are never applied.
    pub edit: EditParams,
    /// Unix seconds.
    pub created: u64,
}

#[derive(Default, Serialize, Deserialize)]
struct Presets {
    presets: Vec<Preset>,
}

#[derive(Deserialize)]
pub struct NewPreset {
    name: String,
    sections: Vec<String>,
    edit: EditParams,
}

fn valid_section(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// `GET /api/presets`: the person's presets, by name.
pub async fn list(State(app): State<Arc<App>>, ApiUser(user): ApiUser) -> Response {
    let mut p: Presets = app.store.get("presets", &user.id).await.unwrap_or_default();
    p.presets.sort_by_key(|x| x.name.to_lowercase());
    Json(p.presets).into_response()
}

/// `POST /api/presets` with `{ name, sections, edit }`.
pub async fn create(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Json(new): Json<NewPreset>,
) -> Response {
    let name: String = new.name.trim().chars().take(80).collect();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "A preset needs a name.").into_response();
    }
    if new.sections.is_empty() || !new.sections.iter().all(|s| valid_section(s)) {
        return (
            StatusCode::BAD_REQUEST,
            "A preset needs at least one section.",
        )
            .into_response();
    }
    let preset = Preset {
        id: random_hex(8),
        name,
        sections: new.sections,
        edit: new.edit,
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    let full = std::sync::atomic::AtomicBool::new(false);
    let saved = app
        .store
        .update("presets", &user.id, |found: Option<Presets>| {
            let mut p = found.unwrap_or_default();
            if p.presets.len() >= MAX_PRESETS {
                full.store(true, std::sync::atomic::Ordering::Relaxed);
            } else {
                p.presets.push(preset.clone());
            }
            p
        })
        .await;
    match saved {
        Ok(_) if full.into_inner() => (
            StatusCode::CONFLICT,
            format!("You have {MAX_PRESETS} presets already; delete some first."),
        )
            .into_response(),
        Ok(_) => Json(preset).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}

/// `DELETE /api/presets/{id}`.
pub async fn remove(
    State(app): State<Arc<App>>,
    ApiUser(user): ApiUser,
    Path(id): Path<String>,
) -> Response {
    let gone = std::sync::atomic::AtomicBool::new(false);
    let saved = app
        .store
        .update("presets", &user.id, |found: Option<Presets>| {
            let mut p = found.unwrap_or_default();
            let before = p.presets.len();
            p.presets.retain(|x| x.id != id);
            gone.store(
                p.presets.len() < before,
                std::sync::atomic::Ordering::Relaxed,
            );
            p
        })
        .await;
    match saved {
        Ok(_) if gone.into_inner() => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response(),
    }
}
