// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Adjacency map, pass 3 of 3, run twice: a Gaussian of standard deviation `P.x` texels along direction
// `P.yz` (1, 0 then 0, 1), over the film's log exposure in the red channel. Edges repeat the last texel.

@group(0) @binding(0) var<uniform> P: vec4f;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let sigma = max(P.x, 0.3);
  let reach = i32(ceil(3.0 * sigma));
  let dir = vec2i(P.yz);
  let last = vec2i(size) - 1;
  var acc = 0.0;
  var total = 0.0;
  for (var i = -reach; i <= reach; i++) {
    let w = exp(-0.5 * f32(i * i) / (sigma * sigma));
    acc += w * textureLoad(src, clamp(vec2i(id.xy) + dir * i, vec2i(0), last), 0).r;
    total += w;
  }
  let v = acc / total;
  textureStore(dst, id.xy, vec4f(v, v, v, 1.0));
}
