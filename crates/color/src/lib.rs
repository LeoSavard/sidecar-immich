// SPDX-License-Identifier: Apache-2.0
//! Colour maths shared by the worker and the browser (via wasm32). Everything the develop shaders need that
//! is cheaper to compute once per edit than once per pixel lives here.

pub mod cct;
pub mod geometry;
pub mod lens;
pub mod mat;
pub mod camera;
pub mod params;
pub mod spline;
pub mod tone;

pub use params::{DevelopParams, DisplaySpace, EditParams, Packed, SourceKind, pack};
