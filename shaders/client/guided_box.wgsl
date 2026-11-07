// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/guided_params.wgsl
// Guided filter, passes 2 and 4: a box mean of radius `G.radius` along one axis (`G.axis`, 0 across, 1 down),
// run twice for a square window. At the image's edge the window is cut, and the mean is over the taps that
// remain — the normalisation He, Sun and Tang use — so a border pixel is not pulled toward zero.

@group(0) @binding(0) var<uniform> G: GuidedParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = G.low;
  if (id.x >= size.x || id.y >= size.y) { return; }
  let r = i32(G.radius);
  let step = select(vec2i(1, 0), vec2i(0, 1), G.axis == 1u);
  let last = vec2i(size) - 1;
  var sum = vec4f(0.0);
  var count = 0.0;
  for (var k = -r; k <= r; k++) {
    let at = vec2i(id.xy) + step * k;
    if (all(at >= vec2i(0)) && all(at <= last)) {
      sum += textureLoad(src, at, 0);
      count += 1.0;
    }
  }
  textureStore(dst, id.xy, sum / count);
}
