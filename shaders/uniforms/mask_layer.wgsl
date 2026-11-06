// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_color::params::MaskLayer` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct MaskLayer {
  // rgb: white-balance gains in camera space at full weight; w: exposure in stops.
  gain: vec4f,
  // x: contrast, y: saturation (each −1…1), z: amount (0…1), w: 1 to invert.
  tone: vec4f,
  // The tone map's parameters at full weight, as `DevelopParams::tone`.
  tone_map: vec4f,
  // x: 1 when it has a curve (row `i` of the layer curves), y: 1 when it moves the mixer, z: 1 when it
  // moves the tone map.
  flags: vec4f,
  // Added to `DevelopParams::mixer` at full weight.
  mixer: array<vec4f, 8>,
}
