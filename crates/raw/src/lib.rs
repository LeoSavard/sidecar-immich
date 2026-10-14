// SPDX-License-Identifier: Apache-2.0
//! Safe wrapper over LibRaw (through `sidecar-libraw-sys`).

use serde::{Deserialize, Serialize};
use sidecar_color::camera::{Calibration, CameraColor};
use sidecar_libraw_sys as sys;
use std::ffi::{CStr, CString, c_char};
use std::path::Path;

pub mod af;
pub mod decoded;
pub mod gain_map;
pub mod heif_aux;
pub mod lens;
pub mod lensfun;
pub mod linear_dng;
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
        Err(RawError(unsafe { CStr::from_ptr(sys::sc_strerror(code)) }.to_string_lossy().into_owned()))
    }
}

fn text(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
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
        if self.orientation_flip & 4 != 0 { [self.height, self.width] } else { [self.width, self.height] }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SensorKind {
    Bayer,
    XTrans,
    Other,
}

/// EXIF LightSource → correlated colour temperature, for DNG calibration illuminants.
pub(crate) fn illuminant_temperature(code: u16) -> Option<f64> {
    Some(match code {
        17 | 3 => 2856.0,
        24 => 3200.0,
        15 => 3450.0,
        14 => 4150.0,
        2 => 4200.0,
        18 => 4874.0,
        13 => 5000.0,
        23 => 5003.0,
        1 | 4 | 9 => 5500.0,
        20 => 5503.0,
        12 => 6430.0,
        10 | 21 => 6504.0,
        19 => 6774.0,
        11 | 22 => 7504.0,
        _ => return None,
    })
}

fn rows3(m: &[[f32; 3]; 4]) -> [[f64; 3]; 3] {
    [0, 1, 2].map(|r| m[r].map(|v| v as f64))
}

fn is_zero(m: &[[f64; 3]; 3]) -> bool {
    m.iter().flatten().all(|v| v.abs() < 1e-9)
}

fn camera_color(i: &sys::sc_info) -> CameraColor {
    let mut calibrations = Vec::new();
    for n in 0..2 {
        let m = rows3(&i.dng_colormatrix[n]);
        if let (Some(t), false) = (illuminant_temperature(i.dng_illuminant[n]), is_zero(&m)) {
            calibrations.push(Calibration { temperature_k: t, xyz_to_camera: m });
        }
    }
    if calibrations.is_empty() {
        let m = rows3(&i.cam_xyz);
        let m = if is_zero(&m) {
            // Unknown camera: treat camera RGB as sRGB primaries (XYZ → linear sRGB).
            [[3.2404542, -1.5371385, -0.4985314], [-0.9692660, 1.8760108, 0.0415560], [0.0556434, -0.2040259, 1.0572252]]
        } else {
            m
        };
        calibrations.push(Calibration { temperature_k: 6504.0, xyz_to_camera: m });
    }
    let pick = |m: [f32; 4]| (m[0] > 0.0 && m[1] > 0.0 && m[2] > 0.0).then(|| [m[0] as f64 / m[1] as f64, 1.0, m[2] as f64 / m[1] as f64]);
    let as_shot_mul = pick(i.cam_mul).or_else(|| pick(i.pre_mul)).unwrap_or([2.0, 1.0, 1.5]);
    CameraColor { as_shot_mul, calibrations }
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
    if max >= 32768 { declared.min((max as u32 * 98 / 100) as u16) } else { declared }
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
    let out = std::process::Command::new("exiftool").arg("-j").args(lens::TAGS).args(af::TAGS).arg(path).output();
    let Some(tags) = out.ok().and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok()).and_then(|v| v.get(0).cloned()) else {
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
        let c = CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| RawError("path contains NUL".into()))?;
        let ptr = unsafe { sys::sc_new() };
        let mut raw = RawFile { ptr, info: unsafe { std::mem::zeroed() } };
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
        let mut img = sys::sc_image { width: 0, height: 0, data: std::ptr::null_mut(), handle: std::ptr::null_mut() };
        check(unsafe { sys::sc_develop_linear(self.ptr, half_size as i32, 11, &mut img) })?;
        let n = (img.width * img.height * 3) as usize;
        let rgb = unsafe { std::slice::from_raw_parts(img.data, n) }.to_vec();
        unsafe { sys::sc_image_free(&mut img) };
        Ok(LinearImage { width: img.width as u32, height: img.height as u32, rgb })
    }

    /// The raw mosaic for GPU processing, or an error for files without a single-channel CFA.
    pub fn mosaic(&self) -> Result<Mosaic, RawError> {
        let mut m: sys::sc_mosaic = unsafe { std::mem::zeroed() };
        check(unsafe { sys::sc_get_mosaic(self.ptr, &mut m) })?;
        let n = (m.width as usize) * (m.height as usize);
        let data = unsafe { std::slice::from_raw_parts(m.data, n) }.to_vec();
        unsafe { sys::sc_mosaic_free(&mut m) };
        Ok(Mosaic { width: m.width as u32, height: m.height as u32, data, black: m.black, white: m.white, pattern: m.pattern, period: m.period as u32, flip: self.info.flip })
    }
}

impl Drop for RawFile {
    fn drop(&mut self) {
        unsafe { sys::sc_free(self.ptr) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturated_samples_are_flagged_in_alpha() {
        let img = LinearImage { width: 2, height: 1, rgb: vec![65535, 0, 0, 32768, 32768, 32768] };
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
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        let mut checked = 0;
        for path in entries.flatten().map(|e| e.path()).filter(|p| sidecar_template::is_raw(p)) {
            // Something LibRaw can't open at all is not what this compares.
            let Ok(full) = RawFile::open(&path) else { continue };
            let full = serde_json::to_value(full.info()).unwrap();
            let quick = serde_json::to_value(RawFile::metadata(&path).unwrap()).unwrap();
            assert_eq!(full, quick, "{}", path.display());
            checked += 1;
        }
        eprintln!("compared {checked} RAW files");
    }
}
