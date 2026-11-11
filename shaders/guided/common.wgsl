// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// The guided filter's guide value (He, Sun and Tang, "Guided Image Filtering", ECCV 2010 / TPAMI 2013): one
// scalar per pixel, the luminance of the guide image. In the log domain the regularisation becomes relative
// to the level — a step from 0.01 to 0.02 counts as much as one from 0.5 to 1.0, as it does to the eye and as
// it does physically (both are a doubling of light) — which is what a photo in linear light wants.

// Rec.2020 luminance, as the rest of the pipeline weighs it.
const GUIDED_LUMA = vec3f(0.2627, 0.6780, 0.0593);

// The darkest level the log guide distinguishes: about 12 stops under white.
const GUIDED_LOG_FLOOR = 0.000244140625;

fn guided_guide(rgb: vec3f, log_mode: u32) -> f32 {
  let y = max(dot(rgb, GUIDED_LUMA), 0.0);
  if (log_mode == 1u) {
    return log2(max(y, GUIDED_LOG_FLOOR));
  }
  return y;
}

// The guide's resolution against the statistics grid's, in whole subcells per low texel, at most 4 per axis:
// each low texel averages that many samples, so a subsampled grid takes the mean of its footprint rather
// than one point of it.
fn guided_subcells(guide: vec2u, low: vec2u) -> vec2u {
  let ratio = vec2f(guide) / vec2f(max(low, vec2u(1u)));
  return clamp(vec2u(round(ratio)), vec2u(1u), vec2u(4u));
}
