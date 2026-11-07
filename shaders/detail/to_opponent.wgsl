// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// Linear RGB → variance-stabilized opponent channels: luminance through a square-root transform, so photon
// noise becomes signal-independent, and the chroma differences scaled by the same derivative. Alpha passes
// through. Two transforms (`P.c`):
//   0  the fixed one, y = 2·sqrt(luminance + ε) — Poisson noise with one assumed read-noise floor, which is
//      what an edit saved before PROCESS_VERSION 8 was denoised with;
//   1  the generalized Anscombe transform for this photo's measured noise, σ² = a·y + b
//      (`sidecar_engine::noise`; a in P.a, b in P.b): unit-variance noise at every level, shadows to highlights.

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let c = textureLoad(src, vec2i(id.xy), 0);
  let lum = max((c.r + c.g + c.b) / 3.0, 0.0);
  if (P.c == 1.0) {
    let a = max(P.a, 1e-12);
    let s = sqrt(max(lum / a + 0.375 + P.b / (a * a), 0.0));
    textureStore(dst, vec2i(id.xy), vec4f(2.0 * s, (c.r - c.g) / (a * s), (c.b - c.g) / (a * s), c.a));
    return;
  }
  let s = sqrt(lum + STAB_EPS);
  textureStore(dst, vec2i(id.xy), vec4f(2.0 * s, (c.r - c.g) / s, (c.b - c.g) / s, c.a));
}
