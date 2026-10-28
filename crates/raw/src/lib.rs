// SPDX-License-Identifier: Apache-2.0
//! Safe wrapper over LibRaw (through `sidecar-libraw-sys`).

use serde::{Deserialize, Serialize};
pub(crate) use sidecar_color::camera::illuminant_temperature;
use sidecar_color::camera::{CameraColor, LibrawColor};
use sidecar_libraw_sys as sys;
use std::ffi::{CStr, CString, c_char};
use std::path::Path;

pub mod af;
pub mod decoded;
pub mod gain_map;
pub mod heif_aux;
pub mod lens;
pub mod lensfun;
pub mod levels;
pub mod linear_dng;
pub mod portrait_depth;
pub use af::AfPoint;
pub use decoded::SourceSpace;
pub use sidecar_color::SourceKind;

#[derive(Debug, thiserror::Error)]
#[error("LibRaw: {0}")]
pub struct RawError(pub String);

fn check(code: i32) -> Result<(), RawError> {
    if code == 0 {
        Ok(())
    } else {
        Err(RawError(
            unsafe { CStr::from_ptr(sys::sc_strerror(code)) }
                .to_string_lossy()
                .into_owned(),
        ))
    }
}

fn text(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawInfo {
    pub make: String,
    pub model: String,
    pub lens: String,
    /// Visible sensor size after orientation.
    pub width: u32,
    pub height: u32,
    pub orientation_flip: i32,
    pub sensor: SensorKind,
    pub iso: f32,
    pub shutter_s: f32,
    pub aperture: f32,
    pub focal_length_mm: f32,
    pub captured_unix: i64,
    pub color: CameraColor,
    /// Set separately by `read_extras`, which needs the file path for ExifTool; `None` until then.
    #[serde(default)]
    pub af_point: Option<AfPoint>,
    /// The lens correction the file carries itself (DNG opcodes, the maker's tables), from `read_extras`.
    #[serde(default)]
    pub lens_profile: Option<sidecar_color::lens::LensProfile>,
    /// What ExifTool identifies the lens as, from `read_extras`: the key into lensfun.
    #[serde(default)]
    pub lens_identity: Option<lens::LensIdentity>,
    /// Scene-referred sensor data or an already-developed picture: decides how the develop pass renders it
    /// (`sidecar_color::SourceKind`) and which controls the editor offers. Distinct from `SensorKind`, which
    /// also says `Other` for real RAW files with no colour-filter mosaic.
    #[serde(default)]
    pub source_kind: SourceKind,
    /// The primaries a developed file's pixels are in, so decoding it again needs no second EXIF read.
    /// `None` for RAW, and in metadata cached before this field existed.
    #[serde(default)]
    pub source_space: Option<SourceSpace>,
}

impl RawInfo {
    /// Width and height before orientation: the frame the sensor data, and every lens radius, is in.
    pub fn sensor_size(&self) -> [u32; 2] {
        if self.orientation_flip & 4 != 0 {
            [self.height, self.width]
        } else {
            [self.width, self.height]
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SensorKind {
    Bayer,
    XTrans,
    Other,
}

/// The colour data LibRaw read, in the shape `sidecar_color` maps to a `CameraColor` — the same mapping the
/// browser's LibRaw build goes through, by way of the colour wasm.
fn camera_color(i: &sys::sc_info) -> CameraColor {
    LibrawColor {
        cam_mul: i.cam_mul,
        pre_mul: i.pre_mul,
        cam_xyz: i.cam_xyz,
        dng_illuminant: i.dng_illuminant,
        dng_colormatrix: i.dng_colormatrix,
        dng_analogbalance: i.dng_analogbalance,
        dng_calibration: i.dng_calibration,
    }
    .camera_color()
}

/// Linear camera RGB, 16-bit, sensor saturation at 65535, orientation applied.
pub struct LinearImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u16>,
}

impl LinearImage {
    /// Linear RGBA f32, alpha 1 where any channel reached sensor saturation — the shape every proxy has,
    /// ready for `RgbaF32::downsampled`, whose area filter turns the flags into clip coverage.
    pub fn rgba(&self) -> Vec<f32> {
        let saturated = saturation_threshold(&self.rgb);
        let mut out = Vec::with_capacity(self.rgb.len() / 3 * 4);
        for p in self.rgb.as_chunks::<3>().0 {
            let clip = p.iter().any(|&v| v >= saturated);
            out.extend(p.iter().map(|&v| v as f32 / 65535.0));
            out.push(clip as u8 as f32);
        }
        out
    }
}

/// Saturation threshold. LibRaw scales the declared white level to 65535, but some cameras saturate below
/// it; when the brightest sample is well up the range, treat anything within 2 % of it as clipped too.
fn saturation_threshold(rgb: &[u16]) -> u16 {
    let max = rgb.iter().copied().max().unwrap_or(0);
    let declared = (65535u32 * 99 / 100) as u16;
    if max >= 32768 {
        declared.min((max as u32 * 98 / 100) as u16)
    } else {
        declared
    }
}

/// The visible sensor mosaic, before orientation. `color_at(y, x)` is 0 R, 1 G, 2 B, 3 G2.
pub struct Mosaic {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u16>,
    pub black: [f32; 4],
    pub white: f32,
    pub pattern: [[u8; 6]; 6],
    pub period: u32,
    pub flip: i32,
    /// Where each CFA colour actually saturates, as a fraction of `white` above its own black: measured from
    /// these samples by [`levels::mosaic_clip`], not guessed from the single brightest one (D6 and D7).
    pub clip: [f32; 4],
}

impl Mosaic {
    pub fn color_at(&self, y: u32, x: u32) -> u8 {
        self.pattern[(y % self.period) as usize][(x % self.period) as usize]
    }
    pub fn is_xtrans(&self) -> bool {
        self.period == 6
    }
}

pub struct RawFile {
    ptr: *mut std::ffi::c_void,
    info: sys::sc_info,
}

// A LibRaw instance may move between threads, but must not be shared.
unsafe impl Send for RawFile {}

/// Everything read from ExifTool rather than LibRaw — the AF point, the lens correction the file carries and
/// what the lens is — in one run of it. Leaves `info` as it was if ExifTool is missing or fails.
pub fn read_extras(path: &Path, info: &mut RawInfo) {
    let out = std::process::Command::new("exiftool")
        .arg("-j")
        .args(lens::TAGS)
        .args(af::TAGS)
        .arg(path)
        .output();
    let Some(tags) = out
        .ok()
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .and_then(|v| v.get(0).cloned())
    else {
        return;
    };
    info.af_point = af::from_tags(&lens::ungrouped(&tags), info.orientation_flip);
    info.lens_profile = lens::from_tags(&tags, info.sensor_size());
    info.lens_identity = Some(lens::identity(&tags));
}

impl RawFile {
    /// Open and unpack the sensor data, ready for `mosaic` or `develop_linear`.
    pub fn open(path: &Path) -> Result<Self, RawError> {
        Self::open_with(path, true)
    }

    /// What `open(path)?.info()` returns, without unpacking the sensor data: LibRaw has parsed everything
    /// `info` reads by the end of `open_file`, and unpacking is the part of an open that costs — the whole
    /// file read and decompressed, for a metadata request.
    pub fn metadata(path: &Path) -> Result<RawInfo, RawError> {
        Ok(Self::open_with(path, false)?.info())
    }

    fn open_with(path: &Path, unpack: bool) -> Result<Self, RawError> {
        let c = CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| RawError("path contains NUL".into()))?;
        let ptr = unsafe { sys::sc_new() };
        let mut raw = RawFile {
            ptr,
            info: unsafe { std::mem::zeroed() },
        };
        check(unsafe { sys::sc_open(ptr, c.as_ptr()) })?;
        if unpack {
            check(unsafe { sys::sc_unpack(ptr) })?;
        }
        unsafe { sys::sc_get_info(ptr, &mut raw.info) };
        Ok(raw)
    }

    pub fn info(&self) -> RawInfo {
        let i = &self.info;
        let rotated = i.flip & 4 != 0;
        let (w, h) = (i.width as u32, i.height as u32);
        RawInfo {
            make: text(&i.make),
            model: text(&i.model),
            lens: text(&i.lens),
            width: if rotated { h } else { w },
            height: if rotated { w } else { h },
            orientation_flip: i.flip,
            sensor: match i.filters {
                9 => SensorKind::XTrans,
                0 => SensorKind::Other,
                _ => SensorKind::Bayer,
            },
            iso: i.iso,
            shutter_s: i.shutter,
            aperture: i.aperture,
            focal_length_mm: i.focal_len,
            captured_unix: i.timestamp,
            color: camera_color(i),
            af_point: None,
            lens_profile: None,
            lens_identity: None,
            source_kind: SourceKind::Raw,
            source_space: None,
        }
    }

    /// Demosaic to linear camera RGB. `half_size` bins 2×2 without interpolation (fast; used for proxies).
    pub fn develop_linear(&mut self, half_size: bool) -> Result<LinearImage, RawError> {
        let mut img = sys::sc_image {
            width: 0,
            height: 0,
            data: std::ptr::null_mut(),
            handle: std::ptr::null_mut(),
        };
        check(unsafe { sys::sc_develop_linear(self.ptr, half_size as i32, 11, &mut img) })?;
        let n = (img.width * img.height * 3) as usize;
        let rgb = unsafe { std::slice::from_raw_parts(img.data, n) }.to_vec();
        unsafe { sys::sc_image_free(&mut img) };
        Ok(LinearImage {
            width: img.width as u32,
            height: img.height as u32,
            rgb,
        })
    }

    /// The raw mosaic for GPU processing, or an error for files without a single-channel CFA.
    ///
    /// The levels are checked here because nothing downstream can: `normalize.wgsl` divides by
    /// `white - black[c]`, so a zero or bogus maximum yields a black frame and no error at all (the October
    /// 2026 pipeline review, D4; ProRAW's black in #34 was the same failure on the linear-DNG path). An `Err`
    /// is the useful answer: both callers already fall back to LibRaw's own demosaic on one.
    pub fn mosaic(&self) -> Result<Mosaic, RawError> {
        let mut m: sys::sc_mosaic = unsafe { std::mem::zeroed() };
        check(unsafe { sys::sc_get_mosaic(self.ptr, &mut m) })?;
        let n = (m.width as usize) * (m.height as usize);
        if let Err(e) = usable_mosaic(n, m.data.is_null(), m.white, &m.black, &m.pattern, m.period)
        {
            unsafe { sys::sc_mosaic_free(&mut m) };
            return Err(e);
        }
        let data = unsafe { std::slice::from_raw_parts(m.data, n) }.to_vec();
        unsafe { sys::sc_mosaic_free(&mut m) };
        let clip = levels::mosaic_clip(
            &data,
            m.width as u32,
            m.white,
            &m.black,
            &m.pattern,
            m.period as u32,
        );
        Ok(Mosaic {
            width: m.width as u32,
            height: m.height as u32,
            data,
            black: m.black,
            white: m.white,
            pattern: m.pattern,
            period: m.period as u32,
            flip: self.info.flip,
            clip,
        })
    }
}

/// Is this mosaic usable at all? Separate from `mosaic()` so it can be tested without a camera file.
///
/// `normalize.wgsl` reads `(sample - black[c]) / (white - black[c])`, so a maximum at or below black divides by
/// zero or flips the sign and the whole frame comes out black with no error anywhere (D4). Only the colours the
/// pattern actually names are checked: a Bayer file leaves `black[3]` (G2) at whatever LibRaw left there, and an
/// X-Trans one never names colour 3.
fn usable_mosaic(
    n: usize,
    data_is_null: bool,
    white: f32,
    black: &[f32; 4],
    pattern: &[[u8; 6]; 6],
    period: i32,
) -> Result<(), RawError> {
    if n == 0 || data_is_null {
        return Err(RawError(format!(
            "mosaic is empty ({n} samples{})",
            if data_is_null { ", no data" } else { "" }
        )));
    }
    if !white.is_finite() {
        return Err(RawError(format!("maximum is not a number ({white})")));
    }
    let period = (period as usize).clamp(1, 6);
    let mut used = [false; 4];
    for row in pattern.iter().take(period) {
        for &c in row.iter().take(period) {
            if let Some(u) = used.get_mut(c as usize) {
                *u = true;
            }
        }
    }
    for (c, _) in used.iter().enumerate().filter(|(_, u)| **u) {
        // One step of headroom: a white equal to black divides by zero, and a gap under one unit cannot carry a
        // 16-bit sample anyway.
        if !black[c].is_finite() || white - black[c] < 1.0 {
            return Err(RawError(format!(
                "levels make no sense: maximum {white} is not above black {} for colour {c}",
                black[c]
            )));
        }
    }
    Ok(())
}

impl Drop for RawFile {
    fn drop(&mut self) {
        unsafe { sys::sc_free(self.ptr) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Bayer pattern (RGGB) and an X-Trans one, for the level checks below.
    fn bayer() -> [[u8; 6]; 6] {
        let mut p = [[0u8; 6]; 6];
        for (y, row) in p.iter_mut().enumerate() {
            for (x, c) in row.iter_mut().enumerate() {
                *c = match (y % 2, x % 2) {
                    (0, 0) => 0,
                    (0, 1) | (1, 0) => 1,
                    _ => 2,
                };
            }
        }
        p
    }

    /// D4: a maximum that is not above black makes `normalize.wgsl` divide by zero or by a negative, and the
    /// frame comes out black with nothing reported. `mosaic()` refuses instead, and both callers fall back to
    /// LibRaw's own demosaic.
    #[test]
    fn levels_that_cannot_normalize_are_refused() {
        let p = bayer();
        let ok = [512.0, 512.0, 512.0, 512.0];
        assert!(usable_mosaic(24, false, 16383.0, &ok, &p, 2).is_ok());

        // No samples, or no buffer to read them from.
        assert!(usable_mosaic(0, false, 16383.0, &ok, &p, 2).is_err());
        assert!(usable_mosaic(24, true, 16383.0, &ok, &p, 2).is_err());

        // A maximum of zero: the shape of the failure this guard exists for.
        assert!(usable_mosaic(24, false, 0.0, &ok, &p, 2).is_err());
        // Maximum equal to black, and below it.
        assert!(usable_mosaic(24, false, 512.0, &ok, &p, 2).is_err());
        assert!(usable_mosaic(24, false, 300.0, &ok, &p, 2).is_err());
        // Not numbers.
        assert!(usable_mosaic(24, false, f32::NAN, &ok, &p, 2).is_err());
        assert!(usable_mosaic(24, false, 16383.0, &[f32::NAN; 4], &p, 2).is_err());
    }

    /// Only the colours the pattern names: a nonsense `black[3]` is no reason to refuse a Bayer file, where
    /// colour 3 never appears, but it is for a four-colour pattern that uses it.
    #[test]
    fn unused_colours_do_not_condemn_a_file() {
        let mut four = bayer();
        four[1][1] = 3;
        four[3][3] = 3;
        four[5][5] = 3;
        let black = [512.0, 512.0, 512.0, 1.0e9];
        assert!(usable_mosaic(24, false, 16383.0, &black, &bayer(), 2).is_ok());
        assert!(usable_mosaic(24, false, 16383.0, &black, &four, 6).is_err());
    }

    #[test]
    fn saturated_samples_are_flagged_in_alpha() {
        let img = LinearImage {
            width: 2,
            height: 1,
            rgb: vec![65535, 0, 0, 32768, 32768, 32768],
        };
        let rgba = img.rgba();
        assert_eq!(rgba.len(), 8);
        assert_eq!((rgba[0], rgba[3], rgba[7]), (1.0, 1.0, 0.0));
        assert!((rgba[4] - 0.5).abs() < 1e-4);
    }

    /// `metadata` skips the unpack; it has to agree with an unpacked open on every field, for every maker in
    /// the test set. Files are copies under testdata/cache; the test passes vacuously without them.
    #[test]
    fn metadata_without_unpacking_matches_a_full_open() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/cache");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut checked = 0;
        for path in entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| sidecar_template::is_raw(p))
        {
            // Something LibRaw can't open at all is not what this compares.
            let Ok(full) = RawFile::open(&path) else {
                continue;
            };
            let full = serde_json::to_value(full.info()).unwrap();
            let quick = serde_json::to_value(RawFile::metadata(&path).unwrap()).unwrap();
            assert_eq!(full, quick, "{}", path.display());
            checked += 1;
        }
        eprintln!("compared {checked} RAW files");
    }
}
