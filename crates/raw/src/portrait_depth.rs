// SPDX-License-Identifier: Apache-2.0
//! Apple's portrait depth signal, read from a HEIC's auxiliary image through the same external-decoder
//! boundary as `decoded::decode_heif` — nothing LGPL runs in this process; see that module's docs and
//! docs/ROADMAP.md, "Portrait depth maps".
//!
//! Not every Portrait photo carries the same aux image, checked against a stratified sample of this user's
//! own library (205 HEICs out of 7,588, `heif-convert --with-aux` + `exiftool`, iPhone 14 Pro through 17 Pro)
//! rather than assumed. In that sample: 91% carry an HDR gain map (near-universal, unrelated to depth); ~16%
//! (33/205) carry a literal raw disparity auxiliary image (`urn:mpeg:hevc:2015:auxid:2`, described further by
//! Apple's own `XMP-apdi:AuxiliaryImageType = disparity`); ~16% (32/205) carry the derived Portrait Effects
//! Matte (`urn:com:apple:photo:2018:aux:portraiteffectsmatte`) — the two overlap but aren't the same set: 18
//! of the 33 disparity-bearing photos have *no* effects matte at all, so the raw map is the more universal of
//! the two (segmentation apparently needs a recognized subject; the stereo disparity capture doesn't).
//!
//! The two are not interchangeable. The effects matte is a continuous 8-bit grey image at exactly half the
//! main image's linear dimensions (e.g. 1512×2016 of 3024×4032) — plenty sharp, but it is Photos' own
//! blur-strength weight, not a distance. The raw disparity image is a real geometric signal, with its own
//! `XMP-depthData` camera-calibration block (focal length, principal point, lens distortion), but at a
//! **fixed** low resolution regardless of the main photo's size — measured 576×768 on both a 3024×4032 and a
//! 4284×5712 capture from the same iPhone 17 Pro, so it is roughly a twentieth to a thirtieth of the main
//! image's pixel count, always. Every example found reports `XMP-depthData:Accuracy = relative`, never
//! `absolute`, even on these LiDAR-equipped Pro models — Apple's photo pipeline evidently publishes the
//! dual/triple-camera stereo disparity into the still photo, not the LiDAR's metric range, so *true* metric
//! depth, while representable in Apple's own format (`AVDepthData.DepthDataAccuracy.absolute`), is not what
//! ordinary Portrait captures on this hardware actually carry. The raw 8-bit samples are quantized per shot:
//! `XMP-apdi:IntMinValue`/`IntMaxValue` (always 0/255 so far) rescale linearly onto that photo's own
//! `FloatMinValue`/`FloatMaxValue` (e.g. 0.0616–0.571, consistent with inverse metres) — reading those two XMP
//! tags to undo the rescale is the next step, not done here yet, so `PortraitDepth::values` for `Disparity`
//! today are the raw 0–1 samples, not Apple's own disparity units.
//!
//! DNG (ProRAW) is a different, unhandled case: it carries the *derived* mattes too (same URNs, seen directly
//! in this library's DNGs), but as extra TIFF SubIFDs tagged `Semantic Name` (DNG 1.6's semantic-mask
//! extension) rather than a HEIF auxiliary image box — `heif-convert` has no reason to find anything in a
//! DNG, and none of the DNGs sampled here carried the raw disparity array or its calibration block at all.
//! Reading a DNG's depth-ish data needs its own TIFF-SubIFD reader; nothing here attempts it.

use crate::decoded::DecodeError;
use crate::heif_aux::AuxImages;
use std::path::Path;

/// What kind of depth signal was found, most to least literal — `read` prefers the first that's present.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DepthKind {
    /// Apple's AVDepthData, continuous, real but relative — see the module docs for what's confirmed (fixed
    /// 576×768 resolution, `Accuracy: relative`, a per-shot linear quantization this type doesn't undo yet).
    Disparity,
    /// `urn:com:apple:photo:2018:aux:portraiteffectsmatte` — how much background blur Photos already baked
    /// in, not a distance.
    EffectsMatte,
}

/// One greyscale sample per pixel, 0–1, linear — these are fractions already (a matte weight, or Apple's
/// normalized disparity), not light to run through an sRGB transfer function.
pub struct PortraitDepth {
    pub width: u32,
    pub height: u32,
    pub values: Vec<f32>,
    pub kind: DepthKind,
}

/// `heif-convert` names each auxiliary file after its aux type, but not uniformly: Apple's own URNs come
/// through as `<output stem>-<urn>.png` (colons sometimes kept, sometimes turned into '_' depending on
/// version and `--no-colons`), while the raw disparity image — tagged with the generic ISO base media type
/// `urn:mpeg:hevc:2015:auxid:2` rather than one of Apple's — comes out named `<output stem>-depth.png`
/// instead (checked against real Portrait photos, not assumed). Matching on the bare word covers both, and
/// none of the other aux types seen in this library (`portraiteffectsmatte`, `semanticskymatte`,
/// `semanticskinmatte`, `semantichairmatte`, `semanticteethmatte`, `semanticglassesmatte`, `hdrgainmap`,
/// `linearthumbnail`, `styledeltamap`) contain "depth" as a substring.
const AUX_NEEDLES: &[(&str, DepthKind)] = &[("disparity", DepthKind::Disparity), ("depth", DepthKind::Disparity), ("portraiteffectsmatte", DepthKind::EffectsMatte)];

fn classify(filename: &str) -> Option<DepthKind> {
    let name = filename.to_ascii_lowercase();
    AUX_NEEDLES.iter().find(|(needle, _)| name.contains(needle)).map(|(_, kind)| *kind)
}

/// Read the most useful depth-like auxiliary image in `path`, if any. `None`, not an error, when there is no
/// known aux-capable decoder installed or the file carries none of `AUX_NEEDLES`: this enriches a photo that
/// can already be opened, and is never why one can't be.
pub fn read(path: &Path) -> Result<Option<PortraitDepth>, DecodeError> {
    match AuxImages::extract(path)? {
        Some(aux) => from_aux(&aux),
        None => Ok(None),
    }
}

/// `read`, from images already extracted — so a caller that also wants the gain map decodes the photo once.
pub fn from_aux(aux: &AuxImages) -> Result<Option<PortraitDepth>, DecodeError> {
    let Some(kind) = aux.names().filter_map(classify).min() else { return Ok(None) };
    let Some(plane) = aux.read(|name| classify(name) == Some(kind))? else { return Ok(None) };
    Ok(Some(PortraitDepth { width: plane.width, height: plane.height, values: plane.values, kind }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disparity_and_literal_depth_outrank_the_effects_matte() {
        assert_eq!(classify("out-urn_com_apple_photo_2020_aux_disparity.png"), Some(DepthKind::Disparity));
        assert_eq!(classify("out-urn:com:apple:photo:2020:aux:depth.png"), Some(DepthKind::Disparity));
        // The real name `heif-convert` gives the raw disparity image on an actual Portrait photo (its aux
        // type is the generic `urn:mpeg:hevc:2015:auxid:2`, not one of Apple's own URNs) — not assumed.
        assert_eq!(classify("out-depth.png"), Some(DepthKind::Disparity));
        assert_eq!(classify("out-urn_com_apple_photo_2018_aux_portraiteffectsmatte.png"), Some(DepthKind::EffectsMatte));
        assert_eq!(classify("out-urn_com_apple_photo_2020_aux_hdrgainmap.png"), None);
        assert_eq!(classify("out-urn_com_apple_photo_2020_aux_semanticskymatte.png"), None);
        assert_eq!(classify("out.png"), None);

        let found = [(DepthKind::EffectsMatte, std::path::PathBuf::from("a")), (DepthKind::Disparity, std::path::PathBuf::from("b"))];
        assert_eq!(found.into_iter().min_by_key(|(kind, _)| *kind).map(|(k, _)| k), Some(DepthKind::Disparity));
    }

    /// Pins the measurement in the module docs against the repo's own fixture. Skips, rather than fails, where
    /// `heif-convert` isn't installed — the published image doesn't ship one either (docs/ROADMAP.md) — or the
    /// fixture isn't there: `testdata/cache` is gitignored test-run output (DESIGN.md, "Development"), not a
    /// committed asset, so a clean checkout or worktree never has it.
    #[test]
    fn img_3042_has_an_effects_matte_at_half_the_main_images_size() {
        if !crate::heif_aux::which("heif-convert") {
            return eprintln!("skipping: heif-convert not installed");
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture = root.join("testdata/cache/IMG_3042.heic");
        if !fixture.is_file() {
            return eprintln!("skipping: no {} (not a committed fixture)", fixture.display());
        }
        let depth = read(&fixture).unwrap().expect("IMG_3042.heic carries a portrait effects matte");
        assert_eq!(depth.kind, DepthKind::EffectsMatte);
        assert_eq!((depth.width, depth.height), (1512, 2016));
        let (min, max) = depth.values.iter().fold((1.0f32, 0.0f32), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(min < 0.01 && max > 0.99, "expected the full 0..1 range, got {min}..{max}");
    }
}
