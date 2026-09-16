// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// sRGB transfer function (IEC 61966-2-1), shared by sRGB and Display P3 output.

fn srgb_encode1(x: f32) -> f32 {
  let v = clamp(x, 0.0, 1.0);
  if (v <= 0.0031308) { return 12.92 * v; }
  return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

fn srgb_decode1(x: f32) -> f32 {
  let v = clamp(x, 0.0, 1.0);
  if (v <= 0.04045) { return v / 12.92; }
  return pow((v + 0.055) / 1.055, 2.4);
}

fn srgb_encode(c: vec3f) -> vec3f {
  return vec3f(srgb_encode1(c.r), srgb_encode1(c.g), srgb_encode1(c.b));
}
