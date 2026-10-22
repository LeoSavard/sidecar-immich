// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Output transform: working space to display primaries, then the display's own ceiling.

// A detector saturates: past the brightest value a display can show there is nothing left to say, and the
// shoulder that used to roll off the top tenth never reached white at all — a neutral blown pixel came out
// at 252 of 255, and a channel had to reach 1.2 to encode as 255. Clipping per channel also bleaches a
// bright colour toward white the way film and sensors do, as one channel reaches the ceiling before the next.
fn soft_clip1(x: f32) -> f32 {
  return clamp(x, 0.0, 1.0);
}

fn output_transform(c: vec3f, work_to_disp: mat3x3f, display_referred: bool) -> vec3f {
  let d = work_to_disp * c;
  // A developed file's values were chosen for this range by whatever wrote it; rolling off its top tenth
  // would dim every highlight the file already placed.
  if (display_referred) { return clamp(d, vec3f(0.0), vec3f(1.0)); }
  return vec3f(soft_clip1(d.r), soft_clip1(d.g), soft_clip1(d.b));
}
