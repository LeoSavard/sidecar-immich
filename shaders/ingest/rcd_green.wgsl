// SPDX-License-Identifier: BSD-2-Clause
// Copyright 2019 Johannes Hanika (vkdt). WGSL port copyright Sidecar contributors.
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that
// the conditions of the BSD 2-Clause licence are met; see NOTICE and THIRD_PARTY_NOTICES.md for the full text.
// Ported from vkdt src/pipe/modules/demosaic/rcd_fill.comp ("g@rb") @ 13756b3e18143f0f468b4a0cf34d6e787fcf2819
//! requires: ingest/rcd_common.wgsl
// Green at red and blue pixels. Output is white-balanced green at every pixel.

@group(0) @binding(0) var<uniform> P: IngestParams;
@group(0) @binding(1) var cfa_tex: texture_2d<f32>;
@group(0) @binding(2) var vh_tex: texture_2d<f32>;
@group(0) @binding(3) var lp_tex: texture_2d<f32>;
@group(0) @binding(4) var g_out: texture_storage_2d<r32float, write>;

var<private> ipos: vec2i;

fn load(t: texture_2d<f32>, x: i32, y: i32) -> f32 {
  return textureLoad(t, clamp_px(ipos + vec2i(x, y), P.in_size), 0).r;
}
fn cfa(x: i32, y: i32) -> f32 { return load(cfa_tex, x, y); }
fn vh(x: i32, y: i32) -> f32 { return load(vh_tex, x, y); }
fn lp(x: i32, y: i32) -> f32 { return load(lp_tex, x, y); }

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.out_size.x || id.y >= P.out_size.y) { return; }
  ipos = vec2i(id.xy);
  if (rgGB(ipos) == 1) {
    textureStore(g_out, ipos, vec4f(cfa(0, 0) * P.wb.g, 0.0, 0.0, 0.0));
    return;
  }
  let eps = 1e-5;
  let vhc = vh(0, 0);
  let vhn = 0.25 * (vh(-1, -1) + vh(1, -1) + vh(-1, 1) + vh(1, 1));
  let vh_discr = discriminator(vhc, vhn);

  let N_grad = eps + abs(cfa(0, -1) - cfa(0, 1)) + abs(cfa(0, 0) - cfa(0, -2)) + abs(cfa(0, -1) - cfa(0, -3)) + abs(cfa(0, -2) - cfa(0, -4));
  let S_grad = eps + abs(cfa(0, -1) - cfa(0, 1)) + abs(cfa(0, 0) - cfa(0, 2)) + abs(cfa(0, 1) - cfa(0, 3)) + abs(cfa(0, 2) - cfa(0, 4));
  let W_grad = eps + abs(cfa(-1, 0) - cfa(1, 0)) + abs(cfa(0, 0) - cfa(-2, 0)) + abs(cfa(-1, 0) - cfa(-3, 0)) + abs(cfa(-2, 0) - cfa(-4, 0));
  let E_grad = eps + abs(cfa(-1, 0) - cfa(1, 0)) + abs(cfa(0, 0) - cfa(2, 0)) + abs(cfa(1, 0) - cfa(3, 0)) + abs(cfa(2, 0) - cfa(4, 0));

  let N_est = cfa(0, -1) * 2.0 * lp(0, 0) / (eps + lp(0, 0) + lp(0, -2));
  let S_est = cfa(0, 1) * 2.0 * lp(0, 0) / (eps + lp(0, 0) + lp(0, 2));
  let W_est = cfa(-1, 0) * 2.0 * lp(0, 0) / (eps + lp(0, 0) + lp(-2, 0));
  let E_est = cfa(1, 0) * 2.0 * lp(0, 0) / (eps + lp(0, 0) + lp(2, 0));

  let v_est = clamp((S_grad * N_est + N_grad * S_est) / (N_grad + S_grad), 0.0, 65534.0);
  let h_est = clamp((W_grad * E_est + E_grad * W_est) / (E_grad + W_grad), 0.0, 65534.0);
  textureStore(g_out, ipos, vec4f(mix(v_est, h_est, vh_discr) * P.wb.g, 0.0, 0.0, 0.0));
}
