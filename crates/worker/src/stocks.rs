// SPDX-License-Identifier: Apache-2.0
//! Spectral colour stocks' profiles (DESIGN.md §7, "Colour film, spectrally"): the one part of a stock's chain
//! that only the licensed standards can give — the slide's colour over the three layers' log H, read through
//! ISO 5-3's Status A, and where ISO 7589's sensitometric daylight puts each layer on its curve.
//!
//! Sidecar carries neither standard and neither is redistributable, so the operator points at their own copies
//! with `SIDECAR_STATUS_A` and `SIDECAR_ISO_DAYLIGHT` and they are read at run time. What is kept — in memory,
//! and in the cache directory between runs — is the *profile*: a table of the slide's colours and three
//! numbers, from which no standard can be recovered. Without both files the server renders no spectral stock
//! and the editor offers none; everything else about a stock (its sheet's sensitivities, curves and dyes) is
//! public and lives in `sidecar_color::slide`.
//!
//! A profile costs a quarter of a million dye solves — about four seconds in release — so it is built once per
//! stock per process and written to the cache; the first render after a deployment pays for it.

use sidecar_color::slide::{Densitometer, SlideProfile};
use sidecar_color::spectral::SAMPLES;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// The environment variables naming the operator's copies of the two standards.
const STATUS_A: &str = "SIDECAR_STATUS_A";
const ISO_DAYLIGHT: &str = "SIDECAR_ISO_DAYLIGHT";

/// Bumped when anything in the chain would change a profile's numbers, so a stale cache file is ignored rather
/// than rendering something this code would not have produced.
const GENERATION: u32 = 1;

fn built() -> &'static Mutex<HashMap<String, Option<Arc<SlideProfile>>>> {
    static BUILT: OnceLock<Mutex<HashMap<String, Option<Arc<SlideProfile>>>>> = OnceLock::new();
    BUILT.get_or_init(Default::default)
}

fn cache_dir() -> &'static Mutex<Option<PathBuf>> {
    static DIR: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    DIR.get_or_init(Default::default)
}

/// Where profiles are kept between runs. Without this they are still built, just once per process.
pub fn set_cache_dir(dir: &Path) {
    let dir = dir.join("stocks");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return tracing::warn!(
            "no cache for film stock profiles at {}: {e:#}",
            dir.display()
        );
    }
    *cache_dir().lock().unwrap() = Some(dir);
}

/// The operator's two standards, read once. `None` when either is unset, missing or unparsable — which is the
/// ordinary case for a deployment that has no copies, not an error.
fn standards() -> Option<&'static (Densitometer, [f64; SAMPLES])> {
    static STANDARDS: OnceLock<Option<(Densitometer, [f64; SAMPLES])>> = OnceLock::new();
    STANDARDS
        .get_or_init(|| {
            let read = |var: &str| match std::env::var(var) {
                Ok(path) => std::fs::read_to_string(&path).map_err(|e| format!("{var}: {path}: {e}")).map(Some),
                Err(_) => Ok(None),
            };
            let (a, d) = match (read(STATUS_A), read(ISO_DAYLIGHT)) {
                (Ok(Some(a)), Ok(Some(d))) => (a, d),
                (Err(e), _) | (_, Err(e)) => {
                    tracing::warn!("spectral colour film is off: {e}");
                    return None;
                }
                _ => {
                    tracing::info!("spectral colour film is off: set {STATUS_A} and {ISO_DAYLIGHT} to render named colour stocks");
                    return None;
                }
            };
            let meter = match Densitometer::parse(&a) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("spectral colour film is off: {STATUS_A} does not parse: {e}");
                    return None;
                }
            };
            let daylight = match sidecar_color::slide::parse_spectrum(&d) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("spectral colour film is off: {ISO_DAYLIGHT} does not parse: {e}");
                    return None;
                }
            };
            tracing::info!("spectral colour film is on: both standards read");
            Some((meter, daylight))
        })
        .as_ref()
}

/// Whether this deployment can render spectral colour stocks at all.
pub fn available() -> bool {
    standards().is_some()
}

/// The stocks this deployment can render, by id: every stock `sidecar_color::slide` knows, if the standards are
/// there, and none otherwise. The editor asks so it offers only what will render.
pub fn offered() -> &'static [&'static str] {
    if available() {
        sidecar_color::slide::STOCKS
    } else {
        &[]
    }
}

/// The profile of the stock `id` names: from memory, then the cache, then measured. `None` for an unknown stock
/// or a deployment without the standards.
pub fn profile(id: &str) -> Option<Arc<SlideProfile>> {
    if !sidecar_color::slide::STOCKS.contains(&id) {
        return None;
    }
    let mut built = built().lock().unwrap();
    if let Some(cached) = built.get(id) {
        return cached.clone();
    }
    let made = measure(id).map(Arc::new);
    built.insert(id.to_string(), made.clone());
    made
}

/// The cache file for a stock, if there is a cache directory.
fn cache_path(id: &str) -> Option<PathBuf> {
    Some(
        cache_dir()
            .lock()
            .unwrap()
            .as_ref()?
            .join(format!("{id}-v{GENERATION}.profile")),
    )
}

fn measure(id: &str) -> Option<SlideProfile> {
    let path = cache_path(id);
    if let Some(path) = &path {
        match std::fs::read(path) {
            Ok(bytes) => match SlideProfile::parse(&bytes) {
                Some(p) => {
                    tracing::debug!("film stock {id}: profile from {}", path.display());
                    return Some(p);
                }
                None => tracing::warn!(
                    "film stock {id}: {} is not a profile, measuring again",
                    path.display()
                ),
            },
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                tracing::warn!("film stock {id}: {}: {e:#}", path.display())
            }
            Err(_) => {}
        }
    }
    let (meter, daylight) = standards()?;
    let stock = sidecar_color::slide::slide(id)?;
    let started = std::time::Instant::now();
    let profile = stock.profile(meter, daylight);
    tracing::info!(
        "film stock {id}: profile measured in {:?}",
        started.elapsed()
    );
    if let Some(path) = &path
        && let Err(e) = std::fs::write(path, profile.to_bytes())
    {
        tracing::warn!(
            "film stock {id}: could not cache its profile at {}: {e:#}",
            path.display()
        );
    }
    Some(profile)
}

/// The profile an edit needs, if it names a stock this deployment can render.
pub fn for_edit(edit: &sidecar_color::EditParams) -> Option<Arc<SlideProfile>> {
    let film = edit.film.as_ref()?;
    (!film.stock.is_empty())
        .then(|| profile(&film.stock))
        .flatten()
}

/// A profile's response table, or `&[]` — what `export::masks_for` takes.
pub fn response(profile: &Option<Arc<SlideProfile>>) -> &[f32] {
    profile.as_ref().map_or(&[], |p| &p.response)
}
