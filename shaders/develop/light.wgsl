// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: common/color.wgsl
// Module 2b — the light controls (DESIGN.md §7, "Light, per pixel"; `sidecar_color::params::light_stops` mirrors
// it): Highlights,
// Shadows and Contrast as functions of each pixel's luminance alone, in stops from middle grey, the same for a
// RAW and a developed file. A pixel is light times colour: its luminance Y and its ratios R/Y, G/Y, B/Y. The
// controls choose a new Y; the colour is then kept either as the light's own (the ratios, as dimming real light
// does) or as how colourful it looks (OKLab a and b, so a darkened colour does not read as a duller one).
//
// Highlights and Shadows each set the slope — the contrast, in stops per stop — of the tones past a ramp: Highlights
// −100 halves it above about two stops over grey, which also brings what lay two stops past white into view;
// Shadows +100 halves it below two stops under grey, lifting the deep shadows while black stays black. The slope
// changes smoothly across the ramp and is never less than half, so tones never turn over, a slider does more the
// further it goes, and grey (u = 0), which both ramps and Contrast leave alone, stays where it is.

/// Where Highlights' and Shadows' ramps begin, in stops from grey, and how wide they are.
const LIGHT_RAMP_FROM = 0.75;
const LIGHT_RAMP_WIDTH = 1.25;

/// The stops a slope change of 1 adds `t` stops past a ramp's start: the integral of the smoothstep that carries
/// the slope from its old value to its new one, then the full change beyond.
fn light_ramp(t: f32) -> f32 {
  let x = clamp(t / LIGHT_RAMP_WIDTH, 0.0, 1.0);
  return LIGHT_RAMP_WIDTH * x * x * x * (1.0 - 0.5 * x) + max(t - LIGHT_RAMP_WIDTH, 0.0);
}

/// `light`: x Highlights' and y Shadows' slope changes past their ramps (−0.5…0.5; positive Shadows lifts), z
/// Contrast as a slope in stops per stop less one, w 1 to keep how colourful a colour looks rather than the
/// light's own colour. All zero is the identity.
fn light_apply(c: vec3f, light: vec4f) -> vec3f {
  if (light.x == 0.0 && light.y == 0.0 && light.z == 0.0) { return c; }
  let y = dot(c, LUMA_2020);
  if (y <= 1e-10) { return c; }
  let u = log2(y / 0.18);
  let v = (1.0 + light.z) * (u + light.x * light_ramp(u - LIGHT_RAMP_FROM) + light.y * light_ramp(-u - LIGHT_RAMP_FROM));
  let lit = c * exp2(v - u);
  if (light.w < 0.5) { return lit; }
  // How colourful it looks: the new lightness, the old a and b.
  let before = rec2020_to_oklab(c);
  let after = rec2020_to_oklab(lit);
  return oklab_to_rec2020(vec3f(after.x, before.y, before.z));
}
