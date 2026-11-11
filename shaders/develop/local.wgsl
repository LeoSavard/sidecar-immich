// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: guided/common.wgsl
// Texture and Clarity (DESIGN.md §7, "Local contrast"): detail in log luminance at two scales, raised or
// lowered. Each band is the difference between edge-preserving bases — self-guided filters of the source's log
// luminance (He, Sun and Tang), whose coefficients the local-contrast map holds at low resolution and which are
// applied here against this pixel's own value, so edges stay at full resolution and don't halo. The change is
// a gain on all three channels alike: hue and saturation stay; only contrast moves.

// The furthest local contrast moves a pixel, in stops, approached smoothly.
const LOCAL_LIMIT = 1.5;

// `coef` is the map at this pixel: the texture band's base coefficients (a, b), then the clarity band's.
// `amount` is `DevelopParams.local`: the texture and clarity bands' change, −1…1.
fn local_contrast(c: vec3f, source: vec3f, coef: vec4f, amount: vec4f) -> vec3f {
  let l = guided_guide(source, 1u);
  let fine_base = coef.x * l + coef.y;
  let mid_base = coef.z * l + coef.w;
  let delta = amount.x * (l - fine_base) + amount.y * (fine_base - mid_base);
  let limited = LOCAL_LIMIT * tanh(delta / LOCAL_LIMIT);
  return c * exp2(limited);
}
