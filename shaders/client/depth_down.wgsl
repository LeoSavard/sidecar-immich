// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: depth/effects.wgsl
// Depth pass 1 of 3, when blurring: the picture averaged down by `scale.x` in each direction, with the haze
// applied, into the low-resolution texture the blur gathers from. Gathering at this scale keeps the number of
// samples a wide blur needs the same at any picture size.

@group(0) @binding(0) var<uniform> D: DepthParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var depth: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
@group(0) @binding(4) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let k = u32(D.scale.x);
  let full = textureDimensions(src);
  var acc = vec3f(0.0);
  var count = 0.0;
  for (var j = 0u; j < k; j++) {
    for (var i = 0u; i < k; i++) {
      let p = id.xy * k + vec2u(i, j);
      if (p.x < full.x && p.y < full.y) {
        acc += textureLoad(src, p, 0).rgb;
        count += 1.0;
      }
    }
  }
  let centre = (vec2f(id.xy) + 0.5) * f32(k);
  let n = depth_nearness(depth, samp, centre, D.frame);
  textureStore(dst, id.xy, vec4f(depth_haze(acc / max(count, 1.0), n, D.haze), 1.0));
}
