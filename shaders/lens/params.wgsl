// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Uniform block for the lens pass (1,072 bytes). Mirrored by hand in crates/color/src/lens.rs, checked by naga.

struct LensParams {
  center: vec2f,             // optical centre, frame pixels
  out_offset: vec2f,         // frame pixel of the output texture's top-left texel
  in_offset: vec2f,          // frame pixel of the input texture's top-left texel
  half_diag: f32,            // the frame's half-diagonal in pixels: radius 1
  zoom: f32,                 // corrected-frame scale about the optical centre that keeps blank edges out
  r_scale: f32,              // knots per unit radius
  flags: u32,                // bit 0 distortion, bit 1 lateral CA, bit 2 vignetting
  _pad: vec2f,
  knots: array<vec4f, 64>,   // per radius: distortion, red CA, blue CA, vignetting gain
}
