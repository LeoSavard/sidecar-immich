// SPDX-License-Identifier: Apache-2.0
//! Apple's HDR gain map, read from a HEIC's auxiliary image through the same external-decoder boundary as
//! `decoded::decode_heif` — nothing LGPL runs in this process. See docs/ROADMAP.md, "HDR originals keep
//! their gain map".
//!
//! Checked against a real iPhone 17 Pro HEIC rather than assumed: the aux image is
//! `urn:com:apple:photo:2020:aux:hdrgainmap`, `heif-convert --with-aux` writes it as
//! `<stem>-urn_com_apple_photo_2020_aux_hdrgainmap.png`, and it is an 8-bit grey image at exactly half the
//! main image's linear size (1512×2016 of 3024×4032; 1512×1512 of a 3024×3024 square crop) — the same
//! resolution convention the Portrait Effects Matte follows (measured on the portrait-depth branch), not a
//! coincidence: both are Apple's "normal-looking derived image, half the main one's size" shape.
//!
//! `exiftool` carries four numbers alongside it, and only two are clearly the same thing:
//! `XMP-HDRGainMap:HDRGainMapVersion` (131072 = 0.2.0.0 — ExifTool's four-byte reading of it) and
//! `XMP-HDRGainMap:HDRGainMapHeadroom` (e.g. 3.325102) sit in the gain map's own tag group and are named for
//! it. `Apple:HDRHeadroom` (e.g. 1.00999999, on the same photo) and `Apple:HDRGain` (e.g. 1.87978518) are
//! MakerNotes scalars that sound related but are carried here unused: nothing confirms what they mean against
//! `HDRGainMapHeadroom`, and guessing would be worse than saying so.
//!
//! **There is no reconstruction formula here, and that is deliberate.** Apple hasn't published one, and the
//! independent attempts to reverse-engineer it that exist call their own results "known to be incorrect" or
//! "unverified" — even Photos on iPhone and on Mac are reported to render the same file differently. A wrong
//! formula would look plausible and be silently wrong, which is worse than not rendering HDR at all. This
//! module's job stops at extracting and preserving the map and its metadata; applying it (docs/ROADMAP.md
//! stage 2) needs a real HDR display to check against before it can be trusted, not another guess.
//!
//! **A second, newer mechanism exists and this module doesn't read it.** Some files also carry an ICC
//! profile (`Display P3 Primaries; PQ (Adaptive Gain Curve …)`) with a binary `HD Gain Map Info` tag — Apple's
//! iOS 18+ "Adaptive HDR" / ISO 21496-1-adjacent encoding, a different mechanism from the auxiliary-image one
//! above (WWDC24; Apple's own encoding still isn't interchangeable with Adobe/Google's ISO 21496-1 files in
//! practice). Investigating that is separate follow-up work, not done here.

use crate::decoded::DecodeError;
use crate::heif_aux::AuxImages;
use std::path::Path;

/// The raw numbers `exiftool` carries beside the map — see the module docs for which are understood.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GainMapMeta {
    pub version: Option<i64>,
    pub headroom: Option<f32>,
    pub apple_hdr_headroom: Option<f32>,
    pub apple_hdr_gain: Option<f32>,
}

/// One greyscale sample per pixel, 0–1, linear — a stored fraction, not light to run through an sRGB
/// transfer function, and not yet run through any reconstruction formula either (see the module docs).
pub struct GainMap {
    pub width: u32,
    pub height: u32,
    pub values: Vec<f32>,
    pub meta: GainMapMeta,
}

fn is_gain_map_file(filename: &str) -> bool {
    filename.to_ascii_lowercase().contains("hdrgainmap")
}

/// Read the HDR gain map in `path`, if any. `None`, not an error, when there is no known aux-capable decoder
/// installed or the file carries none: this enriches a photo that can already be opened, and is never why one
/// can't be.
pub fn read(path: &Path) -> Result<Option<GainMap>, DecodeError> {
    match AuxImages::extract(path)? {
        Some(aux) => from_aux(&aux, path),
        None => Ok(None),
    }
}

/// `read`, from images already extracted — so a caller that also wants the depth map decodes the photo once.
/// `path` is still needed for the scalars `exiftool` reads beside the map.
pub fn from_aux(aux: &AuxImages, path: &Path) -> Result<Option<GainMap>, DecodeError> {
    let Some(plane) = aux.read(is_gain_map_file)? else {
        return Ok(None);
    };
    Ok(Some(GainMap {
        width: plane.width,
        height: plane.height,
        values: plane.values,
        meta: read_meta(path),
    }))
}

/// One `exiftool` run for the four scalars the module docs name — matching `decoded::exif`'s own rule of one
/// run per photo rather than one per tag, even though this one happens alone rather than beside the rest of
/// `decoded::exif`'s tags: it is only ever wanted lazily, when a gain map was actually found.
fn read_meta(path: &Path) -> GainMapMeta {
    let Ok(output) = std::process::Command::new("exiftool")
        .args([
            "-j",
            "-n",
            "-HDRGainMapVersion",
            "-HDRGainMapHeadroom",
            "-HDRHeadroom",
            "-HDRGain",
        ])
        .arg(path)
        .output()
    else {
        return GainMapMeta::default();
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return GainMapMeta::default();
    };
    let Some(tags) = json.as_array().and_then(|a| a.first()) else {
        return GainMapMeta::default();
    };
    GainMapMeta {
        version: tags.get("HDRGainMapVersion").and_then(|v| v.as_i64()),
        headroom: tags
            .get("HDRGainMapHeadroom")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32),
        apple_hdr_headroom: tags
            .get("HDRHeadroom")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32),
        apple_hdr_gain: tags
            .get("HDRGain")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_hdrgainmap_file_is_picked_whatever_colons_became() {
        assert!(is_gain_map_file(
            "out-urn_com_apple_photo_2020_aux_hdrgainmap.png"
        ));
        assert!(is_gain_map_file(
            "out-urn:com:apple:photo:2020:aux:hdrgainmap.png"
        ));
        assert!(!is_gain_map_file(
            "out-urn_com_apple_photo_2018_aux_portraiteffectsmatte.png"
        ));
        assert!(!is_gain_map_file("out-depth.png"));
        assert!(!is_gain_map_file("out.png"));
    }

    /// Pins the measurements in the module docs against a real file. Skips, rather than fails, where
    /// `heif-convert` or `exiftool` isn't installed, or the fixture isn't there (it is a photo of the
    /// owner's, so it is not committed).
    #[test]
    fn a_real_hdr_photo_has_a_half_size_gain_map_and_its_own_headroom() {
        if !crate::heif_aux::which("heif-convert") || !crate::heif_aux::which("exiftool") {
            return eprintln!("skipping: heif-convert or exiftool not installed");
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture = root.join("testdata/cache/IMG_2367.heic");
        if !fixture.is_file() {
            return eprintln!(
                "skipping: no {} (not a committed fixture)",
                fixture.display()
            );
        }
        let map = read(&fixture)
            .unwrap()
            .expect("IMG_2367.heic carries an HDR gain map");
        assert_eq!((map.width, map.height), (1512, 1512));
        assert!(
            map.meta.headroom.is_some_and(|h| h > 1.0),
            "expected a headroom above 1.0, got {:?}",
            map.meta.headroom
        );
        assert!(map.meta.version.is_some());
    }
}
