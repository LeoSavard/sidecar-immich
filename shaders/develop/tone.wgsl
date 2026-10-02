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

/// Middle grey, sRGB-encoded: where Contrast pivots on a developed file, as the RAW curve pivots on it too.
const GREY_ENC = 0.4613561295;
/// How far Blacks and Whites at ±100 move a developed file's black and white points, in encoded units.
const BLACKS_RANGE = 0.1;
const WHITES_RANGE = 0.2;

// Module 4, for a developed file (JPEG, HEIC, PNG, TIFF): its luminance is already display-linear — the camera
// or app that wrote it chose the tone curve — so nothing here re-renders it, and at default settings every
// step is the identity. Worked on sRGB-encoded luminance, the space the file's own tones were chosen in; RGB
// is scaled by the luminance ratio, as for a RAW, so hue is kept. This returns the encoded value the user's
// curve then takes; `tone_apply` finishes.
fn tone_display_curve_in(c: vec3f, tone: vec4f) -> f32 {
  let y = max(dot(c, LUMA_2020), 0.0);
  // Not `srgb_encode1`, which clamps at white: exposure can take values past it, and the shoulder below is
  // what brings them back.
  var e = select(12.92 * y, 1.055 * pow(y, 1.0 / 2.4) - 0.055, y > 0.0031308);
  // Shoulder: raising exposure rolls highlights off instead of clipping them flat. The knee sits at white
  // (no shoulder) until exposure is positive; nothing brighter than white exists in the file to recover.
  let knee = tone.w;
  if (e > knee) {
    e = select(1.0, knee + (1.0 - knee) * (1.0 - exp((knee - e) / (1.0 - knee))), knee < 1.0);
  }
  // Blacks and Whites, as levels: positive lifts the black point or clips toward white, negative crushes
  // blacks or dims white. Neither reaches past what the file holds.
  let in_lo = max(-tone.x, 0.0) * BLACKS_RANGE;
  let out_lo = max(tone.x, 0.0) * BLACKS_RANGE;
  let in_hi = 1.0 - max(tone.y, 0.0) * WHITES_RANGE;
  let out_hi = 1.0 + min(tone.y, 0.0) * WHITES_RANGE;
  e = out_lo + (out_hi - out_lo) * clamp((e - in_lo) / (in_hi - in_lo), 0.0, 1.0);
  // Contrast: a power S-curve through middle grey, the identity at exponent 1, with the same slope on both
  // sides of the pivot.
  let g = tone.z;
  if (e < GREY_ENC) {
    e = GREY_ENC * pow(max(e, 0.0) / GREY_ENC, g);
  } else {
    e = 1.0 - (1.0 - GREY_ENC) * pow(max(1.0 - e, 0.0) / (1.0 - GREY_ENC), g);
  }
  return e;
}

// Module 4 for a RAW, up to the user's curve: the encoded value the logistic gives this luminance.
fn tone_map_curve_in(c: vec3f, tone: vec4f) -> f32 {
  let y = dot(c, LUMA_2020);
  if (y <= 1e-10) { return 0.0; }
  let x = clamp((log2(y / 0.18) - tone.x) / (tone.y - tone.x), 0.0, 1.0);
  let s0 = logistic(0.0, tone.z, tone.w);
  let s1 = logistic(1.0, tone.z, tone.w);
  return (logistic(x, tone.z, tone.w) - s0) / (s1 - s0);
}

// Scale `c` to the luminance the curves settled on, `e` display-encoded.
fn tone_apply(c: vec3f, e: f32, developed: bool) -> vec3f {
  let y_out = srgb_decode1(e);
  if (developed) {
    let y = max(dot(c, LUMA_2020), 0.0);
    // Black has no hue to keep, and a ratio can't lift it: Blacks + raises it to a neutral grey.
    if (y <= 1e-10) { return vec3f(y_out); }
    return c * (y_out / y);
  }
  let y = dot(c, LUMA_2020);
  if (y <= 1e-10) { return vec3f(0.0); }
  return c * (y_out / y);
}

fn tone_display(c: vec3f, tone: vec4f, lut: texture_1d<f32>) -> vec3f {
  return tone_apply(c, lut_sample(lut, tone_display_curve_in(c, tone)), true);
}

fn tone_map(c: vec3f, tone: vec4f, lut: texture_1d<f32>) -> vec3f {
  return tone_apply(c, lut_sample(lut, tone_map_curve_in(c, tone)), false);
}
