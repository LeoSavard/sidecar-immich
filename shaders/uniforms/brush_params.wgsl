// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_engine::brush::BrushParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct BrushParams {
  // The target, in pixels.
  size: vec2f,
  // In pixels.
  radius: f32,
  // 0 hard … 1 soft from the centre.
  feather: f32,
  // 0…1.
  flow: f32,
  _pad0: f32,
  _pad1: f32,
  _pad2: f32,
}
