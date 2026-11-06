// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_engine::guided::GuidedParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct GuidedParams {
  // The statistics grid, in texels.
  low: vec2u,
  // The box radius on that grid, in its texels.
  radius: u32,
  // The box pass's axis: 0 across, 1 down.
  axis: u32,
  // The regularisation ε, in the guide's units squared: a window whose guide varies less than this is
  // treated as flat and smoothed; one that varies more keeps its edge.
  eps: f32,
  // Which channel of the input is filtered, 0…3.
  channel: u32,
  // 1 to guide by log₂ luminance, so ε is relative to the level rather than absolute.
  log_guide: u32,
  _pad: u32,
}
