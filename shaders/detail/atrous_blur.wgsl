// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// One axis of the à-trous B3-spline smoothing (Starck & Murtagh): taps at ±step and ±2·step, weights 1 4 6 4 1 / 16.

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let p = vec2i(id.xy);
  let dir = select(vec2i(1, 0), vec2i(0, 1), P.axis == 1u) * i32(P.step);
  let w = array<f32, 5>(1.0, 4.0, 6.0, 4.0, 1.0);
  var acc = vec3f(0.0);
  for (var k = -2; k <= 2; k++) {
    acc += w[k + 2] * textureLoad(src, clamp_px(p + k * dir, P.size), 0).rgb;
  }
  textureStore(dst, p, vec4f(acc / 16.0, textureLoad(src, p, 0).a));
}
