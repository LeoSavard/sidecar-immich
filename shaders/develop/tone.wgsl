// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: common/transfer.wgsl common/color.wgsl
// Module 4 — tone map. Luminance is log-encoded between black and white EV, shaped by a normalized
// logistic curve, then by the user's monotone curve (a 4,096-entry LUT over display-encoded values).
// RGB is scaled by the luminance ratio, which keeps channel ratios and therefore hue.

fn lut_sample(lut: texture_1d<f32>, v: f32) -> f32 {
  let last = textureDimensions(lut) - 1u;
  let f = clamp(v, 0.0, 1.0) * f32(last);
  let i0 = min(u32(floor(f)), last);
  let i1 = min(i0 + 1u, last);
  return mix(textureLoad(lut, i0, 0).r, textureLoad(lut, i1, 0).r, f - floor(f));
}

fn logistic(x: f32, k: f32, p: f32) -> f32 {
  return 1.0 / (1.0 + exp(-k * (x - p)));
}

fn tone_map(c: vec3f, tone: vec4f, lut: texture_1d<f32>) -> vec3f {
  let y = dot(c, LUMA_2020);
  if (y <= 1e-10) { return vec3f(0.0); }
  let x = clamp((log2(y / 0.18) - tone.x) / (tone.y - tone.x), 0.0, 1.0);
  let s0 = logistic(0.0, tone.z, tone.w);
  let s1 = logistic(1.0, tone.z, tone.w);
  let s = (logistic(x, tone.z, tone.w) - s0) / (s1 - s0);
  let y_out = srgb_decode1(lut_sample(lut, s));
  return c * (y_out / y);
}
