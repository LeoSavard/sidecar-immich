// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// The local-contrast map (`develop/local.wgsl`): the texture band's guided-filter coefficients (mean a, mean b)
// and the clarity band's, both on the same grid, side by side in one texel for develop to read.

@group(0) @binding(0) var<uniform> P: vec4f;
@group(0) @binding(1) var fine: texture_2d<f32>;
@group(0) @binding(2) var mid: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  textureStore(dst, id.xy, vec4f(textureLoad(fine, id.xy, 0).xy, textureLoad(mid, id.xy, 0).xy));
}
