// SPDX-License-Identifier: Apache-2.0
//! The handful of Immich endpoints Sidecar calls, checked against Immich 3.2.0's OpenAPI spec and source.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct Immich {
    pub base: String,
    pub http: reqwest::Client,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImmichUser {
    pub id: String,
    pub name: String,
    pub email: String,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default)]
    pub storage_label: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetStack {
    pub id: String,
    pub primary_asset_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImmichAsset {
    pub id: String,
    pub original_path: String,
    pub original_file_name: String,
    #[serde(default)]
    pub local_date_time: Option<String>,
    #[serde(default)]
    pub stack: Option<AssetStack>,
}

/// Content type for an upload. Everything Sidecar can open that has no registered image type — every RAW —
/// goes up as bytes, which Immich accepts: it types an asset from the extension, not this header.
fn mime_for(file_name: &str) -> &'static str {
    match file_name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("tif" | "tiff") => "image/tiff",
        Some("png") => "image/png",
        Some("heic") => "image/heic",
        Some("heif") => "image/heif",
        _ => "application/octet-stream",
    }
}

impl Immich {
    pub fn enabled(&self) -> bool {
        !self.base.is_empty()
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api{}", self.base.trim_end_matches('/'), path)
    }

    async fn check(resp: reqwest::Response) -> Result<reqwest::Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().await.unwrap_or_default();
        match status.as_u16() {
            401 => bail!("Immich rejected the API key"),
            403 => bail!("the API key is missing a permission Sidecar needs: {body}"),
            _ => bail!("Immich answered {status}: {body}"),
        }
    }

    /// `GET /api/users/me` — needs `user.read`.
    pub async fn me(&self, key: &str) -> Result<ImmichUser> {
        let r = self
            .http
            .get(self.url("/users/me"))
            .header("x-api-key", key)
            .send()
            .await
            .context("reaching Immich")?;
        Ok(Self::check(r).await?.json().await?)
    }

    /// `POST /api/search/metadata` — needs `asset.read`. Returns the first page of assets.
    pub async fn search(&self, key: &str, mut body: Value) -> Result<Vec<ImmichAsset>> {
        body["size"] = json!(50);
        let r = self
            .http
            .post(self.url("/search/metadata"))
            .header("x-api-key", key)
            .json(&body)
            .send()
            .await
            .context("reaching Immich")?;
        let v: Value = Self::check(r).await?.json().await?;
        Ok(serde_json::from_value(v["assets"]["items"].clone())?)
    }

    /// `GET /api/assets/{id}/thumbnail?size=thumbnail` — needs `asset.view`.
    pub async fn thumbnail(&self, key: &str, id: &str) -> Result<reqwest::Response> {
        let r = self
            .http
            .get(self.url(&format!("/assets/{id}/thumbnail?size=thumbnail")))
            .header("x-api-key", key)
            .send()
            .await?;
        Self::check(r).await
    }

    /// `POST /api/assets` (multipart) — needs `asset.upload`. Immich 3.x has no deviceAssetId; it rejects exact
    /// duplicates per user by checksum and answers `{ id, status: "duplicate" }`, which counts as success.
    /// Used both for finished exports and for a photo handed in from the editing device, so `file_name` is
    /// what decides the type — Immich goes by the extension too.
    pub async fn upload(
        &self,
        key: &str,
        file_name: &str,
        bytes: Vec<u8>,
        captured_at: &str,
    ) -> Result<String> {
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(file_name.to_string())
            .mime_str(mime_for(file_name))?;
        let form = reqwest::multipart::Form::new()
            .part("assetData", part)
            .text("filename", file_name.to_string())
            .text("fileCreatedAt", captured_at.to_string())
            .text("fileModifiedAt", sidecar_xmp::utc_now());
        let r = self
            .http
            .post(self.url("/assets"))
            .header("x-api-key", key)
            .multipart(form)
            .send()
            .await
            .context("uploading to Immich")?;
        let v: Value = Self::check(r).await?.json().await?;
        v["id"]
            .as_str()
            .map(String::from)
            .context("Immich didn't return an asset id")
    }

    /// `GET /api/assets/{id}` — needs `asset.read`.
    pub async fn asset(&self, key: &str, id: &str) -> Result<ImmichAsset> {
        let r = self
            .http
            .get(self.url(&format!("/assets/{id}")))
            .header("x-api-key", key)
            .send()
            .await?;
        Ok(Self::check(r).await?.json().await?)
    }

    /// Stack an export with its RAW. `POST /api/stacks` makes the first id primary and merges any existing stack
    /// whose *primary* is listed, so the current primary is passed rather than the RAW once a stack exists.
    /// Needs `stack.create` and `asset.update`.
    pub async fn stack_export(
        &self,
        key: &str,
        raw_id: &str,
        export_id: &str,
        make_primary: bool,
    ) -> Result<()> {
        let raw = self.asset(key, raw_id).await?;
        let anchor = raw
            .stack
            .map(|s| s.primary_asset_id)
            .unwrap_or_else(|| raw_id.to_string());
        if anchor == export_id {
            return Ok(());
        }
        let ids = if make_primary {
            vec![export_id, &anchor]
        } else {
            vec![anchor.as_str(), export_id]
        };
        let r = self
            .http
            .post(self.url("/stacks"))
            .header("x-api-key", key)
            .json(&json!({ "assetIds": ids }))
            .send()
            .await?;
        Self::check(r).await.map(|_| ())
    }

    /// `PUT /api/jobs/sidecar { command: start, force: false }` — deprecated in 3.x but the only way to run
    /// sidecar discovery for assets Immich has no sidecar for. Admin key with `job.create`.
    pub async fn discover_missing_sidecars(&self, admin_key: &str) -> Result<()> {
        let r = self
            .http
            .put(self.url("/jobs/sidecar"))
            .header("x-api-key", admin_key)
            .json(&json!({ "command": "start", "force": false }))
            .send()
            .await?;
        Self::check(r).await.map(|_| ())
    }
}
