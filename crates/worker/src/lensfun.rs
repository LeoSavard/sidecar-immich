// SPDX-License-Identifier: Apache-2.0
//! The lensfun database in the worker: XML files under `/cache/lensfun`, pushed by the api (the worker has no
//! route to the internet), loaded into memory and consulted for photos whose files carry no correction.
//!
//! `GET  /v1/lensfun`         `{ files, updated }` — what is here, and when it last arrived (unix seconds)
//! `PUT  /v1/lensfun/{name}`  one database file (`*.xml`), checked to parse before it is kept
//! `POST /v1/lensfun/done`    the upload is complete: reload, and stamp the time

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::Serialize;
use sidecar_color::lens::LensProfile;
use sidecar_raw::RawInfo;
use sidecar_raw::lensfun::Database;

pub struct Lensfun {
    dir: PathBuf,
    db: RwLock<Arc<Database>>,
}

#[derive(Serialize)]
pub struct Status {
    pub files: usize,
    pub updated: Option<u64>,
}

impl Lensfun {
    pub fn open(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let db = Database::load(&dir);
        if !db.lenses.is_empty() {
            tracing::info!("lensfun: {} lenses, {} cameras", db.lenses.len(), db.cameras.len());
        }
        Self { dir, db: RwLock::new(Arc::new(db)) }
    }

    fn stamp_path(&self) -> PathBuf {
        self.dir.join("updated")
    }

    /// When the database last arrived, as unix seconds — also what cached metadata records to know whether
    /// its lensfun match is current.
    pub fn updated(&self) -> Option<u64> {
        std::fs::read_to_string(self.stamp_path()).ok()?.trim().parse().ok()
    }

    pub fn status(&self) -> Status {
        let files = std::fs::read_dir(&self.dir).map(|d| d.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "xml")).count()).unwrap_or(0);
        Status { files, updated: self.updated() }
    }

    /// Keep one uploaded file. Only plain `name.xml` names, and only XML lensfun can read.
    pub fn put(&self, name: &str, xml: &str) -> anyhow::Result<()> {
        let ok = name.ends_with(".xml") && name.len() < 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.') && !name.starts_with('.');
        anyhow::ensure!(ok, "not a lensfun database file name: {name}");
        Database::default().add(xml).map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
        let target = self.dir.join(name);
        let tmp = self.dir.join(format!(".{name}.tmp"));
        std::fs::write(&tmp, xml)?;
        std::fs::rename(&tmp, &target)?;
        Ok(())
    }

    pub fn finish(&self) -> anyhow::Result<()> {
        let db = Database::load(&self.dir);
        tracing::info!("lensfun: loaded {} lenses, {} cameras", db.lenses.len(), db.cameras.len());
        *self.db.write().unwrap() = Arc::new(db);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
        std::fs::write(self.stamp_path(), now.to_string())?;
        Ok(())
    }

    /// The file's own correction when it has one; lensfun's otherwise — but only for sensor data. lensfun's
    /// calibrations describe what a lens does to an uncorrected sensor image; a linear DNG some program made
    /// (a Lightroom panorama or HDR merge) was corrected by it already, and still names the lens it came from.
    pub fn profile_for(&self, info: &RawInfo) -> Option<LensProfile> {
        if let Some(p) = &info.lens_profile {
            return Some(p.clone());
        }
        if !matches!(info.sensor, sidecar_raw::SensorKind::Bayer | sidecar_raw::SensorKind::XTrans) {
            return None;
        }
        let db = self.db.read().unwrap().clone();
        db.profile(info, info.lens_identity.as_ref()?)
    }
}
