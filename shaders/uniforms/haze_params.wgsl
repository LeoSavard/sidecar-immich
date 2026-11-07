// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_engine::haze::HazeParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct HazeParams {
  // Camera RGB → linear Rec.2020, as develop's: the dark channel compares working-space channels.
  cam_to_work: mat3x3f,
  // The white balance's camera multipliers, w unused.
  wb: vec4f,
  // The patch minimum's radius in texels.
  radius: u32,
  // The minimum pass's axis: 0 across, 1 down.
  axis: u32,
  // How much of the haze the coarse transmission accounts for (He et al.'s ω).
  omega: f32,
  _pad: u32,
}
