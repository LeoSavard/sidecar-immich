// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_engine::detail::DetailParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct DetailParams {
  size: vec2u,
  // À-trous dilation (1, 2, 4, …) or blur radius.
  step: u32,
  // 0 = horizontal, 1 = vertical.
  axis: u32,
  // Pass-specific, as are `b`, `c` and `d`.
  a: f32,
  b: f32,
  c: f32,
  d: f32,
}
