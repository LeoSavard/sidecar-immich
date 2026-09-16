// SPDX-License-Identifier: BSD-2-Clause
// Copyright 2019 Johannes Hanika (vkdt). WGSL port copyright Sidecar contributors.
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that
// the conditions of the BSD 2-Clause licence are met; see NOTICE and THIRD_PARTY_NOTICES.md for the full text.
// Ported from vkdt src/pipe/modules/hilite/doub.comp @ 13756b3e18143f0f468b4a0cf34d6e787fcf2819
//! requires: ingest/common.wgsl
// Write reconstructed colour back into the mosaic where samples approach clipping. Runs on the coarse grid;
// in_size is the mosaic size, out_size the coarse size. Each invocation writes one 2×2 or 3×3 block.

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
    var c0 = at(o);
    var c1 = at(o + vec2i(0, 1));
    var c2 = at(o + vec2i(0, 2));
    var c3 = at(o + vec2i(1, 0));
    var c4 = at(o + vec2i(1, 1));
    var c5 = at(o + vec2i(1, 2));
    var c6 = at(o + vec2i(2, 0));
    var c7 = at(o + vec2i(2, 1));
    var c8 = at(o + vec2i(2, 2));
    let minr = min(c1, c7);
    let minb = min(c3, c5);
    let ming = min(min(min(c0, c2), c4), min(c6, c8));
    let sr = minr / max(0.001, upsm.r);
    let sg = ming / max(0.001, upsm.g);
    let sb = minb / max(0.001, upsm.b);
    let wr = exp(upsm.r - max(upsm.g, upsm.b));
    let wg = exp(upsm.g - max(upsm.r, upsm.b));
    let wbw = exp(upsm.b - max(upsm.r, upsm.g));
    let scale = (sr * wr + sg * wg + sb * wbw) / (wr + wg + wbw);
    let maxr = max(c1, c7);
    let maxb = max(c3, c5);
    let maxg = max(max(max(c0, c2), c4), max(c6, c8));
    let maxrgb = max(maxr, max(maxg, maxb));
    if (maxrgb > softw) {
      var t = smoothstep(softw, white, maxrgb);
      c1 = mix(c1, upsm.r * scale, t);
      c7 = mix(c7, upsm.r * scale, t);
      c3 = mix(c3, upsm.b * scale, t);
      c5 = mix(c5, upsm.b * scale, t);
      t = smoothstep(softw, white, ming);
      c0 = mix(c0, upsm.g * scale, t);
      c2 = mix(c2, upsm.g * scale, t);
      c4 = mix(c4, upsm.g * scale, t);
      c6 = mix(c6, upsm.g * scale, t);
      c8 = mix(c8, upsm.g * scale, t);
    }
    put(o, c0);
    put(o + vec2i(0, 1), c1);
    put(o + vec2i(0, 2), c2);
    put(o + vec2i(1, 0), c3);
    put(o + vec2i(1, 1), c4);
    put(o + vec2i(1, 2), c5);
    put(o + vec2i(2, 0), c6);
    put(o + vec2i(2, 1), c7);
    put(o + vec2i(2, 2), c8);
  } else {
    let o = 2 * ipos;
    // gather order as in vkdt: w = (0,0) R, z = (1,0) G, x = (0,1) G, y = (1,1) B
    var c = vec4f(at(o + vec2i(0, 1)), at(o + vec2i(1, 1)), at(o + vec2i(1, 0)), at(o));
    let ming = min(c.x, c.z);
    let sr = c.w / max(0.001, upsm.r);
    let sg = ming / max(0.001, upsm.g);
    let sb = c.y / max(0.001, upsm.b);
    let wr = exp(upsm.r - max(upsm.g, upsm.b));
    let wg = exp(upsm.g - max(upsm.r, upsm.b));
    let wbw = exp(upsm.b - max(upsm.r, upsm.g));
    let scale = (sr * wr + sg * wg + sb * wbw) / (wr + wg + wbw);
    let maxrgb = max(max(c.x, c.y), max(c.z, c.w));
    if (maxrgb > softw) {
      let t = smoothstep(softw, white, maxrgb);
      c = mix(c, scale * vec4f(upsm.g, upsm.b, upsm.g, upsm.r), vec4f(t));
    }
    put(o, c.w);
    put(o + vec2i(1, 0), c.z);
    put(o + vec2i(0, 1), c.x);
    put(o + vec2i(1, 1), c.y);
  }
}
