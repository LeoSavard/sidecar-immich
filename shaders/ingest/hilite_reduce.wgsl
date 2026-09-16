// SPDX-License-Identifier: BSD-2-Clause
// Copyright 2019 Johannes Hanika (vkdt). WGSL port copyright Sidecar contributors.
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that
// the conditions of the BSD 2-Clause licence are met; see NOTICE and THIRD_PARTY_NOTICES.md for the full text.
// Ported from vkdt src/pipe/modules/hilite/reduce.comp @ 13756b3e18143f0f468b4a0cf34d6e787fcf2819
//! requires: ingest/common.wgsl
// Clipping-aware Gauss reduce: blur one pyramid level down, disregarding clipped pixels, desaturating colours
// near white. Alpha carries the local edge strength.

@group(0) @binding(0) var<uniform> P: IngestParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba32float, write>;

const W = array<f32, 5>(1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0);
const SW = array<f32, 5>(1.0, 2.0, 0.0, -2.0, -1.0);

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.out_size.x || id.y >= P.out_size.y) { return; }
  let opos = vec2i(id.xy);
  var white = P.clip;
  if (!(white > 0.0)) { white = 1.0; }
  let w = W;
  let sw = SW;
  var edge = vec2f(0.0);
  var col = vec3f(0.0);
  var wb = P.wb.rgb;
  if (!(dot(wb, wb) > 1e-3)) { wb = vec3f(1.0); }
  var wgt = 0.0;
  for (var jj = -2; jj <= 2; jj++) {
    for (var ii = -2; ii <= 2; ii++) {
      var rgb = textureLoad(src, clamp_px(2 * opos + vec2i(ii, jj), P.in_size), 0).rgb;
      edge += vec2f(w[jj + 2] * sw[ii + 2], w[ii + 2] * sw[jj + 2]) * luminance_rec2020(rgb);
      let u = w[ii + 2] * w[jj + 2];
      let rgbw = rgb * wb;
      let cmax = max(rgbw.r, max(rgbw.g, rgbw.b));
      let cmin = min(rgbw.r, min(rgbw.g, rgbw.b));
      let sat = (cmax - cmin) / max(1e-3, cmax);
      if (all(rgb < vec3f(white))) {
        let s = smoothstep(0.2, 1.0, max(rgb.r, max(rgb.g, rgb.b)) / white);
        var t = smoothstep(0.15, 0.9, sat);
        let ds = P.desat * P.desat;
        t = clamp(5.0 * ds * ds * s * t, 0.0, 1.0);
        rgb = mix(rgb, vec3f(cmax + cmin) / wb * 0.5, t);
        col += rgb * u;
        wgt += u;
      }
    }
  }
  if (wgt == 0.0) { col = vec3f(1.0); } else { col /= wgt; }
  textureStore(dst, opos, vec4f(col, length(edge)));
}
