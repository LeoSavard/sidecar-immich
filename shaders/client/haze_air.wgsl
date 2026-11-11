// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/haze_params.wgsl
// Dehaze: the airlight, by one workgroup over the whole map — the colour of the haziest place, where the
// patch-minimum dark channel is greatest (He et al. §4.4; ties go to the brighter), into a 1×1 texture. Kept on
// the GPU so the browser needs no read-back between the passes.

@group(0) @binding(0) var<uniform> H: HazeParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba32float, write>;

var<workgroup> best_dark: array<f32, 256>;
var<workgroup> best_lum: array<f32, 256>;
var<workgroup> best_rgb: array<vec3f, 256>;

@compute @workgroup_size(16, 16)
fn main(@builtin(local_invocation_index) li: u32) {
  let size = textureDimensions(src);
  let n = size.x * size.y;
  var d = -1.0;
  var l = -1.0;
  var c = vec3f(1.0);
  for (var i = li; i < n; i += 256u) {
    let t = textureLoad(src, vec2u(i % size.x, i / size.x), 0);
    let lum = dot(t.rgb, vec3f(0.2627, 0.6780, 0.0593));
    if (t.a > d || (t.a == d && lum > l)) {
      d = t.a;
      l = lum;
      c = t.rgb;
    }
  }
  best_dark[li] = d;
  best_lum[li] = l;
  best_rgb[li] = c;
  workgroupBarrier();
  for (var s = 128u; s > 0u; s >>= 1u) {
    if (li < s) {
      let o = li + s;
      if (best_dark[o] > best_dark[li] || (best_dark[o] == best_dark[li] && best_lum[o] > best_lum[li])) {
        best_dark[li] = best_dark[o];
        best_lum[li] = best_lum[o];
        best_rgb[li] = best_rgb[o];
      }
    }
    workgroupBarrier();
  }
  if (li == 0u) {
    textureStore(dst, vec2u(0u), vec4f(max(best_rgb[0], vec3f(1e-6)), 1.0));
  }
}
