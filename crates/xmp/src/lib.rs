// SPDX-License-Identifier: Apache-2.0
//! Sidecar's state inside an XMP sidecar: one `sc:Sidecar` struct holding `ProcessVersion`, `SourceSha1`,
//! `Modified`, `Edit` (the current develop parameters), `History` (every version of the edit, `history.rs`)
//! and `Exports` (the files made from it).
//!
//! Everything else in the file — other namespaces, and `sc:` fields written by a newer Sidecar — is kept.
//! Writes go through a guarded, atomic path: temp file beside the target, fsync, re-check that nobody (Immich)
//! wrote the sidecar in the meantime, rename, fsync the directory.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use sidecar_color::params::{EditParams, PROCESS_VERSION};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use xmp_toolkit::{IterOptions, OpenFileOptions, ToStringOptions, XmpFile, XmpMeta, XmpValue};

pub mod history;
pub use history::{Commit, Committed, History, Node, NodeKind};

/// Sidecar's XMP namespace. Never change it once sidecars exist.
pub const NS: &str = "https://sidecar.ls-tech.ca/ns/1.0/";
pub const PREFIX: &str = "sc";
const ROOT: &str = "Sidecar";

/// Fields that stay strings even when they look numeric. The history's ids and ETags are hex, which a number
/// parser takes for a float often enough ("12e456…"), and a label is whatever the person typed.
const STRING_FIELDS: &[&str] = &[
    "mode", "sourceSha1", "modified", "editEtag", "file", "immichAssetId", "exported", "format", "colorSpace", "head", "id", "parent",
    "etag", "created", "kind", "label", "user", "session", "removed",
];

#[derive(Debug, thiserror::Error)]
pub enum SidecarError {
    #[error("this sidecar was written by a newer Sidecar (process version {found}); update Sidecar to edit this photo")]
    NewerProcessVersion { found: u64 },
    #[error("the sidecar changed on disk while Sidecar was writing it")]
    ChangedOnDisk,
    #[error("this photo's history has no version {id}")]
    UnknownVersion { id: String },
    #[error(
        "Sidecar can't write in {}, so this edit can't be saved. That folder belongs to another user — give the account Sidecar runs as write access to it (in Docker, the `user:` of the api container).",
        .dir.display()
    )]
    NotWritable { dir: PathBuf },
}

/// `DSC_1234.NEF.xmp` if it exists, else `DSC_1234.xmp` if that exists, else `DSC_1234.NEF.xmp`.
pub fn sidecar_path_for(original: &Path) -> PathBuf {
    let full = PathBuf::from(format!("{}.xmp", original.display()));
    if full.exists() {
        return full;
    }
    let stem = original.with_extension("xmp");
    if stem.exists() { stem } else { full }
}

/// Can a sidecar be written beside `original`? The editor asks when it opens, so a photo whose folder
/// Sidecar has no write access to says so before the edit is made rather than after five minutes of work.
///
/// Advisory only. The directory is what has to be writable — an existing sidecar is replaced through a temp
/// file beside it, never opened for writing in place — but ACLs, read-only remounts and a full disk all
/// answer this question later than `access(2)` can. `save` is still the authority, and reports
/// `SidecarError::NotWritable` when this was too optimistic.
pub fn writable_beside(original: &Path) -> bool {
    let Some(dir) = original.parent() else { return false };
    rustix::fs::access(dir, rustix::fs::Access::WRITE_OK | rustix::fs::Access::EXEC_OK).is_ok()
}

fn register() -> Result<()> {
    XmpMeta::register_namespace(NS, PREFIX).context("registering the sc: namespace")?;
    Ok(())
}

fn pascal(k: &str) -> String {
    let mut c = k.chars();
    c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
}

fn camel(k: &str) -> String {
    let mut c = k.chars();
    c.next().map(|f| f.to_ascii_lowercase().to_string() + c.as_str()).unwrap_or_default()
}

/// Iterator names are `sc:Sidecar/sc:Edit`; property paths drop the first prefix: `Sidecar/sc:Edit`.
fn rel(name: &str) -> &str {
    name.strip_prefix(&format!("{PREFIX}:")).unwrap_or(name)
}

fn coerce(key: &str, s: &str) -> Value {
    if STRING_FIELDS.contains(&key) {
        return Value::String(s.to_string());
    }
    if let Ok(i) = s.parse::<i64>() {
        return Value::from(i);
    }
    if let Ok(f) = s.parse::<f64>() {
        return Value::from(f);
    }
    if let Some((a, b)) = s.split_once(',')
        && let (Ok(x), Ok(y)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>())
    {
        return Value::from(vec![x, y]);
    }
    match s {
        "True" | "true" => Value::Bool(true),
        "False" | "false" => Value::Bool(false),
        _ => Value::String(s.to_string()),
    }
}

fn read_node(meta: &XmpMeta, path: &str, key: &str) -> Value {
    let Some(v) = meta.property(NS, path) else { return Value::Null };
    if v.is_struct() {
        let mut obj = Map::new();
        let full = format!("{PREFIX}:{path}");
        for child in meta.iter(IterOptions::default().property(NS, path).immediate_children_only()) {
            if child.name == full || child.value.is_qualifier() {
                continue;
            }
            let step = child.name.rsplit('/').next().unwrap_or(&child.name);
            let field = camel(step.split_once(':').map_or(step, |(_, n)| n));
            let value = read_node(meta, rel(&child.name), &field);
            obj.insert(field, value);
        }
        Value::Object(obj)
    } else if v.is_array() {
        let n = meta.array_len(NS, path);
        Value::Array((1..=n).map(|i| read_node(meta, &format!("{path}[{i}]"), key)).collect())
    } else {
        coerce(key, &v.value)
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn write_node(meta: &mut XmpMeta, path: &str, v: &Value) -> Result<()> {
    match v {
        Value::Null => Ok(()),
        Value::Object(map) => {
            meta.set_property(NS, path, &XmpValue::new(String::new()).set_is_struct(true))?;
            for (k, val) in map {
                write_node(meta, &format!("{path}/{PREFIX}:{}", pascal(k)), val)?;
            }
            Ok(())
        }
        Value::Array(items) => {
            meta.set_property(NS, path, &XmpValue::new(String::new()).set_is_array(true).set_is_ordered(true))?;
            let array = XmpValue::new(path.to_string()).set_is_ordered(true);
            for (i, item) in items.iter().enumerate() {
                match item {
                    Value::Object(map) => {
                        meta.append_array_item(NS, &array, &XmpValue::new(String::new()).set_is_struct(true))?;
                        for (k, val) in map {
                            write_node(meta, &format!("{path}[{}]/{PREFIX}:{}", i + 1, pascal(k)), val)?;
                        }
                    }
                    Value::Array(pair) if pair.len() == 2 && pair.iter().all(Value::is_number) => {
                        meta.append_array_item(NS, &array, &XmpValue::new(format!("{}, {}", pair[0], pair[1])))?;
                    }
                    other => meta.append_array_item(NS, &array, &XmpValue::new(scalar(other)))?,
                }
            }
            Ok(())
        }
        other => Ok(meta.set_property(NS, path, &XmpValue::new(scalar(other)))?),
    }
}

/// Deep merge: objects merge key by key, anything else replaces.
pub(crate) fn merge(dst: &mut Value, src: Value) {
    match (dst, src) {
        (Value::Object(d), Value::Object(s)) => {
            for (k, v) in s {
                merge(d.entry(k).or_insert(Value::Null), v);
            }
        }
        (d, s) => *d = s,
    }
}

/// Copy into `fresh` what `stored` holds that this version doesn't know, at any depth. `round_trip` is
/// `stored` read as an `EditParams` and written back: a key this version knows comes back from that unless
/// it was at its default, so a key missing from it is a newer version's — or a known one at its default,
/// which is harmless to carry and is only carried where `fresh` doesn't set it. Arrays are `fresh`'s alone
/// (a mask layer keeps a newer version's fields in its own `other`).
fn carry_unknown(fresh: &mut Value, stored: &Value, round_trip: &Value) {
    let (Value::Object(f), Value::Object(s)) = (fresh, stored) else { return };
    let r = round_trip.as_object();
    for (k, v) in s {
        match (r.and_then(|r| r.get(k)), f.get_mut(k)) {
            (None, None) => {
                f.insert(k.clone(), v.clone());
            }
            (Some(known), Some(now)) => carry_unknown(now, v, known),
            _ => {}
        }
    }
}

pub fn etag_of(edit: &EditParams) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(serde_json::to_vec(edit).expect("edit params serialize")))
}

/// Now, as an RFC 3339 UTC timestamp.
pub fn utc_now() -> String {
    rfc3339(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
}

/// Unix seconds as an RFC 3339 UTC timestamp, without external crates (days-from-civil, H. Hinnant).
pub fn rfc3339(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

/// The SHA-1 of a file's bytes: the asset key, and the `SourceSha1` a sidecar records. Blocking.
pub fn sha1_file(path: &Path) -> Result<String> {
    use sha1::Digest;
    use std::io::Read;
    let mut f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut h = sha1::Sha1::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let md = std::fs::metadata(path).ok()?;
    Some((md.len(), md.modified().ok()?))
}

pub struct SidecarDoc {
    pub path: PathBuf,
    meta: XmpMeta,
    stamp: Option<(u64, SystemTime)>,
    /// The whole `sc:Sidecar` struct as JSON (camelCase keys), including fields this version doesn't know.
    pub sidecar: Value,
}

impl SidecarDoc {
    pub fn load(original: &Path) -> Result<Self> {
        register()?;
        let path = sidecar_path_for(original);
        let stamp = stamp(&path);
        let meta = if stamp.is_some() {
            let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            text.parse::<XmpMeta>().with_context(|| format!("parsing {}", path.display()))?
        } else {
            XmpMeta::new()?
        };
        let sidecar = match read_node(&meta, ROOT, "sidecar") {
            Value::Object(o) => Value::Object(o),
            _ => Value::Object(Map::new()),
        };
        Ok(Self { path, meta, stamp, sidecar })
    }

    pub fn process_version(&self) -> Option<u64> {
        self.sidecar.get("processVersion").and_then(Value::as_u64)
    }

    /// True when a newer Sidecar wrote this file; the editor opens it read-only.
    pub fn is_read_only(&self) -> bool {
        self.process_version().is_some_and(|v| v > PROCESS_VERSION as u64)
    }

    pub fn edit(&self) -> EditParams {
        let mut edit: EditParams = self.sidecar.get("edit").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        // The sidecar's own version is the one an older Sidecar wrote; the edit inside carries it too, but a
        // sidecar from before the field existed has none, which means 1.
        let stored = self.process_version().unwrap_or(1).min(edit.process_version as u64) as u32;
        edit.process_version = stored;
        edit.migrate();
        edit
    }

    pub fn etag(&self) -> String {
        etag_of(&self.edit())
    }

    pub fn exports(&self) -> Vec<Value> {
        self.sidecar.get("exports").and_then(Value::as_array).cloned().unwrap_or_default()
    }

    pub fn set_edit(&mut self, edit: &EditParams, source_sha1: &str) -> Result<()> {
        if let Some(found) = self.process_version().filter(|&v| v > PROCESS_VERSION as u64) {
            bail!(SidecarError::NewerProcessVersion { found });
        }
        let obj = self.sidecar.as_object_mut().unwrap();
        obj.insert("processVersion".into(), Value::from(PROCESS_VERSION));
        obj.insert("sourceSha1".into(), Value::from(source_sha1));
        obj.insert("modified".into(), Value::from(utc_now()));
        // The edit as it is now, plus whatever a newer Sidecar wrote that this version doesn't know. Not a merge
        // into what was stored: a field left out at its default (no masks, no geometry, saturation 0, film off)
        // must leave the file, or turning something off would never be saved.
        let stored = obj.get("edit").cloned().unwrap_or(Value::Null);
        let mut fresh = serde_json::to_value(edit)?;
        if let Some(round_trip) = serde_json::from_value::<EditParams>(stored.clone()).ok().and_then(|e| serde_json::to_value(e).ok()) {
            carry_unknown(&mut fresh, &stored, &round_trip);
        }
        obj.insert("edit".into(), fresh);
        Ok(())
    }

    pub fn append_export(&mut self, entry: Value) {
        let obj = self.sidecar.as_object_mut().unwrap();
        match obj.entry("exports").or_insert(Value::Array(Vec::new())) {
            Value::Array(a) => a.push(entry),
            other => *other = Value::Array(vec![entry]),
        }
    }

    /// Guarded atomic write. `allowed_roots` are the canonical library mounts the sidecar must live under.
    pub fn save(mut self, original: &Path, allowed_roots: &[PathBuf]) -> Result<()> {
        let original_real = original.canonicalize().with_context(|| format!("{} is missing", original.display()))?;
        if !std::fs::metadata(&original_real)?.is_file() {
            bail!("{} is not a regular file", original.display());
        }
        if !allowed_roots.iter().any(|r| original_real.starts_with(r)) {
            bail!("{} is outside the configured libraries", original.display());
        }
        let dir = original_real.parent().context("original has no directory")?;
        let name = self.path.file_name().context("sidecar has no file name")?.to_string_lossy().into_owned();
        if !name.ends_with(".xmp") || self.path.parent().map(|p| p.canonicalize().ok()) != Some(Some(dir.to_path_buf())) {
            bail!("refusing to write {}: not an .xmp beside its original", self.path.display());
        }

        let _ = self.meta.delete_property(NS, ROOT);
        write_node(&mut self.meta, ROOT, &self.sidecar)?;
        let text = self.meta.to_string_with_options(ToStringOptions::default().omit_packet_wrapper())?;

        let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id() as u64 ^ SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_nanos() as u64));
        let mut f = match std::fs::File::options().write(true).create_new(true).open(&tmp) {
            Ok(f) => f,
            // Worth its own error: the photo opens and edits normally, and only the save fails, so the raw
            // "Permission denied (os error 13)" reaches the user after the work is already done.
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => bail!(SidecarError::NotWritable { dir: dir.to_path_buf() }),
            Err(e) => return Err(anyhow::Error::new(e).context(format!("creating {}", tmp.display()))),
        };
        let result = (|| -> Result<()> {
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            if stamp(&self.path) != self.stamp {
                bail!(SidecarError::ChangedOnDisk);
            }
            std::fs::rename(&tmp, &self.path)?;
            std::fs::File::open(dir)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}

/// Load, apply `change`, save; retried when the file changed on disk in between (up to three attempts).
pub fn update(original: &Path, allowed_roots: &[PathBuf], mut change: impl FnMut(&mut SidecarDoc) -> Result<()>) -> Result<String> {
    for attempt in 0..3 {
        let mut doc = SidecarDoc::load(original)?;
        change(&mut doc)?;
        let etag = doc.etag();
        match doc.save(original, allowed_roots) {
            Ok(()) => return Ok(etag),
            Err(e) if attempt < 2 && e.downcast_ref::<SidecarError>().is_some_and(|e| matches!(e, SidecarError::ChangedOnDisk)) => continue,
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// Write an `sc:Sidecar` struct into the XMP embedded in an exported image (JPEG or TIFF), keeping the XMP
/// exiftool copied from the RAW. The export then records its source and the edit that produced it.
pub fn embed_in_image(path: &Path, sidecar: &Value) -> Result<()> {
    register()?;
    let mut file = XmpFile::new()?;
    file.open_file(path, OpenFileOptions::default().for_update().use_smart_handler())
        .with_context(|| format!("opening {} for XMP update", path.display()))?;
    let mut meta = file.xmp().unwrap_or(XmpMeta::new()?);
    let _ = meta.delete_property(NS, ROOT);
    write_node(&mut meta, ROOT, sidecar)?;
    if !file.can_put_xmp(&meta) {
        bail!("{} can't hold this XMP packet", path.display());
    }
    file.put_xmp(&meta)?;
    file.try_close()?;
    Ok(())
}

#[cfg(test)]
mod tests;
