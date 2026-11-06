// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Burt and Adelson's pyramid ("The Laplacian Pyramid as a Compact Image Code", IEEE Trans. Communications,
// 1983): the 5-tap binomial kernel [1 4 6 4 1] / 16, separable. Used to reduce (blur, then keep every other
// sample) and to expand (insert zeros, then blur, times 4) — the pair whose difference is a Laplacian level,
// and whose sum restores the finer level exactly.

fn pyr_weight(k: i32) -> f32 {
  switch (abs(k)) {
    case 0: { return 0.375; }
    case 1: { return 0.25; }
    default: { return 0.0625; }
  }
}

// Clamp to the image: the border repeats, which keeps a flat image flat at every level.
fn pyr_clamp(p: vec2i, size: vec2u) -> vec2i {
  return clamp(p, vec2i(0), vec2i(size) - vec2i(1));
}
