// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// Linear RGB → variance-stabilized opponent channels: y = 2·sqrt(luminance + ε) (Anscombe-style, so photon noise
// becomes roughly signal-independent), chroma differences scaled by the same derivative. Alpha passes through.

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let c = textureLoad(src, vec2i(id.xy), 0);
  let lum = max((c.r + c.g + c.b) / 3.0, 0.0);
  let s = sqrt(lum + STAB_EPS);
  textureStore(dst, vec2i(id.xy), vec4f(2.0 * s, (c.r - c.g) / s, (c.b - c.g) / s, c.a));
}
