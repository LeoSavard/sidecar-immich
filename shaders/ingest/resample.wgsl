// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: ingest/common.wgsl
// Final bilinear resample to the exact proxy size (the input is at most 2× larger after box reduction).

@group(0) @binding(0) var<uniform> P: IngestParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var bilinear: sampler;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.out_size.x || id.y >= P.out_size.y) { return; }
  let uv = (vec2f(id.xy) + 0.5) / vec2f(P.out_size);
  textureStore(dst, vec2i(id.xy), textureSampleLevel(src, bilinear, uv, 0.0));
}
