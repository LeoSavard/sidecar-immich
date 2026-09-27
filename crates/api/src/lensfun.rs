// SPDX-License-Identifier: Apache-2.0
//! Keeps the worker's copy of the lensfun database current. The database (CC BY-SA 3.0) is not in the
//! published image (DESIGN.md §12): the api fetches it from lensfun's repository on first start and then
//! monthly, and hands it to the worker, which has no route to the internet.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::App;

#[derive(Deserialize)]
struct Status {
    files: usize,
    updated: Option<u64>,
}

/// An entry of the repository contents listing (GitHub's contents API shape).
#[derive(Deserialize)]
struct Entry {
    name: String,
    download_url: Option<String>,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

async fn sync(app: &App) -> Result<Option<usize>> {
    let cfg = &app.config.lensfun;
    let status: Status = app.http.get(app.worker("/v1/lensfun")).send().await?.error_for_status()?.json().await?;
    let fresh = status.updated.is_some_and(|t| now().saturating_sub(t) < cfg.refresh_days * 86400);
    if status.files > 0 && fresh {
        return Ok(None);
    }
    let ua = concat!("sidecar/", env!("CARGO_PKG_VERSION"));
    let entries: Vec<Entry> = app.http.get(&cfg.index).header("user-agent", ua).send().await?.error_for_status()?.json().await.context("reading the lensfun index")?;
    let mut sent = 0;
    for e in entries.iter().filter(|e| e.name.ends_with(".xml")) {
        let Some(url) = &e.download_url else { continue };
        let xml = app.http.get(url).header("user-agent", ua).send().await?.error_for_status()?.text().await?;
        let r = app.http.put(app.worker(&format!("/v1/lensfun/{}", e.name))).body(xml).send().await?;
        if r.status().is_success() {
            sent += 1;
        } else {
            tracing::warn!("lensfun: the worker refused {}: {}", e.name, r.text().await.unwrap_or_default());
        }
    }
    anyhow::ensure!(sent > 0, "the lensfun index listed no database files");
    app.http.post(app.worker("/v1/lensfun/done")).send().await?.error_for_status()?;
    Ok(Some(sent))
}

impl App {
    /// At start and then daily: fetch the lensfun database if the worker has none or it is older than
    /// `refresh_days`. Failures are logged and retried the next day; lens correction from the file itself
    /// never depends on this.
    pub(crate) fn start_lensfun_sync(self: &Arc<Self>) {
        if !self.config.lensfun.enabled {
            return;
        }
        let app = self.clone();
        tokio::spawn(async move {
            // Let the worker come up first when both start together.
            tokio::time::sleep(Duration::from_secs(5)).await;
            loop {
                match sync(&app).await {
                    Ok(Some(n)) => tracing::info!("lensfun: fetched {n} database files for the worker"),
                    Ok(None) => {}
                    Err(e) => tracing::warn!("lensfun: couldn't update the database, lenses without a correction in the file stay uncorrected: {e:#}"),
                }
                tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
            }
        });
    }
}
