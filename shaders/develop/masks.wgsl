// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: common/transfer.wgsl common/color.wgsl develop/tone.wgsl
// Local adjustments through masks (DESIGN.md §15). Each mask weights its adjustment at every pixel, and the
// weighted adjustments add up on top of the global edit: exposure in stops before the tone map, white balance
// as camera-space gains, contrast and saturation after the tone map. No masks is exactly the global edit.
// Layout mirrored by hand in crates/color/src/params.rs (MaskParams, 288 bytes), checked by naga.

struct MaskAdjust {
  gain: vec4f,               // rgb: white-balance gains at full weight; w: exposure in stops
  tone: vec4f,               // x: contrast, y: saturation (−1…1), z: amount (0…1), w: 1 to invert
}

struct MaskParams {
  frame: vec4f,              // xy: the developed texture's offset in the masks' frame; zw: that frame's size (0: its own)
  count: u32,
  _pad0: u32,
  _pad1: u32,
  _pad2: u32,
  adjust: array<MaskAdjust, 8>,
}

// What the masks add up to at one pixel.
struct MaskMix {
  gain: vec3f,
  ev: f32,
  contrast: f32,
  saturation: f32,
}

// `weights[i]` is mask i's value here, 0…1, as sampled; inverting and the amount are applied here.
fn mask_mix(adjust: array<MaskAdjust, 8>, count: u32, weights: array<f32, 8>) -> MaskMix {
  var m = MaskMix(vec3f(1.0), 0.0, 0.0, 0.0);
  for (var i = 0u; i < min(count, 8u); i++) {
    let a = adjust[i];
    var w = clamp(weights[i], 0.0, 1.0);
    if (a.tone.w > 0.5) { w = 1.0 - w; }
    w *= a.tone.z;
    m.gain *= pow(a.gain.rgb, vec3f(w));
    m.ev += w * a.gain.w;
    m.contrast += w * a.tone.x;
    m.saturation += w * a.tone.y;
  }
  return m;
}

// Local contrast on tone-mapped, display-linear light: a power S-curve through middle grey on encoded
// luminance, as a developed file's Contrast works; `amount` −1…1 is an exponent of ½…2. Hue is kept by scaling
// RGB with the luminance ratio.
fn mask_contrast(c: vec3f, amount: f32) -> vec3f {
  if (amount == 0.0) { return c; }
  let y = dot(c, LUMA_2020);
  if (y <= 1e-10) { return c; }
  let e = srgb_encode1(y);
  let g = exp2(amount);
  var o: f32;
  if (e < GREY_ENC) {
    o = GREY_ENC * pow(e / GREY_ENC, g);
  } else {
    o = 1.0 - (1.0 - GREY_ENC) * pow((1.0 - e) / (1.0 - GREY_ENC), g);
  }
  return c * (srgb_decode1(o) / y);
}

// Local saturation: OKLab chroma scaled by 1 + amount, lightness and hue kept.
fn mask_saturation(c: vec3f, amount: f32) -> vec3f {
  if (amount == 0.0) { return c; }
  let lab = rec2020_to_oklab(c);
  return oklab_to_rec2020(vec3f(lab.x, lab.yz * max(1.0 + amount, 0.0)));
}
