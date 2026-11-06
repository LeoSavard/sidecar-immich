// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_color::geometry::WarpParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct WarpParams {
  // Output pixel (its centre) → input texel coordinates, homogeneous, as WGSL `mat3x3f` columns.
  m: mat3x3f,
  out_size: vec2u,
  _pad0: u32,
  _pad1: u32,
}
