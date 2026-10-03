// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// The editor's preview while a brush stroke is being painted: the photo developed without the layer and with
// it at full weight, blended by the mask as it is painted. Exact where the mask is 0 or 1 and close in its soft
// edge; the develop pass renders the real thing when the stroke ends. Browser only — nothing exported uses it.

struct PreviewParams {
  invert: u32,           // 1: the layer applies where the mask is not
  _pad0: u32,
  _pad1: u32,
  _pad2: u32,
}

@group(0) @binding(0) var<uniform> P: PreviewParams;
@group(0) @binding(1) var without_layer: texture_2d<f32>;
@group(0) @binding(2) var with_layer: texture_2d<f32>;
// The mask, lens-corrected: the developed picture's frame and size.
@group(0) @binding(3) var mask: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let p = vec2i(pos.xy);
  var m = clamp(textureLoad(mask, p, 0).r, 0.0, 1.0);
  if (P.invert == 1u) { m = 1.0 - m; }
  return mix(textureLoad(without_layer, p, 0), textureLoad(with_layer, p, 0), m);
}
