// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/haze_params.wgsl
// Dehaze without depth (DESIGN.md §7, "Dehaze"): the dark channel prior (He, Sun and Tang, "Single image haze
// removal using dark channel prior", CVPR 2009 / TPAMI 2011) on the scene's light. Koschmieder's model holds in
// linear light — a hazy pixel is I = J·t + A·(1 − t), the scene's own colour J attenuated by the transmission t
// and the airlight A added — which is where Sidecar has the picture before any tone curve.

// A map texel's colour in the working space, white-balanced, before exposure: what the dark channel compares.
fn haze_colour(H: HazeParams, camera: vec3f) -> vec3f {
  return max(H.cam_to_work * (camera * H.wb.rgb), vec3f(0.0));
}
