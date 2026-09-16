// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// Inverse of to_opponent: coarse residual plus shrunk details, back to linear RGB.

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var coarse: texture_2d<f32>;
@group(0) @binding(2) var details: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let p = vec2i(id.xy);
  let base = textureLoad(coarse, p, 0);
  let o = base.rgb + textureLoad(details, p, 0).rgb;
  let s = max(o.x / 2.0, 0.0);
  let lum = s * s - STAB_EPS;
  let c1 = o.y * s;
  let c2 = o.z * s;
  let g = lum - (c1 + c2) / 3.0;
  textureStore(dst, p, vec4f(g + c1, g, g + c2, base.a));
}
