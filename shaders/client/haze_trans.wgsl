// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/haze_params.wgsl
// Dehaze: the coarse transmission from the normalised dark channel's patch minimum, t = 1 − ω·dark (He et al.,
// eq. 12; ω keeps a trace of haze, as distance needs), in every channel of an rgba16float texture for the guided
// filter to refine against the picture.

@group(0) @binding(0) var<uniform> H: HazeParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let t = clamp(1.0 - H.omega * textureLoad(src, id.xy, 0).a, 0.0, 1.0);
  textureStore(dst, id.xy, vec4f(t, t, t, 1.0));
}
