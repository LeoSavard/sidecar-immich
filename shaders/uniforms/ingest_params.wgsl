// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_engine::ingest::IngestParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct IngestParams {
  // Size of the texture being read.
  in_size: vec2u,
  // Size of the texture being written.
  out_size: vec2u,
  // Raw black level per CFA colour index (R, G, B, G2).
  black: vec4f,
  // As-shot white balance multipliers, green = 1.
  wb: vec4f,
  // Raw saturation level.
  white: f32,
  // Normalized clipping threshold.
  clip: f32,
  // Highlight desaturation.
  desat: f32,
  // Was vkdt's `soft`; the fill no longer blends measured pixels.
  _unused: f32,
  // 0 = Bayer cropped to RGGB, 1 = X-Trans cropped to the vkdt block phase.
  sensor: u32,
  _p0: u32,
  _p1: u32,
  _p2: u32,
  // 6×6 CFA colour indices, row-major.
  pattern: array<vec4u, 9>,
}
