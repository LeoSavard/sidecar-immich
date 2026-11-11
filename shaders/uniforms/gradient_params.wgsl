// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_color::params::GradientParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct GradientParams {
  // The target, in pixels.
  size: vec2f,
  // 0 linear, 1 radial.
  kind: u32,
  // Radial: the share of the radius inside which the weight is full.
  inner: f32,
  // Linear: `from` in xy, `to` in zw. Radial: the centre in xy. Target pixels.
  a: vec4f,
  // Radial: the two semi-axes as vectors from the centre, xy and zw. Target pixels.
  b: vec4f,
}
