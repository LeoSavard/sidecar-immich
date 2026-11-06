// SPDX-License-Identifier: Apache-2.0
//! Sessions. With an Immich, sign-in is with an Immich API key, stored server-side and encrypted with
//! ChaCha20-Poly1305 under a per-install secret. Without one, it is a local account's username and password
//! (`accounts.rs`). Either way the browser holds only an opaque session id in an HttpOnly, SameSite=Lax cookie.

use crate::{App, SignInMode, accounts, store::random_hex};
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
    /// The Immich API key, sealed; empty for a local account.
    #[serde(default)]
    api_key_sealed: String,
    /// A local account's credential when the session began (`accounts::Account::credential`): the session
    /// ends when the account's password changes.
    #[serde(default)]
    credential: Option<String>,
    created_at: String,
}

#[derive(Clone, Debug)]
pub struct User {
    pub id: String,
    pub name: String,
    pub email: String,
    pub api_key: String,
    pub storage_label: Option<String>,
    /// May manage the local accounts. Immich's own administrators are Immich's business, not Sidecar's.
    pub is_admin: bool,
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
    let rec = || async {
        let sid = cookie_value(parts, COOKIE)?;
        app.store.get::<SessionRecord>("sessions", sid).await
    };
    match &app.sign_in {
        SignInMode::Open => Some(User {
            id: "dev".into(),
            name: "Developer".into(),
            email: String::new(),
            api_key: String::new(),
            storage_label: None,
            is_admin: false,
        }),
        SignInMode::Immich => {
            let rec = rec().await?;
            Some(User {
                id: rec.user_id,
                name: rec.name,
                email: rec.email,
                api_key: open(&app.secret, &rec.api_key_sealed)?,
                storage_label: rec.storage_label,
                is_admin: false,
            })
        }
        SignInMode::Accounts(_) => {
            let rec = rec().await?;
            let account: accounts::Account = app.store.get("accounts", &rec.user_id).await?;
            (rec.credential.as_deref() == Some(account.credential.as_str())).then(|| account.user())
        }
    }
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

/// `{ apiKey }` for Immich, `{ username, password }` for a local account.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum SignIn {
    #[serde(rename_all = "camelCase")]
    Immich {
        api_key: String,
    },
    Password(accounts::Credentials),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    id: String,
    name: String,
    email: String,
    auth_required: bool,
    /// How this Sidecar signs people in: `immich`, `password` or `none`.
    sign_in: &'static str,
    /// May manage the local accounts.
    is_admin: bool,
    /// Whether an Immich library is configured at all. Not the same question as `auth_required`: a Sidecar
    /// with local accounts asks for sign-in and has no Immich to upload to.
    immich: bool,
    /// The name of the editing folder, when one is configured: a root Immich knows nothing about, which a
    /// photo can be imported into instead of the library (DESIGN.md §3). `None` where there is none.
    intake_folder: Option<String>,
    /// Whether this image ships the mobile module at `/m` (docs/MOBILE.md), so the home page can offer it.
    mobile: bool,
}

impl SessionInfo {
    fn of(app: &App, user: &User) -> Self {
        Self {
            id: user.id.clone(),
            name: user.name.clone(),
            email: user.email.clone(),
            auth_required: !matches!(app.sign_in, SignInMode::Open),
            sign_in: match app.sign_in {
                SignInMode::Immich => "immich",
                SignInMode::Accounts(_) => "password",
                SignInMode::Open => "none",
            },
            is_admin: user.is_admin,
            immich: app.immich.is_some(),
            intake_folder: app.config.intake_root().map(|r| r.name.clone()),
            mobile: app.config.mobile_root().is_some(),
        }
    }
}

/// Store a session for `rec` and answer with its cookie and what `GET /api/session` would say.
async fn begin(app: &App, rec: SessionRecord, user: &User) -> Response {
    let sid = random_hex(32);
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
        Json(SessionInfo::of(app, user)),
    )
        .into_response()
}

/// A session for a local account that has just proved who it is.
pub async fn start(app: &App, account: &accounts::Account) -> Response {
    let rec = SessionRecord {
        user_id: account.id.clone(),
        name: account.name.clone(),
        email: String::new(),
        is_admin: account.is_admin,
        storage_label: None,
        api_key_sealed: String::new(),
        credential: Some(account.credential.clone()),
        created_at: sidecar_xmp::utc_now(),
    };
    begin(app, rec, &account.user()).await
}

pub async fn sign_in(State(app): State<Arc<App>>, Json(body): Json<SignIn>) -> Response {
    let key = match body {
        SignIn::Password(c) => return accounts::sign_in(&app, c).await,
        SignIn::Immich { api_key } => api_key,
    };
    let Some(immich) = &app.immich else {
        return (
            StatusCode::BAD_REQUEST,
            "This Sidecar signs in with a username and password, not an Immich key.",
        )
            .into_response();
    };
    let key = key.trim();
    let me = match immich.me(key).await {
        Ok(me) => me,
        Err(e) => return (StatusCode::UNAUTHORIZED, format!("{e:#}")).into_response(),
    };
    let rec = SessionRecord {
        user_id: me.id.clone(),
        name: me.name.clone(),
        email: me.email.clone(),
        is_admin: me.is_admin,
        storage_label: me.storage_label.clone(),
        api_key_sealed: seal(&app.secret, key),
        credential: None,
        created_at: sidecar_xmp::utc_now(),
    };
    let user = User {
        id: me.id,
        name: me.name,
        email: me.email,
        api_key: key.to_string(),
        storage_label: me.storage_label,
        is_admin: false,
    };
    begin(&app, rec, &user).await
}

pub async fn session(State(app): State<Arc<App>>, parts: Parts) -> Response {
    match user_from(&parts, &app).await {
        Some(u) => Json(SessionInfo::of(&app, &u)).into_response(),
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
