// SPDX-License-Identifier: BSD-2-Clause
// Copyright 2019 Johannes Hanika (vkdt). WGSL port copyright Sidecar contributors.
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that
// the conditions of the BSD 2-Clause licence are met; see NOTICE and THIRD_PARTY_NOTICES.md for the full text.
// Ported from vkdt src/pipe/modules/hilite/assemble.comp @ 13756b3e18143f0f468b4a0cf34d6e787fcf2819
//! requires: ingest/common.wgsl
// Expand the coarse level into the clipped pixels of the fine one, a push-pull fill. in_size is the coarse
// size; out_size is the fine (output) size.
//
// Changed from vkdt: vkdt blends every pixel toward the coarse estimate (by `soft`, and by the coarse level's
// edge strength) and rescales it with weights exp(c − max(others)) that only make sense for values in [0, 1].
// Here a measured pixel keeps its values, a pixel with nothing measured takes the estimate, and the scale is
// fitted to the measured channels alone and bounded (common.wgsl, `hilite_scale`). Measured against ground
// truth this is closer on every frame tested, and it can't overflow into NaN (D14).
// docs/research/pipeline-review-2026-10.md, "The highlight stack", has the numbers.

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
  let fine = textureLoad(fine_tex, opos, 0).rgb;
  let white = clip_rgb(P.clip);
  let clipped = fine >= white;
  var o = fine;
  if (all(clipped)) {
    // Nothing measured (or a reduce level's "no unclipped tap" fallback): the estimate whole. Only its hue
    // matters, as doub fits its scale to the mosaic.
    o = upsm;
  } else if (any(clipped)) {
    // Measured channels stay; clipped ones come from the estimate, scaled to agree with the measured ones.
    let m = select(vec3f(1.0), vec3f(0.0), clipped);
    let est = hilite_estimate(upsm, hilite_scale(upsm, fine * m, m, fine), white);
    o = select(fine, max(est, fine), clipped);
  }
  textureStore(dst, opos, vec4f(o, 1.0));
}
