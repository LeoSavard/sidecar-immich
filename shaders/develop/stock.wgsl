// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// A named stock's toe (DESIGN.md §7, "Stocks"; `sidecar_color::stock`): the straight line through middle grey,
// bent below `toe.y` stops, over `toe.z` stops, into the fog its deepest shadows flatten into. On the film's
// exposure, after grain, before the tone map.

fn stock_softplus(u: f32) -> f32 {
  return select(log(1.0 + exp(u)), u + log(1.0 + exp(-u)), u > 0.0);
}

fn stock_print(e: f32, toe: vec4f) -> f32 {
  let x = log2(max(e, 1e-10) / 0.18);
  let at = toe.y;
  let w = toe.z;
  return 0.18 * exp2(w * (stock_softplus((x - at) / w) - stock_softplus(-at / w)));
}
