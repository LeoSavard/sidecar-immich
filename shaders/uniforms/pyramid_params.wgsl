// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_engine::pyramid::PyramidParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct PyramidParams {
  // The finer level's size, in texels.
  fine: vec2u,
  // The coarser level's size, in texels.
  coarse: vec2u,
  // Expansion only: 0 write the expansion, 1 the fine level minus it (a Laplacian level), 2 the fine level
  // plus it (collapse).
  mode: u32,
  _pad0: u32,
  _pad1: u32,
  _pad2: u32,
}
