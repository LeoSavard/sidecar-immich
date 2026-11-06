// SPDX-License-Identifier: Apache-2.0
//! Local accounts, for a Sidecar with no Immich to sign in through: one person, or a household. Each has a
//! username and an Argon2id-hashed password; an administrator adds the others, resets a forgotten password and
//! removes an account. No second factor, no e-mail, no self-registration — that is an office's sign-in, and
//! this is a family's.
//!
//! The first account is made in the browser with a one-time setup code that the server prints to its log, so
//! whoever reaches a fresh server first cannot claim it without also reading the server's output. A forgotten
//! administrator password is `sidecar account set-password` on the server itself.
//!
//! Sessions carry the account's `credential`, which changes whenever its password does: resetting or changing
//! a password signs that account out everywhere, and a removed account's sessions find nothing to belong to.

use crate::session::{self, User};
use crate::store::{Store, random_hex};
use crate::{App, SignInMode};
use anyhow::{Result, bail};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const MIN_PASSWORD: usize = 8;
/// Failed attempts on one username before it has to wait, and how long the first wait is. Each further
/// failure doubles it, up to `MAX_WAIT`. Argon2 already makes each guess cost the server a few tens of
/// milliseconds; this is what stops a patient script on the other side of the internet.
const FREE_ATTEMPTS: u32 = 5;
const FIRST_WAIT: Duration = Duration::from_secs(30);
const MAX_WAIT: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    /// What the person types to sign in: lowercase, unique.
    pub username: String,
    /// What Sidecar calls them.
    pub name: String,
    pub password_hash: String,
    pub is_admin: bool,
    /// Changes with every password set. A session is valid only while it holds the current one.
    pub credential: String,
    pub created_at: String,
}

/// What the browser is told about an account: never the hash or the credential.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    id: String,
    username: String,
    name: String,
    is_admin: bool,
}

impl From<&Account> for Person {
    fn from(a: &Account) -> Self {
        Self {
            id: a.id.clone(),
            username: a.username.clone(),
            name: a.name.clone(),
            is_admin: a.is_admin,
        }
    }
}

impl Account {
    pub fn user(&self) -> User {
        User {
            id: self.id.clone(),
            name: self.name.clone(),
            email: String::new(),
            api_key: String::new(),
            storage_label: None,
            is_admin: self.is_admin,
        }
    }
}

/// The accounts' in-memory half: the setup code and the failed-attempt counts. The accounts themselves are
/// files in the state directory, like everything else there.
pub struct Accounts {
    /// Held across every change to the set of accounts, so two requests can't both create the first one or
    /// both remove the last administrator.
    changes: tokio::sync::Mutex<()>,
    failures: Mutex<HashMap<String, (u32, Instant)>>,
    setup_code: String,
}

impl Default for Accounts {
    fn default() -> Self {
        let code = random_hex(4).to_ascii_uppercase();
        Self {
            changes: Default::default(),
            failures: Default::default(),
            setup_code: format!("{}-{}", &code[..4], &code[4..]),
        }
    }
}

impl Accounts {
    pub fn setup_code(&self) -> &str {
        &self.setup_code
    }

    /// How long `key` must still wait, if it has failed too often.
    fn wait(&self, key: &str) -> Option<Duration> {
        let f = self.failures.lock().unwrap();
        let &(n, at) = f.get(key)?;
        if n < FREE_ATTEMPTS {
            return None;
        }
        let wait = FIRST_WAIT
            .saturating_mul(1 << (n - FREE_ATTEMPTS).min(10))
            .min(MAX_WAIT);
        wait.checked_sub(at.elapsed())
    }

    fn failed(&self, key: &str) {
        let mut f = self.failures.lock().unwrap();
        let e = f.entry(key.to_string()).or_insert((0, Instant::now()));
        *e = (e.0 + 1, Instant::now());
    }

    fn succeeded(&self, key: &str) {
        self.failures.lock().unwrap().remove(key);
    }
}

pub fn normalize_username(raw: &str) -> Result<String> {
    let u = raw.trim().to_lowercase();
    let ok = (1..=32).contains(&u.len())
        && u.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && u.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if !ok {
        bail!(
            "A username is 1 to 32 letters, digits, dots, dashes or underscores, starting with a letter or digit."
        );
    }
    Ok(u)
}

fn check_password(p: &str) -> Result<()> {
    if p.chars().count() < MIN_PASSWORD {
        bail!("A password needs at least {MIN_PASSWORD} characters.");
    }
    if p.len() > 1024 {
        bail!("That password is too long.");
    }
    Ok(())
}

fn display_name(name: Option<&str>, username: &str) -> Result<String> {
    let n = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(username);
    if n.chars().count() > 64 {
        bail!("A name can be at most 64 characters.");
    }
    Ok(n.to_string())
}

fn hash(password: &str) -> String {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).expect("system randomness");
    let salt = SaltString::encode_b64(&salt).expect("a 16-byte salt");
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("argon2 with default parameters")
        .to_string()
}

fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| {
        argon2::Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}

/// A hash no password matches, checked against when the username is unknown, so a wrong username takes as
/// long to refuse as a wrong password and the timing doesn't say which names exist.
fn decoy() -> &'static str {
    static DECOY: OnceLock<String> = OnceLock::new();
    DECOY.get_or_init(|| hash(&random_hex(16)))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f)
        .await
        .expect("hashing panicked")
}

pub async fn all(store: &Store) -> Vec<Account> {
    let mut list: Vec<Account> = store.list("accounts").await;
    list.sort_by(|a, b| a.username.cmp(&b.username));
    list
}

pub async fn find(store: &Store, username: &str) -> Option<Account> {
    all(store)
        .await
        .into_iter()
        .find(|a| a.username == username)
}

/// A new account, or a new password for an existing one (which signs it out everywhere). What `sidecar
/// account set-password` runs, and the only way back in when no administrator remembers their password.
pub async fn set_password(store: &Store, username: &str, password: &str) -> Result<Account> {
    let username = normalize_username(username)?;
    check_password(password)?;
    let p = password.to_string();
    let password_hash = blocking(move || hash(&p)).await;
    let account = match find(store, &username).await {
        Some(a) => Account {
            password_hash,
            credential: random_hex(16),
            ..a
        },
        None => Account {
            id: random_hex(16),
            name: username.clone(),
            username,
            password_hash,
            // An account made from the server's own command line is its owner's.
            is_admin: true,
            credential: random_hex(16),
            created_at: sidecar_xmp::utc_now(),
        },
    };
    store.put("accounts", &account.id, &account).await?;
    Ok(account)
}

fn accounts(app: &App) -> Option<&Accounts> {
    match &app.sign_in {
        SignInMode::Accounts(a) => Some(a),
        _ => None,
    }
}

fn refuse(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, msg.into()).into_response()
}

fn too_many(wait: Duration) -> Response {
    refuse(
        StatusCode::TOO_MANY_REQUESTS,
        format!(
            "Too many attempts. Try again in {} seconds.",
            wait.as_secs().max(1)
        ),
    )
}

/// `GET /api/sign-in`, public: how this Sidecar signs people in, and whether it is waiting for its first
/// account.
pub async fn method(State(app): State<Arc<App>>) -> Response {
    let (method, setup) = match &app.sign_in {
        SignInMode::Immich => ("immich", false),
        SignInMode::Accounts(_) => ("password", all(&app.store).await.is_empty()),
        SignInMode::Open => ("none", false),
    };
    Json(serde_json::json!({ "method": method, "setup": setup })).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    username: String,
    password: String,
}

/// `POST /api/session` with a username and password.
pub async fn sign_in(app: &Arc<App>, body: Credentials) -> Response {
    let Some(acc) = accounts(app) else {
        return refuse(
            StatusCode::BAD_REQUEST,
            "This Sidecar doesn't use passwords.",
        );
    };
    let username = body.username.trim().to_lowercase();
    if let Some(w) = acc.wait(&username) {
        return too_many(w);
    }
    let account = find(&app.store, &username).await;
    let hash = account
        .as_ref()
        .map_or_else(|| decoy().to_string(), |a| a.password_hash.clone());
    let password = body.password;
    let ok = blocking(move || verify(&password, &hash)).await;
    match account.filter(|_| ok) {
        Some(a) => {
            acc.succeeded(&username);
            session::start(app, &a).await
        }
        None => {
            acc.failed(&username);
            refuse(
                StatusCode::UNAUTHORIZED,
                "That username and password don't match.",
            )
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    code: String,
    username: String,
    #[serde(default)]
    name: Option<String>,
    password: String,
}

/// `POST /api/accounts/setup`: the first account, an administrator, with the code from the server's log.
pub async fn setup(State(app): State<Arc<App>>, Json(body): Json<Setup>) -> Response {
    let Some(acc) = accounts(&app) else {
        return refuse(StatusCode::NOT_FOUND, "This Sidecar has no local accounts.");
    };
    if let Some(w) = acc.wait("#setup") {
        return too_many(w);
    }
    let _held = acc.changes.lock().await;
    if !all(&app.store).await.is_empty() {
        return refuse(
            StatusCode::CONFLICT,
            "This Sidecar already has an account. Sign in with it.",
        );
    }
    let bare = |c: &str| c.to_ascii_uppercase().replace(['-', ' '], "");
    if bare(&body.code) != bare(&acc.setup_code) {
        acc.failed("#setup");
        return refuse(
            StatusCode::FORBIDDEN,
            "That isn't the setup code. It is in the server's log, on the line that starts \"No accounts yet\".",
        );
    }
    let made = async {
        let username = normalize_username(&body.username)?;
        let name = display_name(body.name.as_deref(), &username)?;
        let mut a = set_password(&app.store, &username, &body.password).await?;
        a.name = name;
        app.store.put("accounts", &a.id, &a).await?;
        anyhow::Ok(a)
    }
    .await;
    match made {
        Ok(a) => {
            acc.succeeded("#setup");
            tracing::info!("created the first account, {}", a.username);
            session::start(&app, &a).await
        }
        Err(e) => refuse(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

/// The refusal for someone who isn't an administrator of local accounts; `None` for one who is.
fn not_admin(app: &App, user: &User) -> Option<Response> {
    if accounts(app).is_none() {
        return Some(refuse(
            StatusCode::NOT_FOUND,
            "This Sidecar has no local accounts.",
        ));
    }
    (!user.is_admin).then(|| {
        refuse(
            StatusCode::FORBIDDEN,
            "Only an administrator can manage accounts.",
        )
    })
}

/// `GET /api/accounts` (administrators).
pub async fn list(
    State(app): State<Arc<App>>,
    session::ApiUser(user): session::ApiUser,
) -> Response {
    if let Some(r) = not_admin(&app, &user) {
        return r;
    }
    let people: Vec<Person> = all(&app.store).await.iter().map(Person::from).collect();
    Json(people).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAccount {
    username: String,
    #[serde(default)]
    name: Option<String>,
    password: String,
    #[serde(default)]
    is_admin: bool,
}

/// `POST /api/accounts` (administrators): someone else in the household.
pub async fn create(
    State(app): State<Arc<App>>,
    session::ApiUser(user): session::ApiUser,
    Json(body): Json<NewAccount>,
) -> Response {
    if let Some(r) = not_admin(&app, &user) {
        return r;
    }
    let acc = accounts(&app).unwrap();
    let _held = acc.changes.lock().await;
    let made = async {
        let username = normalize_username(&body.username)?;
        if find(&app.store, &username).await.is_some() {
            bail!("There is already an account called {username}.");
        }
        let name = display_name(body.name.as_deref(), &username)?;
        check_password(&body.password)?;
        let p = body.password.clone();
        let a = Account {
            id: random_hex(16),
            username,
            name,
            password_hash: blocking(move || hash(&p)).await,
            is_admin: body.is_admin,
            credential: random_hex(16),
            created_at: sidecar_xmp::utc_now(),
        };
        app.store.put("accounts", &a.id, &a).await?;
        anyhow::Ok(a)
    }
    .await;
    match made {
        Ok(a) => (StatusCode::CREATED, Json(Person::from(&a))).into_response(),
        Err(e) => refuse(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    is_admin: Option<bool>,
}

/// Would this change leave nobody able to manage accounts?
fn last_admin(all: &[Account], id: &str, stays_admin: bool) -> bool {
    !stays_admin && all.iter().all(|a| !a.is_admin || a.id == id)
}

/// `PUT /api/accounts/{id}` (administrators): rename, reset the password (which signs them out everywhere),
/// or make or unmake an administrator.
pub async fn update(
    State(app): State<Arc<App>>,
    session::ApiUser(user): session::ApiUser,
    Path(id): Path<String>,
    Json(body): Json<Change>,
) -> Response {
    if let Some(r) = not_admin(&app, &user) {
        return r;
    }
    let acc = accounts(&app).unwrap();
    let _held = acc.changes.lock().await;
    let list = all(&app.store).await;
    let Some(mut a) = list.iter().find(|a| a.id == id).cloned() else {
        return refuse(StatusCode::NOT_FOUND, "There is no such account.");
    };
    let changed = async {
        if let Some(n) = &body.name {
            a.name = display_name(Some(n), &a.username)?;
        }
        if let Some(admin) = body.is_admin {
            if last_admin(&list, &a.id, admin) {
                bail!("Someone has to stay an administrator, or nobody could manage accounts.");
            }
            a.is_admin = admin;
        }
        if let Some(p) = &body.password {
            check_password(p)?;
            let p = p.clone();
            a.password_hash = blocking(move || hash(&p)).await;
            a.credential = random_hex(16);
        }
        app.store.put("accounts", &a.id, &a).await?;
        anyhow::Ok(())
    }
    .await;
    match changed {
        // Resetting your own password here signs this browser out too; the settings page's own change keeps
        // it signed in.
        Ok(()) => Json(Person::from(&a)).into_response(),
        Err(e) => refuse(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

/// `DELETE /api/accounts/{id}` (administrators). Not yourself, and never the last administrator. The photos
/// and their edits stay: they live in the library, not in the account. Its Recent list and settings go.
pub async fn remove(
    State(app): State<Arc<App>>,
    session::ApiUser(user): session::ApiUser,
    Path(id): Path<String>,
) -> Response {
    if let Some(r) = not_admin(&app, &user) {
        return r;
    }
    if id == user.id {
        return refuse(
            StatusCode::BAD_REQUEST,
            "You can't remove your own account while signed in to it.",
        );
    }
    let acc = accounts(&app).unwrap();
    let _held = acc.changes.lock().await;
    let list = all(&app.store).await;
    if !list.iter().any(|a| a.id == id) {
        return refuse(StatusCode::NOT_FOUND, "There is no such account.");
    }
    if last_admin(&list, &id, false) {
        return refuse(StatusCode::BAD_REQUEST, "That is the last administrator.");
    }
    app.store.delete("accounts", &id).await;
    app.store.delete("users", &id).await;
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnPassword {
    current: String,
    password: String,
}

/// `PUT /api/account/password`: change your own. Every other browser signed in to the account is signed
/// out; this one gets a fresh session.
pub async fn change_own(
    State(app): State<Arc<App>>,
    session::ApiUser(user): session::ApiUser,
    Json(body): Json<OwnPassword>,
) -> Response {
    let Some(acc) = accounts(&app) else {
        return refuse(StatusCode::NOT_FOUND, "This Sidecar has no local accounts.");
    };
    let Some(a) = app.store.get::<Account>("accounts", &user.id).await else {
        return refuse(StatusCode::NOT_FOUND, "There is no such account.");
    };
    if let Some(w) = acc.wait(&a.username) {
        return too_many(w);
    }
    let (current, hash_now) = (body.current, a.password_hash.clone());
    if !blocking(move || verify(&current, &hash_now)).await {
        acc.failed(&a.username);
        return refuse(StatusCode::FORBIDDEN, "Your current password isn't that.");
    }
    if let Err(e) = check_password(&body.password) {
        return refuse(StatusCode::BAD_REQUEST, format!("{e:#}"));
    }
    match set_password(&app.store, &a.username, &body.password).await {
        Ok(a) => session::start(&app, &a).await,
        Err(e) => refuse(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames_are_normalized_and_checked() {
        assert_eq!(normalize_username("  Anna.B ").unwrap(), "anna.b");
        assert!(normalize_username("").is_err());
        assert!(normalize_username(".anna").is_err());
        assert!(normalize_username("anna b").is_err());
        assert!(normalize_username(&"a".repeat(33)).is_err());
    }

    #[test]
    fn a_password_verifies_against_its_own_hash_only() {
        let h = hash("correct horse");
        assert!(verify("correct horse", &h));
        assert!(!verify("correct horsf", &h));
        assert!(!verify("correct horse", decoy()));
        assert!(!verify("anything", "not a hash"));
    }

    #[test]
    fn repeated_failures_have_to_wait_and_success_clears_them() {
        let a = Accounts::default();
        for _ in 0..FREE_ATTEMPTS - 1 {
            a.failed("anna");
        }
        assert!(a.wait("anna").is_none(), "a few typos cost nothing");
        a.failed("anna");
        let w = a.wait("anna").unwrap();
        assert!(w > FIRST_WAIT - Duration::from_secs(1) && w <= FIRST_WAIT);
        a.failed("anna");
        assert!(
            a.wait("anna").unwrap() > FIRST_WAIT,
            "each failure doubles it"
        );
        assert!(a.wait("ben").is_none(), "per username");
        a.succeeded("anna");
        assert!(a.wait("anna").is_none());
    }

    #[test]
    fn someone_always_stays_an_administrator() {
        let acct = |id: &str, admin: bool| Account {
            id: id.into(),
            username: id.into(),
            name: id.into(),
            password_hash: String::new(),
            is_admin: admin,
            credential: String::new(),
            created_at: String::new(),
        };
        let one = [acct("a", true), acct("b", false)];
        assert!(last_admin(&one, "a", false));
        assert!(!last_admin(&one, "a", true));
        assert!(!last_admin(&one, "b", false));
        let two = [acct("a", true), acct("b", true)];
        assert!(!last_admin(&two, "a", false));
    }

    #[tokio::test]
    async fn set_password_creates_an_admin_then_replaces_the_credential() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf()).unwrap();
        assert!(set_password(&store, "anna", "short").await.is_err());
        let first = set_password(&store, "Anna", "a long password")
            .await
            .unwrap();
        assert!(first.is_admin);
        assert_eq!(first.username, "anna");
        let again = set_password(&store, "anna", "another password")
            .await
            .unwrap();
        assert_eq!(again.id, first.id, "the same account");
        assert_ne!(
            again.credential, first.credential,
            "every session is signed out"
        );
        assert!(verify("another password", &again.password_hash));
        assert_eq!(all(&store).await.len(), 1);
    }
}
