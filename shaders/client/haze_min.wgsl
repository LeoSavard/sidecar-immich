// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/haze_params.wgsl
// Dehaze: the dark channel's minimum over a square patch, one axis per pass (`H.axis`), `H.radius` texels each
// way; the colour rides along unchanged.

@group(0) @binding(0) var<uniform> H: HazeParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let dir = select(vec2i(1, 0), vec2i(0, 1), H.axis == 1u);
  let last = vec2i(size) - 1;
  let r = i32(H.radius);
  var m = 1e30;
  for (var i = -r; i <= r; i++) {
    m = min(m, textureLoad(src, clamp(vec2i(id.xy) + dir * i, vec2i(0), last), 0).a);
  }
  textureStore(dst, id.xy, vec4f(textureLoad(src, id.xy, 0).rgb, m));
}
