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
    let mut edit = EditParams::default();
    edit.exposure.ev = -1.0;
    update(&original, &roots, |d| d.set_edit(&edit, "x")).unwrap();
    let doc = SidecarDoc::load(&original).unwrap();
    assert_eq!(doc.edit().exposure.ev, -1.0);
    assert_eq!(doc.sidecar["edit"]["masks"][0]["kind"], "radial");
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
