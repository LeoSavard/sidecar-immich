// SPDX-License-Identifier: BSD-2-Clause
// Copyright 2019 Johannes Hanika (vkdt). WGSL port copyright Sidecar contributors.
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that
// the conditions of the BSD 2-Clause licence are met; see NOTICE and THIRD_PARTY_NOTICES.md for the full text.
// Ported from vkdt src/pipe/modules/demosaic/rcd_fill.comp ("rb@br") @ 13756b3e18143f0f468b4a0cf34d6e787fcf2819
//! requires: ingest/rcd_common.wgsl
// Blue at red pixels and red at blue pixels, from the diagonal discriminator. Output: r in .r, b in .b.

@group(0) @binding(0) var<uniform> P: IngestParams;
@group(0) @binding(1) var cfa_tex: texture_2d<f32>;
@group(0) @binding(2) var pq_tex: texture_2d<f32>;
@group(0) @binding(3) var g_tex: texture_2d<f32>;
@group(0) @binding(4) var rb_out: texture_storage_2d<rgba32float, write>;

var<private> ipos: vec2i;
var<private> red: bool;

fn at(t: texture_2d<f32>, x: i32, y: i32) -> f32 {
  return textureLoad(t, clamp_px(ipos + vec2i(x, y), P.in_size), 0).r;
}
fn sg(x: i32, y: i32) -> f32 { return at(g_tex, x, y); }
fn pq(x: i32, y: i32) -> f32 { return at(pq_tex, x, y); }
/// The opposite colour plane: known blue samples around a red pixel, known red samples around a blue one.
fn sc(x: i32, y: i32) -> f32 {
  let q = clamp_px(ipos + vec2i(x, y), P.in_size);
  let c = rgGB(q);
  let v = textureLoad(cfa_tex, q, 0).r;
  if (red) { return select(0.0, v * P.wb.b, c == 2); }
  return select(0.0, v * P.wb.r, c == 0);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.out_size.x || id.y >= P.out_size.y) { return; }
  ipos = vec2i(id.xy);
  let c = rgGB(ipos);
  if (c == 1) {
    textureStore(rb_out, ipos, vec4f(0.0));
    return;
  }
  red = c == 0;
  let own = at(cfa_tex, 0, 0) * select(P.wb.b, P.wb.r, red);
  let eps = 1e-5;
  let pqc = pq(0, 0);
  let pqn = 0.25 * (pq(-1, -1) + pq(1, -1) + pq(-1, 1) + pq(1, 1));
  let pq_discr = discriminator(pqc, pqn);
  let NW_grad = eps + abs(sc(-1, -1) - sc(1, 1)) + abs(sc(-1, -1) - sc(-3, -3)) + abs(sg(0, 0) - sg(-2, -2));
  let NE_grad = eps + abs(sc(1, -1) - sc(-1, 1)) + abs(sc(1, -1) - sc(3, -3)) + abs(sg(0, 0) - sg(2, -2));
  let SW_grad = eps + abs(sc(1, -1) - sc(-1, 1)) + abs(sc(-1, 1) - sc(-3, 3)) + abs(sg(0, 0) - sg(-2, 2));
  let SE_grad = eps + abs(sc(-1, -1) - sc(1, 1)) + abs(sc(1, 1) - sc(3, 3)) + abs(sg(0, 0) - sg(2, 2));
  let NW_est = sc(-1, -1) - sg(-1, -1);
  let NE_est = sc(1, -1) - sg(1, -1);
  let SW_est = sc(-1, 1) - sg(-1, 1);
  let SE_est = sc(1, 1) - sg(1, 1);
  let p_est = (NW_grad * SE_est + SE_grad * NW_est) / (NW_grad + SE_grad);
  let q_est = (NE_grad * SW_est + SW_grad * NE_est) / (NE_grad + SW_grad);
  let other = clamp(sg(0, 0) + mix(p_est, q_est, pq_discr), 0.0, 65535.0);
  if (red) {
    textureStore(rb_out, ipos, vec4f(own, 0.0, other, 1.0));
  } else {
    textureStore(rb_out, ipos, vec4f(other, 0.0, own, 1.0));
  }
}
