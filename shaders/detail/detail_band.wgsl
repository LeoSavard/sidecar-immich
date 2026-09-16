// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// |fine − coarse| of the luma channel, for estimating the noise level (median absolute deviation, on the CPU).

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var fine: texture_2d<f32>;
@group(0) @binding(2) var coarse: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let p = vec2i(id.xy);
  let d = abs(textureLoad(fine, p, 0).x - textureLoad(coarse, p, 0).x);
  textureStore(dst, p, vec4f(d, 0.0, 0.0, 1.0));
}
