// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: lens/params.wgsl lens/remap.wgsl
// Lens pass: distortion, lateral chromatic aberration and vignetting, from camera-native linear RGBA into a
// texture of the output's size. Runs before develop — in the browser only when a lens setting changes, on the
// server after denoise and sharpening. Bicubic (Catmull-Rom) reads, clamped at the image edge.

@group(0) @binding(0) var<uniform> L: LensParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

fn curves(r: f32) -> vec4f {
  let k = lens_knot_at(r, L.r_scale);
  let i = u32(k.x);
  return mix(L.knots[i], L.knots[i + 1u], k.y);
}

fn tap(p: vec2i) -> vec4f {
  let size = vec2i(textureDimensions(src));
  return textureLoad(src, clamp(p, vec2i(0), size - 1), 0);
}

// Frame pixel → bicubic read of the input texture. Negative lobes can ring below zero at hard edges; light
// can't, so the result is floored there.
fn read(frame: vec2f) -> vec4f {
  let p = frame - L.in_offset - 0.5;
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
  return max(acc, vec4f(0.0));
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let u = lens_u(vec2f(id.xy) + 0.5 + L.out_offset, L.center, L.half_diag, L.zoom);
  let s = lens_points(u, curves(length(u)), L.center, L.half_diag);
  let g = read(s.green);
  var c = g;
  if ((L.flags & 2u) != 0u) {
    c.r = read(s.red).r;
    c.b = read(s.blue).b;
  }
  // Vignetting happens on the sensor: its gain is a function of the radius the pixel is read from.
  let gain = curves(length(s.green - L.center) / L.half_diag).w;
  textureStore(dst, vec2i(id.xy), vec4f(c.rgb * gain, clamp(g.a, 0.0, 1.0)));
}
