// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// Scale RGB by the sharpened/original luminance ratio, blended by amount (a), so hue is untouched.

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var observed: texture_2d<f32>;
@group(0) @binding(3) var estimate: texture_2d<f32>;
@group(0) @binding(4) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let p = vec2i(id.xy);
  let c = textureLoad(src, p, 0);
  let y = textureLoad(observed, p, 0).r;
  let e = max(textureLoad(estimate, p, 0).r, 0.0);
  let target_y = mix(y, e, P.a);
  let k = select(1.0, target_y / y, y > 1e-6);
  textureStore(dst, p, vec4f(c.rgb * k, c.a));
}
