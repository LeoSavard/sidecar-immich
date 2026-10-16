// SPDX-License-Identifier: Apache-2.0
//! Full-resolution render for export: decode → GPU demosaic (Bayer) or LibRaw Markesteijn (X-Trans) → develop
//! (shared WGSL) → orientation, or the edit's crop and geometry with it → optional resample → 8-bit JPEG with dither or 16-bit TIFF → ICC profile →
//! metadata copied from the RAW by ExifTool → `sc:Sidecar` in the export's XMP.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sidecar_color::camera::CameraColor;
use sidecar_color::lens::{LensParams, LensProfile};
use sidecar_color::{DisplaySpace, EditParams, Packed, SourceKind};
use sidecar_engine::{MosaicInput, RgbaF32};
use sidecar_raw::RawFile;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc::UnboundedSender as Sender;

const SRGB_ICC: &[u8] = include_bytes!("../../../assets/icc/sRGB-v4.icc");
const P3_ICC: &[u8] = include_bytes!("../../../assets/icc/DisplayP3-v4.icc");

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Jpeg,
    Tiff,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Tiff => "tif",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderRequest {
    pub job_id: String,
    pub path: PathBuf,
    pub file_name: String,
    pub edit: EditParams,
    pub format: Format,
    pub color_space: DisplaySpace,
    pub quality: u8,
    pub long_edge: Option<u32>,
    /// Written into the export's XMP.
    pub sidecar: serde_json::Value,
    /// The asset key, for the masks the edit names.
    #[serde(default)]
    pub key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "camelCase")]
pub enum Progress {
    #[serde(rename_all = "camelCase")]
    Stage { stage: String, overall: f32 },
    #[serde(rename_all = "camelCase")]
    Done { file: String, sha1: String, bytes: u64, width: u32, height: u32 },
    #[serde(rename_all = "camelCase")]
    Failed { message: String },
}

pub struct Kit {
    pub gpu: sidecar_engine::Gpu,
    pub ingest: sidecar_engine::Ingest,
    pub develop: sidecar_engine::DevelopPass,
    pub detail: sidecar_engine::detail::Detail,
    pub lens: sidecar_engine::lens::LensPass,
    pub warp: sidecar_engine::warp::WarpPass,
    pub brush: sidecar_engine::brush::Brush,
    pub depth: sidecar_engine::depth::DepthPass,
    pub lensfun: std::sync::Arc<crate::lensfun::Lensfun>,
}

pub fn detail_settings(edit: &EditParams) -> sidecar_engine::detail::DetailSettings {
    let d = &edit.detail;
    sidecar_engine::detail::DetailSettings {
        denoise_luminance: d.denoise_luminance as f32,
        denoise_chroma: d.denoise_chroma as f32,
        sharpen_amount: d.sharpen_amount as f32,
        sharpen_radius: d.sharpen_radius as f32,
        sharpen_iterations: d.sharpen_iterations,
    }
}

/// How a photo's pixels are developed: the colour model, whether they are sensor light or a picture
/// something else already rendered (`SourceKind`, which picks the develop pass's tone path), and the lens
/// correction the file carries.
#[derive(Clone, Debug)]
pub struct Rendering {
    pub color: CameraColor,
    pub kind: SourceKind,
    pub lens: Option<LensProfile>,
    /// Width ÷ height before orientation: where the frame's edges are for the lens pass's zoom.
    pub sensor_aspect: f64,
    /// The orientation the texture being corrected already has, relative to the sensor: the RAW path's
    /// full-resolution texture is unoriented (0), a proxy or LibRaw's X-Trans decode is oriented.
    pub lens_flip: i32,
}

impl Rendering {
    /// For a texture oriented as the file says — a proxy.
    pub fn of(info: &sidecar_raw::RawInfo) -> Self {
        let [w, h] = info.sensor_size();
        Self {
            color: info.color.clone(),
            kind: info.source_kind,
            lens: info.lens_profile.clone(),
            sensor_aspect: if h > 0 { w as f64 / h as f64 } else { 1.5 },
            lens_flip: info.orientation_flip,
        }
    }

    /// A picture something else rendered: no sensor data and nothing about its lens left to correct.
    pub fn developed(color: CameraColor) -> Self {
        Self { color, kind: SourceKind::Developed, lens: None, sensor_aspect: 1.5, lens_flip: 0 }
    }

    pub fn pack(&self, edit: &EditParams, display: DisplaySpace) -> Packed {
        sidecar_color::pack(edit, &self.color, self.kind, display)
    }

    /// The lens pass's uniforms for this edit over a texture holding the whole frame at `frame` pixels, with
    /// the output and input textures placed at `out_offset` and `in_offset` in it. `None` when there is no
    /// profile, the edit leaves lens correction off, or every strength is zero.
    pub fn lens_params(&self, edit: &EditParams, frame: [u32; 2], out_offset: [f64; 2], in_offset: [f64; 2]) -> Option<LensParams> {
        let (profile, lens) = (self.lens.as_ref()?, edit.lens.as_ref()?);
        let e = sidecar_color::lens::effective(profile, lens, self.sensor_aspect);
        (e.flags != 0).then(|| sidecar_color::lens::pack(&e, profile.center, frame, self.lens_flip, out_offset, in_offset))
    }

    /// Correct a whole-frame texture, if the edit asks for it.
    pub fn correct(&self, kit: &Kit, edit: &EditParams, source: wgpu::Texture) -> wgpu::Texture {
        let frame = [source.width(), source.height()];
        match self.lens_params(edit, frame, [0.0; 2], [0.0; 2]) {
            Some(p) => kit.lens.apply(&kit.gpu, &source, &p, frame),
            None => source,
        }
    }

    /// The edit's masks as textures covering the same frame as a source of `frame` pixels this rendering
    /// corrects. A model's mask comes in the proxy's frame, oriented as the photo is shown, and brush strokes
    /// are placed in that frame too; `flip` is the orientation the source still has to have applied, which
    /// they are taken back through. The lens correction then moves them as it moves the source — without
    /// vignetting, whose gain would change a mask's values rather than where they are.
    pub fn mask_textures(&self, kit: &Kit, edit: &EditParams, masks: &[Option<RgbaF32>], flip: i32, frame: [u32; 2]) -> Vec<Option<wgpu::Texture>> {
        let mut geometry = edit.clone();
        if let Some(lens) = &mut geometry.lens {
            lens.vignetting = 0.0;
        }
        let to_source = sidecar_color::geometry::sensor_from_oriented(flip);
        edit.masks
            .iter()
            .take(sidecar_color::params::MAX_MASKS)
            .enumerate()
            .map(|(i, layer)| {
                if !layer.renders() {
                    return None;
                }
                let base = masks.get(i).cloned().flatten().map(|m| sidecar_engine::upload_rgba16(&kit.gpu, &m.oriented(sidecar_engine::inverse_flip(flip))));
                if layer.strokes.is_empty() && layer.range.is_none() && layer.edge.is_none() {
                    return Some(self.correct(kit, &geometry, base?));
                }
                // A range, strokes or an edge: painted, the range windowing the base first, the edge last. At the source's resolution
                // up to 4,096 px on the long edge: a mask is sampled through a linear filter, and nothing a
                // brush draws needs more.
                let scale = (MASK_EDGE as f64 / frame[0].max(frame[1]) as f64).min(1.0);
                let size = frame.map(|v| ((v as f64 * scale).round() as u32).max(1));
                let strokes: Vec<_> = layer.strokes.iter().map(|s| sidecar_engine::brush::stroke_px(s, size, |p| sidecar_color::geometry::map(&to_source, p))).collect();
                let painted = kit.brush.paint(&kit.gpu, base.as_ref(), size, sidecar_color::params::MaskRange::uniform(layer.range.as_ref()), &strokes);
                let painted = match &layer.edge {
                    Some(edge) => kit.brush.edge(&kit.gpu, painted, edge),
                    None => painted,
                };
                Some(match self.lens_params(&geometry, size, [0.0; 2], [0.0; 2]) {
                    Some(p) => kit.brush.correct(&kit.gpu, &painted, &p),
                    None => painted,
                })
            })
            .collect()
    }

    /// The depth map in the frame a source of `frame` pixels this rendering corrects: taken back through the
    /// orientation `flip` still has to apply and lens-corrected as the source is (without vignetting), as a
    /// mask is.
    pub fn depth_texture(&self, kit: &Kit, edit: &EditParams, depth: &RgbaF32, flip: i32) -> wgpu::Texture {
        let mut geometry = edit.clone();
        if let Some(lens) = &mut geometry.lens {
            lens.vignetting = 0.0;
        }
        let base = sidecar_engine::upload_rgba16(&kit.gpu, &depth.clone().oriented(sidecar_engine::inverse_flip(flip)));
        self.correct(kit, &geometry, base)
    }

    /// `source` — the whole frame, `frame` pixels, or a tile of it at `offset` — through the edit's haze and lens
    /// blur, when it has either and `depth` is here. A tile needs `DepthParams::reach` of margin around what is
    /// kept of it to come out as the whole picture would.
    #[allow(clippy::too_many_arguments)]
    pub fn depth_effects(&self, kit: &Kit, edit: &EditParams, depth: Option<(&crate::masks::DepthMap, &wgpu::Texture)>, source: wgpu::Texture, frame: [u32; 2], offset: [u32; 2]) -> wgpu::Texture {
        let Some((map, texture)) = depth else { return source };
        match sidecar_color::depth::pack(edit, map.airlight, frame, [offset[0] as f32, offset[1] as f32]) {
            Some(params) => kit.depth.apply(&kit.gpu, &source, texture, &params),
            None => source,
        }
    }

    /// Crop, straighten, keystone and turn a developed texture holding the whole picture in the orientation
    /// `flip` leaves pending, into the output, oriented. `None` when the edit has no geometry: the caller
    /// orients as before.
    pub fn warp(&self, kit: &Kit, edit: &EditParams, developed: &wgpu::Texture, flip: i32) -> Option<wgpu::Texture> {
        let g = edit.geometry.as_ref().filter(|g| !g.is_identity())?;
        let picture = oriented_size([developed.width(), developed.height()], flip);
        let out = sidecar_color::geometry::output_size(g, picture);
        Some(kit.warp.apply(&kit.gpu, developed, &sidecar_color::geometry::warp_params(g, picture, flip, out, [0.0; 2], out, [0.0; 2])))
    }
}

/// Long edge brush strokes are drawn at on the worker.
const MASK_EDGE: u32 = 4096;

/// A texture's size once `flip` is applied.
pub fn oriented_size(size: [u32; 2], flip: i32) -> [u32; 2] {
    if flip & 4 != 0 { [size[1], size[0]] } else { size }
}

/// `pack`'s mask uniforms, curves, and the textures for them, ready for `DevelopPass::render`.
pub fn masks_for<'a>(packed: &'a Packed, textures: &'a [Option<wgpu::Texture>], frame: [f32; 4]) -> sidecar_engine::Masks<'a> {
    let mut params = packed.masks;
    params.frame = frame;
    sidecar_engine::Masks { params, textures: textures.iter().map(Option::as_ref).collect(), curves: &packed.mask_curves }
}

/// Full-resolution linear RGB on the GPU, the orientation still to apply, and how to develop it. RAW comes
/// from LibRaw and the demosaic; a JPEG, HEIC or TIFF was developed by whatever wrote it, so it is decoded
/// straight to display-linear light, with a colour model standing for its own primaries.
pub fn full_source_for(kit: &Kit, path: &Path) -> Result<(wgpu::Texture, i32, Rendering)> {
    if sidecar_raw::decoded::is_supported(path) {
        let exif = sidecar_raw::decoded::exif(path);
        let decoded = sidecar_raw::decoded::decode(path, exif.orientation_flip, exif.space)?;
        let img = RgbaF32 { width: decoded.width, height: decoded.height, data: decoded.rgba };
        let rendering = Rendering::developed(decoded.space.camera_color());
        return Ok((sidecar_engine::upload_rgba16(&kit.gpu, &img), decoded.pending_flip, rendering));
    }
    if sidecar_raw::linear_dng::is_supported(path) {
        // Held to the largest texture the GPU takes: a merged panorama can be several times wider.
        let info = kit.lensfun.described(path, sidecar_raw::linear_dng::info(path, &sidecar_raw::decoded::exif(path))?);
        let mut rendering = Rendering::of(&info);
        rendering.lens_flip = 0;
        let d = sidecar_raw::linear_dng::decode(path, kit.gpu.device.limits().max_texture_dimension_2d)?;
        let img = RgbaF32 { width: d.width, height: d.height, data: d.rgba };
        return Ok((sidecar_engine::upload_rgba16(&kit.gpu, &img), info.orientation_flip, rendering));
    }
    let mut raw = RawFile::open(path)?;
    let info = kit.lensfun.described(path, raw.info());
    let mut rendering = Rendering::of(&info);
    let (texture, flip) = full_source(kit, &mut raw)?;
    // `full_source` either leaves the orientation to apply (the GPU demosaic) or has applied it (LibRaw).
    rendering.lens_flip = if flip == 0 { info.orientation_flip } else { 0 };
    Ok((texture, flip, rendering))
}

/// Full-resolution camera-native linear RGB on the GPU, and the orientation still to apply.
pub fn full_source(kit: &Kit, raw: &mut RawFile) -> Result<(wgpu::Texture, i32)> {
    let info = raw.info();
    let mul = info.color.as_shot_mul;
    let wb = [mul[0] as f32, 1.0, mul[2] as f32];
    Ok(match raw.mosaic() {
        Ok(m) if !m.is_xtrans() => {
            let input = MosaicInput { width: m.width, height: m.height, data: &m.data, black: m.black, white: m.white, pattern: m.pattern, period: m.period };
            (kit.ingest.demosaic(&kit.gpu, &input, wb, true)?, m.flip)
        }
        _ => {
            // X-Trans and non-mosaic files: LibRaw on the CPU (Markesteijn 3-pass for X-Trans), orientation
            // applied. Clipping is flagged as the proxy flags it, so the two agree on what was clipped.
            let linear = raw.develop_linear(false)?;
            (sidecar_engine::upload_rgba16(&kit.gpu, &RgbaF32 { width: linear.width, height: linear.height, data: linear.rgba() }), 0)
        }
    })
}

fn stage(tx: &Sender<Progress>, name: &str, overall: f32) {
    let _ = tx.send(Progress::Stage { stage: name.into(), overall });
}

/// Triangular dither in [-1, 1) from a cheap integer hash, so 8-bit output has no banding.
fn tri_noise(i: u64) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    let a = (x & 0xFFFF) as f32 / 65536.0;
    let b = ((x >> 16) & 0xFFFF) as f32 / 65536.0;
    a - b
}

pub fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.0031308 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

pub fn icc_for(space: DisplaySpace) -> &'static [u8] {
    match space {
        DisplaySpace::Srgb => SRGB_ICC,
        DisplaySpace::DisplayP3 => P3_ICC,
    }
}

/// Display-linear RGBA → 8-bit sRGB-encoded RGB, dithered so gradients don't band.
fn to_rgb8(img: &RgbaF32) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((img.width * img.height * 3) as usize);
    for (i, p) in img.data.as_chunks::<4>().0.iter().enumerate() {
        for (c, v) in p[..3].iter().enumerate() {
            let n = tri_noise((i as u64) * 3 + c as u64);
            rgb.push((srgb_encode(*v) * 255.0 + n + 0.5).clamp(0.0, 255.0) as u8);
        }
    }
    rgb
}

/// A JPEG in memory: the export's encoder, and the server-rendered preview that stands in for the browser's
/// develop pass where there is no WebGPU.
pub fn jpeg_bytes(img: &RgbaF32, quality: u8, space: DisplaySpace) -> Result<Vec<u8>> {
    if img.width > u16::MAX as u32 || img.height > u16::MAX as u32 {
        bail!("JPEG is limited to 65,535 pixels per side; export a TIFF or set a long edge");
    }
    let mut out = Vec::new();
    let mut enc = jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100));
    enc.set_sampling_factor(jpeg_encoder::SamplingFactor::R_4_4_4);
    enc.add_icc_profile(icc_for(space))?;
    enc.encode(&to_rgb8(img), img.width as u16, img.height as u16, jpeg_encoder::ColorType::Rgb)?;
    Ok(out)
}

fn encode(img: &RgbaF32, req: &RenderRequest, out: &Path) -> Result<()> {
    match req.format {
        Format::Jpeg => std::fs::write(out, jpeg_bytes(img, req.quality, req.color_space)?)?,
        Format::Tiff => {
            use tiff::encoder::{TiffEncoder, colortype};
            let data: Vec<u16> = img.data.as_chunks::<4>().0.iter().flat_map(|p| [0, 1, 2].map(|c| (srgb_encode(p[c]) * 65535.0 + 0.5) as u16)).collect();
            let mut t = TiffEncoder::new(std::io::BufWriter::new(std::fs::File::create(out)?))?;
            let mut image = t.new_image::<colortype::RGB16>(img.width, img.height)?;
            image.encoder().write_tag(tiff::tags::Tag::IccProfile, icc_for(req.color_space))?;
            image.write_data(&data)?;
        }
    }
    Ok(())
}

fn copy_metadata(raw: &Path, out: &Path, width: u32, height: u32) -> Result<()> {
    let status = std::process::Command::new("exiftool")
        .args(["-q", "-q", "-overwrite_original", "-TagsFromFile"])
        .arg(raw)
        .args(["-all:all", "--PreviewImage", "--ThumbnailImage", "--JpgFromRaw", "--OtherImage", "--ICC_Profile:all", "-Orientation#=1"])
        .arg(format!("-ExifImageWidth={width}"))
        .arg(format!("-ExifImageHeight={height}"))
        .arg(format!("-Software=Sidecar {}", env!("CARGO_PKG_VERSION")))
        .arg(out)
        .status()
        .context("running exiftool — is it installed?")?;
    if !status.success() {
        bail!("exiftool couldn't copy metadata ({status})");
    }
    Ok(())
}

/// `masks` are the edit's masks from the cache, in the order it names them (`Worker::mask_images`).
pub fn render(kit: &Kit, req: &RenderRequest, masks: &[Option<RgbaF32>], depth: Option<&crate::masks::DepthMap>, outbox: &Path, tx: &Sender<Progress>) -> Result<Progress> {
    stage(tx, "decode", 0.02);
    stage(tx, "demosaic", 0.15);
    let (mut source, flip, rendering) = full_source_for(kit, &req.path)?;

    let settings = detail_settings(&req.edit);
    if !settings.is_noop() {
        stage(tx, "detail", 0.35);
        let sigma = kit.detail.noise_sigma(&kit.gpu, &source)?;
        source = kit.detail.apply(&kit.gpu, source, sigma, &settings);
    }
    if req.edit.lens.is_some() {
        stage(tx, "lens", 0.42);
        source = rendering.correct(kit, &req.edit, source);
    }
    if let Some(map) = depth {
        stage(tx, "depth", 0.44);
        let frame = [source.width(), source.height()];
        let texture = rendering.depth_texture(kit, &req.edit, &map.image, flip);
        source = rendering.depth_effects(kit, &req.edit, Some((map, &texture)), source, frame, [0, 0]);
    }

    stage(tx, "develop", 0.45);
    let packed = rendering.pack(&req.edit, req.color_space);
    let mask_textures = rendering.mask_textures(kit, &req.edit, masks, flip, [source.width(), source.height()]);
    let developed = kit.develop.render(&kit.gpu, &source, &packed.uniform, &packed.lut, &masks_for(&packed, &mask_textures, [0.0; 4]))?;
    drop(source);
    drop(mask_textures);
    let mut img = match rendering.warp(kit, &req.edit, &developed, flip) {
        Some(warped) => {
            stage(tx, "geometry", 0.55);
            drop(developed);
            sidecar_engine::read_rgba16(&kit.gpu, &warped)?
        }
        None => {
            let img = sidecar_engine::read_rgba16(&kit.gpu, &developed)?;
            drop(developed);
            img.oriented(flip)
        }
    };
    if let Some(edge) = req.long_edge {
        stage(tx, "resample", 0.6);
        img = img.downsampled(edge);
    }

    stage(tx, "encode", 0.7);
    let dir = outbox.join(&req.job_id);
    std::fs::create_dir_all(&dir)?;
    let out = dir.join(&req.file_name);
    let tmp = dir.join(format!(".{}.tmp.{}", req.file_name, req.format.extension()));
    encode(&img, req, &tmp)?;

    stage(tx, "metadata", 0.88);
    copy_metadata(&req.path, &tmp, img.width, img.height)?;
    sidecar_xmp::embed_in_image(&tmp, &req.sidecar)?;
    std::fs::rename(&tmp, &out)?;
    let sha1 = sidecar_xmp::sha1_file(&out)?;
    let bytes = std::fs::metadata(&out)?.len();
    Ok(Progress::Done { file: req.file_name.clone(), sha1, bytes, width: img.width, height: img.height })
}
