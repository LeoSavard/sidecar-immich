// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: common/color.wgsl
// Module 5 — colour mixer in OKLCh. Eight bands with raised-cosine weights between neighbouring band
// centres; the weights always sum to 1, so there are no seams between bands. Neutrals are left alone.

const MIX_CENTERS = array<f32, 8>(0.436, 0.960, 1.745, 2.443, 3.403, 4.538, 5.149, 5.760);
const TAU = 6.283185307;
const PI = 3.141592654;

fn color_mixer(c: vec3f, mixer: array<vec4f, 8>) -> vec3f {
  let lab = rec2020_to_oklab(c);
  let chroma = length(lab.yz);
  if (chroma < 1e-5) { return c; }
  var h = atan2(lab.z, lab.y);
  if (h < 0.0) { h += TAU; }
  let centers = MIX_CENTERS;
  var adj = vec3f(0.0);
  for (var i = 0u; i < 8u; i++) {
    let j = (i + 1u) % 8u;
    let c0 = centers[i];
    var c1 = centers[j];
    var hh = h;
    if (j == 0u) {
      c1 += TAU;
      if (hh < c0) { hh += TAU; }
    }
    if (hh >= c0 && hh < c1) {
      let t = (hh - c0) / (c1 - c0);
      let w0 = 0.5 + 0.5 * cos(PI * t);
      adj = w0 * mixer[i].xyz + (1.0 - w0) * mixer[j].xyz;
      break;
    }
  }
  let neutral_guard = smoothstep(0.02, 0.08, chroma);
  let hue = h + adj.x * neutral_guard;
  let new_chroma = chroma * max(1.0 + adj.y * neutral_guard, 0.0);
  let lightness = lab.x + adj.z * neutral_guard;
  return oklab_to_rec2020(vec3f(lightness, new_chroma * cos(hue), new_chroma * sin(hue)));
}
