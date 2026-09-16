// SPDX-License-Identifier: Apache-2.0
//! `sidecar.toml`.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: Server,
    #[serde(default)]
    pub immich: Immich,
    #[serde(default)]
    pub roots: Vec<Root>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Server {
    pub listen: String,
    pub worker_url: String,
    pub web_root: PathBuf,
    pub state_dir: PathBuf,
    /// Mark cookies `Secure`. On behind HTTPS (the normal deployment); off only for plain-HTTP development.
    pub secure_cookies: bool,
    /// Allow `/open?path=…` (development and desktop use). Access still requires a library root.
    pub allow_path_open: bool,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            listen: "0.0.0.0:8080".into(),
            worker_url: "http://127.0.0.1:8081".into(),
            web_root: "/usr/share/sidecar/web".into(),
            state_dir: "/state".into(),
            secure_cookies: true,
            allow_path_open: false,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Immich {
    pub url: String,
    pub admin_api_key_file: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Root {
    pub name: String,
    /// `originalPath` prefix as immich-server sees it.
    pub immich_prefix: String,
    /// Where this library is mounted for Sidecar.
    pub mount: PathBuf,
    pub template: String,
    #[serde(default = "english")]
    pub month_locale: String,
    #[serde(default)]
    pub moves_files: bool,
    /// Immich's internal library keeps one folder per user (storage label, or user id) before the template.
    #[serde(default)]
    pub per_user: bool,
}

fn english() -> String {
    "en".into()
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Canonical mounts of every root, for the sidecar writer's guard.
    pub fn canonical_mounts(&self) -> Vec<PathBuf> {
        self.roots.iter().filter_map(|r| r.mount.canonicalize().ok()).collect()
    }

    /// Map an Immich `originalPath` to Sidecar's view of the same file.
    pub fn mount_path(&self, original_path: &str) -> Option<(&Root, PathBuf)> {
        self.roots.iter().find_map(|r| {
            let rest = original_path.strip_prefix(r.immich_prefix.trim_end_matches('/'))?.strip_prefix('/')?;
            Some((r, r.mount.join(rest)))
        })
    }

    /// Map a mounted path back to the `originalPath` Immich uses.
    pub fn immich_path(&self, root: &Root, mounted: &Path) -> Option<String> {
        let rest = mounted.strip_prefix(root.mount.canonicalize().ok()?).ok()?;
        Some(format!("{}/{}", root.immich_prefix.trim_end_matches('/'), rest.to_string_lossy()))
    }

    /// The root containing `path`, after resolving symlinks and `..`.
    pub fn root_of(&self, path: &Path) -> Option<(&Root, PathBuf)> {
        let real = path.canonicalize().ok()?;
        self.roots.iter().find_map(|r| {
            let mount = r.mount.canonicalize().ok()?;
            real.starts_with(&mount).then(|| (r, real.clone()))
        })
    }
}
