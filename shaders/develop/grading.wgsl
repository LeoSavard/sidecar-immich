// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: common/color.wgsl
// Colour grading wheels (DESIGN.md §7, "Colour grading"), after the tone map and the mixer: the picture as it
// will be seen, in OKLab. Each wheel adds a fixed (a, b) — a push toward its hue, chroma only — and a lightness
// shift, weighted by how much this pixel belongs to its tonal range. The ranges are smooth weights over OKLab
// lightness that always sum to one: shadows fall from 1 at black to 0 at the crossover `split.y`, highlights
// rise from 0 there to 1 at white, midtones are the rest; `split.z` is the exponent that narrows or widens
// them. The global wheel weighs 1 everywhere. `ColorGrading::uniform` in crates/color packs the values.

fn grade_weights(l: f32, split: vec4f) -> vec3f {
  let m = clamp(split.y, 0.05, 0.95);
  let s = pow(clamp(1.0 - l / m, 0.0, 1.0), split.z);
  let h = pow(clamp((l - m) / (1.0 - m), 0.0, 1.0), split.z);
  return vec3f(s, 1.0 - s - h, h);
}

fn color_grade(c: vec3f, wheels: array<vec4f, 4>, split: vec4f) -> vec3f {
  var lab = rec2020_to_oklab(c);
  let w = grade_weights(clamp(lab.x, 0.0, 1.0), split);
  let shift = w.x * wheels[0].xyz + w.y * wheels[1].xyz + w.z * wheels[2].xyz + wheels[3].xyz;
  // Lightness first, then colour, so a darkened shadow is tinted as the darker tone it now is.
  lab.x = max(lab.x + shift.z, 0.0);
  lab = vec3f(lab.x, lab.y + shift.x, lab.z + shift.y);
  return oklab_to_rec2020(lab);
}
