// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Uniform block for the develop modules. Layout (400 bytes) is derived on the CPU by encase; see crates/color.

struct DevelopParams {
  wb_mul: vec3f,             // camera-space white balance multipliers, green = 1
  exposure: f32,             // 2^EV
  cam_to_work: mat3x3f,      // camera RGB -> linear Rec.2020, recomputed on every WB change
  work_to_disp: mat3x3f,     // linear Rec.2020 -> display primaries (sRGB or Display P3)
  tone: vec4f,               // RAW: black EV, white EV, steepness, pivot · developed: blacks, whites, contrast, knee
  hl_strength: f32,          // highlight blend strength
  display_referred: f32,     // 0 RAW (scene-referred), 1 JPEG/HEIC/PNG/TIFF (display-referred)
  saturation: f32,           // global chroma gain - 1 (OKLCh), the mask-saturation baseline every layer adds to
  _pad1: f32,
  mixer: array<vec4f, 8>,    // per band: hue shift (radians), chroma gain - 1, lightness shift, unused
  film: vec4f,               // film rendering: amount 0-1 (0 = neutral), interlayer spread in log exposure, unused x2
  mono: vec4f,               // black and white: x 1 when on, y the side of the mono table's cube, z 1 to output the adjacency map's log exposure, unused
  grain: vec4f,              // grain: standard deviation in stops (0 none), size as a share of the long edge, stain masking, unused
  developer: vec4f,          // black and white: adjacency, its reach as a share of the long edge, development contrast, compensation
  stock: vec4f,              // a named stock's curve: x 1 on, y its toe in stops from grey, z the toe's width, w the shoulder's strength
  shoulder: vec4f,           // the stock's shoulder in stops from grey, its width, unused x2
  slide: vec4f,              // colour film, spectrally: amount 0-1 (0 off), the exposure table's side, the response table's side, unused
  slide_log_h: vec4f,        // where a layer response of 1 lands in the response table (per layer) and a decade of log H, as fractions of its domain
}
