// SPDX-License-Identifier: Apache-2.0
//! From what a share gives us (hash, name, capture date, size) to an original on disk, in this order:
//! Immich checksum search → storage template by date and name → Immich filename search → picker.

use crate::{App, config::Root, immich::ImmichAsset, session::User};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sidecar_template::{find_by_date_and_name, parse_capture_date};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct OpenQuery {
    pub sha1: Option<String>,
    pub name: Option<String>,
    pub taken: Option<String>,
    /// Kept as text: iOS may format it with thousands separators ("43,165,696") or units.
    pub size: Option<String>,
    pub path: Option<String>,
    /// An Immich asset id, from a pasted photo link (desktop).
    pub immich: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub path: PathBuf,
    pub file_name: String,
    pub immich_asset_id: Option<String>,
    pub taken: Option<String>,
}

impl OpenQuery {
    pub fn size_bytes(&self) -> Option<u64> {
        let digits: String = self.size.as_deref()?.chars().take_while(|c| !c.is_alphabetic()).filter(char::is_ascii_digit).collect();
        digits.parse().ok()
    }
}

pub enum Resolved {
    Found(Candidate),
    Choose(Vec<Candidate>),
    NotFound,
}

fn stem(name: &str) -> String {
    let s = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let s = match s.rsplit_once('+') {
        Some((base, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => s,
    };
    s.to_ascii_lowercase()
}

fn from_asset(app: &App, a: &ImmichAsset) -> Option<Candidate> {
    let (_, path) = app.config.mount_path(&a.original_path)?;
    path.is_file().then(|| Candidate { path, file_name: a.original_file_name.clone(), immich_asset_id: Some(a.id.clone()), taken: a.local_date_time.clone() })
}

/// The Immich asset at a mounted path. External-library assets can only be found this way: Immich gives them
/// `sha1("path:" + originalPath)` as their checksum instead of hashing the file.
pub async fn immich_asset_at(app: &App, user: &User, path: &std::path::Path) -> Result<Option<ImmichAsset>> {
    let Some((root, _)) = app.config.root_of(path) else { return Ok(None) };
    let Some(original_path) = app.config.immich_path(root, &path.canonicalize()?) else { return Ok(None) };
    let assets = app.immich.search(&user.api_key, json!({ "originalPath": original_path })).await?;
    Ok(assets.into_iter().find(|a| a.original_path == original_path))
}

/// Keep only candidates whose bytes hash to `sha1`, when there are few enough to hash (the originals are local).
async fn narrow_by_content(list: Vec<Candidate>, sha1: Option<&str>) -> Vec<Candidate> {
    let Some(sha1) = sha1.filter(|s| s.len() == 40) else { return list };
    if list.len() > 10 {
        return list;
    }
    let mut exact = Vec::new();
    for c in &list {
        if sha1_of(c.path.clone()).await.as_deref() == Some(sha1) {
            exact.push(c.clone());
        }
    }
    if exact.is_empty() { list } else { exact }
}

async fn sha1_of(path: PathBuf) -> Option<String> {
    tokio::task::spawn_blocking(move || {
        use sha1::Digest;
        use std::io::Read;
        let mut f = std::fs::File::open(path).ok()?;
        let mut h = sha1::Sha1::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf).ok()?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        Some(hex::encode(h.finalize()))
    })
    .await
    .ok()?
}

pub async fn resolve(app: &App, user: &User, q: &OpenQuery) -> Result<Resolved> {
    let immich = app.immich.enabled();

    if let Some(path) = &q.path {
        if !app.config.server.allow_path_open {
            bail!("opening by path is turned off on this server");
        }
        let Some((_, real)) = app.config.root_of(std::path::Path::new(path)) else { bail!("that file isn't inside a configured library") };
        let file_name = real.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        return Ok(Resolved::Found(Candidate { path: real, file_name, immich_asset_id: None, taken: None }));
    }

    // A pasted Immich link: the asset id gives the original path directly (and proves access).
    if let (true, Some(id)) = (immich, q.immich.as_deref().filter(|s| s.len() == 36)) {
        let asset = app.immich.asset(&user.api_key, id).await?;
        return Ok(match from_asset(app, &asset) {
            Some(c) => Resolved::Found(c),
            None => bail!("Immich has this photo at {}, which isn't inside a library Sidecar is configured for", asset.original_path),
        });
    }

    // 1. Checksum: exact bytes, and proof that this user can see the asset. Finds internal-library assets only.
    if let (true, Some(sha1)) = (immich, q.sha1.as_deref().filter(|s| s.len() == 40)) {
        let assets = app.immich.search(&user.api_key, json!({ "checksum": sha1 })).await?;
        if let Some(found) = assets.iter().find_map(|a| from_asset(app, a)) {
            return Ok(Resolved::Found(found));
        }
        if let Some(a) = assets.first() {
            bail!("Immich has this photo at {}, which isn't inside a library Sidecar is configured for", a.original_path);
        }
    }

    // 2. Storage template, by capture date and name.
    if let (Some(name), Some(date)) = (q.name.as_deref(), q.taken.as_deref().and_then(parse_capture_date)) {
        let mut found: Vec<(&Root, sidecar_template::Candidate)> = Vec::new();
        for root in &app.config.roots {
            let mount = if root.per_user { root.mount.join(user.library_folder()) } else { root.mount.clone() };
            let (template, locale) = (root.template.clone(), root.month_locale.clone());
            let (name, size) = (name.to_string(), q.size_bytes());
            let list = tokio::task::spawn_blocking(move || find_by_date_and_name(&mount, &template, &locale, date, &name, size)).await?;
            found.extend(list.into_iter().map(|c| (root, c)));
        }
        found.sort_by_key(|(_, c)| c.rank);
        if let Some(best) = found.first().map(|(_, c)| c.rank) {
            // With a hash, the file whose bytes match wins regardless of rank (a converted share won't match any).
            let mut tied: Vec<_> = Vec::new();
            if let Some(sha1) = q.sha1.as_deref().filter(|s| s.len() == 40) {
                for (root, c) in found.iter().take(10) {
                    if sha1_of(c.path.clone()).await.as_deref() == Some(sha1) {
                        tied.push((*root, c.clone()));
                    }
                }
            }
            if tied.is_empty() {
                tied = found.into_iter().filter(|(_, c)| c.rank == best).collect();
            }
            let mut confirmed = Vec::new();
            for (root, c) in tied {
                let file_name = c.path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                if !immich {
                    confirmed.push(Candidate { path: c.path, file_name, immich_asset_id: None, taken: None });
                    continue;
                }
                let Some(original_path) = app.config.immich_path(root, &c.path) else { continue };
                let assets = app.immich.search(&user.api_key, json!({ "originalPath": original_path })).await?;
                if let Some(a) = assets.iter().find(|a| a.original_path == original_path) {
                    confirmed.push(Candidate { path: c.path, file_name, immich_asset_id: Some(a.id.clone()), taken: a.local_date_time.clone() });
                }
            }
            match confirmed.len() {
                0 => {}
                1 => return Ok(Resolved::Found(confirmed.remove(0))),
                _ => return Ok(Resolved::Choose(confirmed)),
            }
        }
    }

    // 3. Filename search in Immich.
    if let (true, Some(name)) = (immich, q.name.as_deref()) {
        let want = stem(name);
        let assets = app.immich.search(&user.api_key, json!({ "originalFileName": want })).await?;
        let mut list: Vec<Candidate> = assets.iter().filter(|a| stem(&a.original_file_name) == want).filter_map(|a| from_asset(app, a)).collect();
        list.sort_by_key(|c| !sidecar_template::is_raw(&c.path));
        let mut list = narrow_by_content(list, q.sha1.as_deref()).await;
        match list.len() {
            0 => {}
            1 => return Ok(Resolved::Found(list.remove(0))),
            _ => return Ok(Resolved::Choose(list)),
        }
    }

    Ok(Resolved::NotFound)
}

/// Picker search: filename search in Immich (or nothing without Immich).
pub async fn search_by_name(app: &App, user: &User, name: &str) -> Result<Vec<Candidate>> {
    if !app.immich.enabled() || name.trim().is_empty() {
        return Ok(Vec::new());
    }
    let assets = app.immich.search(&user.api_key, json!({ "originalFileName": name.trim() })).await?;
    Ok(assets.iter().filter_map(|a| from_asset(app, a)).collect())
}
