// SPDX-License-Identifier: Apache-2.0
//! Browser entry point for the colour maths. No bindings generator: the page allocates a buffer, writes
//! JSON `{ edit, camera, display }`, and reads back a packed result. See web/src/lib/color/pack.ts.
//!
//! Result layout: `[u32 len][272 uniform bytes][4096 × f32 tone LUT][f64 temperature][f64 tint]`, or len 0
//! with the error message available from `sc_last_error`.

use serde::Deserialize;
use sidecar_color::camera::CameraColor;
use sidecar_color::{DisplaySpace, EditParams, pack};
use std::cell::RefCell;

#[derive(Deserialize)]
struct Request {
    edit: EditParams,
    camera: CameraColor,
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

fn run(input: &[u8]) -> Result<Vec<u8>, String> {
    let req: Request = serde_json::from_slice(input).map_err(|e| e.to_string())?;
    let p = pack(&req.edit, &req.camera, req.display);
    let mut out = Vec::with_capacity(4 + p.uniform.len() + p.lut.len() * 4 + 16);
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&p.uniform);
    for v in &p.lut {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&p.temperature_k.to_le_bytes());
    out.extend_from_slice(&p.tint.to_le_bytes());
    let len = (out.len() - 4) as u32;
    out[..4].copy_from_slice(&len.to_le_bytes());
    Ok(out)
}

/// # Safety
/// `ptr` must point to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sc_pack(ptr: *const u8, len: usize) -> *const u8 {
    let input = unsafe { std::slice::from_raw_parts(ptr, len) };
    match run(input) {
        Ok(out) => RESULT.with(|r| {
            *r.borrow_mut() = out;
            r.borrow().as_ptr()
        }),
        Err(msg) => {
            ERROR.with(|e| {
                let mut e = e.borrow_mut();
                e.clear();
                e.extend_from_slice(&(msg.len() as u32).to_le_bytes());
                e.extend_from_slice(msg.as_bytes());
            });
            RESULT.with(|r| {
                *r.borrow_mut() = vec![0; 4];
                r.borrow().as_ptr()
            })
        }
    }
}

/// Default edit parameters as length-prefixed JSON, so the web app never duplicates them.
#[unsafe(no_mangle)]
pub extern "C" fn sc_default_edit() -> *const u8 {
    let json = serde_json::to_vec(&EditParams::default()).unwrap();
    RESULT.with(|r| {
        let mut r = r.borrow_mut();
        r.clear();
        r.extend_from_slice(&(json.len() as u32).to_le_bytes());
        r.extend_from_slice(&json);
        r.as_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn sc_last_error() -> *const u8 {
    ERROR.with(|e| e.borrow().as_ptr())
}
