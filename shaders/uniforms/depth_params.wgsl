// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_color::depth::DepthParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct DepthParams {
  // xy: this texture's offset in the depth map's frame, in its pixels; zw: that frame's size.
  frame: vec4f,
  // rgb: the airlight in camera RGB; w: the optical depth at the farthest point, + adds haze, − removes it.
  haze: vec4f,
  // x: the largest blur radius in this texture's pixels; y: focus as nearness 0…1; z: half the in-focus
  // band; w: 1 when blurring.
  blur: vec4f,
  // x: how many of this texture's pixels a low-resolution texel covers, each way.
  scale: vec4f,
}
