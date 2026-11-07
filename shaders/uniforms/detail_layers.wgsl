// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_engine::detail::DetailLayers` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct DetailLayers {
  // Per mask slot: luminance noise reduction and sharpening to add (−1…1), the layer's amount (0…1), and 1
  // when it is inverted. All zero for a slot without a layer.
  layers: array<vec4f, 8>,
  // Where the texture being detailed sits in the masks' frame: x, y, and the frame's width and height.
  frame: vec4f,
  // The global noise reduction and sharpening (0…1), unused ×2.
  base: vec4f,
}
