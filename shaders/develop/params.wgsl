// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Uniform block for the develop modules. Layout (272 bytes) is derived on the CPU by encase; see crates/color.

struct DevelopParams {
  wb_mul: vec3f,             // camera-space white balance multipliers, green = 1
  exposure: f32,             // 2^EV
  cam_to_work: mat3x3f,      // camera RGB -> linear Rec.2020, recomputed on every WB change
  work_to_disp: mat3x3f,     // linear Rec.2020 -> display primaries (sRGB or Display P3)
  tone: vec4f,               // RAW: black EV, white EV, steepness, pivot · developed: blacks, whites, contrast, knee
  hl_strength: f32,          // highlight blend strength
  display_referred: f32,     // 0 RAW (scene-referred), 1 JPEG/HEIC/PNG/TIFF (display-referred)
  _pad1: vec2f,
  mixer: array<vec4f, 8>,    // per band: hue shift (radians), chroma gain - 1, lightness shift, unused
}
