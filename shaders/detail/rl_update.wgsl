// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// estimate ← estimate × (ratio ⊗ PSF*). The Gaussian PSF is symmetric, so PSF* = PSF.

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var estimate: texture_2d<f32>;
@group(0) @binding(2) var correction: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<r32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let p = vec2i(id.xy);
  textureStore(dst, p, vec4f(textureLoad(estimate, p, 0).r * textureLoad(correction, p, 0).r, 0.0, 0.0, 0.0));
}
