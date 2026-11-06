// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/pyramid_params.wgsl pyramid/common.wgsl
// Pyramid: one Gaussian level from the finer one — the binomial blur sampled at every other texel. A level of
// n texels gives ceil(n / 2), so odd sizes keep their last column.

@group(0) @binding(0) var<uniform> P: PyramidParams;
@group(0) @binding(1) var fine: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.coarse.x || id.y >= P.coarse.y) { return; }
  let centre = vec2i(id.xy) * 2;
  var acc = vec4f(0.0);
  for (var dy = -2; dy <= 2; dy++) {
    for (var dx = -2; dx <= 2; dx++) {
      acc += pyr_weight(dx) * pyr_weight(dy) * textureLoad(fine, pyr_clamp(centre + vec2i(dx, dy), P.fine), 0);
    }
  }
  textureStore(dst, id.xy, acc);
}
