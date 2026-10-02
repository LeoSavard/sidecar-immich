// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: common/transfer.wgsl common/color.wgsl develop/tone.wgsl
// Mask layers (DESIGN.md §15). Each mask weights its layer at every pixel, and the weighted layers add up on
// top of the global edit: white balance as camera-space gains, exposure in stops, the tone map's parameters
// and the colour mixer's blended toward the layer's, each layer's own curve after the global curve, and
// contrast and saturation after the tone map. No masks is exactly the global edit.
// Layout mirrored by hand in crates/color/src/params.rs (MaskParams, 1,568 bytes), checked by naga.

struct MaskLayer {
  gain: vec4f,               // rgb: white-balance gains at full weight; w: exposure in stops
  tone: vec4f,               // x: contrast, y: saturation (−1…1), z: amount (0…1), w: 1 to invert
  tone_map: vec4f,           // the tone map's parameters at full weight, as DevelopParams.tone
  flags: vec4f,              // x: has a curve (row of the layer curves), y: moves the mixer, z: moves the tone map
  mixer: array<vec4f, 8>,    // added to DevelopParams.mixer at full weight
}

struct MaskParams {
  frame: vec4f,              // xy: the developed texture's offset in the masks' frame; zw: that frame's size (0: its own)
  count: u32,
  _pad0: u32,
  _pad1: u32,
  _pad2: u32,
  layers: array<MaskLayer, 8>,
}

// What the layers add up to at one pixel.
struct MaskMix {
  gain: vec3f,
  ev: f32,
  contrast: f32,
  saturation: f32,
  tone: vec4f,
  mixer: array<vec4f, 8>,
  curves: array<f32, 8>,     // each layer's weight on its curve, 0 for a layer without one
}

// `weights[i]` is mask i's value here, 0…1, as sampled; inverting and the amount are applied here. `tone` and
// `mixer` are the global edit's.
fn mask_mix(layers: array<MaskLayer, 8>, count: u32, weights: array<f32, 8>, tone: vec4f, mixer: array<vec4f, 8>) -> MaskMix {
  var m: MaskMix;
  m.gain = vec3f(1.0);
  m.tone = tone;
  m.mixer = mixer;
  for (var i = 0u; i < min(count, 8u); i++) {
    let l = layers[i];
    var w = clamp(weights[i], 0.0, 1.0);
    if (l.tone.w > 0.5) { w = 1.0 - w; }
    w *= l.tone.z;
    if (w <= 0.0) { continue; }
    m.gain *= pow(l.gain.rgb, vec3f(w));
    m.ev += w * l.gain.w;
    m.contrast += w * l.tone.x;
    m.saturation += w * l.tone.y;
    if (l.flags.z > 0.5) { m.tone += w * (l.tone_map - tone); }
    if (l.flags.y > 0.5) {
      for (var b = 0u; b < 8u; b++) { m.mixer[b] += w * l.mixer[b]; }
    }
    if (l.flags.x > 0.5) { m.curves[i] = w; }
  }
  return m;
}

// Row `row` of the layer curves at `v`, linearly between entries (r32float isn't filterable).
fn layer_curve_at(curves: texture_2d<f32>, row: u32, v: f32) -> f32 {
  let last = textureDimensions(curves).x - 1u;
  let f = clamp(v, 0.0, 1.0) * f32(last);
  let i0 = min(u32(floor(f)), last);
  let i1 = min(i0 + 1u, last);
  return mix(textureLoad(curves, vec2u(i0, row), 0).r, textureLoad(curves, vec2u(i1, row), 0).r, f - floor(f));
}

// The layers' curves on a display-encoded value the global curve already shaped, each by its weight.
fn layer_curves(e: f32, curves: texture_2d<f32>, weights: array<f32, 8>, count: u32) -> f32 {
  var out = e;
  for (var i = 0u; i < min(count, 8u); i++) {
    if (weights[i] > 0.0) { out += weights[i] * (layer_curve_at(curves, i, e) - e); }
  }
  return out;
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
