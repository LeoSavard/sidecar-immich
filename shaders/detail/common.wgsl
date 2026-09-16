// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Uniforms for the detail passes (denoise and deconvolution). Mirrored by DetailParams in crates/engine/src/detail.rs.

struct DetailParams {
  size: vec2u,
  step: u32,     // à-trous dilation (1, 2, 4, …) or blur radius
  axis: u32,     // 0 = horizontal, 1 = vertical
  a: f32,        // pass-specific
  b: f32,
  c: f32,
  d: f32,
}

fn clamp_px(p: vec2i, size: vec2u) -> vec2i {
  return clamp(p, vec2i(0), vec2i(size) - vec2i(1));
}

const STAB_EPS = 0.0005;
