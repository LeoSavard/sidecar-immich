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
    #[serde(default)]
    pub lensfun: Lensfun,
}

/// The lensfun database, fetched for lenses whose files carry no correction (DESIGN.md §15). Not shipped in
/// the image: downloaded at run time, CC BY-SA 3.0, credited in the notices.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Lensfun {
    pub enabled: bool,
    /// A listing of the database files in the shape of GitHub's contents API.
    pub index: String,
    /// Fetch again when the worker's copy is older than this.
    pub refresh_days: u64,
}

impl Default for Lensfun {
    fn default() -> Self {
        Self {
            enabled: true,
            index: "https://api.github.com/repos/lensfun/lensfun/contents/data/db".into(),
            refresh_days: 30,
        }
    }
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
    /// `originalPath` prefix as immich-server sees it. `None` for a folder Immich knows nothing about: an
    /// editing folder, which no Immich lookup can ever match and nothing here tries to map an asset into.
    #[serde(default)]
    pub immich_prefix: Option<String>,
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
    /// Photos chosen on the editing device land here instead of being uploaded to Immich. At most one root
    /// is the intake; the first one marked wins.
    #[serde(default)]
    pub intake: bool,
    /// How many photographs the editing folder may hold. Unlike a library, which is a disk someone chose,
    /// this folder is usually a volume inside the image — so left alone it grows until the host fills up,
    /// one original at a time, for as long as anyone keeps editing. The limit is what stops that, and it is
    /// deliberately small: the folder is a workbench, not a shelf. Only the intake root reads this.
    #[serde(default = "four")]
    pub intake_limit: usize,
}

fn four() -> usize {
    4
}

fn english() -> String {
    "en".into()
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Canonical mounts of every root, for the sidecar writer's guard.
    pub fn canonical_mounts(&self) -> Vec<PathBuf> {
        self.roots
            .iter()
            .filter_map(|r| r.mount.canonicalize().ok())
            .collect()
    }

    /// Map an Immich `originalPath` to Sidecar's view of the same file.
    pub fn mount_path(&self, original_path: &str) -> Option<(&Root, PathBuf)> {
        self.roots.iter().find_map(|r| {
            let rest = original_path
                .strip_prefix(r.immich_prefix.as_deref()?.trim_end_matches('/'))?
                .strip_prefix('/')?;
            Some((r, r.mount.join(rest)))
        })
    }

    /// Map a mounted path back to the `originalPath` Immich uses. Both sides are resolved first: a mount
    /// reached through a symlink (say `/mnt/photos`, pointing at another volume) otherwise never shares a
    /// prefix with its own canonical form, and every file under it maps to nothing.
    pub fn immich_path(&self, root: &Root, mounted: &Path) -> Option<String> {
        let rest = mounted
            .canonicalize()
            .ok()?
            .strip_prefix(root.mount.canonicalize().ok()?)
            .ok()?
            .to_path_buf();
        Some(format!(
            "{}/{}",
            root.immich_prefix.as_deref()?.trim_end_matches('/'),
            rest.to_string_lossy()
        ))
    }

    /// Where a photo imported from the editing device is written, when the deployment has such a folder.
    pub fn intake_root(&self) -> Option<&Root> {
        self.roots.iter().find(|r| r.intake)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The template lookup hands over paths built on the configured mount, which may run through a symlink.
    #[cfg(unix)]
    #[test]
    fn a_mount_behind_a_symlink_still_maps_back_to_immich() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(real.join("2026")).unwrap();
        std::fs::write(real.join("2026/a.NEF"), b"raw").unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let root = Root {
            name: "r".into(),
            immich_prefix: Some("/data/library/".into()),
            mount: link.clone(),
            template: String::new(),
            month_locale: english(),
            moves_files: false,
            per_user: false,
            intake: false,
            intake_limit: four(),
        };
        let config = Config {
            server: Server::default(),
            immich: Immich::default(),
            roots: vec![root.clone()],
            lensfun: Lensfun::default(),
        };
        assert_eq!(
            config
                .immich_path(&root, &link.join("2026/a.NEF"))
                .as_deref(),
            Some("/data/library/2026/a.NEF")
        );
        assert_eq!(
            config
                .immich_path(&root, &real.join("2026/a.NEF"))
                .as_deref(),
            Some("/data/library/2026/a.NEF")
        );
    }
}

#[cfg(test)]
mod editing_folder_tests {
    use super::*;

    fn config(toml_text: &str) -> Config {
        toml::from_str(toml_text).unwrap()
    }

    /// A root with no `immich_prefix` is the editing folder: Immich can never resolve into it, and nothing
    /// tries to give one of its files an `originalPath`.
    #[test]
    fn a_root_without_a_prefix_is_invisible_to_immich() {
        let c = config(
            r#"
            [[roots]]
            name = "Editing folder"
            mount = "/photos/editing"
            template = "{{y}}/{{filename}}"
            intake = true
            "#,
        );
        assert!(c.mount_path("/photos/editing/2026/a.NEF").is_none());
        assert!(
            c.immich_path(&c.roots[0], Path::new("/photos/editing/2026/a.NEF"))
                .is_none()
        );
        assert_eq!(
            c.intake_root().map(|r| r.name.as_str()),
            Some("Editing folder")
        );
    }

    /// A library root beside it still works exactly as before, and is not mistaken for the intake.
    #[test]
    fn a_library_root_beside_it_is_unchanged() {
        let c = config(
            r#"
            [[roots]]
            name = "library"
            immich_prefix = "/data/library"
            mount = "/photos/library"
            template = "{{y}}/{{filename}}"

            [[roots]]
            name = "Editing folder"
            mount = "/photos/editing"
            template = "{{y}}/{{filename}}"
            intake = true
            "#,
        );
        let (root, path) = c.mount_path("/data/library/2026/a.NEF").unwrap();
        assert_eq!(root.name, "library");
        assert_eq!(path, Path::new("/photos/library/2026/a.NEF"));
        assert_eq!(
            c.intake_root().map(|r| r.name.as_str()),
            Some("Editing folder")
        );
    }

    /// No root marked, no editing folder: the import has nowhere to go but the library, as before.
    #[test]
    fn without_a_marked_root_there_is_no_editing_folder() {
        let c = config(
            r#"
            [[roots]]
            name = "library"
            immich_prefix = "/data/library"
            mount = "/photos/library"
            template = "{{y}}/{{filename}}"
            "#,
        );
        assert!(c.intake_root().is_none());
    }
}
