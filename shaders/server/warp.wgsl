// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: lens/remap.wgsl
// Warp pass (worker): crop, straighten, keystone and quarter turns (DESIGN.md §15), applied to a developed
// texture for export, the fallback preview and loupe tiles. One projective map, computed by
// sidecar_color::geometry, takes each output pixel to where it reads; the editor's view pass applies the same
// map on screen. Bicubic (Catmull-Rom) reads, clamped at the edge.

struct WarpParams {
  m: mat3x3f,            // output pixel centre → input texel coordinates, homogeneous
  out_size: vec2u,
  _pad: vec2u,
}

@group(0) @binding(0) var<uniform> W: WarpParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

fn tap(p: vec2i) -> vec4f {
  let size = vec2i(textureDimensions(src));
  return textureLoad(src, clamp(p, vec2i(0), size - 1), 0);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= W.out_size.x || id.y >= W.out_size.y) { return; }
  let h = W.m * vec3f(vec2f(id.xy) + 0.5, 1.0);
  let p = h.xy / h.z - 0.5;
  let base = floor(p);
  let wx = catmull_rom(p.x - base.x);
  let wy = catmull_rom(p.y - base.y);
  let b = vec2i(base) - 1;
  var acc = vec4f(0.0);
  for (var j = 0; j < 4; j++) {
    var row = vec4f(0.0);
    for (var i = 0; i < 4; i++) {
      row += tap(b + vec2i(i, j)) * wx[i];
    }
    acc += row * wy[j];
  }
  textureStore(dst, vec2i(id.xy), max(acc, vec4f(0.0)));
}
