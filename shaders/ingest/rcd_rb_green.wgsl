// SPDX-License-Identifier: BSD-2-Clause
// Copyright 2019 Johannes Hanika (vkdt). WGSL port copyright Sidecar contributors.
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that
// the conditions of the BSD 2-Clause licence are met; see NOTICE and THIRD_PARTY_NOTICES.md for the full text.
// Ported from vkdt src/pipe/modules/demosaic/rcd_fill.comp ("rb@g") and its output step @ 13756b3e18143f0f468b4a0cf34d6e787fcf2819
//! requires: ingest/rcd_common.wgsl
// Red and blue at green pixels; then undo white balance and attach clip coverage in alpha.

@group(0) @binding(0) var<uniform> P: IngestParams;
@group(0) @binding(1) var vh_tex: texture_2d<f32>;
@group(0) @binding(2) var g_tex: texture_2d<f32>;
@group(0) @binding(3) var rb_tex: texture_2d<f32>;
@group(0) @binding(4) var norm_tex: texture_2d<f32>;
@group(0) @binding(5) var rgb_out: texture_storage_2d<rgba16float, write>;

var<private> ipos: vec2i;

fn px(x: i32, y: i32) -> vec2i { return clamp_px(ipos + vec2i(x, y), P.in_size); }
fn sg(x: i32, y: i32) -> f32 { return textureLoad(g_tex, px(x, y), 0).r; }
fn vh(x: i32, y: i32) -> f32 { return textureLoad(vh_tex, px(x, y), 0).r; }
fn sc(c: i32, x: i32, y: i32) -> f32 {
  let v = textureLoad(rb_tex, px(x, y), 0);
  return select(v.b, v.r, c == 0);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.out_size.x || id.y >= P.out_size.y) { return; }
  ipos = vec2i(id.xy);
  var rgb = vec3f(0.0);
  if (rgGB(ipos) != 1) {
    let v = textureLoad(rb_tex, ipos, 0);
    rgb = vec3f(v.r, sg(0, 0), v.b);
  } else {
    let eps = 1e-5;
    let vhc = vh(0, 0);
    let vhn = 0.25 * (vh(-1, -1) + vh(1, -1) + vh(-1, 1) + vh(1, 1));
    let vh_discr = discriminator(vhc, vhn);
    let N1 = eps + abs(sg(0, 0) - sg(0, -2));
    let S1 = eps + abs(sg(0, 0) - sg(0, 2));
    let W1 = eps + abs(sg(0, 0) - sg(-2, 0));
    let E1 = eps + abs(sg(0, 0) - sg(2, 0));
    var out = vec2f(0.0);
    for (var k = 0; k < 2; k++) {
      let c = select(2, 0, k == 0);
      let SNabs = abs(sc(c, 0, -1) - sc(c, 0, 1));
      let EWabs = abs(sc(c, -1, 0) - sc(c, 1, 0));
      let N_grad = N1 + SNabs + abs(sc(c, 0, -1) - sc(c, 0, -3));
      let S_grad = S1 + SNabs + abs(sc(c, 0, 1) - sc(c, 0, 3));
      let W_grad = W1 + EWabs + abs(sc(c, -1, 0) - sc(c, -3, 0));
      let E_grad = E1 + EWabs + abs(sc(c, 1, 0) - sc(c, 3, 0));
      let N_est = sc(c, 0, -1) - sg(0, -1);
      let S_est = sc(c, 0, 1) - sg(0, 1);
      let W_est = sc(c, -1, 0) - sg(-1, 0);
      let E_est = sc(c, 1, 0) - sg(1, 0);
      let v_est = (N_grad * S_est + S_grad * N_est) / (N_grad + S_grad);
      let h_est = (E_grad * W_est + W_grad * E_est) / (E_grad + W_grad);
      out[k] = clamp(sg(0, 0) + mix(v_est, h_est, vh_discr), 0.0, 65535.0);
    }
    rgb = vec3f(out.x, sg(0, 0), out.y);
  }
  // Clip coverage: any original sample in this pixel's 2×2 neighbourhood at saturation, each colour against
  // its own threshold.
  var clip = 0.0;
  for (var j = 0; j <= 1; j++) {
    for (var i = 0; i <= 1; i++) {
      let q = px(i, j);
      if (textureLoad(norm_tex, q, 0).r >= clip_for(vec2u(q), P.pattern, P.clip)) { clip = 1.0; }
    }
  }
  textureStore(rgb_out, ipos, vec4f(rgb / P.wb.rgb, clip));
}
