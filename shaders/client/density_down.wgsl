// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Adjacency map, pass 1 of 3 (`sidecar_color::developer`): the frame averaged down by `P.x` in each direction —
// camera light and the highlight flag alike — for develop to turn into the film's log exposure (pass 2, the
// develop pass itself with `mono.z` set), which `density_blur.wgsl` then blurs (pass 3).

@group(0) @binding(0) var<uniform> P: vec4f;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let k = u32(P.x);
  let full = textureDimensions(src);
  var acc = vec4f(0.0);
  var count = 0.0;
  for (var j = 0u; j < k; j++) {
    for (var i = 0u; i < k; i++) {
      let p = id.xy * k + vec2u(i, j);
      if (p.x < full.x && p.y < full.y) {
        acc += textureLoad(src, p, 0);
        count += 1.0;
      }
    }
  }
  textureStore(dst, id.xy, acc / max(count, 1.0));
}
