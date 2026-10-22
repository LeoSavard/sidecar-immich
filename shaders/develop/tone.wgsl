// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: common/transfer.wgsl common/color.wgsl
// Module 4 — tone map. Luminance is log-encoded between black and white EV, shaped by a normalized
// logistic curve, then by the user's monotone curve (a 4,096-entry LUT over display-encoded values).
// RGB is scaled by the luminance ratio, which keeps channel ratios and therefore hue.

/// Where the fade to neutral begins, in display-linear luminance — about a stop under white. Swept against
/// pictures: starting here clears the coloured band beside a blown area while leaving a bright blue sky and
/// every midtone alone (0.25 took more of the sky with it, 0.75 left the band showing).
const BLEACH_FROM = 0.5;

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

/// The display's headroom above middle grey, in stops: sRGB's white is 1.0 and grey 0.18, so `log2(1/0.18)`.
/// Everything between grey and `whiteEv` has to fit in it, so a shape can only choose where the compression
/// is spent. Mirrors `sidecar_color::tone::HEADROOM_STOPS`.
const HEADROOM_STOPS = 2.47393119;

/// The candidate curve above middle grey, in stops: a straight segment in log-log — the identity in linear
/// light, so a stop of scene light is a stop of picture — meeting a shoulder `1 − (1 − t)ⁿ` that reaches
/// white at `white_ev` with no slope left over. Requiring them to meet without a kink fixes the knee at
/// `(nH − W)/(n − 1)`, so the order is the whole choice. Mirrors `sidecar_color::tone::straight_stops`.
fn straight_stops(u: f32, white_ev: f32, order: f32) -> f32 {
  let w = max(white_ev, 1e-6);
  if (w <= HEADROOM_STOPS) { return u; }
  let n = max(order, 1.000001);
  let k = clamp((n * HEADROOM_STOPS - w) / (n - 1.0), 0.0, HEADROOM_STOPS);
  if (u <= k) { return u; }
  if (u >= w) { return HEADROOM_STOPS; }
  let t = (u - k) / (w - k);
  return k + (HEADROOM_STOPS - k) * (1.0 - pow(1.0 - t, n));
}

/// Contrast above grey. A fixed segment has no steepness of its own for the slider to set, so Contrast
/// grades it the way it grades a developed file: a power S-curve through encoded middle grey, the identity
/// at exponent 1, which leaves grey itself exactly where the segment puts it.
fn contrast_above_grey(e: f32, g: f32) -> f32 {
  return 1.0 - (1.0 - GREY_ENC) * pow(max(1.0 - e, 0.0) / (1.0 - GREY_ENC), g);
}

// Module 4 for a RAW, up to the user's curve: the encoded value the base curve gives this luminance.
// `shape.x` is the shoulder's order above grey, 0 for the logistic an edit from before `PROCESS_VERSION` 6
// renders with; `shape.y` is Contrast as an exponent. Below grey it is the logistic either way.
fn tone_map_curve_in(c: vec3f, tone: vec4f, shape: vec4f) -> f32 {
  let y = dot(c, LUMA_2020);
  if (y <= 1e-10) { return 0.0; }
  if (shape.x > 1.0) {
    let u = log2(y / 0.18);
    if (u >= 0.0) {
      return contrast_above_grey(srgb_encode1(0.18 * exp2(straight_stops(u, tone.y, shape.x))), shape.y);
    }
  }
  let x = clamp((log2(y / 0.18) - tone.x) / (tone.y - tone.x), 0.0, 1.0);
  let s0 = logistic(0.0, tone.z, tone.w);
  let s1 = logistic(1.0, tone.z, tone.w);
  return (logistic(x, tone.z, tone.w) - s0) / (s1 - s0);
}

// Module 4 for a RAW, rendered as film: each channel through the characteristic curve on its own, as each of a
// colour film's three layers has its own, after interlayer effects spread the channels apart in log exposure
// (`spread` stops per stop away from their mean; a neutral has no spread to widen). Returns the encoded value
// per channel that the user's curve then takes; decoding it per channel finishes.
fn film_curve_in(c: vec3f, tone: vec4f, spread: f32) -> vec3f {
  // A channel at zero has no log: hold it four stops under the black point, where the curve is flat anyway.
  let lowest = 0.18 * exp2(tone.x - 4.0);
  let x = log2(max(c, vec3f(lowest)) / 0.18);
  let xs = x + spread * (x - vec3f((x.r + x.g + x.b) / 3.0));
  let t = clamp((xs - vec3f(tone.x)) / (tone.y - tone.x), vec3f(0.0), vec3f(1.0));
  let s0 = logistic(0.0, tone.z, tone.w);
  let s1 = logistic(1.0, tone.z, tone.w);
  return (vec3f(logistic(t.r, tone.z, tone.w), logistic(t.g, tone.z, tone.w), logistic(t.b, tone.z, tone.w)) - vec3f(s0)) / (s1 - s0);
}

// Scale `c` to the luminance the curves settled on, `e` display-encoded.
fn tone_apply(c: vec3f, e: f32, developed: bool, bleach: f32) -> vec3f {
  let y_out = srgb_decode1(e);
  if (developed) {
    let y = max(dot(c, LUMA_2020), 0.0);
    // Black has no hue to keep, and a ratio can't lift it: Blacks + raises it to a neutral grey.
    if (y <= 1e-10) { return vec3f(y_out); }
    return c * (y_out / y);
  }
  let y = dot(c, LUMA_2020);
  if (y <= 1e-10) { return vec3f(0.0); }
  // A detector's receptors saturate one after another, so a bright colour loses its cast before it reaches
  // white. Scaling by the luminance ratio alone holds chromaticity at full strength right up to the ceiling,
  // which leaves a coloured band wherever the picture gets there — so fade toward a neutral of the same
  // luminance over the last stop. `vec3f(y_out)` has exactly that luminance: the weights sum to 1.
  let t = smoothstep(BLEACH_FROM, 1.0, y_out) * bleach;
  return mix(c * (y_out / y), vec3f(y_out), t);
}

fn tone_display(c: vec3f, tone: vec4f, lut: texture_1d<f32>) -> vec3f {
  return tone_apply(c, lut_sample(lut, tone_display_curve_in(c, tone)), true, 0.0);
}

fn tone_map(c: vec3f, tone: vec4f, shape: vec4f, lut: texture_1d<f32>) -> vec3f {
  return tone_apply(c, lut_sample(lut, tone_map_curve_in(c, tone, shape)), false, 0.0);
}
