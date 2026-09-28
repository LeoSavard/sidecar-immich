// SPDX-License-Identifier: Apache-2.0
//! Browser entry point for the colour maths. No bindings generator: the page allocates a buffer, writes
//! JSON `{ edit, camera, source, display }`, and reads back a packed result. See web/src/lib/color/pack.ts.
//!
//! Result layout: `[u32 len][272 uniform bytes][4096 × f32 tone LUT][f64 temperature][f64 tint]`, or len 0
//! with the error message available from `sc_last_error`.

use serde::Deserialize;
use sidecar_color::camera::CameraColor;
use sidecar_color::{DisplaySpace, EditParams, SourceKind, pack};
use std::cell::RefCell;

#[derive(Deserialize)]
struct Request {
    edit: EditParams,
    camera: CameraColor,
    #[serde(default)]
    source: SourceKind,
    #[serde(default)]
    display: DisplaySpace,
}

thread_local! {
    static RESULT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static ERROR: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
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
    respond(serde_json::from_slice::<Request>(input).map_err(|e| e.to_string()).map(|req| {
        let p = pack(&req.edit, &req.camera, req.source, req.display);
        let mut out = Vec::with_capacity(p.uniform.len() + p.lut.len() * 4 + 16);
        out.extend_from_slice(&p.uniform);
        p.lut.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
        out.extend_from_slice(&p.temperature_k.to_le_bytes());
        out.extend_from_slice(&p.tint.to_le_bytes());
        out
    }))
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
    respond(serde_json::from_slice::<LensRequest>(input).map_err(|e| e.to_string()).map(|req| {
        let e = sidecar_color::lens::effective(&req.profile, &req.lens, req.sensor_aspect);
        if e.flags == 0 {
            return Vec::new();
        }
        let p = sidecar_color::lens::pack(&e, req.profile.center, req.frame, req.flip, [0.0; 2], [0.0; 2]);
        bytemuck::bytes_of(&p).to_vec()
    }))
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
