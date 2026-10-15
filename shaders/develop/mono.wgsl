// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Black and white (DESIGN.md §7, "Black and white"): the exposure a film type gets from a colour, read from the
// table `sidecar_color::spectral::mono_lut` makes over the colour cube (each colour's spectrum, lit by D65, seen
// through the filter and the film's spectral sensitivity). A colour is scaled into the cube by its largest
// channel, which scales the exposure back: light is linear. r32float isn't filterable, so the eight corners are
// read and blended here.

fn mono_exposure(c: vec3f, table: texture_3d<f32>, side: f32) -> f32 {
  let rgb = max(c, vec3f(0.0));
  let m = max(max(rgb.r, rgb.g), rgb.b);
  if (m <= 1e-10) { return 0.0; }
  let last = u32(side) - 1u;
  let p = clamp(rgb / m, vec3f(0.0), vec3f(1.0)) * f32(last);
  let i0 = min(vec3u(floor(p)), vec3u(last));
  let i1 = min(i0 + 1u, vec3u(last));
  let f = p - floor(p);
  let c00 = mix(textureLoad(table, vec3u(i0.x, i0.y, i0.z), 0).r, textureLoad(table, vec3u(i1.x, i0.y, i0.z), 0).r, f.x);
  let c10 = mix(textureLoad(table, vec3u(i0.x, i1.y, i0.z), 0).r, textureLoad(table, vec3u(i1.x, i1.y, i0.z), 0).r, f.x);
  let c01 = mix(textureLoad(table, vec3u(i0.x, i0.y, i1.z), 0).r, textureLoad(table, vec3u(i1.x, i0.y, i1.z), 0).r, f.x);
  let c11 = mix(textureLoad(table, vec3u(i0.x, i1.y, i1.z), 0).r, textureLoad(table, vec3u(i1.x, i1.y, i1.z), 0).r, f.x);
  return m * mix(mix(c00, c10, f.y), mix(c01, c11, f.y), f.z);
}
