// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Shared uniforms for the server ingest passes. Mirrored by IngestParams in crates/engine/src/ingest.rs.

struct IngestParams {
  in_size: vec2u,             // size of the texture being read
  out_size: vec2u,            // size of the texture being written
  black: vec4f,               // raw black level per CFA colour index (R, G, B, G2)
  wb: vec4f,                  // as-shot white balance multipliers, green = 1
  white: f32,                 // raw saturation level
  clip: f32,                  // normalized clipping threshold
  desat: f32,                 // highlight desaturation
  soft: f32,                  // highlight reconstruction softness
  sensor: u32,                // 0 = Bayer cropped to RGGB, 1 = X-Trans cropped to the vkdt block phase
  _p0: u32,
  _p1: u32,
  _p2: u32,
  pattern: array<vec4u, 9>,   // 6×6 CFA colour indices, row-major
}

fn colour_at(p: vec2u, pattern: array<vec4u, 9>) -> u32 {
  let i = (p.y % 6u) * 6u + (p.x % 6u);
  return pattern[i / 4u][i % 4u];
}

fn clamp_px(p: vec2i, size: vec2u) -> vec2i {
  return clamp(p, vec2i(0), vec2i(size) - vec2i(1));
}

fn luminance_rec2020(c: vec3f) -> f32 {
  return dot(vec3f(0.2627002120, 0.6779980720, 0.0593017165), c);
}
