// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_color::lut::LookParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct LookParams {
  // Develop's display-linear primaries → the LUT space's, linear; and back.
  to_lut: mat3x3f,
  from_lut: mat3x3f,
  // The table's input domain, from and to; `domain_min.w` is the amount (0…1), `domain_max.w` the encoding
  // (0 the sRGB curve, 1 γ 2.4).
  domain_min: vec4f,
  domain_max: vec4f,
  // x: the table's size on a side; y, z, w unused.
  size: vec4f,
}
