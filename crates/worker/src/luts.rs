// SPDX-License-Identifier: Apache-2.0
//! 3D LUTs the api hands over (DESIGN.md §15, "3D LUTs"), kept in `<cache>/luts/` by id and in memory once read.
//! The api holds the originals and gives them again whenever it starts, so losing the cache loses nothing.

use anyhow::{Context, Result, bail};
use sidecar_color::lut::{CubeHeader, parse_cube};
use sidecar_color::{DisplaySpace, EditParams};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// A LUT as the GPU takes it: its header and its table as RGBA floats.
pub struct Table {
    pub header: CubeHeader,
    pub rgba: Vec<f32>,
}

fn loaded() -> &'static Mutex<HashMap<String, Arc<Table>>> {
    static LOADED: OnceLock<Mutex<HashMap<String, Arc<Table>>>> = OnceLock::new();
    LOADED.get_or_init(Default::default)
}

fn dir() -> &'static Mutex<Option<PathBuf>> {
    static DIR: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    DIR.get_or_init(Default::default)
}

pub fn set_cache_dir(cache: &Path) -> Result<()> {
    let d = cache.join("luts");
    std::fs::create_dir_all(&d).with_context(|| d.display().to_string())?;
    *dir().lock().unwrap() = Some(d);
    Ok(())
}

fn valid_id(id: &str) -> bool {
    id.len() == 16
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn path(id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        bail!("not a LUT id: {id:?}");
    }
    let d = dir().lock().unwrap().clone().context("no LUT directory")?;
    Ok(d.join(format!("{id}.cube")))
}

/// Keep `text` as LUT `id`, after checking it parses.
pub fn put(id: &str, text: &str) -> Result<()> {
    let lut = parse_cube(text).map_err(anyhow::Error::msg)?;
    std::fs::write(path(id)?, text)?;
    let table = Arc::new(Table {
        header: lut.header(),
        rgba: lut.rgba(),
    });
    loaded().lock().unwrap().insert(id.to_string(), table);
    Ok(())
}

pub fn remove(id: &str) -> Result<()> {
    loaded().lock().unwrap().remove(id);
    std::fs::remove_file(path(id)?)?;
    Ok(())
}

pub fn get(id: &str) -> Result<Arc<Table>> {
    if let Some(t) = loaded().lock().unwrap().get(id) {
        return Ok(t.clone());
    }
    let p = path(id)?;
    let text =
        std::fs::read_to_string(&p).with_context(|| format!("LUT {id} isn't on this worker"))?;
    let lut = parse_cube(&text).map_err(anyhow::Error::msg)?;
    let table = Arc::new(Table {
        header: lut.header(),
        rgba: lut.rgba(),
    });
    loaded()
        .lock()
        .unwrap()
        .insert(id.to_string(), table.clone());
    Ok(table)
}

/// The edit's LUT, ready for `DevelopPass::render`: its pass's uniform for `display` and its table. `None`
/// for no LUT or none of it; an error when the edit names one this worker doesn't have, rather than a
/// render that quietly leaves the look out.
pub struct Loaded {
    pub uniform: Vec<u8>,
    pub table: Arc<Table>,
}

impl Loaded {
    pub fn look(&self) -> sidecar_engine::develop::Look<'_> {
        sidecar_engine::develop::Look {
            uniform: &self.uniform,
            size: self.table.header.size,
            table: &self.table.rgba,
        }
    }
}

pub fn for_edit(edit: &EditParams, display: DisplaySpace) -> Result<Option<Loaded>> {
    let Some(look) = &edit.lut else {
        return Ok(None);
    };
    let table = get(&look.id).with_context(|| format!("the LUT “{}”", look.name))?;
    Ok(look
        .uniform(display, &table.header)
        .map(|uniform| Loaded { uniform, table }))
}
