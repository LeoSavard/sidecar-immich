// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Colour reversal film, spectrally (DESIGN.md §7, "Colour film, spectrally"): a named stock rendered through
// its own data sheet, from the two tables `sidecar_color::slide` builds. One holds each colour's three layer
// responses over the colour cube (`exposure_lut`: the sheet's spectral sensitivities, under the scene's light,
// at the exposure a meter gives the stock's ISO speed); the other holds the slide's colour over the three
// layers' log H (`response_lut`: the characteristic curves, the dyes they call for, and the light table,
// in log2 — densities interpolate far better in log than linear values do). Neither table is filterable
// (rgba32float), so the eight corners are read and blended here, as `mono.wgsl` does.

// A decade in log2, for log10 — which WGSL has no function for.
const SLIDE_LOG10: f32 = 0.30102999566398;

// Trilinear blend of a table of `side`³ vec4 entries at `p`, in lattice units.
fn slide_sample(table: texture_3d<f32>, side: f32, p: vec3f) -> vec3f {
  let last = u32(side) - 1u;
  let q = clamp(p, vec3f(0.0), vec3f(side - 1.0));
  let i0 = min(vec3u(floor(q)), vec3u(last));
  let i1 = min(i0 + 1u, vec3u(last));
  let f = q - floor(q);
  let c00 = mix(textureLoad(table, vec3u(i0.x, i0.y, i0.z), 0).rgb, textureLoad(table, vec3u(i1.x, i0.y, i0.z), 0).rgb, f.x);
  let c10 = mix(textureLoad(table, vec3u(i0.x, i1.y, i0.z), 0).rgb, textureLoad(table, vec3u(i1.x, i1.y, i0.z), 0).rgb, f.x);
  let c01 = mix(textureLoad(table, vec3u(i0.x, i0.y, i1.z), 0).rgb, textureLoad(table, vec3u(i1.x, i0.y, i1.z), 0).rgb, f.x);
  let c11 = mix(textureLoad(table, vec3u(i0.x, i1.y, i1.z), 0).rgb, textureLoad(table, vec3u(i1.x, i1.y, i1.z), 0).rgb, f.x);
  return mix(mix(c00, c10, f.y), mix(c01, c11, f.y), f.z);
}

// The slide's colour, linear Rec.2020 on the display with the light table's white at 1, for the light `c` the
// film is exposed to (linear Rec.2020, white-balanced, middle grey 0.18). `slide` is [amount, the exposure
// table's side, the response table's side, unused]; `log_h` is where a layer response of 1 lands in the
// response table, per layer, and a decade of log H, both as fractions of that table's domain
// (`slide::SlideProfile`).
fn slide_colour(c: vec3f, exposures: texture_3d<f32>, responses: texture_3d<f32>, slide: vec4f, log_h: vec4f) -> vec3f {
  let rgb = max(c, vec3f(0.0));
  let m = max(max(rgb.r, rgb.g), rgb.b);
  // Light is linear, so the table holds a colour's responses per unit of its largest channel: a pixel's are
  // that channel times the table's at its chromaticity.
  var q = vec3f(0.0);
  if (m > 1e-10) {
    q = m * slide_sample(exposures, slide.y, clamp(rgb / m, vec3f(0.0), vec3f(1.0)) * (slide.y - 1.0));
  }
  // Each layer's log H on its own curve. Past either end of the table every curve is flat, which is what
  // clamping to the domain gives — including an unexposed frame, which a reversal stock leaves at full density.
  let t = log_h.xyz + log2(max(q, vec3f(1e-30))) * SLIDE_LOG10 * log_h.w;
  return exp2(slide_sample(responses, slide.z, t * (slide.z - 1.0)));
}
