// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// Copy a rectangle (origin in a, b as pixels) of an RGBA16F texture into a new texture.

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let s = vec2i(id.xy) + vec2i(i32(P.a), i32(P.b));
  textureStore(dst, vec2i(id.xy), textureLoad(src, max(s, vec2i(0)), 0));
}
