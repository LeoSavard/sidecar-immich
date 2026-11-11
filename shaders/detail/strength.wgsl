// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/detail_layers.wgsl
// Detail per layer (DESIGN.md §15, "Detail per layer"): the strength of luminance noise reduction (r) and of
// sharpening (g) at each pixel, 0…1 — the global settings plus each mask layer's own, weighted by its mask as
// develop weighs it (inverted, scaled by its amount). The masks are in develop's frame; this samples them at the
// same place in the frame, which is the lens correction's few pixels away near the corners of a distorting lens.

@group(0) @binding(0) var<uniform> L: DetailLayers;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var mask0: texture_2d<f32>;
@group(0) @binding(3) var mask1: texture_2d<f32>;
@group(0) @binding(4) var mask2: texture_2d<f32>;
@group(0) @binding(5) var mask3: texture_2d<f32>;
@group(0) @binding(6) var mask4: texture_2d<f32>;
@group(0) @binding(7) var mask5: texture_2d<f32>;
@group(0) @binding(8) var mask6: texture_2d<f32>;
@group(0) @binding(9) var mask7: texture_2d<f32>;
@group(0) @binding(10) var dst: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let uv = (vec2f(id.xy) + 0.5 + L.frame.xy) / L.frame.zw;
  let m = array<f32, 8>(
    textureSampleLevel(mask0, samp, uv, 0.0).r,
    textureSampleLevel(mask1, samp, uv, 0.0).r,
    textureSampleLevel(mask2, samp, uv, 0.0).r,
    textureSampleLevel(mask3, samp, uv, 0.0).r,
    textureSampleLevel(mask4, samp, uv, 0.0).r,
    textureSampleLevel(mask5, samp, uv, 0.0).r,
    textureSampleLevel(mask6, samp, uv, 0.0).r,
    textureSampleLevel(mask7, samp, uv, 0.0).r,
  );
  var s = L.base.xy;
  for (var i = 0u; i < 8u; i++) {
    let l = L.layers[i];
    var w = clamp(m[i], 0.0, 1.0);
    if (l.w > 0.5) { w = 1.0 - w; }
    s += w * l.z * l.xy;
  }
  s = clamp(s, vec2f(0.0), vec2f(1.0));
  textureStore(dst, id.xy, vec4f(s, 0.0, 1.0));
}
