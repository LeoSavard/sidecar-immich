// SPDX-License-Identifier: Apache-2.0
//! Small JSON files under the state directory: sessions, asset records, recent lists, picker tokens.
//! Everything here is rebuildable or disposable; edit state lives in the sidecars, never here.

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Clone)]
pub struct Store {
    pub dir: PathBuf,
    /// Held across `update`'s read-modify-write. One lock for the whole store: writes are a handful a
    /// minute, and a record per lock would need its own cleanup.
    updates: Arc<tokio::sync::Mutex<()>>,
}

fn safe(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Store {
    pub fn new(dir: PathBuf) -> Result<Self> {
        for sub in ["sessions", "assets", "users", "picks"] {
            std::fs::create_dir_all(dir.join(sub))
                .with_context(|| format!("creating {}", dir.join(sub).display()))?;
        }
        Ok(Self {
            dir,
            updates: Arc::default(),
        })
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

    /// Read, change and write back one record, with no other `update` in between — two requests that each
    /// read a user's record and write it back would otherwise drop whichever change landed first (an entry
    /// in Recent, or a settings change). `change` gets `None` when there is no record yet.
    pub async fn update<T: Serialize + DeserializeOwned>(
        &self,
        kind: &str,
        id: &str,
        change: impl FnOnce(Option<T>) -> T,
    ) -> Result<T> {
        let _held = self.updates.lock().await;
        let value = change(self.get(kind, id).await);
        self.put(kind, id, &value).await?;
        Ok(value)
    }

    /// Delete records of `kind` last written more than `age` ago. Returns how many went.
    pub async fn sweep(&self, kind: &str, age: Duration) -> usize {
        let mut removed = 0;
        let Ok(mut entries) = tokio::fs::read_dir(self.dir.join(kind)).await else {
            return 0;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let old = entry
                .metadata()
                .await
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|a| a > age);
            if old && tokio::fs::remove_file(entry.path()).await.is_ok() {
                removed += 1;
            }
        }
        removed
    }

    pub async fn delete(&self, kind: &str, id: &str) {
        if let Some(p) = self.path(kind, id) {
            let _ = tokio::fs::remove_file(p).await;
        }
    }
}

/// Temp-then-rename. The temp name is unique per write: with one fixed `.tmp` name, two concurrent writes of
/// the same record collided and the second rename failed with "No such file or directory".
pub async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("system randomness");
    hex::encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_writes_of_one_record_all_succeed() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf()).unwrap();
        let writes = (0..16).map(|i| {
            let store = store.clone();
            tokio::spawn(async move { store.put("users", "u", &i).await })
        });
        for w in writes {
            w.await.unwrap().unwrap();
        }
        assert_eq!(
            std::fs::read_dir(tmp.path().join("users")).unwrap().count(),
            1,
            "no temp file left behind"
        );
    }

    #[tokio::test]
    async fn concurrent_updates_lose_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf()).unwrap();
        let updates = (0..16).map(|i| {
            let store = store.clone();
            tokio::spawn(async move {
                store
                    .update("users", "u", |list: Option<Vec<u32>>| {
                        let mut list = list.unwrap_or_default();
                        list.push(i);
                        list
                    })
                    .await
            })
        });
        for u in updates {
            u.await.unwrap().unwrap();
        }
        assert_eq!(store.get::<Vec<u32>>("users", "u").await.unwrap().len(), 16);
    }
}
