// SPDX-License-Identifier: BSD-2-Clause
// Copyright 2019 Johannes Hanika (vkdt). WGSL port copyright Sidecar contributors.
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that
// the conditions of the BSD 2-Clause licence are met; see NOTICE and THIRD_PARTY_NOTICES.md for the full text.
// Ported from vkdt src/pipe/modules/hilite/half.comp @ 13756b3e18143f0f468b4a0cf34d6e787fcf2819
//! requires: ingest/common.wgsl
// Reduced-size demosaic for highlight reconstruction: gather each 2×2 (Bayer) or 3×3 (X-Trans) block into RGB,
// ignoring clipped greens where the other green is usable.

@group(0) @binding(0) var<uniform> P: IngestParams;
@group(0) @binding(1) var cfa: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba32float, write>;

fn at(p: vec2i) -> f32 {
  return textureLoad(cfa, clamp_px(p, P.in_size), 0).r;
}

// Where the colour sampled at `p` saturates. Read off the pattern rather than assumed from the block phase,
// so the X-Trans blocks' alternating red and blue get their own answers.
fn white_at(p: vec2i) -> f32 {
  return clip_for(vec2u(clamp_px(p, P.in_size)), P.pattern, P.clip);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.out_size.x || id.y >= P.out_size.y) { return; }
  let ipos = vec2i(id.xy);
  var rgba = vec4f(0.0, 0.0, 0.0, 1.0);
  if (P.sensor == 1u) {
    let o = 3 * ipos;
    // c1 and c7 are one chroma colour, c3 and c5 the other, the rest green; each has its own saturation.
    let wg = white_at(o);
    let w17 = white_at(o + vec2i(0, 1));
    let w35 = white_at(o + vec2i(1, 0));
    var c0 = at(o);
    var c1 = at(o + vec2i(0, 1));
    var c2 = at(o + vec2i(0, 2));
    var c3 = at(o + vec2i(1, 0));
    var c4 = at(o + vec2i(1, 1));
    var c5 = at(o + vec2i(1, 2));
    var c6 = at(o + vec2i(2, 0));
    var c7 = at(o + vec2i(2, 1));
    var c8 = at(o + vec2i(2, 2));
    if (c1 >= w17) { c1 = c7; }
    if (c7 >= w17) { c7 = c1; }
    if (c3 >= w35) { c3 = c5; }
    if (c5 >= w35) { c5 = c3; }
    let maxg = max(max(max(c0, c2), c4), max(c6, c8));
    if (maxg >= wg) {
      c0 = 1.0; c2 = 1.0; c4 = 1.0; c6 = 1.0; c8 = 1.0;
    }
    let col = vec2f((c1 + c7) * 0.5, (c3 + c5) * 0.5);
    if (((ipos.x + ipos.y) & 1) > 0) {
      rgba.r = col.x;
      rgba.b = col.y;
    } else {
      rgba.b = col.x;
      rgba.r = col.y;
    }
    rgba.g = (c0 + c2 + c4 + c6 + c8) / 5.0;
  } else {
    let o = 2 * ipos;
    // Cropped to RGGB, so (1,0) and (0,1) are the two greens and only their threshold is needed here.
    let wg = white_at(o + vec2i(1, 0));
    let w = at(o);
    var z = at(o + vec2i(1, 0));
    var x = at(o + vec2i(0, 1));
    let y = at(o + vec2i(1, 1));
    if (x >= wg) { x = z; }
    if (z >= wg) { z = x; }
    rgba = vec4f(w, (x + z) / 2.0, y, 1.0);
  }
  textureStore(dst, ipos, rgba);
}
