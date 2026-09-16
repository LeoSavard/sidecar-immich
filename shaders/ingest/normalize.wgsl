// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: ingest/common.wgsl
// Black level subtraction and scaling to [0, 1] at sensor saturation, with hot-pixel suppression on Bayer
// sensors: a sample far above all four same-colour neighbours two pixels away is replaced by their mean.

@group(0) @binding(0) var<uniform> P: IngestParams;
@group(0) @binding(1) var raw: texture_2d<u32>;
@group(0) @binding(2) var dst: texture_storage_2d<r32float, write>;

fn norm(p: vec2i) -> f32 {
  let q = clamp_px(p, P.in_size);
  let c = colour_at(vec2u(q), P.pattern);
  return max((f32(textureLoad(raw, q, 0).r) - P.black[c]) / (P.white - P.black[c]), 0.0);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.out_size.x || id.y >= P.out_size.y) { return; }
  let p = vec2i(id.xy);
  var v = norm(p);
  if (P.sensor == 0u) {
    let a = norm(p + vec2i(2, 0));
    let b = norm(p - vec2i(2, 0));
    let c = norm(p + vec2i(0, 2));
    let d = norm(p - vec2i(0, 2));
    let m = max(max(a, b), max(c, d));
    if (v > 4.0 * m && v > 0.05 && v < P.clip) {
      v = 0.25 * (a + b + c + d);
    }
  }
  textureStore(dst, p, vec4f(v, 0.0, 0.0, 0.0));
}
