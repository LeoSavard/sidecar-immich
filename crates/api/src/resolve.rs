// SPDX-License-Identifier: Apache-2.0
//! From what a share gives us (hash, name, capture date, size) to an original on disk, in this order:
//! Immich checksum search → storage template by date and name → Immich filename search → picker.

use crate::{
    App,
    config::Root,
    immich::{Immich, ImmichAsset},
    session::User,
};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sidecar_template::{find_by_date_and_name, find_by_name, parse_capture_date};
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
        let digits: String = self
            .size
            .as_deref()?
            .chars()
            .take_while(|c| !c.is_alphabetic())
            .filter(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    }
}

#[derive(Debug)]
pub enum Resolved {
    Found(Candidate),
    Choose(Vec<Candidate>),
    NotFound,
}

/// Compared case-insensitively: Immich's filename search is.
fn stem(name: &str) -> String {
    sidecar_template::stem_of(name).to_ascii_lowercase()
}

pub(crate) fn from_asset(app: &App, a: &ImmichAsset) -> Option<Candidate> {
    let (_, path) = app.config.mount_path(&a.original_path)?;
    path.is_file().then(|| Candidate {
        path,
        file_name: a.original_file_name.clone(),
        immich_asset_id: Some(a.id.clone()),
        taken: a.local_date_time.clone(),
    })
}

/// The Immich asset at a mounted path. External-library assets can only be found this way: Immich gives them
/// `sha1("path:" + originalPath)` as their checksum instead of hashing the file.
pub async fn immich_asset_at(
    app: &App,
    immich: &Immich,
    user: &User,
    path: &std::path::Path,
) -> Result<Option<ImmichAsset>> {
    let Some((root, _)) = app.config.root_of(path) else {
        return Ok(None);
    };
    let Some(original_path) = app.config.immich_path(root, path) else {
        return Ok(None);
    };
    let assets = immich
        .search(&user.api_key, json!({ "originalPath": original_path }))
        .await?;
    Ok(assets
        .into_iter()
        .find(|a| a.original_path == original_path))
}

/// Keep only candidates whose bytes hash to `sha1`, when there are few enough to hash (the originals are local).
async fn narrow_by_content(list: Vec<Candidate>, sha1: Option<&str>) -> Vec<Candidate> {
    let Some(sha1) = sha1.filter(|s| s.len() == 40) else {
        return list;
    };
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

pub(crate) async fn sha1_of(path: PathBuf) -> Option<String> {
    tokio::task::spawn_blocking(move || sidecar_xmp::sha1_file(&path).ok())
        .await
        .ok()?
}

pub async fn resolve(app: &App, user: &User, q: &OpenQuery) -> Result<Resolved> {
    let immich = app.immich.as_ref();

    if let Some(path) = &q.path {
        if !app.config.server.allow_path_open {
            bail!("opening by path is turned off on this server");
        }
        let Some((_, real)) = app.config.root_of(std::path::Path::new(path)) else {
            bail!("that file isn't inside a configured library")
        };
        let file_name = real
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Ok(Resolved::Found(Candidate {
            path: real,
            file_name,
            immich_asset_id: None,
            taken: None,
        }));
    }

    // A pasted Immich link: the asset id gives the original path directly (and proves access).
    if let (Some(immich), Some(id)) = (immich, q.immich.as_deref().filter(|s| s.len() == 36)) {
        let asset = immich.asset(&user.api_key, id).await?;
        return Ok(match from_asset(app, &asset) {
            Some(c) => Resolved::Found(c),
            None => bail!(
                "Immich has this photo at {}, which isn't inside a library Sidecar is configured for",
                asset.original_path
            ),
        });
    }

    // 1. Checksum: exact bytes, and proof that this user can see the asset. Finds internal-library assets only.
    if let (Some(immich), Some(sha1)) = (immich, q.sha1.as_deref().filter(|s| s.len() == 40)) {
        let assets = immich
            .search(&user.api_key, json!({ "checksum": sha1 }))
            .await?;
        if let Some(found) = assets.iter().find_map(|a| from_asset(app, a)) {
            return Ok(Resolved::Found(found));
        }
        if let Some(a) = assets.first() {
            bail!(
                "Immich has this photo at {}, which isn't inside a library Sidecar is configured for",
                a.original_path
            );
        }
    }

    // 2. Storage template, by capture date and name: the folders the template gives for the capture date,
    // matched rather than joined, so a layout that isn't quite the template's is still searched.
    if let (Some(name), Some(date)) = (
        q.name.as_deref(),
        q.taken.as_deref().and_then(parse_capture_date),
    ) {
        let mut found: Vec<(&Root, sidecar_template::Candidate)> = Vec::new();
        for root in &app.config.roots {
            let mount = mount_for(root, user);
            let (template, locale) = (root.template.clone(), root.month_locale.clone());
            let (name, size) = (name.to_string(), q.size_bytes());
            let list = tokio::task::spawn_blocking(move || {
                find_by_date_and_name(&mount, &template, &locale, date, &name, size)
            })
            .await?;
            found.extend(list.into_iter().map(|c| (root, c)));
        }
        if let Some(r) = settle(app, user, q, found).await? {
            return Ok(r);
        }
    }

    // 3. Filename search in Immich.
    if let (Some(immich), Some(name)) = (immich, q.name.as_deref()) {
        let want = stem(name);
        let assets = immich
            .search(&user.api_key, json!({ "originalFileName": want }))
            .await?;
        let mut list: Vec<Candidate> = assets
            .iter()
            .filter(|a| stem(&a.original_file_name) == want)
            .filter_map(|a| from_asset(app, a))
            .collect();
        list.sort_by_key(|c| !sidecar_template::is_raw(&c.path));
        let mut list = narrow_by_content(list, q.sha1.as_deref()).await;
        match list.len() {
            0 => {}
            1 => return Ok(Resolved::Found(list.remove(0))),
            _ => return Ok(Resolved::Choose(list)),
        }
    }

    // 4. The name walk: every configured library, by name alone — what is left when the date path missed (a
    // layout the template doesn't describe, a capture date the share didn't carry) and Immich can't say. Only
    // the exact name, and what the walk finds is confirmed as the date path's finds are.
    if let Some(name) = q.name.as_deref() {
        let want = stem(name);
        let mut found: Vec<(&Root, sidecar_template::Candidate)> = Vec::new();
        for root in &app.config.roots {
            let (mount, name) = (mount_for(root, user), name.to_string());
            let list = tokio::task::spawn_blocking(move || find_by_name(&mount, &name, WALK_LIMIT))
                .await?;
            found.extend(
                list.into_iter()
                    .filter(|c| {
                        c.path
                            .file_name()
                            .and_then(|f| f.to_str())
                            .is_some_and(|f| stem(f) == want)
                    })
                    .map(|c| (root, c)),
            );
        }
        if let Some(r) = settle(app, user, q, found).await? {
            return Ok(r);
        }
    }

    Ok(Resolved::NotFound)
}

/// How many files the name walk gathers before it stops looking.
const WALK_LIMIT: usize = 20;

/// The folder a root's photos are under for this user.
pub(crate) fn mount_for(root: &Root, user: &User) -> std::path::PathBuf {
    if root.per_user {
        root.mount.join(user.library_folder())
    } else {
        root.mount.clone()
    }
}

/// From files found on disk, the answer: the best-ranked — or, given a hash, the one whose bytes match whatever
/// its rank (a converted share matches none) — each confirmed with Immich where there is one, which also proves
/// the user may see it. `None` when nothing survives.
async fn settle(
    app: &App,
    user: &User,
    q: &OpenQuery,
    mut found: Vec<(&Root, sidecar_template::Candidate)>,
) -> Result<Option<Resolved>> {
    let immich = app.immich.as_ref();
    found.sort_by_key(|(_, c)| c.rank);
    let Some(best) = found.first().map(|(_, c)| c.rank) else {
        return Ok(None);
    };
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
        let file_name = c
            .path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(immich) = immich else {
            confirmed.push(Candidate {
                path: c.path,
                file_name,
                immich_asset_id: None,
                taken: None,
            });
            continue;
        };
        let Some(original_path) = app.config.immich_path(root, &c.path) else {
            continue;
        };
        let assets = immich
            .search(&user.api_key, json!({ "originalPath": original_path }))
            .await?;
        if let Some(a) = assets.iter().find(|a| a.original_path == original_path) {
            confirmed.push(Candidate {
                path: c.path,
                file_name,
                immich_asset_id: Some(a.id.clone()),
                taken: a.local_date_time.clone(),
            });
        }
    }
    Ok(match confirmed.len() {
        0 => None,
        1 => Some(Resolved::Found(confirmed.remove(0))),
        _ => Some(Resolved::Choose(confirmed)),
    })
}

/// How many files the picker offers.
const PICKER_LIMIT: usize = 50;

/// Picker search: filename search in Immich, or the libraries' own files without it.
pub async fn search_by_name(app: &App, user: &User, name: &str) -> Result<Vec<Candidate>> {
    if name.trim().is_empty() {
        return Ok(Vec::new());
    }
    // Without Immich, the libraries themselves: by name, as a substring, as Immich's own search does.
    let Some(immich) = app.immich.as_ref() else {
        let mut out = Vec::new();
        for root in &app.config.roots {
            let (mount, name) = (mount_for(root, user), name.trim().to_string());
            let list =
                tokio::task::spawn_blocking(move || find_by_name(&mount, &name, PICKER_LIMIT))
                    .await?;
            out.extend(list);
        }
        out.sort_by_key(|c| c.rank);
        out.truncate(PICKER_LIMIT);
        return Ok(out
            .into_iter()
            .map(|c| Candidate {
                file_name: c
                    .path
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: c.path,
                immich_asset_id: None,
                taken: None,
            })
            .collect());
    };
    let assets = immich
        .search(&user.api_key, json!({ "originalFileName": name.trim() }))
        .await?;
    Ok(assets.iter().filter_map(|a| from_asset(app, a)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library with no Immich and a layout its template doesn't describe — `2019/Holidays/` where the
    /// template expects `2019/07.July/2019-07-04/` — still opens a shared photo by its name, and the picker
    /// still finds it by part of one.
    #[tokio::test]
    async fn a_layout_the_template_misses_degrades_to_the_name_walk() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("photos");
        std::fs::create_dir_all(lib.join("2019/Holidays")).unwrap();
        std::fs::write(lib.join("2019/Holidays/DSC_0001.NEF"), b"raw").unwrap();
        std::fs::write(lib.join("2019/Holidays/DSC_0001.JPG"), b"jpeg").unwrap();
        let mut config: crate::config::Config = toml::from_str(&format!(
            r#"
            [[roots]]
            name = "Library"
            mount = "{}"
            template = "{{{{y}}}}/{{{{MM}}}}.{{{{MMMM}}}}/{{{{y}}}}-{{{{MM}}}}-{{{{dd}}}}/{{{{filename}}}}"
            "#,
            lib.display()
        ))
        .unwrap();
        config.server.state_dir = tmp.path().join("state");
        let app = App::new(config).unwrap();
        let user = User {
            id: "u".into(),
            name: "U".into(),
            email: "u@example.com".into(),
            api_key: String::new(),
            storage_label: None,
        };
        let q = OpenQuery {
            name: Some("DSC_0001.NEF".into()),
            taken: Some("2019-07-04T10:00:00".into()),
            ..Default::default()
        };
        match resolve(&app, &user, &q).await.unwrap() {
            Resolved::Found(c) => assert!(c.path.ends_with("2019/Holidays/DSC_0001.NEF"), "{c:?}"),
            r => panic!("expected the RAW, got {r:?}"),
        }
        // No date at all: the walk alone.
        let q = OpenQuery {
            name: Some("DSC_0001.NEF".into()),
            ..Default::default()
        };
        assert!(matches!(
            resolve(&app, &user, &q).await.unwrap(),
            Resolved::Found(_)
        ));
        // The picker, by part of a name.
        let found = search_by_name(&app, &user, "0001").await.unwrap();
        assert_eq!(found.len(), 2);
        assert!(found[0].path.ends_with("DSC_0001.NEF"), "RAW first");
        // A name that isn't there is not found, rather than anything close to it.
        let q = OpenQuery {
            name: Some("DSC_0002.NEF".into()),
            ..Default::default()
        };
        assert!(matches!(
            resolve(&app, &user, &q).await.unwrap(),
            Resolved::NotFound
        ));
    }
}
