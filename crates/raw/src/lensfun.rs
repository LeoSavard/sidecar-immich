// SPDX-License-Identifier: Apache-2.0
//! The lensfun database as a fallback for lenses whose files carry no correction of their own — the Nikon
//! DSLRs, Canon and Olympus bodies in this library. The database (CC BY-SA 3.0) is downloaded at run time
//! into the cache, never shipped (DESIGN.md §12); the library (LGPL-3.0) is not used. What is here is written
//! from lensfun's published documentation of its file format and models, and from the PanoTools wiki's
//! definition of the normalized radius those models use:
//!
//! - distortion `ptlens`: source radius = r · (a r³ + b r² + c r + 1 − a − b − c); `poly3`: r · (1 − k1 + k1 r²);
//!   `poly5`: r · (1 + k1 r² + k2 r⁴). r = 1 is half the shorter side of the calibration camera's frame.
//! - lateral CA `poly3`: red source radius = r · (br r² + cr r + vr), likewise blue, relative to green, in the
//!   uncorrected image; `linear`: r · kr.
//! - vignetting `pa`: recorded brightness = true · (1 + k1 r² + k2 r⁴ + k3 r⁶). Its radius runs to the
//!   calibration frame's corner: on the shorter side's scale, a typical kit zoom's table would dim the
//!   corners by more than all the light there is.
//!
//! A calibration made on one sensor size applies to another by the ratio of their crop factors: the radii are
//! physical distances on the sensor.
//!
//! Checked against the cameras' own renderings (`sidecar-dev camera-jpeg` with `SIDECAR_LENSFUN`): lensfun's
//! ZV-1 profile, read this way, aligns a RAW with the camera's corrected JPEG best at exactly the radius scale
//! computed here (a stretch of 1.00, median of 7 frames at 9.4 mm, where the lens distorts by 10 %) — and
//! better than no correction in 7 of 7, though Sony's own table does better still (0.979 against 0.963),
//! which is why a file's own correction wins. Nine Nikon DSLR frames shot with in-camera distortion control
//! align better with the camera's JPEG through the matched profile in 9 of 9.

use std::collections::HashMap;
use std::path::Path;

use sidecar_color::lens::{Curve, LensProfile, R_MAX};

use crate::RawInfo;
use crate::lens::LensIdentity;

#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    pub maker: String,
    pub models: Vec<String>,
    pub mount: String,
    pub crop: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Distortion {
    Poly3 { k1: f64 },
    Poly5 { k1: f64, k2: f64 },
    PtLens { a: f64, b: f64, c: f64 },
}

impl Distortion {
    /// Source radius ÷ corrected radius at `r` (lensfun's normalization).
    pub fn scale(self, r: f64) -> f64 {
        match self {
            Self::Poly3 { k1 } => 1.0 - k1 + k1 * r * r,
            Self::Poly5 { k1, k2 } => 1.0 + k1 * r * r + k2 * r.powi(4),
            Self::PtLens { a, b, c } => a * r.powi(3) + b * r * r + c * r + 1.0 - a - b - c,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tca {
    Linear { kr: f64, kb: f64 },
    Poly3 { vr: f64, cr: f64, br: f64, vb: f64, cb: f64, bb: f64 },
}

impl Tca {
    /// Red and blue source radius ÷ green's, at green's source radius `r`.
    pub fn scale(self, r: f64) -> (f64, f64) {
        match self {
            Self::Linear { kr, kb } => (kr, kb),
            Self::Poly3 { vr, cr, br, vb, cb, bb } => (br * r * r + cr * r + vr, bb * r * r + cb * r + vb),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vignetting {
    pub focal: f64,
    pub aperture: f64,
    pub distance: f64,
    pub k: [f64; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lens {
    pub maker: String,
    pub models: Vec<String>,
    pub mounts: Vec<String>,
    pub crop: f64,
    /// Longer edge ÷ shorter edge of the calibration camera.
    pub aspect: f64,
    pub distortion: Vec<(f64, Distortion)>,
    pub tca: Vec<(f64, Tca)>,
    pub vignetting: Vec<Vignetting>,
}

#[derive(Clone, Debug, Default)]
pub struct Database {
    pub cameras: Vec<Camera>,
    pub lenses: Vec<Lens>,
    /// Mount → the mounts whose lenses fit it.
    pub compat: HashMap<String, Vec<String>>,
}

fn attr(n: &roxmltree::Node, name: &str) -> Option<f64> {
    n.attribute(name)?.trim().parse().ok()
}

fn child_texts(n: &roxmltree::Node, tag: &str) -> Vec<String> {
    n.children().filter(|c| c.has_tag_name(tag)).filter_map(|c| c.text()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
}

fn aspect(s: &str) -> Option<f64> {
    match s.split_once(':') {
        Some((a, b)) => Some(a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok()?),
        None => s.trim().parse().ok(),
    }
}

impl Database {
    /// Every `*.xml` in `dir`. Files that fail to parse are skipped, not fatal: one bad download shouldn't
    /// take the other 58 with it.
    pub fn load(dir: &Path) -> Self {
        let mut db = Self::default();
        let mut files: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "xml")).collect();
        files.sort();
        for f in files {
            match std::fs::read_to_string(&f).map_err(|e| e.to_string()).and_then(|xml| db.add(&xml)) {
                Ok(()) => {}
                Err(e) => tracing::warn!("lensfun: skipping {}: {e}", f.display()),
            }
        }
        db
    }

    pub fn add(&mut self, xml: &str) -> Result<(), String> {
        // The repository's files name a DTD; roxmltree never fetches one, so allowing it is safe.
        let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
        let doc = roxmltree::Document::parse_with_options(xml, opts).map_err(|e| e.to_string())?;
        for n in doc.root_element().children().filter(|n| n.is_element()) {
            match n.tag_name().name() {
                "mount" => {
                    if let Some(name) = child_texts(&n, "name").into_iter().next() {
                        self.compat.entry(name).or_default().extend(child_texts(&n, "compat"));
                    }
                }
                "camera" => {
                    let (Some(maker), Some(mount)) = (child_texts(&n, "maker").into_iter().next(), child_texts(&n, "mount").into_iter().next()) else { continue };
                    let crop = child_texts(&n, "cropfactor").first().and_then(|c| c.parse().ok()).unwrap_or(1.0);
                    self.cameras.push(Camera { maker, models: child_texts(&n, "model"), mount, crop });
                }
                "lens" => {
                    let Some(maker) = child_texts(&n, "maker").into_iter().next() else { continue };
                    let mut lens = Lens {
                        maker,
                        models: child_texts(&n, "model"),
                        mounts: child_texts(&n, "mount"),
                        crop: child_texts(&n, "cropfactor").first().and_then(|c| c.parse().ok()).unwrap_or(1.0),
                        aspect: child_texts(&n, "aspect-ratio").first().and_then(|a| aspect(a)).unwrap_or(1.5),
                        distortion: Vec::new(),
                        tca: Vec::new(),
                        vignetting: Vec::new(),
                    };
                    for cal in n.children().filter(|c| c.has_tag_name("calibration")) {
                        for e in cal.children().filter(|c| c.is_element()) {
                            let Some(focal) = attr(&e, "focal") else { continue };
                            let model = e.attribute("model").unwrap_or_default();
                            let z = |k: &str| attr(&e, k).unwrap_or(0.0);
                            match (e.tag_name().name(), model) {
                                ("distortion", "ptlens") => lens.distortion.push((focal, Distortion::PtLens { a: z("a"), b: z("b"), c: z("c") })),
                                ("distortion", "poly3") => lens.distortion.push((focal, Distortion::Poly3 { k1: z("k1") })),
                                ("distortion", "poly5") => lens.distortion.push((focal, Distortion::Poly5 { k1: z("k1"), k2: z("k2") })),
                                ("tca", "poly3") => lens.tca.push((focal, Tca::Poly3 { vr: attr(&e, "vr").unwrap_or(1.0), cr: z("cr"), br: z("br"), vb: attr(&e, "vb").unwrap_or(1.0), cb: z("cb"), bb: z("bb") })),
                                ("tca", "linear") => lens.tca.push((focal, Tca::Linear { kr: attr(&e, "kr").unwrap_or(1.0), kb: attr(&e, "kb").unwrap_or(1.0) })),
                                ("vignetting", "pa") => lens.vignetting.push(Vignetting { focal, aperture: z("aperture"), distance: attr(&e, "distance").unwrap_or(1000.0), k: [z("k1"), z("k2"), z("k3")] }),
                                // The Adobe camera model measures in focal lengths, not frames; not read.
                                _ => {}
                            }
                        }
                    }
                    lens.distortion.sort_by(|a, b| a.0.total_cmp(&b.0));
                    lens.tca.sort_by(|a, b| a.0.total_cmp(&b.0));
                    self.lenses.push(lens);
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The camera as LibRaw names it (`Nikon`, `D3200`). lensfun spells makers in full (`Nikon Corporation`)
    /// and models with or without the maker (`Nikon D3200`, `D3200`), so both are compared loosely.
    pub fn camera(&self, make: &str, model: &str) -> Option<&Camera> {
        let make = norm(make);
        let first = make.split_whitespace().next().unwrap_or_default().to_string();
        let model = norm(model);
        let bare = |m: &str| {
            let m = norm(m);
            m.strip_prefix(&format!("{first} ")).map(str::to_string).unwrap_or(m)
        };
        self.cameras.iter().find(|c| norm(&c.maker).split_whitespace().next() == Some(first.as_str()) && c.models.iter().any(|m| bare(m) == bare(&model)))
    }

    fn mounts_for<'a>(&'a self, camera: &'a Camera) -> Vec<&'a str> {
        let mut out = vec![camera.mount.as_str()];
        out.extend(self.compat.get(&camera.mount).into_iter().flatten().map(String::as_str));
        out
    }

    /// The lens a photo was taken with. A fixed-lens camera's own mount holds its lens; an interchangeable
    /// one is matched on the name ExifTool gives it, requiring the same focal range and maximum aperture and
    /// scoring the other words, among lenses that fit the camera's mount.
    pub fn lens(&self, camera: Option<&Camera>, id: &LensIdentity) -> Option<&Lens> {
        let fits = |l: &Lens| camera.is_none_or(|c| self.mounts_for(c).iter().any(|m| l.mounts.iter().any(|lm| lm == m)));
        if let Some(c) = camera {
            let own: Vec<&Lens> = self.lenses.iter().filter(|l| l.mounts.contains(&c.mount)).collect();
            let fixed = !self.compat.contains_key(&c.mount) && own.len() == 1;
            if fixed {
                return own.into_iter().next();
            }
        }
        let names: Vec<&str> = [id.lens_id.as_deref(), id.lens_model.as_deref()].into_iter().flatten().collect();
        let mut best: Option<(f64, &Lens)> = None;
        for l in self.lenses.iter().filter(|l| fits(l)) {
            let score = names.iter().flat_map(|n| l.models.iter().map(move |m| name_score(n, m))).fold(0.0, f64::max);
            if score > best.map_or(0.0, |b| b.0) {
                best = Some((score, l));
            }
        }
        best.filter(|b| b.0 >= 0.5).map(|b| b.1)
    }

    /// A lensfun profile for this photo, or `None` when the database has no camera, no lens or no calibration
    /// for it. `info` supplies the focal length and aperture; `id` the lens name, crop factor and distance.
    pub fn profile(&self, info: &RawInfo, id: &LensIdentity) -> Option<LensProfile> {
        let camera = self.camera(&info.make, &info.model);
        let lens = self.lens(camera, id)?;
        // The database's crop factor for the body first: ExifTool derives its own from the 35 mm equivalent the
        // camera writes, which can be a rounded marketing figure — a ZV-1 says 24 mm at 9.4 mm, a crop of 2.55 where
        // the sensor's is 2.7, and that 6 % showed as the profile's radius being off by as much against the
        // camera's own JPEG.
        let crop = camera.map(|c| c.crop).or(id.crop_factor).unwrap_or(lens.crop);
        lens.profile(crop, info.focal_length_mm as f64, info.aperture as f64, id.focus_distance_m.unwrap_or(1000.0))
    }
}

fn norm(s: &str) -> String {
    s.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Words of a lens name, with the focal range and maximum aperture pulled out: `(focal, aperture, words)`.
fn lens_tokens(name: &str) -> (Option<String>, Option<String>, Vec<String>) {
    let s = name.to_lowercase().replace(['(', ')', ',', '|'], " ");
    let (mut focal, mut aperture, mut words) = (None, None, Vec::new());
    for t in s.split_whitespace() {
        if let Some(f) = t.strip_suffix("mm").filter(|f| f.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-') && !f.is_empty()) {
            focal = Some(f.split('-').map(|v| v.trim_end_matches(".0").to_string()).collect::<Vec<_>>().join("-"));
            continue;
        }
        let ap = t.strip_prefix("f/").or_else(|| t.strip_prefix("1:")).or_else(|| t.strip_prefix('f').filter(|r| r.starts_with(|c: char| c.is_ascii_digit())));
        if let Some(ap) = ap {
            let digits: String = ap.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
            if !digits.is_empty() {
                aperture = Some(digits.split('-').map(|v| v.trim_end_matches(".0").to_string()).collect::<Vec<_>>().join("-"));
                let rest = &ap[digits.len()..];
                if !rest.is_empty() {
                    words.push(rest.to_string());
                }
                continue;
            }
        }
        words.extend(t.split('-').filter(|w| !w.is_empty()).map(str::to_string));
    }
    (focal, aperture, words)
}

/// How well two lens names agree, 0…1: nothing unless focal range and maximum aperture both match, then the
/// share of the remaining words they have in common. The maker's name is left out — it is matched by mount.
pub fn name_score(a: &str, b: &str) -> f64 {
    let (fa, aa, wa) = lens_tokens(a);
    let (fb, ab, wb) = lens_tokens(b);
    if fa.is_none() || fa != fb || aa != ab {
        return 0.0;
    }
    const MAKERS: [&str; 12] = ["nikon", "canon", "sony", "fujifilm", "fujinon", "olympus", "panasonic", "lumix", "leica", "sigma", "tamron", "tokina"];
    let set = |w: &[String]| -> std::collections::BTreeSet<String> { w.iter().filter(|w| !MAKERS.contains(&w.as_str())).cloned().collect() };
    let (sa, sb) = (set(&wa), set(&wb));
    let union = sa.union(&sb).count();
    if union == 0 {
        return 1.0;
    }
    0.5 + 0.5 * sa.intersection(&sb).count() as f64 / union as f64
}

/// Linear interpolation between the calibrations on either side of `x`, held at the ends.
fn between<T: Copy>(points: &[(f64, T)], x: f64) -> Option<(T, T, f64)> {
    let first = points.first()?;
    if points.len() == 1 || x <= first.0 {
        return Some((first.1, first.1, 0.0));
    }
    let last = points.last()?;
    if x >= last.0 {
        return Some((last.1, last.1, 0.0));
    }
    let i = points.windows(2).position(|w| x < w[1].0)?;
    let (a, b) = (points[i], points[i + 1]);
    Some((a.1, b.1, (x - a.0) / (b.0 - a.0)))
}

impl Lens {
    pub fn name(&self) -> &str {
        self.models.first().map(String::as_str).unwrap_or("lens")
    }

    /// The correction at this focal length, aperture and distance, for a camera of crop factor `crop`, in
    /// `sidecar_color::lens` units (radius 1 = the photo's half-diagonal).
    pub fn profile(&self, crop: f64, focal: f64, aperture: f64, distance: f64) -> Option<LensProfile> {
        const N: usize = 97;
        let to_photo = self.crop / crop;
        // The photo's half-diagonal on lensfun's distortion scale (half the calibration frame's shorter side).
        let dist_unit = to_photo * (1.0 + self.aspect * self.aspect).sqrt();
        let distortion = between(&self.distortion, focal).map(|(a, b, t)| move |r: f64| (1.0 - t) * a.scale(r) + t * b.scale(r));
        let green = |r: f64| distortion.map_or(1.0, |d| d(r * dist_unit));
        let tca = between(&self.tca, focal).map(|(a, b, t)| {
            move |r: f64| {
                let (ra, ba) = a.scale(r);
                let (rb, bb) = b.scale(r);
                ((1.0 - t) * ra + t * rb, (1.0 - t) * ba + t * bb)
            }
        });
        let vignetting = (aperture > 0.0).then(|| self.vignetting_at(focal, aperture, distance)).flatten();
        if distortion.is_none() && tca.is_none() && vignetting.is_none() {
            return None;
        }
        Some(LensProfile {
            id: format!("lensfun:{}/{}", self.maker, self.name()),
            label: format!("lensfun: {}", self.name()),
            center: [0.5, 0.5],
            distortion: distortion.map(|_| Curve::sample(N, R_MAX, green)),
            // lensfun measures CA in the uncorrected image, at green's radius there.
            ca_red: tca.map(|f| Curve::sample(N, R_MAX, |r| f(r * dist_unit * green(r)).0)),
            ca_blue: tca.map(|f| Curve::sample(N, R_MAX, |r| f(r * dist_unit * green(r)).1)),
            vignetting: vignetting.map(|k| {
                Curve::sample(N, R_MAX, |r| {
                    let x2 = (r * to_photo).powi(2);
                    1.0 / (1.0 + k[0] * x2 + k[1] * x2 * x2 + k[2] * x2 * x2 * x2).max(0.05)
                })
            }),
            required: false,
        })
    }

    /// The `pa` coefficients at a focal length, aperture and distance: the nearest distance measured, then
    /// linear in stops between apertures and linear between focal lengths, held at the ends.
    fn vignetting_at(&self, focal: f64, aperture: f64, distance: f64) -> Option<[f64; 3]> {
        if self.vignetting.is_empty() {
            return None;
        }
        let mut focals: Vec<f64> = self.vignetting.iter().map(|v| v.focal).collect();
        focals.sort_by(f64::total_cmp);
        focals.dedup();
        let at_focal = |f: f64| -> Option<[f64; 3]> {
            let here: Vec<&Vignetting> = self.vignetting.iter().filter(|v| v.focal == f).collect();
            let d = here.iter().map(|v| v.distance).min_by(|a, b| (a.ln() - distance.max(0.01).ln()).abs().total_cmp(&(b.ln() - distance.max(0.01).ln()).abs()))?;
            let mut pts: Vec<(f64, [f64; 3])> = here.iter().filter(|v| v.distance == d).map(|v| (v.aperture.max(0.1).log2(), v.k)).collect();
            pts.sort_by(|a, b| a.0.total_cmp(&b.0));
            let (a, b, t) = between(&pts, aperture.log2())?;
            Some([0, 1, 2].map(|i| (1.0 - t) * a[i] + t * b[i]))
        };
        let pts: Vec<(f64, [f64; 3])> = focals.iter().filter_map(|&f| Some((f, at_focal(f)?))).collect();
        let (a, b, t) = between(&pts, focal)?;
        Some([0, 1, 2].map(|i| (1.0 - t) * a[i] + t * b[i]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<lensdatabase version="2">
    <mount><name>Nikon F AF</name><compat>Nikon F</compat><compat>Nikon F AI</compat></mount>
    <camera><maker>Nikon Corporation</maker><model>Nikon D3200</model><model lang="en">D3200</model><mount>Nikon F AF</mount><cropfactor>1.523</cropfactor></camera>
    <camera><maker>Fujifilm</maker><model>X100F</model><mount>fujix100</mount><cropfactor>1.528</cropfactor></camera>
    <lens>
        <maker>Nikon</maker>
        <model>Nikon AF-S DX Zoom-Nikkor 18-55mm f/3.5-5.6G VR</model>
        <mount>Nikon F AF</mount>
        <cropfactor>1.523</cropfactor>
        <calibration>
            <distortion model="ptlens" focal="18" a="0.0195" b="-0.0591" c="0.0105"/>
            <distortion model="ptlens" focal="55" a="0.0" b="0.004" c="0.0"/>
            <tca model="poly3" focal="18" br="-0.0001" vr="1.0004" bb="0.0001" vb="0.9997"/>
            <vignetting model="pa" focal="18" aperture="3.5" distance="10" k1="-0.9" k2="0.6" k3="-0.2"/>
            <vignetting model="pa" focal="18" aperture="8" distance="10" k1="-0.3" k2="0.1" k3="0.0"/>
        </calibration>
    </lens>
    <lens>
        <maker>Nikon</maker>
        <model>Nikon AF-P DX Nikkor 18-55mm f/3.5-5.6G VR</model>
        <mount>Nikon F AF</mount>
        <cropfactor>1.523</cropfactor>
        <calibration><distortion model="ptlens" focal="18" a="0.01" b="-0.03" c="0.0"/></calibration>
    </lens>
    <lens>
        <maker>Fujifilm</maker>
        <model>X100 &amp; compatibles (Standard)</model>
        <mount>fujix100</mount>
        <cropfactor>1.523</cropfactor>
        <calibration>
            <vignetting model="pa" focal="23.0" aperture="2.0" distance="1000" k1="-1.0737795" k2="0.5794291" k3="-0.1580935"/>
        </calibration>
    </lens>
</lensdatabase>"#;

    fn db() -> Database {
        let mut db = Database::default();
        db.add(XML).unwrap();
        db
    }

    #[test]
    fn matches_the_lens_exiftool_names() {
        let db = db();
        let cam = db.camera("Nikon", "D3200").expect("camera");
        let id = |s: &str| LensIdentity { lens_id: Some(s.into()), ..Default::default() };
        assert_eq!(db.lens(Some(cam), &id("AF-S DX VR Zoom-Nikkor 18-55mm f/3.5-5.6G")).unwrap().name(), "Nikon AF-S DX Zoom-Nikkor 18-55mm f/3.5-5.6G VR");
        assert_eq!(db.lens(Some(cam), &id("AF-P DX Nikkor 18-55mm f/3.5-5.6G")).unwrap().name(), "Nikon AF-P DX Nikkor 18-55mm f/3.5-5.6G VR");
        assert!(db.lens(Some(cam), &id("AF-S Nikkor 85mm f/1.8G")).is_none(), "a lens the database lacks matches nothing");
        // A fixed-lens camera's mount holds its one lens; no name needed.
        let x100 = db.camera("Fujifilm", "X100F").unwrap();
        assert_eq!(db.lens(Some(x100), &LensIdentity::default()).unwrap().name(), "X100 & compatibles (Standard)");
    }

    #[test]
    fn names_need_the_same_focal_range_and_aperture() {
        assert_eq!(name_score("AF-S Nikkor 50mm f/1.8G", "Nikon AF-S Nikkor 50mm f/1.4G"), 0.0);
        assert_eq!(name_score("AF-S Nikkor 50mm f/1.8G", "Nikon AF-S Nikkor 35mm f/1.8G"), 0.0);
        assert!(name_score("AF-S DX Nikkor 18-140mm f/3.5-5.6G ED VR", "Nikon AF-S DX Nikkor 18-140mm f/3.5-5.6G ED VR") > 0.95);
        assert!(name_score("Canon EF-M 18-55mm f/3.5-5.6 IS STM", "Canon EF-M 18-55mm f/3.5-5.6 IS STM") > 0.95);
    }

    #[test]
    fn distortion_interpolates_between_focal_lengths_and_scales_by_crop() {
        let db = db();
        let lens = &db.lenses[0];
        let at18 = lens.profile(1.523, 18.0, 0.0, 10.0).unwrap();
        let at55 = lens.profile(1.523, 55.0, 0.0, 10.0).unwrap();
        let mid = lens.profile(1.523, 36.5, 0.0, 10.0).unwrap();
        let d = |p: &LensProfile, r: f64| p.distortion.as_ref().unwrap().eval(r);
        assert!((d(&mid, 0.8) - (d(&at18, 0.8) + d(&at55, 0.8)) / 2.0).abs() < 1e-3);
        // On the half-diagonal, r = 1 is 1.803 half-heights of a 3:2 frame.
        let direct = Distortion::PtLens { a: 0.0195, b: -0.0591, c: 0.0105 }.scale(1.0 * (1.0f64 + 2.25).sqrt());
        assert!((d(&at18, 1.0) - direct).abs() < 1e-6);
        // On a full-frame body the photo reaches 1.523× further out of the DX calibration frame.
        let ff = lens.profile(1.0, 18.0, 0.0, 10.0).unwrap();
        let expect = Distortion::PtLens { a: 0.0195, b: -0.0591, c: 0.0105 }.scale(1.523 * 3.25f64.sqrt());
        assert!((d(&ff, 1.0) - expect).abs() < 1e-6);
        // No aperture, no vignetting; with one, interpolated between f/3.5 and f/8 in stops.
        assert!(at18.vignetting.is_none());
        let v = lens.profile(1.523, 18.0, 5.6, 10.0).unwrap().vignetting.unwrap();
        let (open, closed) = (lens.profile(1.523, 18.0, 3.5, 10.0).unwrap().vignetting.unwrap(), lens.profile(1.523, 18.0, 8.0, 10.0).unwrap().vignetting.unwrap());
        assert!(v.eval(1.0) < open.eval(1.0) && v.eval(1.0) > closed.eval(1.0), "{} between {} and {}", v.eval(1.0), open.eval(1.0), closed.eval(1.0));
    }
}
