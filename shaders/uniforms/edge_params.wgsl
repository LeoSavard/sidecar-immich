// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_color::params::EdgeParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct EdgeParams {
  // The target, in pixels.
  size: vec2f,
  // 0 average down, 1 blur along `dir`, 2 threshold at full size.
  mode: u32,
  // Mode 0: how many source pixels one texel averages, each way.
  scale: f32,
  // Mode 1: one texel across (1, 0) or down (0, 1).
  dir: vec2f,
  // Mode 1: the Gaussian's reach in texels (sigma is half of it).
  radius: f32,
  _pad: f32,
  // Mode 2: x the threshold (0.5 keeps the edge where it was; lower grows the mask), y the ramp's width.
  ramp: vec4f,
}
