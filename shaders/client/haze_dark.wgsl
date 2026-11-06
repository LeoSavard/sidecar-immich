// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/haze_params.wgsl haze/common.wgsl
// Dehaze, the dark channel before its patch minimum: each texel's colour and its smallest channel divided by the
// airlight's — with an airlight of 1 the plain dark channel, which finds the airlight; with the airlight found,
// the normalised one, which gives the transmission.

@group(0) @binding(0) var<uniform> H: HazeParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var air: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let c = haze_colour(H, textureLoad(src, id.xy, 0).rgb);
  let a = max(textureLoad(air, vec2u(0u), 0).rgb, vec3f(1e-6));
  let n = c / a;
  textureStore(dst, id.xy, vec4f(c, min(n.r, min(n.g, n.b))));
}
