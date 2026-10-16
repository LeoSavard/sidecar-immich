// SPDX-License-Identifier: Apache-2.0
use super::*;

const LIGHTROOM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 5.6-c140">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:Rating="4"
   crs:Exposure2012="+0.35"
   crs:Temperature="5200">
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

fn fixture(dir: &Path, name: &str) -> PathBuf {
    let original = dir.join(name);
    std::fs::write(&original, b"raw bytes").unwrap();
    original
}

/// `sc:Sidecar` from an image's embedded XMP.
fn read_from_image(path: &Path) -> Result<Value> {
    register()?;
    let mut file = XmpFile::new()?;
    file.open_file(path, OpenFileOptions::default().for_read().use_smart_handler())?;
    let meta = file.xmp().context("no XMP in image")?;
    Ok(read_node(&meta, ROOT, "sidecar"))
}

/// An export carries the edit that made it, in the same shape the sidecar stores, beside whatever XMP the
/// file already had.
#[test]
fn an_export_carries_its_edit() {
    let tmp = tempfile::tempdir().unwrap();
    let jpeg = tmp.path().join("DSC_1234-v1.jpg");
    let mut bytes = Vec::new();
    jpeg_encoder::Encoder::new(&mut bytes, 90).encode(&[128u8; 8 * 8 * 3], 8, 8, jpeg_encoder::ColorType::Rgb).unwrap();
    std::fs::write(&jpeg, bytes).unwrap();
    let mut edit = EditParams::default();
    edit.exposure.ev = 0.65;
    let sidecar = serde_json::json!({ "edit": serde_json::to_value(&edit).unwrap(), "source": { "file": "DSC_1234.NEF" } });
    embed_in_image(&jpeg, &sidecar).unwrap();
    let back = read_from_image(&jpeg).unwrap();
    assert_eq!(back["source"]["file"], "DSC_1234.NEF");
    assert_eq!(serde_json::from_value::<EditParams>(back["edit"].clone()).unwrap(), edit);
}

/// Masks go through the file and come back, and removing the last one removes it from the file.
#[test]
fn masks_round_trip_and_can_be_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let roots = [tmp.path().canonicalize().unwrap()];
    let original = fixture(tmp.path(), "DSC_2.NEF");
    let mut edit = EditParams::default();
    edit.masks.push(sidecar_color::params::MaskAdjustment { mask: "subject".into(), invert: true, exposure: 0.5, contrast: -20.0, ..Default::default() });
    edit.masks.push(sidecar_color::params::MaskAdjustment { mask: "skin".into(), temperature: 15.0, amount: 60.0, ..Default::default() });
    update(&original, &roots, |d| d.set_edit(&edit, "x")).unwrap();
    assert_eq!(SidecarDoc::load(&original).unwrap().edit(), edit);
    edit.masks.clear();
    update(&original, &roots, |d| d.set_edit(&edit, "x")).unwrap();
    let doc = SidecarDoc::load(&original).unwrap();
    assert_eq!(doc.edit(), edit);
    assert!(doc.sidecar["edit"].get("masks").is_none());
}

/// Geometry and mask layers with strokes, curves and a mixer go through the file and come back exactly — points
/// are arrays inside structs inside an array — and a geometry reset removes it from the file.
#[test]
fn geometry_and_brushed_layers_round_trip() {
    use sidecar_color::geometry::{Crop, Geometry};
    use sidecar_color::params::{MaskAdjustment, Stroke};
    let tmp = tempfile::tempdir().unwrap();
    let roots = [tmp.path().canonicalize().unwrap()];
    let original = fixture(tmp.path(), "DSC_3.NEF");
    let mut edit = EditParams {
        geometry: Some(Geometry { rotate: 1, angle: -2.5, vertical: 30.0, crop: Crop { left: 0.1, top: 0.05, right: 0.9, bottom: 0.95 }, ratio: 0.8, ..Default::default() }),
        ..Default::default()
    };
    let mut layer = MaskAdjustment { mask: "brush".into(), whites: -20.0, curve: vec![[0.0, 0.0], [0.75, 0.8], [1.0, 1.0]], ..Default::default() };
    layer.mixer.blue.saturation = 40.0;
    layer.strokes = vec![
        Stroke { radius: 0.03, feather: 0.5, flow: 1.0, points: vec![[0.1234, 0.5], [0.2, 0.55], [0.31, 0.6]], ..Default::default() },
        Stroke { erase: true, radius: 0.01, feather: 0.0, flow: 0.4, points: vec![[0.25, 0.5]] },
    ];
    edit.masks.push(layer);
    update(&original, &roots, |d| d.set_edit(&edit, "x")).unwrap();
    let doc = SidecarDoc::load(&original).unwrap();
    assert_eq!(doc.edit(), edit);
    assert_eq!(doc.etag(), etag_of(&edit));
    edit.geometry = None;
    update(&original, &roots, |d| d.set_edit(&edit, "x")).unwrap();
    let doc = SidecarDoc::load(&original).unwrap();
    assert_eq!(doc.edit(), edit);
    assert!(doc.sidecar["edit"].get("geometry").is_none());
}

#[test]
fn unix_seconds_format_as_rfc3339() {
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339(951868800), "2000-03-01T00:00:00Z");
    assert_eq!(rfc3339(1785349800), "2026-07-29T18:30:00Z");
}

#[test]
fn creates_reads_back_and_keeps_etag() {
    let tmp = tempfile::tempdir().unwrap();
    let roots = [tmp.path().canonicalize().unwrap()];
    let original = fixture(tmp.path(), "DSC_1234.NEF");
    let mut edit = EditParams::default();
    edit.exposure.ev = 0.65;
    edit.mixer.blue.luminance = -35.0;
    edit.tone.curve = vec![[0.0, 0.0], [0.25, 0.215], [1.0, 1.0]];
    let etag = update(&original, &roots, |d| d.set_edit(&edit, "9b2f")).unwrap();
    assert!(tmp.path().join("DSC_1234.NEF.xmp").exists());

    let doc = SidecarDoc::load(&original).unwrap();
    assert_eq!(doc.edit(), edit);
    assert_eq!(doc.etag(), etag);
    assert_eq!(doc.sidecar["sourceSha1"], "9b2f");
    assert_eq!(doc.process_version(), Some(PROCESS_VERSION as u64));
}

#[test]
fn lens_round_trips_and_is_absent_from_disk_when_off() {
    let tmp = tempfile::tempdir().unwrap();
    let roots = [tmp.path().canonicalize().unwrap()];
    let original = fixture(tmp.path(), "f.NEF");
    update(&original, &roots, |d| d.set_edit(&EditParams::default(), "x")).unwrap();
    let text = std::fs::read_to_string(tmp.path().join("f.NEF.xmp")).unwrap();
    assert!(!text.contains("Lens"), "no sc:Lens should be written while lens is absent: {text}");

    let edit = EditParams {
        lens: Some(sidecar_color::params::Lens { profile: "fujifilm/xf16mmf14".into(), distortion: 80.0, ..Default::default() }),
        ..EditParams::default()
    };
    update(&original, &roots, |d| d.set_edit(&edit, "x")).unwrap();
    let doc = SidecarDoc::load(&original).unwrap();
    assert_eq!(doc.edit(), edit);
    assert_eq!(doc.edit().lens.unwrap().profile, "fujifilm/xf16mmf14");
}

#[test]
fn preserves_lightroom_fields_and_uses_the_existing_stem_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let roots = [tmp.path().canonicalize().unwrap()];
    let original = fixture(tmp.path(), "IMG_2411.CR2");
    std::fs::write(tmp.path().join("IMG_2411.xmp"), LIGHTROOM).unwrap();
    update(&original, &roots, |d| d.set_edit(&EditParams::default(), "abc")).unwrap();
    assert!(!tmp.path().join("IMG_2411.CR2.xmp").exists());
    let text = std::fs::read_to_string(tmp.path().join("IMG_2411.xmp")).unwrap();
    assert!(text.contains("crs:Exposure2012=\"+0.35\"") || text.contains("<crs:Exposure2012>+0.35</crs:Exposure2012>"), "{text}");
    assert!(text.contains("xmp:Rating") && text.contains("sc:Sidecar"), "{text}");
}

#[test]
fn keeps_fields_from_a_newer_sidecar_and_exports() {
    let tmp = tempfile::tempdir().unwrap();
    let roots = [tmp.path().canonicalize().unwrap()];
    let original = fixture(tmp.path(), "a.RAF");
    update(&original, &roots, |d| {
        d.set_edit(&EditParams::default(), "x")?;
        d.sidecar["edit"]["masks"] = serde_json::json!([{ "kind": "radial", "exposure": 0.5 }]);
        d.append_export(serde_json::json!({ "version": 1, "editEtag": "123456", "file": "a-v1.jpg" }));
        Ok(())
    })
    .unwrap();
    // Edited here the way the editor edits: loaded, changed, saved.
    let mut edit = SidecarDoc::load(&original).unwrap().edit();
    assert!(!edit.masks[0].renders(), "a mask of a kind this version doesn't know is carried, not rendered");
    edit.exposure.ev = -1.0;
    update(&original, &roots, |d| d.set_edit(&edit, "x")).unwrap();
    let doc = SidecarDoc::load(&original).unwrap();
    assert_eq!(doc.edit().exposure.ev, -1.0);
    assert_eq!(doc.sidecar["edit"]["masks"][0]["kind"], "radial");
    assert_eq!(doc.sidecar["edit"]["masks"][0]["exposure"], 0.5);
    assert_eq!(doc.exports()[0]["editEtag"], "123456", "all-digit ETags stay strings");
}

#[test]
fn newer_process_version_is_read_only() {
    let tmp = tempfile::tempdir().unwrap();
    let roots = [tmp.path().canonicalize().unwrap()];
    let original = fixture(tmp.path(), "b.NEF");
    update(&original, &roots, |d| {
        d.set_edit(&EditParams::default(), "x")?;
        d.sidecar["processVersion"] = serde_json::json!(99);
        Ok(())
    })
    .unwrap();
    let mut doc = SidecarDoc::load(&original).unwrap();
    assert!(doc.is_read_only());
    assert!(doc.set_edit(&EditParams::default(), "x").is_err());
}

#[test]
fn refuses_to_write_outside_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let original = fixture(tmp.path(), "c.NEF");
    let err = update(&original, &[other.path().canonicalize().unwrap()], |d| d.set_edit(&EditParams::default(), "x")).unwrap_err();
    assert!(err.to_string().contains("outside"), "{err}");
}

#[test]
fn detects_a_concurrent_write() {
    let tmp = tempfile::tempdir().unwrap();
    let roots = [tmp.path().canonicalize().unwrap()];
    let original = fixture(tmp.path(), "d.NEF");
    update(&original, &roots, |d| d.set_edit(&EditParams::default(), "x")).unwrap();
    let mut doc = SidecarDoc::load(&original).unwrap();
    doc.set_edit(&EditParams::default(), "x").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(tmp.path().join("d.NEF.xmp"), LIGHTROOM).unwrap();
    let err = doc.save(&original, &roots).unwrap_err();
    assert!(err.downcast_ref::<SidecarError>().is_some());
}

/// With exiftool available (it is in the dev image), the tag names Immich would merge contain no standard ones.
#[test]
fn exiftool_sees_only_prefixed_tag_names() {
    let tmp = tempfile::tempdir().unwrap();
    let roots = [tmp.path().canonicalize().unwrap()];
    let original = fixture(tmp.path(), "e.NEF");
    update(&original, &roots, |d| d.set_edit(&EditParams::default(), "x")).unwrap();
    let Ok(out) = std::process::Command::new("exiftool").args(["-j", "-XMP-sc:all"]).arg(tmp.path().join("e.NEF.xmp")).output() else {
        eprintln!("exiftool not installed; skipping");
        return;
    };
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    let keys: Vec<&String> = json[0].as_object().unwrap().keys().filter(|k| *k != "SourceFile").collect();
    assert!(!keys.is_empty());
    for k in keys {
        assert!(k.starts_with("Sidecar"), "exiftool reports `{k}`, which could collide with a standard tag");
    }
}

/// The failure this was written for: Immich's internal library is owned by root and the api container runs
/// as the photo owner, so creating the sidecar fails with `Permission denied (os error 13)` — after the edit
/// has been made. `writable_beside` answers before it, and `save` names the folder rather than the errno.
#[test]
fn an_unwritable_folder_is_reported_up_front_and_by_name() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let roots = [tmp.path().canonicalize().unwrap()];
    let dir = tmp.path().join("readonly");
    std::fs::create_dir(&dir).unwrap();
    let original = fixture(&dir, "r.NEF");
    assert!(crate::writable_beside(&original), "a normal folder takes a sidecar");

    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Running as root ignores the mode bits entirely, so there is nothing to test there.
    if crate::writable_beside(&original) {
        eprintln!("running as root; skipping");
        return;
    }

    let err = update(&original, &roots, |d| d.set_edit(&EditParams::default(), "x")).unwrap_err();
    assert!(
        matches!(err.downcast_ref::<crate::SidecarError>(), Some(crate::SidecarError::NotWritable { .. })),
        "expected NotWritable, got {err:#}"
    );
    assert!(err.to_string().contains("readonly"), "the message should name the folder: {err}");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
}

mod history {
    use super::*;
    use crate::history::{AMEND_WINDOW_S, MAX_NODES, ORIGINAL};

    fn ev(v: f64) -> EditParams {
        let mut e = EditParams::default();
        e.exposure.ev = v;
        e
    }

    fn save(h: &mut History, edit: &EditParams, base: &str) -> Committed {
        h.commit(&Commit { edit, base, checkpoint: false, session: Some("tab"), user: None }, &[]).unwrap()
    }

    /// Everything goes through the real file: ids, ETags and labels must come back as strings, deltas as
    /// the numbers they were, and a cleared optional field as cleared.
    #[test]
    fn the_tree_round_trips_through_the_sidecar_file() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = [tmp.path().canonicalize().unwrap()];
        let original = fixture(tmp.path(), "IMG_1.HEIC");
        let with_lens = EditParams { lens: Some(sidecar_color::params::Lens { profile: "a/b".into(), ..Default::default() }), ..ev(0.5) };
        let edits = [ev(0.25), with_lens, ev(-1.0)];
        let mut heads = Vec::new();
        for e in &edits {
            let base = SidecarDoc::load(&original).unwrap().history().head;
            update(&original, &roots, |d| {
                heads.push(d.commit(&Commit { edit: e, base: &base, checkpoint: false, session: None, user: Some("Léo") }, "sha")?.head);
                Ok(())
            })
            .unwrap();
        }
        update(&original, &roots, |d| d.label(&heads[1], Some("1234e5".into()))).unwrap();

        let doc = SidecarDoc::load(&original).unwrap();
        let h = doc.history();
        assert_eq!(h.nodes.len(), 4, "the original and three saves");
        assert_eq!(h.head, heads[2]);
        for (id, e) in heads.iter().zip(&edits) {
            assert_eq!(&h.edit_of(id).unwrap(), e, "node {id}");
        }
        assert_eq!(h.edit_of(ORIGINAL).unwrap(), EditParams::default());
        assert_eq!(h.node(&heads[1]).unwrap().label.as_deref(), Some("1234e5"), "a label that looks like a number stays text");
        assert_eq!(h.edit_of(&heads[2]).unwrap().lens, None, "turning lens correction off survives the file");
        assert_eq!(doc.edit(), edits[2], "sc:Edit is still the current edit on its own");
    }

    /// A version that drops something an earlier one had — the geometry reset, the last mask removed — comes
    /// back without it: a field left out at its default is a change too.
    #[test]
    fn a_field_left_out_is_removed_in_the_version_that_left_it_out() {
        let mut h = History::begin(&EditParams::default());
        let shaped = EditParams {
            geometry: Some(sidecar_color::geometry::Geometry { angle: 3.0, ..Default::default() }),
            masks: vec![sidecar_color::params::MaskAdjustment { mask: "subject".into(), exposure: 1.0, ..Default::default() }],
            ..ev(0.5)
        };
        let a = save(&mut h, &shaped, ORIGINAL).head;
        let b = save(&mut h, &ev(0.5), &a).head;
        assert_eq!(h.edit_of(&a).unwrap(), shaped);
        assert_eq!(h.edit_of(&b).unwrap(), ev(0.5));
        let removed = &h.node(&b).unwrap().removed;
        assert!(removed.contains(&"geometry".to_string()) && removed.contains(&"masks".to_string()), "{removed:?}");
    }

    #[test]
    fn reverting_to_the_original_keeps_everything() {
        let mut h = History::begin(&EditParams::default());
        let a = save(&mut h, &ev(1.0), ORIGINAL).head;
        assert_eq!(h.checkout(ORIGINAL).unwrap(), EditParams::default());
        assert_eq!(h.head, ORIGINAL);
        assert_eq!(h.edit_of(&a).unwrap(), ev(1.0), "the reverted version is still there");
    }

    /// Going back and changing something branches; it never discards what came after.
    #[test]
    fn editing_an_older_version_branches() {
        let mut h = History::begin(&EditParams::default());
        let a = save(&mut h, &ev(1.0), ORIGINAL).head;
        let b = save(&mut h, &ev(2.0), &a).head;
        h.checkout(&a).unwrap();
        let c = save(&mut h, &ev(-1.0), &a);
        assert!(!c.forked, "checking out and editing is not a conflict");
        assert_eq!(h.nodes.iter().filter(|n| n.parent.as_deref() == Some(a.as_str())).count(), 2);
        assert_eq!(h.edit_of(&b).unwrap(), ev(2.0));
        assert_eq!(h.edit_of(&c.head).unwrap(), ev(-1.0));
    }

    /// Two devices saving from the same version used to make one of them choose whose edit to throw away.
    #[test]
    fn two_devices_saving_from_the_same_version_fork() {
        let mut h = History::begin(&EditParams::default());
        let phone = h.commit(&Commit { edit: &ev(1.0), base: ORIGINAL, checkpoint: false, session: Some("phone"), user: None }, &[]).unwrap();
        let desk = h.commit(&Commit { edit: &ev(-1.0), base: ORIGINAL, checkpoint: false, session: Some("desk"), user: None }, &[]).unwrap();
        assert!(!phone.forked && desk.forked);
        assert_eq!(h.head, desk.head);
        assert_eq!(h.edit_of(&phone.head).unwrap(), ev(1.0), "the phone's edit is kept");
    }

    #[test]
    fn checkpoints_fold_together_until_sealed() {
        let mut h = History::begin(&EditParams::default());
        let auto = |h: &mut History, v: f64, session: &str| {
            let base = h.head.clone();
            h.commit(&Commit { edit: &ev(v), base: &base, checkpoint: true, session: Some(session), user: None }, &[]).unwrap()
        };
        auto(&mut h, 0.1, "tab");
        auto(&mut h, 0.2, "tab");
        let last = auto(&mut h, 0.3, "tab");
        assert_eq!(h.nodes.len(), 2, "three checkpoints in a row are one step");
        assert_eq!(h.edit_of(&last.head).unwrap(), ev(0.3));

        // Save with nothing new seals the checkpoint rather than adding a copy of it.
        save(&mut h, &ev(0.3), &last.head);
        assert_eq!((h.nodes.len(), h.node(&last.head).unwrap().kind), (2, NodeKind::Save));
        auto(&mut h, 0.4, "tab");
        assert_eq!(h.nodes.len(), 3, "a sealed step isn't amended");
        auto(&mut h, 0.5, "other tab");
        assert_eq!(h.nodes.len(), 4, "another device's checkpoint never folds into this one");
    }

    /// One step per control run: dragging exposure again folds in, touching contrast starts the next step.
    #[test]
    fn a_checkpoint_folds_only_more_of_the_same_controls() {
        let mut h = History::begin(&EditParams::default());
        let auto = |h: &mut History, e: &EditParams| {
            let base = h.head.clone();
            h.commit(&Commit { edit: e, base: &base, checkpoint: true, session: Some("tab"), user: None }, &[]).unwrap()
        };
        auto(&mut h, &ev(0.2));
        auto(&mut h, &ev(0.4));
        assert_eq!(h.nodes.len(), 2);
        let mut both = ev(0.4);
        both.tone.contrast = 1.2;
        auto(&mut h, &both);
        assert_eq!(h.nodes.len(), 3, "contrast is a new step");
        both.tone.contrast = 1.3;
        auto(&mut h, &both);
        assert_eq!(h.nodes.len(), 3, "and more contrast folds into it");
        assert_eq!(h.edit_of(&h.head).unwrap(), both);
        let before_contrast = h.nodes[1].id.clone();
        auto(&mut h, &ev(0.4));
        assert_eq!((h.nodes.len(), h.head.as_str()), (2, before_contrast.as_str()), "undoing the contrast step removes it");
    }

    /// The sequence from a real session: white balance, then white balance again with exposure in the same
    /// pause, then only tint, then highlights and shadows. The white balance tweaks are one step, not three.
    #[test]
    fn going_on_with_a_setting_folds_in_whatever_came_with_it() {
        let mut h = History::begin(&EditParams::default());
        let auto = |h: &mut History, e: &EditParams| {
            let base = h.head.clone();
            h.commit(&Commit { edit: e, base: &base, checkpoint: true, session: Some("tab"), user: None }, &[]).unwrap();
        };
        let wb = sidecar_color::params::WhiteBalance { mode: sidecar_color::params::WhiteBalanceMode::Custom, temperature_k: 5460.0, tint: 15.0 };
        let mut e = EditParams { white_balance: wb, ..Default::default() };
        auto(&mut h, &e);
        e.white_balance.temperature_k = 4130.0;
        e.exposure.ev = 1.8;
        auto(&mut h, &e);
        assert_eq!(h.nodes.len(), 2, "more white balance, with exposure alongside, is the same step");
        e.white_balance.tint = 10.0;
        auto(&mut h, &e);
        assert_eq!(h.nodes.len(), 2, "tint is the same setting as temperature");
        e.tone.curve = vec![[0.0, 0.0], [0.25, 0.3], [0.75, 0.7], [1.0, 1.0]];
        auto(&mut h, &e);
        assert_eq!(h.nodes.len(), 3, "a setting the step never touched starts the next one");
        assert_eq!(h.edit_of(&h.nodes[1].id).unwrap().white_balance.temperature_k, 4130.0);
        assert_eq!(h.edit_of(&h.head).unwrap(), e);
    }

    #[test]
    fn a_checkpoint_that_was_exported_or_is_old_is_not_amended() {
        let mut h = History::begin(&EditParams::default());
        let first = h.commit(&Commit { edit: &ev(0.1), base: ORIGINAL, checkpoint: true, session: Some("t"), user: None }, &[]).unwrap();
        let exported = [h.node(&first.head).unwrap().etag.clone()];
        h.commit(&Commit { edit: &ev(0.2), base: &first.head, checkpoint: true, session: Some("t"), user: None }, &exported).unwrap();
        assert_eq!(h.nodes.len(), 3, "the exported version stays what was exported");

        let mut h = History::begin(&EditParams::default());
        let first = h.commit(&Commit { edit: &ev(0.1), base: ORIGINAL, checkpoint: true, session: Some("t"), user: None }, &[]).unwrap();
        h.nodes[1].created = rfc3339(now_secs() - AMEND_WINDOW_S - 1);
        h.commit(&Commit { edit: &ev(0.2), base: &first.head, checkpoint: true, session: Some("t"), user: None }, &[]).unwrap();
        assert_eq!(h.nodes.len(), 3, "an afternoon of editing leaves a trail, not one step");
    }

    fn now_secs() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }

    /// Sidecars written before history existed still open, with their edit as the first step after the original.
    #[test]
    fn a_sidecar_from_before_history_begins_with_its_edit() {
        let h = History::begin(&ev(0.7));
        assert_eq!(h.nodes.len(), 2);
        assert_eq!(h.nodes[1].kind, NodeKind::Imported);
        assert_eq!(h.edit_of(&h.head).unwrap(), ev(0.7));
        assert_eq!(History::begin(&EditParams::default()).nodes.len(), 1, "an unedited photo is just the original");
    }

    /// The editor opens a photo edited before 0.3.0, then saves with the head it was shown as its base. That
    /// history isn't stored until this first save, so it's rebuilt on each read and must name its nodes the
    /// same way every time.
    #[test]
    fn a_sidecar_from_before_history_takes_a_save_based_on_what_was_shown() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = [tmp.path().canonicalize().unwrap()];
        let original = fixture(tmp.path(), "DSC_1.ARW");
        update(&original, &roots, |d| d.set_edit(&ev(0.7), "sha")).unwrap();

        let shown = SidecarDoc::load(&original).unwrap().history().head;
        assert_eq!(SidecarDoc::load(&original).unwrap().history().head, shown, "the same id on every read");
        assert_ne!(shown, ORIGINAL);
        update(&original, &roots, |d| {
            d.commit(&Commit { edit: &ev(1.2), base: &shown, checkpoint: false, session: None, user: None }, "sha")?;
            Ok(())
        })
        .unwrap();

        let h = SidecarDoc::load(&original).unwrap().history();
        assert_eq!(h.nodes.len(), 3, "the original, the edit from before history, the save");
        assert_eq!(h.edit_of(&shown).unwrap(), ev(0.7));
        assert_eq!(h.edit_of(&h.head).unwrap(), ev(1.2));
    }

    #[test]
    fn pruning_keeps_the_head_labels_and_exports_exact() {
        let mut h = History::begin(&EditParams::default());
        let mut exported = Vec::new();
        let mut labelled = String::new();
        for i in 0..MAX_NODES + 40 {
            let base = h.head.clone();
            let e = ev(i as f64 / 100.0);
            let done = h.commit(&Commit { edit: &e, base: &base, checkpoint: false, session: None, user: None }, &exported).unwrap();
            if i == 5 {
                exported.push(h.node(&done.head).unwrap().etag.clone());
            }
            if i == 9 {
                labelled = done.head.clone();
                h.set_label(&done.head, Some("print".into())).unwrap();
            }
        }
        assert!(h.nodes.len() <= MAX_NODES);
        assert_eq!(h.edit_of(&h.head).unwrap(), ev((MAX_NODES + 39) as f64 / 100.0));
        assert_eq!(h.edit_of(&labelled).unwrap(), ev(0.09));
        assert!(h.nodes.iter().any(|n| n.etag == exported[0]), "the exported version survives");
        assert!(h.node(ORIGINAL).is_some());
        for n in &h.nodes {
            h.edit_of(&n.id).unwrap();
        }
    }

    #[test]
    fn an_unknown_base_is_refused() {
        let mut h = History::begin(&EditParams::default());
        let err = h.commit(&Commit { edit: &ev(1.0), base: "nope", checkpoint: false, session: None, user: None }, &[]).unwrap_err();
        assert!(matches!(err.downcast_ref::<SidecarError>(), Some(SidecarError::UnknownVersion { .. })));
    }
}

/// Saving writes the edit as it is now: a field this version knows and leaves out (at its default) is gone
/// from the file, however the last save had it, while what a newer Sidecar wrote — a field this version doesn't
/// know, at the top or inside one it does — is carried along.
#[test]
fn saving_drops_known_fields_left_out_and_keeps_unknown_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let roots = [tmp.path().canonicalize().unwrap()];
    let original = fixture(tmp.path(), "DSC_9.NEF");
    let shaped = EditParams {
        geometry: Some(sidecar_color::geometry::Geometry { angle: 3.0, ..Default::default() }),
        masks: vec![sidecar_color::params::MaskAdjustment { mask: "subject".into(), exposure: 1.0, ..Default::default() }],
        ..Default::default()
    };
    update(&original, &roots, |d| {
        d.set_edit(&shaped, "x")?;
        let stored = d.sidecar["edit"].as_object_mut().unwrap();
        stored.insert("fromTheFuture".into(), serde_json::json!({ "strength": 7 }));
        stored["tone"].as_object_mut().unwrap().insert("knee".into(), serde_json::json!(0.25));
        Ok(())
    })
    .unwrap();
    update(&original, &roots, |d| d.set_edit(&EditParams::default(), "x")).unwrap();
    let doc = SidecarDoc::load(&original).unwrap();
    let stored = &doc.sidecar["edit"];
    assert!(stored.get("geometry").is_none() && stored.get("masks").is_none(), "{stored}");
    assert_eq!(stored["fromTheFuture"]["strength"], 7);
    assert_eq!(stored["tone"]["knee"], 0.25);
    assert_eq!(doc.edit(), EditParams::default());
}
