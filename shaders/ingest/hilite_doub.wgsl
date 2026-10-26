// SPDX-License-Identifier: BSD-2-Clause
// Copyright 2019 Johannes Hanika (vkdt). WGSL port copyright Sidecar contributors.
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that
// the conditions of the BSD 2-Clause licence are met; see NOTICE and THIRD_PARTY_NOTICES.md for the full text.
// Ported from vkdt src/pipe/modules/hilite/doub.comp @ 13756b3e18143f0f468b4a0cf34d6e787fcf2819
//! requires: ingest/common.wgsl
// Write reconstructed colour back into the mosaic where samples approach clipping. Runs on the coarse grid;
// in_size is the mosaic size, out_size the coarse size. Each invocation writes one 2×2 or 3×3 block.
//
// Changed from vkdt: vkdt replaces every sample of a block once any of them nears white, measured ones
// included, with a scale that averages in the clipped samples' ratios (which are underestimates) under
// weights that overflow outside [0, 1]. Here only a sample nearing white is replaced, by the estimate scaled
// to the block's measured samples, never below its own value and within a bounded headroom (common.wgsl).
// docs/research/pipeline-review-2026-10.md, "The highlight stack", has the measurements.

@group(0) @binding(0) var<uniform> P: IngestParams;
@group(0) @binding(1) var cfa: texture_2d<f32>;
@group(0) @binding(2) var coarse: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<r32float, write>;

fn at(p: vec2i) -> f32 {
  return textureLoad(cfa, clamp_px(p, P.in_size), 0).r;
}

fn put(p: vec2i, v: f32) {
  textureStore(dst, p, vec4f(v, 0.0, 0.0, 0.0));
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.out_size.x || id.y >= P.out_size.y) { return; }
  let ipos = vec2i(id.xy);
  var upsm = textureLoad(coarse, ipos, 0).rgb;
  let white = P.clip;
  let softw = 0.97 * white;
  if (P.sensor == 1u) {
    if (((ipos.x + ipos.y) & 1) == 0) { upsm = upsm.bgr; }
    let o = 3 * ipos;
    // c1, c7 red; c3, c5 blue; the rest green.
    var c = array<f32, 9>(
      at(o), at(o + vec2i(0, 1)), at(o + vec2i(0, 2)),
      at(o + vec2i(1, 0)), at(o + vec2i(1, 1)), at(o + vec2i(1, 2)),
      at(o + vec2i(2, 0)), at(o + vec2i(2, 1)), at(o + vec2i(2, 2)));
    var ch = array<u32, 9>(1u, 0u, 1u, 2u, 1u, 2u, 1u, 0u, 1u);
    var sum = vec3f(0.0);
    var n = vec3f(0.0);
    var top = vec3f(0.0);
    for (var i = 0; i < 9; i++) {
      let k = ch[i];
      top[k] = max(top[k], c[i]);
      if (c[i] < softw) {
        sum[k] += c[i];
        n[k] += 1.0;
      }
    }
    if (max(top.r, max(top.g, top.b)) > softw) {
      let est = hilite_estimate(upsm, hilite_scale(upsm, sum, n, top), white);
      for (var i = 0; i < 9; i++) {
        c[i] = mix(c[i], max(est[ch[i]], c[i]), smoothstep(softw, white, c[i]));
      }
    }
    put(o, c[0]);
    put(o + vec2i(0, 1), c[1]);
    put(o + vec2i(0, 2), c[2]);
    put(o + vec2i(1, 0), c[3]);
    put(o + vec2i(1, 1), c[4]);
    put(o + vec2i(1, 2), c[5]);
    put(o + vec2i(2, 0), c[6]);
    put(o + vec2i(2, 1), c[7]);
    put(o + vec2i(2, 2), c[8]);
  } else {
    let o = 2 * ipos;
    // gather order as in vkdt: w = (0,0) R, z = (1,0) G, x = (0,1) G, y = (1,1) B
    var c = vec4f(at(o + vec2i(0, 1)), at(o + vec2i(1, 1)), at(o + vec2i(1, 0)), at(o));
    let maxrgb = max(max(c.x, c.y), max(c.z, c.w));
    if (maxrgb > softw) {
      let m = select(vec4f(0.0), vec4f(1.0), c < vec4f(softw));
      let sum = vec3f(c.w * m.w, c.x * m.x + c.z * m.z, c.y * m.y);
      let n = vec3f(m.w, m.x + m.z, m.y);
      let top = vec3f(c.w, max(c.x, c.z), c.y);
      let est = hilite_estimate(upsm, hilite_scale(upsm, sum, n, top), white);
      let t = smoothstep(vec4f(softw), vec4f(white), c);
      c = mix(c, max(vec4f(est.g, est.b, est.g, est.r), c), t);
    }
    put(o, c.w);
    put(o + vec2i(1, 0), c.z);
    put(o + vec2i(0, 1), c.x);
    put(o + vec2i(1, 1), c.y);
  }
}
