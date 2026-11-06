// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// A 3D LUT's table, sampled by tetrahedral interpolation (Kasson, Nin, Plouffe and Hafner, 1995): the cube
// around the point is split along its grey diagonal into six tetrahedra, and the point is weighted over the
// four corners of the one it is in, so a grey stays on the table's greys. `sidecar_color::lut::sample` is the
// same on the CPU.

fn lut3d_at(table: texture_3d<f32>, p: vec3u) -> vec3f {
  return textureLoad(table, p, 0).rgb;
}

// `t` is the input normalised to the table's domain (0…1 on each axis); `n` the table's size on a side.
fn lut3d_tetrahedral(table: texture_3d<f32>, n: u32, t: vec3f) -> vec3f {
  let p = clamp(t, vec3f(0.0), vec3f(1.0)) * f32(n - 1u);
  let base = min(vec3u(floor(p)), vec3u(n - 2u));
  let f = p - vec3f(base);
  let c0 = lut3d_at(table, base);
  let c1 = lut3d_at(table, base + vec3u(1u));
  // The two corners between, by the order of the fractions: first along the largest, then the next.
  var a = vec3u(0u);
  var ab = vec3u(1u);
  var w = vec3f(0.0); // fₐ, f_b, f_c
  if (f.r >= f.g && f.g >= f.b) {
    a = vec3u(1u, 0u, 0u); ab = vec3u(1u, 1u, 0u); w = f.rgb;
  } else if (f.r >= f.b && f.b >= f.g) {
    a = vec3u(1u, 0u, 0u); ab = vec3u(1u, 0u, 1u); w = f.rbg;
  } else if (f.g >= f.r && f.r >= f.b) {
    a = vec3u(0u, 1u, 0u); ab = vec3u(1u, 1u, 0u); w = f.grb;
  } else if (f.g >= f.b && f.b >= f.r) {
    a = vec3u(0u, 1u, 0u); ab = vec3u(0u, 1u, 1u); w = f.gbr;
  } else if (f.b >= f.r && f.r >= f.g) {
    a = vec3u(0u, 0u, 1u); ab = vec3u(1u, 0u, 1u); w = f.brg;
  } else {
    a = vec3u(0u, 0u, 1u); ab = vec3u(0u, 1u, 1u); w = f.bgr;
  }
  let ca = lut3d_at(table, base + a);
  let cab = lut3d_at(table, base + ab);
  return (1.0 - w.x) * c0 + (w.x - w.y) * ca + (w.y - w.z) * cab + w.z * c1;
}
