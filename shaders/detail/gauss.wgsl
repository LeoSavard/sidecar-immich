// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// One axis of a Gaussian blur of the r channel (a = sigma, step = radius). Used by Richardson–Lucy.

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<r32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let p = vec2i(id.xy);
  let dir = select(vec2i(1, 0), vec2i(0, 1), P.axis == 1u);
  let r = i32(P.step);
  var acc = 0.0;
  var wsum = 0.0;
  for (var k = -r; k <= r; k++) {
    let w = exp(-f32(k * k) / (2.0 * P.a * P.a));
    acc += w * textureLoad(src, clamp_px(p + k * dir, P.size), 0).r;
    wsum += w;
  }
  textureStore(dst, p, vec4f(acc / wsum, 0.0, 0.0, 0.0));
}
