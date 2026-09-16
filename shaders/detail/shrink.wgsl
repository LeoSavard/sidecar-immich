// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// Detail band d = fine − coarse, soft-thresholded per channel and added to the accumulated details.
// a = luma threshold, b = chroma threshold (both already scaled by this band's noise level).

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var fine: texture_2d<f32>;
@group(0) @binding(2) var coarse: texture_2d<f32>;
@group(0) @binding(3) var acc_in: texture_2d<f32>;
@group(0) @binding(4) var acc_out: texture_storage_2d<rgba32float, write>;

fn soft(v: f32, t: f32) -> f32 {
  return sign(v) * max(abs(v) - t, 0.0);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let p = vec2i(id.xy);
  let d = textureLoad(fine, p, 0).rgb - textureLoad(coarse, p, 0).rgb;
  let shrunk = vec3f(soft(d.x, P.a), soft(d.y, P.b), soft(d.z, P.b));
  textureStore(acc_out, p, vec4f(textureLoad(acc_in, p, 0).rgb + shrunk, 0.0));
}
