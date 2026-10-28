// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: ingest/common.wgsl
// Proxy-resolution X-Trans demosaic: each 3×3 block (five greens, two reds, two blues on the vkdt block phase)
// becomes one RGB pixel. Full-resolution X-Trans demosaicing is a separate path.

@group(0) @binding(0) var<uniform> P: IngestParams;
@group(0) @binding(1) var cfa: texture_2d<f32>;
@group(0) @binding(2) var norm_tex: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;

fn at(t: texture_2d<f32>, p: vec2i) -> f32 { return textureLoad(t, clamp_px(p, P.in_size), 0).r; }

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.out_size.x || id.y >= P.out_size.y) { return; }
  let ipos = vec2i(id.xy);
  let o = 3 * ipos;
  let g = (at(cfa, o) + at(cfa, o + vec2i(0, 2)) + at(cfa, o + vec2i(1, 1)) + at(cfa, o + vec2i(2, 0)) + at(cfa, o + vec2i(2, 2))) / 5.0;
  let a = (at(cfa, o + vec2i(0, 1)) + at(cfa, o + vec2i(2, 1))) * 0.5;
  let b = (at(cfa, o + vec2i(1, 0)) + at(cfa, o + vec2i(1, 2))) * 0.5;
  var rgb = vec3f(b, g, a);
  if (((ipos.x + ipos.y) & 1) > 0) { rgb = vec3f(a, g, b); }
  var clip = 0.0;
  for (var j = 0; j < 3; j++) {
    for (var i = 0; i < 3; i++) {
      let q = clamp_px(o + vec2i(i, j), P.in_size);
      if (at(norm_tex, q) >= clip_for(vec2u(q), P.pattern, P.clip)) { clip = 1.0; }
    }
  }
  textureStore(dst, ipos, vec4f(rgb, clip));
}
