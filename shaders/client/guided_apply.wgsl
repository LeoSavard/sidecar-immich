// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/guided_params.wgsl guided/common.wgsl
// Guided filter, the last pass: q = mean(a)·I + mean(b) at the output's own resolution. The coefficients are
// smooth by construction — box means of box means — so upsampling them bilinearly loses nothing, and the
// guide supplies the full-resolution edges (the fast guided filter's point). The coefficient texture is
// rgba32float, which is not filterable everywhere, so the bilinear tap is done by hand.

@group(0) @binding(0) var<uniform> G: GuidedParams;
@group(0) @binding(1) var guide: texture_2d<f32>;
@group(0) @binding(2) var coef: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
@group(0) @binding(4) var dst: texture_storage_2d<rgba16float, write>;

fn coef_at(p: vec2f) -> vec2f {
  let last = vec2i(G.low) - 1;
  let f = p - 0.5;
  let base = vec2i(floor(f));
  let t = f - floor(f);
  let c00 = textureLoad(coef, clamp(base, vec2i(0), last), 0).xy;
  let c10 = textureLoad(coef, clamp(base + vec2i(1, 0), vec2i(0), last), 0).xy;
  let c01 = textureLoad(coef, clamp(base + vec2i(0, 1), vec2i(0), last), 0).xy;
  let c11 = textureLoad(coef, clamp(base + vec2i(1, 1), vec2i(0), last), 0).xy;
  return mix(mix(c00, c10, t.x), mix(c01, c11, t.x), t.y);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let uv = (vec2f(id.xy) + 0.5) / vec2f(size);
  let i = guided_guide(textureSampleLevel(guide, samp, uv, 0.0).rgb, G.log_guide);
  let ab = coef_at(uv * vec2f(G.low));
  let q = ab.x * i + ab.y;
  textureStore(dst, id.xy, vec4f(q, q, q, 1.0));
}
