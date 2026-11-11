// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_color::params::ProfileParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct ProfileParams {
  // White-balanced camera RGB → linear ProPhoto RGB (D50): the profile's forward matrix, as develop's matrix
  // carries it, taken on into the space the DNG specification applies its tables in.
  to_prophoto: mat3x3f,
  from_prophoto: mat3x3f,
  // The camera multipliers develop applies (`DevelopParams::wb_mul`); w unused.
  wb: vec4f,
  // The HueSatMap's hue, saturation and value divisions (0 for none), and 1 when its values are sRGB-spaced.
  hue_sat: vec4u,
  // The LookTable's, likewise.
  look: vec4u,
  // x: the exposure the LookTable applies at — the file's BaselineExposure and the person's exposure, as the DNG
  // specification has it; y: develop's own (`DevelopParams::exposure`), which the input already carries; z, w
  // unused.
  exposure: vec4f,
}
