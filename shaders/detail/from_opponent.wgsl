// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: detail/common.wgsl
// Inverse of to_opponent: coarse residual plus shrunk details, back to linear RGB. With the generalized
// Anscombe transform (`P.c` 1) the luminance comes back through Mäkitalo and Foi's closed-form approximation of
// the exact unbiased inverse (IEEE TIP 2013): the denoised value estimates the *mean* of the transformed noise,
// and inverting it algebraically would darken the shadows, where that mean sits above the transform of the
// mean. `sidecar_engine::noise::inverse` is the same arithmetic.

@group(0) @binding(0) var<uniform> P: DetailParams;
@group(0) @binding(1) var coarse: texture_2d<f32>;
@group(0) @binding(2) var details: texture_2d<f32>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;

fn unbiased(d: f32, a: f32, s2: f32) -> f32 {
  var x = 0.25 * d * d - 0.375 - s2;
  if (d >= 1.0) {
    let r = 1.2247449;
    x = 0.25 * d * d + 0.25 * r / d - 1.375 / (d * d) + 0.625 * r / (d * d * d) - 0.125 - s2;
  }
  return a * max(x, 0.0);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= P.size.x || id.y >= P.size.y) { return; }
  let p = vec2i(id.xy);
  let base = textureLoad(coarse, p, 0);
  let o = base.rgb + textureLoad(details, p, 0).rgb;
  let s = max(o.x / 2.0, 0.0);
  var lum = s * s - STAB_EPS;
  var scale = s;
  if (P.c == 1.0) {
    let a = max(P.a, 1e-12);
    lum = unbiased(o.x, a, P.b / (a * a));
    scale = a * s;
  }
  let c1 = o.y * scale;
  let c2 = o.z * scale;
  let g = lum - (c1 + c2) / 3.0;
  textureStore(dst, p, vec4f(g + c1, g, g + c2, base.a));
}
