// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: common/transfer.wgsl common/hash.wgsl
// View pass: developedTex (display-linear) → canvas. Pan/zoom geometry, clip overlays, sRGB encoding and
// triangular dither. Runs on view changes and after each develop; never touches the develop uniforms.

struct ViewParams {
  uv_from_px: mat3x3f,   // canvas pixel → developed texture uv (pan, zoom; crop and straighten later)
  flags: u32,            // bit 0 clip highlights, bit 1 clip shadows, bit 2 focus peaking, bit 3 show a mask, bit 4 it is inverted
  hi_t: f32,             // display-linear threshold for "at white"
  lo_t: f32,             // display-linear threshold for "at black"
  background: f32,       // encoded grey outside the image
  peak_t: f32,           // focus_mask threshold (0…1) that counts as in focus
}

const CLIP_HIGHS = 1u;
const CLIP_LOWS = 2u;
const PEAKING = 4u;
const SHOW_MASK = 8u;
const MASK_INVERTED = 16u;

const PEAK_COLOUR = vec3f(0.98, 0.86, 0.15);

@group(0) @binding(0) var<uniform> V: ViewParams;
@group(0) @binding(1) var developed: texture_2d<f32>;
@group(0) @binding(2) var bilinear: sampler;
// Worker-computed sharpness (Laplacian energy over full-resolution luminance, normalized by local contrast,
// max-pooled to this proxy's resolution): see docs/ROADMAP.md, "Focus peaking measures the wrong thing."
@group(0) @binding(3) var focus_mask: texture_2d<f32>;
// The mask being edited, in the developed texture's frame (DESIGN.md §15).
@group(0) @binding(4) var overlay_mask: texture_2d<f32>;

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
  var c = textureSampleLevel(developed, bilinear, uv, 0.0).rgb;
  if ((V.flags & SHOW_MASK) != 0u) {
    // What the mask leaves out goes grey and dim, so what it covers stands out in its own colours.
    var w = textureSampleLevel(overlay_mask, bilinear, uv, 0.0).r;
    if ((V.flags & MASK_INVERTED) != 0u) { w = 1.0 - w; }
    c = mix(vec3f(dot(c, vec3f(0.2126, 0.7152, 0.0722)) * 0.35), c, w);
  }
  let m = max(c.r, max(c.g, c.b));
  if ((V.flags & CLIP_HIGHS) != 0u && m >= V.hi_t) { return vec4f(1.0, 0.0, 0.0, 1.0); }
  if ((V.flags & CLIP_LOWS) != 0u && m <= V.lo_t) { return vec4f(0.0, 0.0, 1.0, 1.0); }
  if ((V.flags & PEAKING) != 0u) {
    if (textureSampleLevel(focus_mask, bilinear, uv, 0.0).r >= V.peak_t) { return vec4f(PEAK_COLOUR, 1.0); }
  }
  let p = vec2u(pos.xy);
  let noise = (hash_unit(p, 1u) - hash_unit(p, 2u)) / 255.0;
  return vec4f(srgb_encode(c) + noise, 1.0);
}
