// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Module 2 — exposure. No clamp: values above 1.0 are real scene range for the tone map to compress.

fn exposure(c: vec3f, gain: f32) -> vec3f {
  return c * gain;
}

// Where the proxy's alpha says source pixels were clipped at the sensor, move toward a neutral of the
// same mean level. This suppresses the magenta cast white balance multipliers put on reconstructed highlights.
fn highlight_blend(c: vec3f, clip: f32, strength: f32) -> vec3f {
  let t = clamp(clip * strength, 0.0, 1.0);
  return mix(c, vec3f((c.r + c.g + c.b) / 3.0), t);
}
