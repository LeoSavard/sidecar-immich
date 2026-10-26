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
  _unused: f32,               // was vkdt's `soft`; the fill no longer blends measured pixels
  sensor: u32,               // 0 = Bayer cropped to RGGB, 1 = X-Trans cropped to the vkdt block phase
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

// Highlight reconstruction never draws a value more than this many times the clip level, four stops past it,
// whatever the estimate's ratios say. Unbounded, a coarse estimate with a channel near zero multiplies into
// the thousands and overflows (D14 in docs/research/pipeline-review-2026-10.md). Measured against ground
// truth, the result doesn't move from 16 up and gets worse below 4 (`sidecar-dev hilite-truth`).
const HILITE_HEADROOM = 16.0;

// How far to scale a reconstruction estimate `u` (one value per channel) to fit what a pixel recorded. `sum`
// and `n` are the pixel's measured (unclipped) samples per channel, added up and counted; `top` is the largest
// sample of each channel, clipped or not. With measurements, the ratio of sums: each sample's ratio to the
// estimate, averaged with the estimate as weight, so a channel the estimate barely has carries little weight
// and can't blow the scale up. With none, the least scale that leaves no channel below what it recorded, as a
// clipped sample is a lower bound on the light that reached it. 0 when there is no estimate to scale.
fn hilite_scale(u: vec3f, sum: vec3f, n: vec3f, top: vec3f) -> f32 {
  let umax = max(u.r, max(u.g, u.b));
  if (!(umax > 1e-6)) { return 0.0; }
  let den = dot(n, u);
  if (den > 1e-6 * umax) { return (sum.r + sum.g + sum.b) / den; }
  let r = top / max(u, vec3f(umax / HILITE_HEADROOM));
  return max(r.r, max(r.g, r.b));
}

// The estimate `u` scaled by `s`, per channel, within the headroom. Each clipped sample then takes the larger
// of this and its own value.
fn hilite_estimate(u: vec3f, s: f32, clip: f32) -> vec3f {
  return min(u * s, vec3f(HILITE_HEADROOM * clip));
}
