// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/guided_params.wgsl
// Guided filter, pass 3 of 4: the local linear model q = a·I + b for each window, from the window means of I,
// p, I·p and I·I (He, Sun and Tang, eq. 5–6): a = cov(I, p) / (var(I) + ε), b = mean(p) − a·mean(I). Where the
// guide is flat within the window (var ≪ ε) a → 0 and q is the window's mean of p — smoothing; where the guide
// has an edge (var ≫ ε) a follows the input's covariance with it, and the edge passes through.

@group(0) @binding(0) var<uniform> G: GuidedParams;
@group(0) @binding(1) var means: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= G.low.x || id.y >= G.low.y) { return; }
  let m = textureLoad(means, id.xy, 0);
  let var_i = max(m.w - m.x * m.x, 0.0);
  let cov = m.z - m.x * m.y;
  let a = cov / (var_i + G.eps);
  let b = m.y - a * m.x;
  textureStore(dst, id.xy, vec4f(a, b, 0.0, 0.0));
}
