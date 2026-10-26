// SPDX-License-Identifier: Apache-2.0
//! Sign-in with an Immich API key. The key is stored server-side, encrypted with ChaCha20-Poly1305 under a
//! per-install secret; the browser holds only an opaque session id in an HttpOnly, SameSite=Lax cookie.

use crate::{App, store::random_hex};
use anyhow::{Context, Result, anyhow};
use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, aead::AeadInPlace};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const COOKIE: &str = "sidecar_session";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRecord {
    user_id: String,
    name: String,
    email: String,
    is_admin: bool,
    #[serde(default)]
    storage_label: Option<String>,
    api_key_sealed: String,
    created_at: String,
}

#[derive(Clone, Debug)]
pub struct User {
    pub id: String,
    pub name: String,
    pub email: String,
    pub api_key: String,
    pub storage_label: Option<String>,
}

impl User {
    /// This user's folder inside Immich's internal library.
    pub fn library_folder(&self) -> &str {
        self.storage_label
            .as_deref()
            .filter(|l| !l.is_empty())
            .unwrap_or(&self.id)
    }
}

pub fn load_secret(state_dir: &std::path::Path) -> Result<[u8; 32]> {
    let path = state_dir.join("secret.key");
    if let Ok(bytes) = std::fs::read(&path) {
        return bytes
            .try_into()
            .map_err(|_| anyhow!("{} is not 32 bytes", path.display()));
    }
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|e| anyhow!("{e}"))?;
    std::fs::write(&path, key).with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(key)
}

fn seal(secret: &[u8; 32], plain: &str) -> String {
    let cipher = ChaCha20Poly1305::new(secret.into());
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).expect("system randomness");
    let mut buf = plain.as_bytes().to_vec();
    cipher
        .encrypt_in_place(&nonce.into(), b"sidecar-api-key", &mut buf)
        .expect("encryption");
    let mut out = nonce.to_vec();
    out.extend(buf);
    B64.encode(out)
}

fn open(secret: &[u8; 32], sealed: &str) -> Option<String> {
    let bytes = B64.decode(sealed).ok()?;
    let (nonce, ct) = bytes.split_at_checked(12)?;
    let cipher = ChaCha20Poly1305::new(secret.into());
    let mut buf = ct.to_vec();
    cipher
        .decrypt_in_place(nonce.into(), b"sidecar-api-key", &mut buf)
        .ok()?;
    String::from_utf8(buf).ok()
}

fn cookie_value<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    parts
        .headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|kv| {
            let (k, v) = kv.trim().split_once('=')?;
            (k == name).then_some(v)
        })
}

async fn user_from(parts: &Parts, app: &App) -> Option<User> {
    if !app.immich.enabled() {
        return Some(User {
            id: "dev".into(),
            name: "Developer".into(),
            email: String::new(),
            api_key: String::new(),
            storage_label: None,
        });
    }
    let sid = cookie_value(parts, COOKIE)?;
    let rec: SessionRecord = app.store.get("sessions", sid).await?;
    Some(User {
        id: rec.user_id,
        name: rec.name,
        email: rec.email,
        api_key: open(&app.secret, &rec.api_key_sealed)?,
        storage_label: rec.storage_label,
    })
}

/// For `/api/*`: 401 when signed out.
pub struct ApiUser(pub User);

impl FromRequestParts<Arc<App>> for ApiUser {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, app: &Arc<App>) -> Result<Self, Response> {
        user_from(parts, app)
            .await
            .map(ApiUser)
            .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Sign in to Sidecar first.").into_response())
    }
}

/// For pages such as `/open`: redirect to sign-in, then back.
pub struct PageUser(pub User);

impl FromRequestParts<Arc<App>> for PageUser {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, app: &Arc<App>) -> Result<Self, Response> {
        match user_from(parts, app).await {
            Some(u) => Ok(PageUser(u)),
            None => {
                let next = parts
                    .uri
                    .path_and_query()
                    .map(|p| p.as_str())
                    .unwrap_or("/");
                let encoded: String = next
                    .bytes()
                    .map(|b| {
                        if b.is_ascii_alphanumeric() || b"-_.~/".contains(&b) {
                            (b as char).to_string()
                        } else {
                            format!("%{b:02X}")
                        }
                    })
                    .collect();
                Err(Redirect::to(&format!("/signin?next={encoded}")).into_response())
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignIn {
    api_key: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    name: String,
    email: String,
    auth_required: bool,
    /// Whether an Immich library is configured at all. Equal to `auth_required` today — signing in *is*
    /// signing in to Immich — but the export sheet asks this one, because "is there somewhere to upload to"
    /// and "does this deployment have accounts" are not the same question and won't always share an answer.
    immich: bool,
    /// The name of the editing folder, when one is configured: a root Immich knows nothing about, which a
    /// photo can be imported into instead of the library (DESIGN.md §3). `None` where there is none.
    intake_folder: Option<String>,
    /// Whether this image ships the mobile module at `/m` (docs/MOBILE.md), so the home page can offer it.
    mobile: bool,
}

pub async fn sign_in(State(app): State<Arc<App>>, Json(body): Json<SignIn>) -> Response {
    let key = body.api_key.trim();
    let me = match app.immich.me(key).await {
        Ok(me) => me,
        Err(e) => return (StatusCode::UNAUTHORIZED, format!("{e:#}")).into_response(),
    };
    let sid = random_hex(32);
    let rec = SessionRecord {
        user_id: me.id,
        name: me.name.clone(),
        email: me.email.clone(),
        is_admin: me.is_admin,
        storage_label: me.storage_label.clone(),
        api_key_sealed: seal(&app.secret, key),
        created_at: sidecar_xmp::utc_now(),
    };
    if let Err(e) = app.store.put("sessions", &sid, &rec).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")).into_response();
    }
    let cookie = format!(
        "{COOKIE}={sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000{}",
        if app.config.server.secure_cookies {
            "; Secure"
        } else {
            ""
        }
    );
    (
        [(header::SET_COOKIE, cookie)],
        Json(SessionInfo {
            name: me.name,
            email: me.email,
            auth_required: true,
            immich: app.immich.enabled(),
            intake_folder: app.config.intake_root().map(|r| r.name.clone()),
            mobile: app.config.mobile_root().is_some(),
        }),
    )
        .into_response()
}

pub async fn session(State(app): State<Arc<App>>, parts: Parts) -> Response {
    match user_from(&parts, &app).await {
        Some(u) => Json(SessionInfo {
            name: u.name,
            email: u.email,
            auth_required: app.immich.enabled(),
            immich: app.immich.enabled(),
            intake_folder: app.config.intake_root().map(|r| r.name.clone()),
            mobile: app.config.mobile_root().is_some(),
        })
        .into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

pub async fn sign_out(State(app): State<Arc<App>>, parts: Parts) -> Response {
    if let Some(sid) = cookie_value(&parts, COOKIE) {
        app.store.delete("sessions", sid).await;
    }
    (
        [(
            header::SET_COOKIE,
            format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
        )],
        StatusCode::NO_CONTENT,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    #[test]
    fn seal_round_trips_and_rejects_tampering() {
        let secret = [7u8; 32];
        let sealed = super::seal(&secret, "immich-key");
        assert_eq!(super::open(&secret, &sealed).as_deref(), Some("immich-key"));
        assert!(super::open(&[8u8; 32], &sealed).is_none());
    }
}
