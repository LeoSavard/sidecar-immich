// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/guided_params.wgsl guided/common.wgsl
// Guided filter, pass 1 of 4: the per-texel statistics on the low-resolution grid — guide I, input p, I·p and
// I·I — each the mean over the texel's footprint, so a subsampled grid is the fast guided filter (He and Sun,
// 2015) rather than an aliased one. Products are averaged, not multiplied after averaging: E[I·p] is what the
// covariance needs. Written to rgba32float, because the variance is a difference of two means and half
// precision would cancel it away.

@group(0) @binding(0) var<uniform> G: GuidedParams;
@group(0) @binding(1) var guide: texture_2d<f32>;
@group(0) @binding(2) var input: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
@group(0) @binding(4) var dst: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let low = G.low;
  if (id.x >= low.x || id.y >= low.y) { return; }
  let n = guided_subcells(textureDimensions(guide), low);
  var acc = vec4f(0.0);
  for (var sy = 0u; sy < n.y; sy++) {
    for (var sx = 0u; sx < n.x; sx++) {
      // The centre of subcell (sx, sy) of this low texel, in the frame's 0…1 coordinates. With one subcell and
      // a guide of the grid's own size, that is the guide texel's centre exactly.
      let uv = (vec2f(id.xy) + (vec2f(f32(sx), f32(sy)) + 0.5) / vec2f(n)) / vec2f(low);
      let i = guided_guide(textureSampleLevel(guide, samp, uv, 0.0).rgb, G.log_guide);
      // Channel 4 is the guide itself: a self-guided filter, as local contrast needs.
      let p = select(textureSampleLevel(input, samp, uv, 0.0)[min(G.channel, 3u)], i, G.channel == 4u);
      acc += vec4f(i, p, i * p, i * i);
    }
  }
  textureStore(dst, id.xy, acc / f32(n.x * n.y));
}
