// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: lens/params.wgsl lens/remap.wgsl
// The lens pass for a mask: the same geometry as client/lens.wgsl (green channel, no vignetting — that would
// change a mask's values rather than move them), as a render pass so it can write the one-channel textures
// masks are kept in. Bilinear reads, which is all a mask needs.

@group(0) @binding(0) var<uniform> L: LensParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var linear_sampler: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let u = lens_u(pos.xy + L.out_offset, L.center, L.half_diag, L.zoom);
  let k = lens_knot_at(length(u), L.r_scale);
  let i = u32(k.x);
  let s = lens_points(u, mix(L.knots[i], L.knots[i + 1u], k.y), L.center, L.half_diag);
  let uv = (s.green - L.in_offset) / vec2f(textureDimensions(src));
  return vec4f(textureSampleLevel(src, linear_sampler, uv, 0.0).r, 0.0, 0.0, 1.0);
}
