// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_color::params::DefringeParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct DefringeParams {
  // Develop's matrix and its inverse: white-balanced camera RGB ↔ linear Rec.2020, where hue is judged.
  cam_to_work: mat3x3f,
  work_to_cam: mat3x3f,
  // Develop's camera multipliers; w unused.
  wb: vec4f,
  // Amount (0…1) and the hue band (radians, from and to); w unused.
  purple: vec4f,
  green: vec4f,
  // x: the reach as a share of the long edge; y, z: the edge's contrast in stops, from and to; w: the whole
  // frame's long edge in pixels, 0 to take the texture's.
  reach: vec4f,
}
