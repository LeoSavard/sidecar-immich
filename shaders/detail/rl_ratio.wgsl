// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// Richardson–Lucy (Richardson 1972; Lucy 1974): observed / (estimate ⊗ PSF), damped to [0.25, 4].

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var observed: texture_2d<f32>;
@group(0) @binding(2) var reblurred: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<r32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let p = vec2i(id.xy);
  let o = textureLoad(observed, p, 0).r;
  let b = textureLoad(reblurred, p, 0).r;
  textureStore(dst, p, vec4f(clamp(o / max(b, 1e-6), 0.25, 4.0), 0.0, 0.0, 0.0));
}
