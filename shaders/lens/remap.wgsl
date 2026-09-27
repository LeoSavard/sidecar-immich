// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Where a corrected pixel reads from, per channel. The radial model is described in crates/color/src/lens.rs;
// the entry file looks the curves up in its uniform knots and hands the values in.

struct LensSource {
  red: vec2f,     // frame pixels
  green: vec2f,
  blue: vec2f,
}

// A corrected-image point relative to the optical centre, in half-diagonals, zoom applied: the radius the
// curves are looked up at is its length.
fn lens_u(pixel: vec2f, center: vec2f, half_diag: f32, zoom: f32) -> vec2f {
  return (pixel - center) / half_diag * zoom;
}

// `k` is the curves at |u|: distortion, red CA, blue CA (vignetting unused here).
fn lens_points(u: vec2f, k: vec4f, center: vec2f, half_diag: f32) -> LensSource {
  let g = u * k.x;
  var s: LensSource;
  s.green = center + g * half_diag;
  s.red = center + g * k.y * half_diag;
  s.blue = center + g * k.z * half_diag;
  return s;
}

// Knot position for a radius: integer knot and the fraction towards the next. 64 knots.
fn lens_knot_at(r: f32, r_scale: f32) -> vec2f {
  let x = clamp(r * r_scale, 0.0, 63.0);
  let i = min(floor(x), 62.0);
  return vec2f(i, x - i);
}

// Catmull-Rom weights for the four taps around a sample at fractional offset t.
fn catmull_rom(t: f32) -> vec4f {
  let t2 = t * t;
  let t3 = t2 * t;
  return vec4f(-0.5 * t3 + t2 - 0.5 * t, 1.5 * t3 - 2.5 * t2 + 1.0, -1.5 * t3 + 2.0 * t2 + 0.5 * t, 0.5 * t3 - 0.5 * t2);
}
