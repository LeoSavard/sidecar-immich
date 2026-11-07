// SPDX-License-Identifier: Apache-2.0
//! Browser entry point for the colour maths. No bindings generator: the page allocates a buffer, writes
//! JSON `{ edit, camera, source, display }`, and reads back a packed result. See web/src/lib/color/pack.ts.
//!
//! Result layout: `[u32 len][`sc_develop_uniform_bytes` uniform bytes][4096 × f32 tone LUT][f64 temperature][f64 tint][1,568 mask
//! uniform bytes][8 × 4096 × f32 layer curves][f32 × n black and white table, n = 0 in colour]`, or len 0 with the error message available from `sc_last_error`.
//! Geometry calls take and return JSON.

use serde::Deserialize;
use sidecar_color::camera::CameraColor;
use sidecar_color::{DisplaySpace, EditParams, SourceKind};
use std::cell::RefCell;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    edit: EditParams,
    camera: CameraColor,
    #[serde(default)]
    source: SourceKind,
    #[serde(default)]
    display: DisplaySpace,
    /// A spectral colour stock's profile, as the browser downloaded it from
    /// `GET /api/film/stocks/{id}` — its header only; the response table goes straight to the GPU. Absent
    /// unless the edit names a stock and the server had the standards to measure it.
    #[serde(default)]
    slide_profile: Option<SlideHeader>,
}

/// The profile numbers the uniform needs (`slide::SlideProfile`).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SlideHeader {
    offsets: [f32; 3],
    scale: f32,
    side: u32,
}

thread_local! {
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static ERROR: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// The develop uniform's size (`DevelopParams`): the browser checks its own copy of it against this on load.
#[unsafe(no_mangle)]
pub extern "C" fn sc_develop_uniform_bytes() -> u32 {
    std::mem::size_of::<sidecar_color::params::DevelopParams>() as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn sc_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `ptr` must come from `sc_alloc(len)` and not have been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_free(ptr: *mut u8, len: usize) {
    drop(unsafe { Vec::from_raw_parts(ptr, 0, len) });
}

/// Hands `body` to the page as `[u32 len][body]`, valid until the next call.
fn framed(body: &[u8]) -> *const u8 {
    RESULT.with(|r| {
        let mut r = r.borrow_mut();
        r.clear();
        r.extend_from_slice(&(body.len() as u32).to_le_bytes());
        r.extend_from_slice(body);
        r.as_ptr()
    })
}

/// An empty result, with `msg` waiting in `sc_last_error`.
fn failed(msg: &str) -> *const u8 {
    ERROR.with(|e| {
        let mut e = e.borrow_mut();
        e.clear();
        e.extend_from_slice(&(msg.len() as u32).to_le_bytes());
        e.extend_from_slice(msg.as_bytes());
    });
    framed(&[])
}

fn respond(result: Result<Vec<u8>, String>) -> *const u8 {
    match result {
        Ok(body) => framed(&body),
        Err(msg) => failed(&msg),
    }
}

/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_pack(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<Request>(input)
            .map_err(|e| e.to_string())
            .map(|req| {
                let profile = req.slide_profile.map(|h| {
                    sidecar_color::slide::SlideProfile::header(h.offsets, h.scale, h.side)
                });
                let p = sidecar_color::params::pack_with(
                    &req.edit,
                    &req.camera,
                    req.source,
                    req.display,
                    profile.as_ref(),
                );
                let mut out = Vec::with_capacity(p.uniform.len() + p.lut.len() * 4 + 16);
                out.extend_from_slice(&p.uniform);
                p.lut
                    .iter()
                    .for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
                out.extend_from_slice(&p.temperature_k.to_le_bytes());
                out.extend_from_slice(&p.tint.to_le_bytes());
                out.extend_from_slice(bytemuck::bytes_of(&p.masks));
                out.extend_from_slice(bytemuck::cast_slice(&p.mask_curves));
                // Two tables over the colour cube follow, at most one of them non-empty: black and white's exposures,
                // and a spectral stock's layer responses — the latter built here from the sheet's sensitivities and the
                // scene's light, all public (its *response* table is not in this stream; the browser already has it).
                // Each is preceded by its length in floats, so the reader never has to guess from the uniform or from
                // the sizes.
                out.extend_from_slice(&(p.mono_lut.len() as u32).to_le_bytes());
                out.extend_from_slice(bytemuck::cast_slice(&p.mono_lut));
                out.extend_from_slice(&(p.slide_exposure.len() as u32).to_le_bytes());
                out.extend_from_slice(bytemuck::cast_slice(&p.slide_exposure));
                // The camera profile pass, when the edit renders with one: its uniform's length in bytes (0 for
                // none), the uniform, then its two tables, each preceded by its length in floats.
                match &p.profile {
                    Some(pp) => {
                        out.extend_from_slice(&(pp.uniform.len() as u32).to_le_bytes());
                        out.extend_from_slice(&pp.uniform);
                        for t in [&pp.hue_sat, &pp.look] {
                            out.extend_from_slice(&(t.len() as u32).to_le_bytes());
                            out.extend_from_slice(bytemuck::cast_slice(t));
                        }
                    }
                    None => out.extend_from_slice(&0u32.to_le_bytes()),
                }
                // The defringe pass's uniform, preceded by its length in bytes (0 for none).
                let defringe = p.defringe.as_deref().unwrap_or(&[]);
                out.extend_from_slice(&(defringe.len() as u32).to_le_bytes());
                out.extend_from_slice(defringe);
                out
            }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdoptRequest {
    edit: EditParams,
    /// A developed file has no baseline, shoulder or bleach to take up (`adopt_current_rendering`).
    #[serde(default)]
    developed: bool,
}

/// `{ edit, developed }` → `{ edit, changed }`: the edit rendering by today's rules, and whether that is
/// any different from how it renders now. The rule lives in `sidecar_color` so the browser and the server
/// cannot drift on what "today's rendering" means.
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_adopt_rendering(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<AdoptRequest>(input)
            .map_err(|e| e.to_string())
            .and_then(|req| {
                let mut edit = req.edit;
                let changed = edit.behind_current_rendering(req.developed);
                edit.adopt_current_rendering(req.developed);
                serde_json::to_vec(&serde_json::json!({ "edit": edit, "changed": changed }))
                    .map_err(|e| e.to_string())
            }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeometryRequest {
    geometry: sidecar_color::geometry::Geometry,
    /// The picture's size in pixels (its proportions are what matter).
    size: [u32; 2],
    /// For `sc_geometry_fit`: the geometry before the change.
    #[serde(default)]
    before: Option<sidecar_color::geometry::Geometry>,
}

/// `{ geometry, size }` → `geometry::Frame` as JSON.
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_geometry(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<GeometryRequest>(input)
            .map_err(|e| e.to_string())
            .and_then(|req| {
                serde_json::to_vec(&sidecar_color::geometry::frame(&req.geometry, req.size))
                    .map_err(|e| e.to_string())
            }),
    )
}

/// `{ geometry, before, size }` → the geometry with its crop fitted after the change (`geometry::fit`), as JSON.
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_geometry_fit(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<GeometryRequest>(input)
            .map_err(|e| e.to_string())
            .and_then(|req| {
                let a = req.size[0] as f64 / req.size[1].max(1) as f64;
                let before = req.before.unwrap_or_default();
                serde_json::to_vec(&sidecar_color::geometry::fit(&req.geometry, &before, a))
                    .map_err(|e| e.to_string())
            }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UprightRequest {
    geometry: sidecar_color::geometry::Geometry,
    /// The picture's size in pixels (its proportions are what matter).
    size: [u32; 2],
    /// The picture's luminance, row-major, at `lum_size` (any scale).
    lum: Vec<f32>,
    lum_size: [u32; 2],
    mode: sidecar_color::upright::Mode,
    /// The lens's focal length in picture diagonals, when the photo says it: for Aspect.
    #[serde(default)]
    focal: Option<f64>,
}

/// `{ geometry, size, lum, lumSize, mode, focal }` → the geometry stood up (`sidecar_color::upright::solve`), as
/// JSON; an error when the picture hasn't the lines the mode needs.
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_upright(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<UprightRequest>(input)
            .map_err(|e| e.to_string())
            .and_then(|req| {
                use sidecar_color::upright;
                let [lw, lh] = req.lum_size.map(|v| v as usize);
                if req.lum.len() != lw * lh {
                    return Err("lum is not lumSize".into());
                }
                let a = req.size[0] as f64 / req.size[1].max(1) as f64;
                let segs = upright::segments(&req.lum, lw, lh);
                let g = upright::solve(&req.geometry, a, &segs, req.mode, req.focal)
                    .ok_or("not enough straight lines in this photo")?;
                serde_json::to_vec(&g).map_err(|e| e.to_string())
            }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LensRequest {
    profile: sidecar_color::lens::LensProfile,
    lens: sidecar_color::params::Lens,
    /// Width ÷ height before orientation.
    sensor_aspect: f64,
    /// The proxy's size, and the orientation it already has.
    frame: [u32; 2],
    flip: i32,
}

/// Lens-pass uniforms for the proxy: `[u32 len][1,072 bytes]`, len 0 when nothing is to be corrected (no
/// profile curve survives the strengths), or len 0 with an error from `sc_last_error`.
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_pack_lens(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<LensRequest>(input)
            .map_err(|e| e.to_string())
            .map(|req| {
                let e = sidecar_color::lens::effective(&req.profile, &req.lens, req.sensor_aspect);
                if e.flags == 0 {
                    return Vec::new();
                }
                let p = sidecar_color::lens::pack(
                    &e,
                    req.profile.center,
                    req.frame,
                    req.flip,
                    [0.0; 2],
                    [0.0; 2],
                );
                bytemuck::bytes_of(&p).to_vec()
            }),
    )
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToneCurveRequest {
    #[serde(default)]
    curve: Vec<[f64; 2]>,
    #[serde(default)]
    highlights: f64,
    #[serde(default)]
    shadows: f64,
}

/// `{ curve, highlights, shadows }` → `4,096 × f32` LUT bytes (`sidecar_color::spline::tone_curve_lut`), the
/// same one `sc_pack` bakes into its own result. `curve: []` with the current Highlights/Shadows is the
/// "ghost" line the curve editor draws for what the sliders alone would do — see
/// `web/src/lib/ui/CurveEditor.svelte` and docs/ROADMAP.md.
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_tone_curve_lut(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<ToneCurveRequest>(input)
            .map_err(|e| e.to_string())
            .map(|req| {
                let lut = sidecar_color::spline::tone_curve_lut(
                    &req.curve,
                    req.highlights,
                    req.shadows,
                    sidecar_color::spline::LUT_SIZE,
                );
                let mut out = Vec::with_capacity(lut.len() * 4);
                lut.iter()
                    .for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
                out
            }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NeutralRequest {
    camera: CameraColor,
    /// The camera RGB of the patch to make grey.
    neutral: [f64; 3],
}

/// `{ camera, neutral }` → `{ temperatureK, tint }` under which that patch is grey: the white balance
/// eyedropper (`CameraColor::neutral_xy`).
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_white_from_neutral(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<NeutralRequest>(input)
            .map_err(|e| e.to_string())
            .and_then(|req| {
                if !req.neutral.iter().all(|v| v.is_finite() && *v > 0.0) {
                    return Err("the patch has a channel at zero".into());
                }
                let (t, tint) =
                    sidecar_color::cct::xy_to_temp_tint(req.camera.neutral_xy(req.neutral));
                serde_json::to_vec(&serde_json::json!({ "temperatureK": t, "tint": tint }))
                    .map_err(|e| e.to_string())
            }),
    )
}

/// `"srgb"`, `"display-p3"` or `"adobe-rgb"` → the `CameraColor` a developed photo in that space renders through
/// (`SourceSpace::camera_color`): what the worker puts in a JPEG's metadata, for a photo the browser decoded
/// itself and the server has never seen.
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_source_color(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<sidecar_color::camera::SourceSpace>(input)
            .map_err(|e| e.to_string())
            .and_then(|space| serde_json::to_vec(&space.camera_color()).map_err(|e| e.to_string())),
    )
}

/// `LibrawColor` (what the browser's LibRaw build read from a RAW) → the `CameraColor` the server would give the
/// same file: one mapping for both (`LibrawColor::camera_color`).
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_camera_color(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<sidecar_color::camera::LibrawColor>(input)
            .map_err(|e| e.to_string())
            .and_then(|c| serde_json::to_vec(&c.camera_color()).map_err(|e| e.to_string())),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DepthRequest {
    edit: EditParams,
    /// The photo's airlight, camera RGB (from the mask set).
    airlight: [f32; 3],
    /// The texture's size: the whole proxy, which is the frame the depth map covers.
    frame: [u32; 2],
}

/// `{ edit, airlight, frame }` → the depth passes' uniform (`sidecar_color::depth::pack`, 64 bytes), or nothing
/// when the edit neither blurs nor hazes by distance.
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_pack_depth(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<DepthRequest>(input)
            .map_err(|e| e.to_string())
            .map(|req| {
                sidecar_color::depth::pack(&req.edit, req.airlight, req.frame, [0.0; 2])
                    .map(|p| bytemuck::bytes_of(&p).to_vec())
                    .unwrap_or_default()
            }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LookRequest {
    lut: sidecar_color::lut::LookLut,
    display: DisplaySpace,
    header: sidecar_color::lut::CubeHeader,
}

/// `{ lut, display, header }` → the LUT pass's uniform (`LookLut::uniform`, 128 bytes), or no bytes when the
/// amount leaves nothing to do.
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_pack_look(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<LookRequest>(input)
            .map_err(|e| e.to_string())
            .map(|req| {
                req.lut
                    .uniform(req.display, &req.header)
                    .unwrap_or_default()
            }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProofRequest {
    display: DisplaySpace,
    header: sidecar_color::lut::CubeHeader,
    warn: bool,
}

/// `{ display, header, warn }` → a soft proof's uniform for the LUT pass (`lut::proof_uniform`, 128 bytes).
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_pack_proof(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<ProofRequest>(input)
            .map_err(|e| e.to_string())
            .map(|req| sidecar_color::lut::proof_uniform(req.display, &req.header, req.warn)),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EdgeRequest {
    edge: sidecar_color::params::MaskEdge,
    size: [u32; 2],
}

/// `{ edge, size }` → the edge passes' plan as JSON (`MaskEdge::plan`), `null` when the edge is too narrow to show.
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_mask_edge(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<EdgeRequest>(input)
            .map_err(|e| e.to_string())
            .and_then(|req| {
                serde_json::to_vec(&req.edge.plan(req.size)).map_err(|e| e.to_string())
            }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GradientRequest {
    gradient: sidecar_color::params::Gradient,
    size: [u32; 2],
}

/// `{ gradient, size }` → the gradient pass's uniform (`GradientParams`, 48 bytes) for a mask `size` pixels big
/// in the photo's shown frame, which is the proxy's.
///
/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_gradient(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    respond(
        serde_json::from_slice::<GradientRequest>(input)
            .map_err(|e| e.to_string())
            .map(|req| {
                bytemuck::bytes_of(&req.gradient.uniform(req.size, req.size, |p| p)).to_vec()
            }),
    )
}

/// Default edit parameters as length-prefixed JSON, so the web app never duplicates them.
#[unsafe(no_mangle)]
pub extern "C" fn sc_default_edit() -> *const u8 {
    framed(&serde_json::to_vec(&EditParams::default()).expect("edit params serialize"))
}

#[unsafe(no_mangle)]
pub extern "C" fn sc_last_error() -> *const u8 {
    ERROR.with(|e| e.borrow().as_ptr())
}

#[cfg(test)]
mod tests {
    /// The browser sends camelCase. A request struct without the rename reads a two-word field as absent, and
    /// without `deny_unknown_fields` that is a quiet default rather than an error, so the feature does nothing
    /// with no message anywhere. Every struct the browser's JSON lands in carries both.
    #[test]
    fn every_request_struct_is_camel_case_and_strict() {
        let src = include_str!("lib.rs");
        let lines: Vec<&str> = src.lines().collect();
        let mut seen = 0;
        for (i, line) in lines.iter().enumerate() {
            if line.starts_with("#[derive(") && line.contains("Deserialize") {
                seen += 1;
                assert_eq!(
                    lines[i + 1],
                    r#"#[serde(rename_all = "camelCase", deny_unknown_fields)]"#,
                    "the struct after line {} needs camelCase and deny_unknown_fields",
                    i + 1
                );
            }
        }
        assert!(
            seen >= 9,
            "found {seen} request structs; has the source layout changed?"
        );
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SelectRequest {
    range: sidecar_color::color_range::ColorRange,
    width: u32,
    height: u32,
    #[serde(default)]
    mask_width: u32,
    #[serde(default)]
    mask_height: u32,
}

/// A colour-range layer's mask (`color_range::select`), one byte a pixel at `width` × `height`: the request as
/// JSON, the develop uniform the edit packs to (`DevelopParams`), the proxy's camera RGB as little-endian f32 RGBA,
/// and the `within` model mask's bytes (`mask_width` × `mask_height`), or none.
///
/// # Safety
/// Each pointer must point to its length's readable bytes.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn sc_color_select(
    req: *const u8,
    req_len: usize,
    uniform: *const u8,
    uniform_len: usize,
    pixels: *const u8,
    pixels_len: usize,
    mask: *const u8,
    mask_len: usize,
) -> *const u8 {
    let req = unsafe { std::slice::from_raw_parts(req, req_len) };
    let uniform = unsafe { std::slice::from_raw_parts(uniform, uniform_len) };
    let pixels = unsafe { std::slice::from_raw_parts(pixels, pixels_len) };
    let mask = (mask_len > 0).then(|| unsafe { std::slice::from_raw_parts(mask, mask_len) });
    respond(
        serde_json::from_slice::<SelectRequest>(req)
            .map_err(|e| e.to_string())
            .and_then(|r| {
                if uniform.len() != std::mem::size_of::<sidecar_color::DevelopParams>() {
                    return Err("the develop uniform is the wrong size".into());
                }
                if pixels.len() != (r.width * r.height * 16) as usize {
                    return Err("the pixels are not width × height RGBA floats".into());
                }
                let develop: sidecar_color::DevelopParams = bytemuck::pod_read_unaligned(uniform);
                let rgba: Vec<f32> = pixels
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes(*b))
                    .collect();
                let within = mask
                    .filter(|m| m.len() == (r.mask_width * r.mask_height) as usize && !m.is_empty())
                    .map(|m| (m, r.mask_width, r.mask_height));
                Ok(sidecar_color::color_range::select(
                    &r.range, &develop, &rgba, r.width, r.height, within,
                ))
            }),
    )
}
