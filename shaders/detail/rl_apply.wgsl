// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// Scale RGB by the sharpened/original luminance ratio, blended by the amount at this pixel — the strength map's
// green (`detail/strength.wgsl`), 1×1 when the same everywhere — so hue is untouched.

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var observed: texture_2d<f32>;
@group(0) @binding(3) var estimate: texture_2d<f32>;
@group(0) @binding(4) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(5) var strength: texture_2d<f32>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let p = vec2i(id.xy);
  let c = textureLoad(src, p, 0);
  let y = textureLoad(observed, p, 0).r;
  let e = max(textureLoad(estimate, p, 0).r, 0.0);
  let amount = textureLoad(strength, min(id.xy, textureDimensions(strength) - 1u), 0).g;
  let target_y = mix(y, e, amount);
  let k = select(1.0, target_y / y, y > 1e-6);
  textureStore(dst, p, vec4f(c.rgb * k, c.a));
}
