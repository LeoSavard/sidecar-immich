// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/gradient_params.wgsl
// A gradient layer's mask (DESIGN.md §15, "Gradients"), drawn analytically into an r8unorm target — the base
// the layer's range, strokes and edge then apply to, as a model's mask is for other layers. The weight falls
// linearly with position across the transition: a layer adds stops weighted by its mask, so a linear weight is
// a density linear in position, which is what a soft graduated neutral-density filter is.
// `sidecar_color::params::Gradient::weight` is the same arithmetic.
//
// Linear (kind 0): full at and before `a.xy`, none at and beyond `a.zw`, along the line between them.
// Radial (kind 1): centre `a.xy`, semi-axes `b.xy` and `b.zw` as vectors; full inside `inner` of the radius,
// none at the ellipse.

@group(0) @binding(0) var<uniform> G: GradientParams;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let p = pos.xy;
  var w = 0.0;
  if (G.kind == 0u) {
    let d = G.a.zw - G.a.xy;
    let t = dot(p - G.a.xy, d) / max(dot(d, d), 1e-12);
    w = 1.0 - clamp(t, 0.0, 1.0);
  } else {
    let v = p - G.a.xy;
    let pa = dot(v, G.b.xy) / max(dot(G.b.xy, G.b.xy), 1e-12);
    let pb = dot(v, G.b.zw) / max(dot(G.b.zw, G.b.zw), 1e-12);
    let r = sqrt(pa * pa + pb * pb);
    if (G.inner >= 1.0) {
      w = select(0.0, 1.0, r <= 1.0);
    } else {
      w = clamp(1.0 - (r - G.inner) / (1.0 - G.inner), 0.0, 1.0);
    }
  }
  return vec4f(w, 0.0, 0.0, 1.0);
}
