// SPDX-License-Identifier: Apache-2.0
//! Edit history: every saved edit is a node in a tree kept inside `sc:Sidecar`, so it travels with the photo.
//!
//! The root is the untouched original, holding the complete default parameters it was created with; every
//! other node holds only what differs from its parent. Saving from an older node starts a branch instead of
//! discarding what came after it, and two devices saving from the same node fork rather than one overwriting
//! the other. `head` is the node `sc:Edit` currently holds — `sc:Edit` stays the full, current edit, so
//! anything that reads only it (an older Sidecar, an export) is unaffected. See DESIGN.md §4, "Edit history".

use crate::{SidecarDoc, SidecarError, etag_of, merge, utc_now};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sidecar_color::params::EditParams;

/// The root's id. There is one root, and it is always the original.
pub const ORIGINAL: &str = "original";

/// Past this many nodes, the least interesting ones are squashed into their children (see `prune`).
pub const MAX_NODES: usize = 300;

/// How long one autosaved node keeps absorbing further autosaves from the same editing session. Past it, the
/// next checkpoint starts a new node, so an afternoon's editing leaves a trail rather than a single step.
pub const AMEND_WINDOW_S: i64 = 180;

/// Leaf paths a delta sets, plus the ones it clears: `exposure.ev`, `mixer.orange.saturation`, `tone.curve`.
fn touched(delta: &Value, removed: &[String]) -> Vec<String> {
    fn walk(v: &Value, path: String, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => m.iter().for_each(|(k, v)| walk(v, if path.is_empty() { k.clone() } else { format!("{path}.{k}") }, out)),
            _ => out.push(path),
        }
    }
    let mut out = removed.to_vec();
    walk(delta, String::new(), &mut out);
    out
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    /// The untouched original.
    Original,
    /// The edit a sidecar already held when history began.
    Imported,
    /// A save the person asked for (Save, leaving the page, exporting).
    Save,
    /// A checkpoint taken after a pause in editing; folds further checkpoints in until sealed.
    Auto,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// `etag_of` this node's full edit: what exports record, and what a client edits from.
    pub etag: String,
    pub created: String,
    pub kind: NodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Who saved it (the Immich user's name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// The editor tab that saved it, so its own checkpoints fold together and another device's never do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The root: the complete edit. Any other node: only the fields that differ from its parent.
    pub delta: Value,
    /// Dotted paths the parent had and this node clears (an optional field turned off). XMP has no null, so
    /// a removal can't live in `delta`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct History {
    pub head: String,
    /// In creation order: a node always comes after its parent.
    pub nodes: Vec<Node>,
}

/// What `SidecarDoc::commit` is asked to record.
pub struct Commit<'a> {
    pub edit: &'a EditParams,
    /// The node the client edited from. When it isn't the head, the save forks.
    pub base: &'a str,
    /// A pause-triggered checkpoint rather than a save the person asked for.
    pub checkpoint: bool,
    pub session: Option<&'a str>,
    pub user: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Committed {
    pub head: String,
    /// The save branched off a node that was no longer the head: someone else saved in between. Nothing was
    /// lost; both are in the tree.
    pub forked: bool,
}

fn canonical(edit: &EditParams) -> Value {
    serde_json::to_value(edit).expect("edit params serialize")
}

/// Fields of `child` that differ from `parent`, recursing into objects; arrays and scalars are replaced whole.
fn diff(parent: &Value, child: &Value, path: &str, delta: &mut Map<String, Value>, removed: &mut Vec<String>) {
    let Value::Object(c) = child else { return };
    for (k, v) in c {
        let p = parent.get(k).unwrap_or(&Value::Null);
        if p == v {
            continue;
        }
        let here = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
        match (p, v) {
            (_, Value::Null) => removed.push(here),
            (Value::Object(_), Value::Object(_)) => {
                let mut sub = Map::new();
                diff(p, v, &here, &mut sub, removed);
                if !sub.is_empty() {
                    delta.insert(k.clone(), Value::Object(sub));
                }
            }
            _ => {
                delta.insert(k.clone(), v.clone());
            }
        }
    }
}

fn clear(v: &mut Value, dotted: &str) {
    let mut at = v;
    let mut parts = dotted.split('.').peekable();
    while let Some(part) = parts.next() {
        let Some(obj) = at.as_object_mut() else { return };
        if parts.peek().is_none() {
            obj.insert(part.to_string(), Value::Null);
            return;
        }
        let Some(next) = obj.get_mut(part) else { return };
        at = next;
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ`, as `utc_now` writes it → Unix seconds.
fn unix_of(stamp: &str) -> Option<i64> {
    let n: Vec<i64> = stamp.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| s.parse().ok()).collect::<Option<_>>()?;
    let [year, month, day, hour, minute, second] = <[i64; 6]>::try_from(n.get(..6)?).ok()?;
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    Some(days * 86400 + hour * 3600 + minute * 60 + second)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A short id, unique within one photo's history.
fn new_id(parent: &str, etag: &str) -> String {
    use sha2::Digest;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let seed = format!("{parent}:{etag}:{nanos}:{}:{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed));
    hex::encode(&sha2::Sha256::digest(seed.as_bytes())[..6])
}

/// The id of the edit a sidecar held before it had a history: a function of that edit alone.
fn imported_id(etag: &str) -> String {
    use sha2::Digest;
    hex::encode(&sha2::Sha256::digest(format!("imported:{etag}").as_bytes())[..6])
}

impl History {
    /// A new history: the original, and — when the sidecar already holds an edit — that edit as its child.
    pub fn begin(current: &EditParams) -> Self {
        let original = EditParams::default();
        let root = Node {
            id: ORIGINAL.into(),
            parent: None,
            etag: etag_of(&original),
            created: utc_now(),
            kind: NodeKind::Original,
            label: None,
            user: None,
            session: None,
            delta: canonical(&original),
            removed: Vec::new(),
        };
        let mut h = History { head: ORIGINAL.into(), nodes: vec![root] };
        if etag_of(current) != h.nodes[0].etag {
            h.push(ORIGINAL, current, NodeKind::Imported, None, None).expect("the root exists");
            // Until the first save stores it, this history is rebuilt on every read, so the node's id must come
            // out the same each time: the editor sends it back as the base of its save, and a random id would
            // be gone by then ("this photo's history has no version …" on every photo edited before 0.3.0).
            let stable = imported_id(&h.nodes[1].etag);
            h.nodes[1].id = stable.clone();
            h.head = stable;
        }
        h
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    fn node_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    fn has_children(&self, id: &str) -> bool {
        self.nodes.iter().any(|n| n.parent.as_deref() == Some(id))
    }

    /// The full edit a node stands for, rebuilt from the root down its ancestry.
    pub fn edit_of(&self, id: &str) -> Result<EditParams> {
        let mut chain = Vec::new();
        let mut at = Some(id.to_string());
        while let Some(i) = at {
            let n = self.node(&i).ok_or_else(|| SidecarError::UnknownVersion { id: i.clone() })?;
            chain.push(n);
            at = n.parent.clone();
            if chain.len() > self.nodes.len() {
                bail!("the edit history has a cycle at {id}");
            }
        }
        let mut full = Value::Object(Map::new());
        for n in chain.iter().rev() {
            merge(&mut full, n.delta.clone());
            for r in &n.removed {
                clear(&mut full, r);
            }
        }
        serde_json::from_value(full).context("rebuilding an edit from the history")
    }

    /// Append a node for `edit` under `parent`, storing only what differs from it.
    fn push(&mut self, parent: &str, edit: &EditParams, kind: NodeKind, session: Option<&str>, user: Option<&str>) -> Result<String> {
        let (delta, removed) = self.delta_from(parent, edit)?;
        let etag = etag_of(edit);
        let id = new_id(parent, &etag);
        self.nodes.push(Node {
            id: id.clone(),
            parent: Some(parent.into()),
            etag,
            created: utc_now(),
            kind,
            label: None,
            user: user.map(String::from),
            session: session.map(String::from),
            delta,
            removed,
        });
        Ok(id)
    }

    fn delta_from(&self, parent: &str, edit: &EditParams) -> Result<(Value, Vec<String>)> {
        let base = canonical(&self.edit_of(parent)?);
        let (mut delta, mut removed) = (Map::new(), Vec::new());
        diff(&base, &canonical(edit), "", &mut delta, &mut removed);
        Ok((Value::Object(delta), removed))
    }

    /// Record `c` against this history. `exported` are the ETags exports were made from: a node an export
    /// came from is never rewritten or squashed, so the version history keeps pointing at what was exported.
    pub fn commit(&mut self, c: &Commit, exported: &[String]) -> Result<Committed> {
        if self.node(c.base).is_none() {
            bail!(SidecarError::UnknownVersion { id: c.base.into() });
        }
        let etag = etag_of(c.edit);
        let forked = c.base != self.head;

        // Nothing changed since the base: the save moves the head there (a return to an older version) or,
        // for a save the person asked for, seals the checkpoint it would otherwise have amended.
        if self.node(c.base).is_some_and(|b| b.etag == etag) {
            self.head = c.base.to_string();
            if !c.checkpoint && self.amendable(c, exported) {
                self.node_mut(c.base).expect("checked").kind = NodeKind::Save;
            }
            return Ok(Committed { head: self.head.clone(), forked: false });
        }

        let kind = if c.checkpoint { NodeKind::Auto } else { NodeKind::Save };
        if !forked && self.amendable(c, exported) && self.same_controls(c)? {
            let parent = self.node(c.base).and_then(|n| n.parent.clone()).expect("an amendable node has a parent");
            let (delta, removed) = self.delta_from(&parent, c.edit)?;
            if delta.as_object().is_some_and(Map::is_empty) && removed.is_empty() {
                // Undone back to where the step began: the step goes, rather than staying as "no changes".
                self.nodes.retain(|n| n.id != c.base);
                self.head = parent;
                return Ok(Committed { head: self.head.clone(), forked: false });
            }
            let n = self.node_mut(c.base).expect("checked");
            (n.delta, n.removed, n.etag, n.kind) = (delta, removed, etag, kind);
        } else {
            self.head = self.push(c.base, c.edit, kind, c.session, c.user)?;
        }
        self.prune(exported);
        Ok(Committed { head: self.head.clone(), forked })
    }

    /// Can this save fold into the head instead of adding a node? Only this session's own recent,
    /// unlabelled, unexported checkpoint that nothing has branched from.
    fn amendable(&self, c: &Commit, exported: &[String]) -> bool {
        let Some(head) = self.node(&self.head) else { return false };
        head.id == c.base
            && head.kind == NodeKind::Auto
            && c.session.is_some()
            && head.session.as_deref() == c.session
            && head.label.is_none()
            && !exported.contains(&head.etag)
            && !self.has_children(&head.id)
            && unix_of(&head.created).is_some_and(|t| now_unix() - t < AMEND_WINDOW_S)
    }

    /// Does this save only go on moving what the head checkpoint already moved? Exposure dragged three times
    /// is one step; exposure and then contrast are two, so the history reads the way the photo was edited.
    fn same_controls(&self, c: &Commit) -> Result<bool> {
        let head = self.node(&self.head).expect("amendable checked it");
        let (delta, removed) = self.delta_from(&head.id, c.edit)?;
        let before = touched(&head.delta, &head.removed);
        Ok(touched(&delta, &removed).iter().all(|p| before.contains(p)))
    }

    /// Move the head to `id`, and return the edit it holds.
    pub fn checkout(&mut self, id: &str) -> Result<EditParams> {
        let edit = self.edit_of(id)?;
        self.head = id.to_string();
        Ok(edit)
    }

    pub fn set_label(&mut self, id: &str, label: Option<String>) -> Result<()> {
        let n = self.node_mut(id).ok_or_else(|| SidecarError::UnknownVersion { id: id.into() })?;
        n.label = label.map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
        Ok(())
    }

    /// Past `MAX_NODES`, squash the oldest node that carries nothing of its own worth keeping — not the
    /// original, the head, a label, an export, or a fork point — into its only child.
    fn prune(&mut self, exported: &[String]) {
        while self.nodes.len() > MAX_NODES {
            let squashable = self.nodes.iter().position(|n| {
                n.kind != NodeKind::Original
                    && n.id != self.head
                    && n.label.is_none()
                    && !exported.contains(&n.etag)
                    && self.nodes.iter().filter(|c| c.parent.as_deref() == Some(n.id.as_str())).count() == 1
            });
            let Some(i) = squashable else { return };
            let gone = self.nodes.remove(i);
            let parent = gone.parent.clone().expect("only the original has no parent");
            let child = self.nodes.iter().position(|c| c.parent.as_deref() == Some(gone.id.as_str())).expect("counted above");
            let Ok(edit) = self.edit_of_through(&self.nodes[child].id, &gone) else { return };
            let Ok((delta, removed)) = self.delta_from(&parent, &edit) else { return };
            let c = &mut self.nodes[child];
            (c.parent, c.delta, c.removed) = (Some(parent), delta, removed);
        }
    }

    /// `edit_of` for a node whose parent was just removed from `nodes` as `gone`.
    fn edit_of_through(&self, id: &str, gone: &Node) -> Result<EditParams> {
        let mut with = self.clone();
        with.nodes.push(gone.clone());
        with.edit_of(id)
    }
}

impl SidecarDoc {
    /// This photo's history — begun from the current edit when the sidecar predates history.
    pub fn history(&self) -> History {
        self.sidecar
            .get("history")
            .cloned()
            .and_then(|v| serde_json::from_value::<History>(v).ok())
            .filter(|h| h.node(&h.head).is_some())
            .unwrap_or_else(|| History::begin(&self.edit()))
    }

    fn set_history(&mut self, h: &History) -> Result<()> {
        let obj = self.sidecar.as_object_mut().expect("sc:Sidecar is an object");
        obj.insert("history".into(), serde_json::to_value(h)?);
        Ok(())
    }

    /// Save `c.edit` as a node of the history and make it the current edit.
    pub fn commit(&mut self, c: &Commit, source_sha1: &str) -> Result<Committed> {
        let mut h = self.history();
        let exported = self.exported_etags();
        let done = h.commit(c, &exported)?;
        self.set_edit(c.edit, source_sha1)?;
        self.set_history(&h)?;
        Ok(done)
    }

    /// Make an earlier version — or the original — current again. Nothing is removed from the history.
    pub fn checkout(&mut self, id: &str, source_sha1: &str) -> Result<()> {
        let mut h = self.history();
        let edit = h.checkout(id)?;
        self.set_edit(&edit, source_sha1)?;
        self.set_history(&h)
    }

    pub fn label(&mut self, id: &str, label: Option<String>) -> Result<()> {
        let mut h = self.history();
        h.set_label(id, label)?;
        self.set_history(&h)
    }

    /// ETags recorded by `sc:Exports`: the edits exports were made from.
    pub fn exported_etags(&self) -> Vec<String> {
        self.exports().iter().filter_map(|e| e["editEtag"].as_str().map(String::from)).collect()
    }
}
