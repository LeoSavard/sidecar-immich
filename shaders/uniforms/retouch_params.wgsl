// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_engine::retouch::RetouchParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct RetouchParams {
  // The white balance's camera multipliers, for red eye's test; w unused.
  wb: vec4f,
  // The spot's centre and its source's, in pixels.
  centre: vec2f,
  take_from: vec2f,
  // Pixels.
  radius: f32,
  // 0…1: the share of the radius the edge fades over.
  feather: f32,
  // 0…1.
  opacity: f32,
  // 0 heal, 1 clone, 2 red eye.
  kind: u32,
  // The box this dispatch covers: its corner and size, in pixels.
  origin: vec2u,
  size: vec2u,
}
