// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: common/transfer.wgsl common/hash.wgsl
// View pass: developedTex (display-linear) → canvas. Pan/zoom geometry, clip overlays, sRGB encoding and
// triangular dither. Runs on view changes and after each develop; never touches the develop uniforms.

struct ViewParams {
  uv_from_px: mat3x3f,   // canvas pixel → developed texture uv (pan, zoom; crop and straighten later)
  flags: u32,            // bit 0 clip highlights, bit 1 clip shadows, bit 2 focus peaking
  hi_t: f32,             // display-linear threshold for "at white"
  lo_t: f32,             // display-linear threshold for "at black"
  background: f32,       // encoded grey outside the image
  peak_t: f32,           // encoded-luminance gradient that counts as in focus
}

const CLIP_HIGHS = 1u;
const CLIP_LOWS = 2u;
const PEAKING = 4u;

// The developed texture is already in display primaries, so display luminance weights apply.
const LUMA_DISPLAY = vec3f(0.2126, 0.7152, 0.0722);
const PEAK_COLOUR = vec3f(0.98, 0.86, 0.15);

fn luma_encoded(uv: vec2f) -> f32 {
  return srgb_encode1(dot(textureSampleLevel(developed, bilinear, uv, 0.0).rgb, LUMA_DISPLAY));
}

// Central-difference gradient of encoded luminance, one step apart. The step is the larger of a texel and a
// canvas pixel, so zooming out averages detail away instead of aliasing it into noise.
fn focus_energy(uv: vec2f, step: vec2f) -> f32 {
  let gx = luma_encoded(uv + vec2f(step.x, 0.0)) - luma_encoded(uv - vec2f(step.x, 0.0));
  let gy = luma_encoded(uv + vec2f(0.0, step.y)) - luma_encoded(uv - vec2f(0.0, step.y));
  return sqrt(gx * gx + gy * gy);
}

@group(0) @binding(0) var<uniform> V: ViewParams;
@group(0) @binding(1) var developed: texture_2d<f32>;
@group(0) @binding(2) var bilinear: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let uv = (V.uv_from_px * vec3f(pos.xy, 1.0)).xy;
  if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0) {
    return vec4f(vec3f(V.background), 1.0);
  }
  let c = textureSampleLevel(developed, bilinear, uv, 0.0).rgb;
  let m = max(c.r, max(c.g, c.b));
  if ((V.flags & CLIP_HIGHS) != 0u && m >= V.hi_t) { return vec4f(1.0, 0.0, 0.0, 1.0); }
  if ((V.flags & CLIP_LOWS) != 0u && m <= V.lo_t) { return vec4f(0.0, 0.0, 1.0, 1.0); }
  if ((V.flags & PEAKING) != 0u) {
    let step = max(1.0 / vec2f(textureDimensions(developed)), vec2f(V.uv_from_px[0][0], V.uv_from_px[1][1]));
    if (focus_energy(uv, step) >= V.peak_t) { return vec4f(PEAK_COLOUR, 1.0); }
  }
  let p = vec2u(pos.xy);
  let noise = (hash_unit(p, 1u) - hash_unit(p, 2u)) / 255.0;
  return vec4f(srgb_encode(c) + noise, 1.0);
}
