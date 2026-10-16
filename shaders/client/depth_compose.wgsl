// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: depth/effects.wgsl
// Depth pass 3 of 3 (the only one when there is haze and no blur): at full resolution, the haze on each pixel,
// then — when blurring — the low-resolution blur over it by how blurred this pixel is: sharp where it is in
// focus, the blur where its disc is a couple of low-resolution texels or more, and the blur wherever a nearer
// blurred point spreads over it. Alpha, the ingest's clip flags, passes through for develop's highlight blend.

@group(0) @binding(0) var<uniform> D: DepthParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var blurred: texture_2d<f32>;
@group(0) @binding(3) var depth: texture_2d<f32>;
@group(0) @binding(4) var samp: sampler;
@group(0) @binding(5) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let s = textureLoad(src, id.xy, 0);
  let p = vec2f(id.xy) + 0.5;
  let n = depth_nearness(depth, samp, p, D.frame);
  var c = depth_haze(s.rgb, n, D.haze);
  if (D.blur.w > 0.5) {
    let k = D.scale.x;
    let b = textureSampleLevel(blurred, samp, p / k / vec2f(textureDimensions(blurred)), 0.0);
    let a = max(clamp(depth_coc(n, D.blur) / (2.0 * k), 0.0, 1.0), b.a);
    c = mix(c, b.rgb, a);
  }
  textureStore(dst, id.xy, vec4f(c, s.a));
}
