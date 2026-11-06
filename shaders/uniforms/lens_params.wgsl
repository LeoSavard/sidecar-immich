// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_color::lens::LensParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct LensParams {
  // Optical centre, in frame pixels.
  center: vec2f,
  // Frame pixel of the output texture's top-left texel.
  out_offset: vec2f,
  // Frame pixel of the input texture's top-left texel.
  in_offset: vec2f,
  // The frame's half-diagonal in pixels: radius 1.
  half_diag: f32,
  // Corrected-frame scale about the optical centre that keeps blank edges out.
  zoom: f32,
  // Knots per unit radius.
  r_scale: f32,
  // Bit 0 distortion, bit 1 lateral CA, bit 2 vignetting.
  flags: u32,
  _pad0: f32,
  _pad1: f32,
  // Per radius: distortion, red CA, blue CA, vignetting gain.
  knots: array<vec4f, 64>,
}
