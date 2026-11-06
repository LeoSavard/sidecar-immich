// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_color::params::MaskParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.
//! requires: uniforms/mask_layer.wgsl

struct MaskParams {
  // xy: where the texture being developed sits in the frame the masks cover, in its pixels; zw: that
  // frame's size (0 for the texture's own size — a whole image). A loupe tile sets both.
  frame: vec4f,
  count: u32,
  _pad0: u32,
  _pad1: u32,
  _pad2: u32,
  layers: array<MaskLayer, 8>,
}
