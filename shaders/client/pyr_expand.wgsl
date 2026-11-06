// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/pyramid_params.wgsl pyramid/common.wgsl
// Pyramid: the coarse level expanded to the fine level's size — each fine texel the binomially weighted sum of
// the coarse texels whose doubled position lands within two of it, the weights renormalised per parity so a
// flat level expands to the same flat level. `P.mode` says what to do with it:
//   0  write the expansion;
//   1  write `fine` − expansion: one Laplacian level;
//   2  write `fine` + expansion: collapse, `fine` being a Laplacian level.

@group(0) @binding(0) var<uniform> P: PyramidParams;
@group(0) @binding(1) var coarse: texture_2d<f32>;
@group(0) @binding(2) var fine: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.fine.x || id.y >= P.fine.y) { return; }
  let p = vec2i(id.xy);
  var acc = vec4f(0.0);
  var norm = 0.0;
  for (var dy = -2; dy <= 2; dy++) {
    for (var dx = -2; dx <= 2; dx++) {
      let q = p + vec2i(dx, dy);
      // Only fine positions that were coarse samples (even ones) carry a value; the rest were the zeros
      // that expansion inserts.
      if ((q.x & 1) != 0 || (q.y & 1) != 0) { continue; }
      let w = pyr_weight(dx) * pyr_weight(dy);
      acc += w * textureLoad(coarse, pyr_clamp(q / 2, P.coarse), 0);
      norm += w;
    }
  }
  let expanded = acc / norm;
  var out = expanded;
  if (P.mode == 1u) {
    out = textureLoad(fine, p, 0) - expanded;
  } else if (P.mode == 2u) {
    out = textureLoad(fine, p, 0) + expanded;
  }
  textureStore(dst, id.xy, out);
}
