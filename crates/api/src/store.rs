// SPDX-License-Identifier: Apache-2.0
//! Small JSON files under the state directory: sessions, asset records, recent lists, picker tokens.
//! Everything here is rebuildable or disposable; edit state lives in the sidecars, never here.

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Store {
    pub dir: PathBuf,
}

fn safe(segment: &str) -> bool {
    !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Store {
    pub fn new(dir: PathBuf) -> Result<Self> {
        for sub in ["sessions", "assets", "users", "picks"] {
            std::fs::create_dir_all(dir.join(sub)).with_context(|| format!("creating {}", dir.join(sub).display()))?;
        }
        Ok(Self { dir })
    }

    pub fn path(&self, kind: &str, id: &str) -> Option<PathBuf> {
        safe(id).then(|| self.dir.join(kind).join(format!("{id}.json")))
    }

    pub async fn get<T: DeserializeOwned>(&self, kind: &str, id: &str) -> Option<T> {
        let bytes = tokio::fs::read(self.path(kind, id)?).await.ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub async fn put<T: Serialize>(&self, kind: &str, id: &str, value: &T) -> Result<()> {
        let path = self.path(kind, id).context("invalid id")?;
        write_atomic(&path, &serde_json::to_vec_pretty(value)?).await
    }

    pub async fn delete(&self, kind: &str, id: &str) {
        if let Some(p) = self.path(kind, id) {
            let _ = tokio::fs::remove_file(p).await;
        }
    }
}

pub async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("system randomness");
    hex::encode(buf)
}
