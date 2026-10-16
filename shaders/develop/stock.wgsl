// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// A named stock's curve (DESIGN.md §7, "Stocks"; `sidecar_color::stock::Curve`): through middle grey with slope 1,
// bent below its toe into the fog its deepest shadows flatten into, and over above its shoulder. On the film's
// exposure, after grain, before the tone map.

fn stock_softplus(u: f32) -> f32 {
  return select(log(1.0 + exp(u)), u + log(1.0 + exp(-u)), u > 0.0);
}

fn stock_sigmoid(u: f32) -> f32 {
  return 1.0 / (1.0 + exp(-u));
}

// The curve's shape in stops: the toe's softplus less the shoulder's. `s` is `DevelopParams.stock`, `h` its
// `shoulder`.
fn stock_shape(x: f32, s: vec4f, h: vec4f) -> f32 {
  return s.z * stock_softplus((x - s.y) / s.z) - s.w * h.y * stock_softplus((x - h.x) / h.y);
}

fn stock_print(e: f32, s: vec4f, h: vec4f) -> f32 {
  let x = log2(max(e, 1e-10) / 0.18);
  let slope = stock_sigmoid(-s.y / s.z) - s.w * stock_sigmoid(-h.x / h.y);
  return 0.18 * exp2((stock_shape(x, s, h) - stock_shape(0.0, s, h)) / slope);
}
