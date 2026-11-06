// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/look_params.wgsl common/transfer.wgsl common/lut3d.wgsl
// A 3D LUT as a look (DESIGN.md §15, "3D LUTs"; `sidecar_color::lut`), after develop: display-linear colour into
// the LUT's own space and encoding, held to that space's gamut as the tool that built the table held it, through
// the table, and back. The amount mixes in the LUT's encoding, where a grading tool's opacity does; what the
// gamut held back is returned in proportion to what the LUT left, so an amount of 0 changes nothing.

@group(0) @binding(0) var<uniform> L: LookParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(3) var table: texture_3d<f32>;

fn look_encode(c: vec3f) -> vec3f {
  if (L.domain_max.w > 0.5) { return pow(clamp(c, vec3f(0.0), vec3f(1.0)), vec3f(1.0 / 2.4)); }
  return srgb_encode(c);
}

fn look_decode(e: vec3f) -> vec3f {
  if (L.domain_max.w > 0.5) { return pow(clamp(e, vec3f(0.0), vec3f(1.0)), vec3f(2.4)); }
  return vec3f(srgb_decode1(e.r), srgb_decode1(e.g), srgb_decode1(e.b));
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let p = vec2i(id.xy);
  let here = textureLoad(src, p, 0);
  let inside = clamp(L.to_lut * here.rgb, vec3f(0.0), vec3f(1.0));
  let e = look_encode(inside);
  let t = (e - L.domain_min.xyz) / (L.domain_max.xyz - L.domain_min.xyz);
  let graded = lut3d_tetrahedral(table, u32(L.size.x), t);
  let amount = L.domain_min.w;
  let held = here.rgb - L.from_lut * inside;
  let out = L.from_lut * look_decode(mix(e, graded, amount)) + (1.0 - amount) * held;
  textureStore(dst, p, vec4f(out, here.a));
}
