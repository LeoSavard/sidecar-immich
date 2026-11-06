// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_color::depth::DepthRefineParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct DepthRefineParams {
  // The distance between taps, in the texture being refined: one texel of the model's own map.
  spacing: f32,
  // The luminance difference that halves a tap's weight, in the photo's linear light: `REFINE_SIGMA`.
  sigma_range: f32,
  _pad0: f32,
  _pad1: f32,
}
