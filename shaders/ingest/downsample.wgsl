// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: ingest/common.wgsl
// 2× box reduction in linear light, alpha averaged (clip coverage becomes a fraction).

@group(0) @binding(0) var<uniform> P: IngestParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.out_size.x || id.y >= P.out_size.y) { return; }
  let o = 2 * vec2i(id.xy);
  var acc = vec4f(0.0);
  for (var j = 0; j <= 1; j++) {
    for (var i = 0; i <= 1; i++) {
      acc += textureLoad(src, clamp_px(o + vec2i(i, j), P.in_size), 0);
    }
  }
  textureStore(dst, vec2i(id.xy), acc * 0.25);
}
