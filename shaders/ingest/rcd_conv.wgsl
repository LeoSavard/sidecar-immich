// SPDX-License-Identifier: BSD-2-Clause
// Copyright 2019 Johannes Hanika (vkdt). WGSL port copyright Sidecar contributors.
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that
// the conditions of the BSD 2-Clause licence are met; see NOTICE and THIRD_PARTY_NOTICES.md for the full text.
// Ported from vkdt src/pipe/modules/demosaic/rcd_conv.comp @ 13756b3e18143f0f468b4a0cf34d6e787fcf2819
//! requires: ingest/rcd_common.wgsl
// Vertical/horizontal discriminator at every pixel; diagonal discriminator and low-pass green at red/blue pixels.

@group(0) @binding(0) var<uniform> P: IngestParams;
@group(0) @binding(1) var cfa_tex: texture_2d<f32>;
@group(0) @binding(2) var vh_out: texture_storage_2d<r32float, write>;
@group(0) @binding(3) var pq_out: texture_storage_2d<r32float, write>;
@group(0) @binding(4) var lp_out: texture_storage_2d<r32float, write>;

var<private> ipos: vec2i;

fn cfa(x: i32, y: i32) -> f32 {
  return textureLoad(cfa_tex, clamp_px(ipos + vec2i(x, y), P.in_size), 0).r;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.out_size.x || id.y >= P.out_size.y) { return; }
  ipos = vec2i(id.xy);
  var vhs = vec2f(0.0);
  for (var i = -1; i <= 1; i++) {
    vhs += vec2f(
      cfa(i, -3) - 3.0 * cfa(i, -2) - cfa(i, -1) + 6.0 * cfa(i, 0) - cfa(i, 1) - 3.0 * cfa(i, 2) + cfa(i, 3),
      cfa(-3, i) - 3.0 * cfa(-2, i) - cfa(-1, i) + 6.0 * cfa(0, i) - cfa(1, i) - 3.0 * cfa(2, i) + cfa(3, i));
  }
  vhs *= vhs;
  textureStore(vh_out, ipos, vec4f(vhs.x / (1e-5 + vhs.x + vhs.y), 0.0, 0.0, 0.0));

  if (((ipos.x + ipos.y) & 1) == 0) {
    var pqs = vec2f(1e-5);
    for (var i = -1; i <= 1; i++) {
      pqs += vec2f(
        cfa(-3 + i, -3 + i) - cfa(1 + i, -1 + i) - cfa(1 + i, 1 + i) + cfa(3 + i, 3 + i) - 3.0 * (cfa(-2 + i, -2 + i) + cfa(2 + i, 2 + i)) + 6.0 * cfa(i, i),
        cfa(3 + i, -3 - i) - cfa(1 + i, -1 - i) - cfa(-1 + i, 1 - i) + cfa(-3 + i, 3 - i) - 3.0 * (cfa(2 + i, -2 - i) + cfa(-2 + i, 2 - i)) + 6.0 * cfa(i, -i));
    }
    pqs *= pqs;
    textureStore(pq_out, ipos, vec4f(pqs.x / (pqs.x + pqs.y), 0.0, 0.0, 0.0));
    // vkdt computes this at the adjacent green pixel, centred on this red/blue pixel.
    let w = array<f32, 3>(0.5, 1.0, 0.5);
    var lp = 0.0;
    for (var j = -1; j <= 1; j++) {
      for (var i = -1; i <= 1; i++) {
        lp += w[j + 1] * w[i + 1] * cfa(i, j);
      }
    }
    textureStore(lp_out, ipos, vec4f(max(1e-6, lp), 0.0, 0.0, 0.0));
  } else {
    textureStore(pq_out, ipos, vec4f(0.0));
    textureStore(lp_out, ipos, vec4f(0.0));
  }
}
