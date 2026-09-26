// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Output transform: working space to display primaries, then a smooth shoulder above 0.9.

fn soft_clip1(x: f32) -> f32 {
  if (x <= 0.9) { return max(x, 0.0); }
  return 0.9 + 0.1 * tanh((x - 0.9) / 0.1);
}

fn output_transform(c: vec3f, work_to_disp: mat3x3f, display_referred: bool) -> vec3f {
  let d = work_to_disp * c;
  // A developed file's values were chosen for this range by whatever wrote it; rolling off its top tenth
  // would dim every highlight the file already placed.
  if (display_referred) { return clamp(d, vec3f(0.0), vec3f(1.0)); }
  return vec3f(soft_clip1(d.r), soft_clip1(d.g), soft_clip1(d.b));
}
