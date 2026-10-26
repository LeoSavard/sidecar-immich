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
#[serde(rename_all = "camelCase")]
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
                out
            }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
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

/// `"srgb"` or `"display-p3"` → the `CameraColor` a developed photo in that space renders through
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

/// Default edit parameters as length-prefixed JSON, so the web app never duplicates them.
#[unsafe(no_mangle)]
pub extern "C" fn sc_default_edit() -> *const u8 {
    framed(&serde_json::to_vec(&EditParams::default()).expect("edit params serialize"))
}

#[unsafe(no_mangle)]
pub extern "C" fn sc_last_error() -> *const u8 {
    ERROR.with(|e| e.borrow().as_ptr())
}
