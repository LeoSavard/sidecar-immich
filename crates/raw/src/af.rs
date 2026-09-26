// SPDX-License-Identifier: Apache-2.0
//! Where the camera aimed autofocus, read from MakerNotes ExifTool decodes but LibRaw does not. See
//! docs/ROADMAP.md, "AF points from the RAW" — a separate, complementary overlay, not a driver for the
//! focus-peaking mask (`sidecar_engine::focus_energy`), which keeps measuring actual pixels.
//!
//! Coverage: Sony `FocusLocation` and Fujifilm `FocusPixel`, the two makers whose tags resolve to plain
//! pixel coordinates. Canon `AFPointsInFocus` and Nikon `AFInfo2` exist but decode to named zones or
//! model-specific point indices, not coordinates — verified against real files in this library, not
//! assumed — so they're left for a per-model lookup table later rather than guessed at now.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// Normalized to the oriented image, 0…1, top-left origin — the same space as the loupe's `center`
/// (DESIGN.md §5). The tag says where the camera *aimed*, not whether it succeeded.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AfPoint {
    pub x: f64,
    pub y: f64,
}

/// The ExifTool tags the AF point is read from, for a caller already running ExifTool to ask for as well.
pub const TAGS: [&str; 4] = ["-FocusLocation", "-FocusPixel", "-ExifImageWidth", "-ExifImageHeight"];

/// `None` for manual focus, focus-and-recompose, an adapted lens, or a maker/tag this doesn't cover.
pub fn af_point(path: &Path, orientation_flip: i32) -> Option<AfPoint> {
    let output = std::process::Command::new("exiftool").arg("-j").args(TAGS).arg(path).output().ok()?;
    let json: Value = serde_json::from_slice(&output.stdout).ok()?;
    from_tags(json.as_array()?.first()?, orientation_flip)
}

/// The AF point from one file's ExifTool JSON object, `-n` or not (checked: these tags print the same).
pub fn from_tags(tags: &Value, orientation_flip: i32) -> Option<AfPoint> {
    let (u, v) = sony_focus_location(tags).or_else(|| fujifilm_focus_pixel(tags))?;
    Some(oriented_point(u, v, orientation_flip))
}

fn nums(s: &str) -> Vec<f64> {
    s.split_whitespace().filter_map(|t| t.parse().ok()).collect()
}

/// "refWidth refHeight x y" — the tag carries its own reference frame, e.g. `9504 6336 3267 2930`.
fn sony_focus_location(tags: &Value) -> Option<(f64, f64)> {
    let [rw, rh, x, y]: [f64; 4] = nums(tags.get("FocusLocation")?.as_str()?).try_into().ok()?;
    (rw > 0.0 && rh > 0.0).then_some((x / rw, y / rh))
}

/// "x y", relative to `ExifImageWidth`/`ExifImageHeight` — not the raw sensor size, verified against a real
/// RAF where those tags reflect the embedded preview's dimensions, not `RawImageWidth`/`RawImageHeight`.
fn fujifilm_focus_pixel(tags: &Value) -> Option<(f64, f64)> {
    let [x, y]: [f64; 2] = nums(tags.get("FocusPixel")?.as_str()?).try_into().ok()?;
    let rw = tags.get("ExifImageWidth")?.as_f64()?;
    let rh = tags.get("ExifImageHeight")?.as_f64()?;
    (rw > 0.0 && rh > 0.0).then_some((x / rw, y / rh))
}

/// Maps a point through the same flip `RgbaF32::oriented` applies to the image, so a marker lands where the
/// oriented photo shows it. Derived from, and must stay in step with, that function's pixel remap.
fn oriented_point(u: f64, v: f64, flip: i32) -> AfPoint {
    let u = if flip & 1 != 0 { 1.0 - u } else { u };
    let v = if flip & 2 != 0 { 1.0 - v } else { v };
    let (x, y) = if flip & 4 != 0 { (v, u) } else { (u, v) };
    AfPoint { x, y }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sony_focus_location_normalizes_against_its_own_reference_frame() {
        let tags: Value = serde_json::json!({ "FocusLocation": "9504 6336 3267 2930" });
        let (u, v) = sony_focus_location(&tags).unwrap();
        assert!((u - 3267.0 / 9504.0).abs() < 1e-9);
        assert!((v - 2930.0 / 6336.0).abs() < 1e-9);
    }

    #[test]
    fn fujifilm_focus_pixel_normalizes_against_exif_image_dimensions() {
        let tags: Value = serde_json::json!({ "FocusPixel": "1028 641", "ExifImageWidth": 1920, "ExifImageHeight": 1280 });
        let (u, v) = fujifilm_focus_pixel(&tags).unwrap();
        assert!((u - 1028.0 / 1920.0).abs() < 1e-9);
        assert!((v - 641.0 / 1280.0).abs() < 1e-9);
    }

    #[test]
    fn missing_tags_resolve_to_neither() {
        let tags: Value = serde_json::json!({});
        assert!(sony_focus_location(&tags).is_none());
        assert!(fujifilm_focus_pixel(&tags).is_none());
    }

    /// Matches `RgbaF32::oriented` in sidecar-engine pixel-for-pixel: a point at a given fraction of the
    /// unrotated image must land at the same fraction of the rotated one.
    #[test]
    fn oriented_point_matches_rgbaf32_oriented() {
        // Identity.
        let p = oriented_point(0.2, 0.7, 0);
        assert_eq!((p.x, p.y), (0.2, 0.7));
        // 180°: both axes flip.
        let p = oriented_point(0.2, 0.7, 3);
        assert!((p.x - 0.8).abs() < 1e-9 && (p.y - 0.3).abs() < 1e-9);
        // 90° CCW (flip 5): x' = y, y' = 1 - x.
        let p = oriented_point(0.2, 0.7, 5);
        assert!((p.x - 0.7).abs() < 1e-9 && (p.y - 0.8).abs() < 1e-9);
        // 90° CW (flip 6): x' = 1 - y, y' = x.
        let p = oriented_point(0.2, 0.7, 6);
        assert!((p.x - 0.3).abs() < 1e-9 && (p.y - 0.2).abs() < 1e-9);
        // Mirrored left to right (EXIF 2): x' = 1 - x.
        let p = oriented_point(0.2, 0.7, 1);
        assert!((p.x - 0.8).abs() < 1e-9 && (p.y - 0.7).abs() < 1e-9);
        // Transposed (EXIF 5): x' = y, y' = x.
        let p = oriented_point(0.2, 0.7, 4);
        assert!((p.x - 0.7).abs() < 1e-9 && (p.y - 0.2).abs() < 1e-9);
    }
}
