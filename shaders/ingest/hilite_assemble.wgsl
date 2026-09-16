// SPDX-License-Identifier: BSD-2-Clause
// Copyright 2019 Johannes Hanika (vkdt). WGSL port copyright Sidecar contributors.
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that
// the conditions of the BSD 2-Clause licence are met; see NOTICE and THIRD_PARTY_NOTICES.md for the full text.
// Ported from vkdt src/pipe/modules/hilite/assemble.comp @ 13756b3e18143f0f468b4a0cf34d6e787fcf2819
//! requires: ingest/common.wgsl
// Expand the coarse level and rescale it to match the unclipped channels of the fine level.
// in_size is the coarse size; out_size is the fine (output) size.

@group(0) @binding(0) var<uniform> P: IngestParams;
@group(0) @binding(1) var fine_tex: texture_2d<f32>;
@group(0) @binding(2) var coarse: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba32float, write>;

const W = array<f32, 5>(1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0);

fn load_coarse(p: vec2i) -> vec3f {
  return textureLoad(coarse, clamp_px(p, P.in_size), 0).rgb;
}

fn gauss_expand(opos: vec2i) -> vec3f {
  let w = W;
  var c = vec3f(0.0);
  var wgt = 0.0;
  let ipos = opos / 2;
  let d = (opos.x & 1) + 2 * (opos.y & 1);
  if (d == 0) {
    for (var ii = -1; ii <= 1; ii++) {
      for (var jj = -1; jj <= 1; jj++) {
        let k = w[2 * jj + 2] * w[2 * ii + 2];
        c += load_coarse(ipos + vec2i(ii, jj)) * k;
        wgt += k;
      }
    }
  } else if (d == 1) {
    for (var ii = 0; ii <= 1; ii++) {
      for (var jj = -1; jj <= 1; jj++) {
        let k = w[2 * jj + 2] * w[2 * ii + 1];
        c += load_coarse(ipos + vec2i(ii, jj)) * k;
        wgt += k;
      }
    }
  } else if (d == 2) {
    for (var ii = -1; ii <= 1; ii++) {
      for (var jj = 0; jj <= 1; jj++) {
        let k = w[2 * jj + 1] * w[2 * ii + 2];
        c += load_coarse(ipos + vec2i(ii, jj)) * k;
        wgt += k;
      }
    }
  } else {
    for (var ii = 0; ii <= 1; ii++) {
      for (var jj = 0; jj <= 1; jj++) {
        let k = w[2 * jj + 1] * w[2 * ii + 1];
        c += load_coarse(ipos + vec2i(ii, jj)) * k;
        wgt += k;
      }
    }
  }
  if (wgt == 0.0) { return vec3f(0.0, 1.0, 1.0); }
  return c / wgt;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.out_size.x || id.y >= P.out_size.y) { return; }
  let opos = vec2i(id.xy);
  let upsm = gauss_expand(opos);
  var fine = textureLoad(fine_tex, opos, 0);
  let white = P.clip;
  let sr = fine.r / max(0.001, upsm.r);
  let sg = fine.g / max(0.001, upsm.g);
  let sb = fine.b / max(0.001, upsm.b);
  let wr = exp(upsm.r - max(upsm.g, upsm.b));
  let wg = exp(upsm.g - max(upsm.r, upsm.b));
  let wbw = exp(upsm.b - max(upsm.r, upsm.g));
  let scale = (sr * wr + sg * wg + sb * wbw) / (wr + wg + wbw);
  let rec = clamp(upsm * scale, vec3f(-65535.0), vec3f(65535.0));
  var t = P.soft;
  if (any(fine.rgb >= vec3f(white))) { t = 1.0; }
  if (fine.a != fine.a) { fine.a = 0.0; }
  t = clamp(mix(t, 1.0, sqrt(max(0.0, fine.a))), 0.0, 1.0);
  textureStore(dst, opos, vec4f(mix(fine.rgb, rec, t), 1.0));
}
