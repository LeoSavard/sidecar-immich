// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// Luminance of linear RGB, the observation and initial estimate for Richardson–Lucy.

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<r32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let c = textureLoad(src, vec2i(id.xy), 0).rgb;
  textureStore(dst, vec2i(id.xy), vec4f(max((c.r + c.g + c.b) / 3.0, 0.0), 0.0, 0.0, 0.0));
}
