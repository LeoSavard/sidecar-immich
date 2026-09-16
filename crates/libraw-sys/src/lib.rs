// SPDX-License-Identifier: Apache-2.0
//! Raw FFI to shim/shim.h. See the `sidecar-raw` crate for the safe API.
#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_void};

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct sc_info {
    pub width: i32,
    pub height: i32,
    pub flip: i32,
    pub colors: i32,
    pub filters: u32,
    pub is_dng: i32,
    pub black: f32,
    pub maximum: f32,
    pub cam_mul: [f32; 4],
    pub pre_mul: [f32; 4],
    pub cam_xyz: [[f32; 3]; 4],
    pub dng_illuminant: [u16; 2],
    pub dng_colormatrix: [[[f32; 3]; 4]; 2],
    pub iso: f32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal_len: f32,
    pub timestamp: i64,
    pub make: [c_char; 64],
    pub model: [c_char; 64],
    pub lens: [c_char; 128],
}

#[repr(C)]
pub struct sc_image {
    pub width: i32,
    pub height: i32,
    pub data: *mut u16,
    pub handle: *mut c_void,
}

#[repr(C)]
pub struct sc_mosaic {
    pub width: i32,
    pub height: i32,
    pub data: *mut u16,
    pub black: [f32; 4],
    pub white: f32,
    pub pattern: [[u8; 6]; 6],
    pub period: i32,
}

unsafe extern "C" {
    pub fn sc_get_mosaic(raw: *mut c_void, out: *mut sc_mosaic) -> c_int;
    pub fn sc_mosaic_free(m: *mut sc_mosaic);
    pub fn sc_new() -> *mut c_void;
    pub fn sc_free(raw: *mut c_void);
    pub fn sc_open(raw: *mut c_void, path: *const c_char) -> c_int;
    pub fn sc_unpack(raw: *mut c_void) -> c_int;
    pub fn sc_get_info(raw: *mut c_void, info: *mut sc_info);
    pub fn sc_develop_linear(raw: *mut c_void, half_size: c_int, quality: c_int, out: *mut sc_image) -> c_int;
    pub fn sc_image_free(img: *mut sc_image);
    pub fn sc_strerror(code: c_int) -> *const c_char;
}
