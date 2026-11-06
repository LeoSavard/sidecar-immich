// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_engine::brush::CompositeParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct CompositeParams {
  // The target, in pixels.
  size: vec2f,
  // 0 paint, 1 erase, 2 copy the base.
  mode: u32,
  _pad: u32,
  // Copying the base: `MaskRange::uniform` — x..y the values kept, z the feather beyond each end, w 1 to
  // apply (0: as is).
  range: vec4f,
}
