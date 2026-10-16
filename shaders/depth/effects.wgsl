// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Lens blur and haze by distance (DESIGN.md §15, "Depth effects"): the uniform block and the maths the three
// depth passes share. They run on camera-native linear light after the lens pass and before develop, so a
// blurred highlight spreads as the light it is, and haze is light in the scene before white balance sees it.
// Layout mirrored by hand in crates/color/src/depth.rs (DepthParams, 64 bytes), checked by naga.

struct DepthParams {
  frame: vec4f,  // xy: this texture's offset in the depth map's frame, in its pixels; zw: that frame's size
  haze: vec4f,   // rgb: the airlight, camera RGB; w: optical depth at the farthest point, + adds haze, − removes it
  blur: vec4f,   // x: the largest blur radius, in this texture's pixels; y: focus, as nearness 0…1; z: half the
                 // in-focus band; w: 1 when blurring
  scale: vec4f,  // x: how many pixels of this texture one low-resolution texel covers (k ≥ 1); yzw unused
}

// How near the point at `p` (pixels of the texture being processed) is, 0 farthest … 1 nearest.
fn depth_nearness(depth: texture_2d<f32>, samp: sampler, p: vec2f, frame: vec4f) -> f32 {
  return textureSampleLevel(depth, samp, (p + frame.xy) / frame.zw, 0.0).r;
}

// The blur radius at nearness `n`, in this texture's pixels. Depth Anything gives inverse depth, and a thin
// lens's circle of confusion grows with the difference in inverse depth from the focal plane: linear in `n`.
fn depth_coc(n: f32, blur: vec4f) -> f32 {
  return blur.x * max(abs(n - blur.y) - blur.z, 0.0);
}

// The largest radius any point of this photo can have, in this texture's pixels.
fn depth_max_coc(blur: vec4f) -> f32 {
  return blur.x * max(max(blur.y, 1.0 - blur.y) - blur.z, 0.0);
}

// Haze along the line of sight: transmittance t = exp(−τ·d), d = 1 − nearness. Adding it mixes the light
// toward the airlight as distance grows; removing it inverts that, with t floored so the far end can't run
// away, and never below black.
fn depth_haze(c: vec3f, n: f32, haze: vec4f) -> vec3f {
  if (haze.w == 0.0) { return c; }
  if (haze.w > 0.0) {
    let t = exp(-haze.w * (1.0 - n));
    return c * t + haze.rgb * (1.0 - t);
  }
  let t = max(exp(haze.w * (1.0 - n)), 0.2);
  return max((c - haze.rgb * (1.0 - t)) / t, vec3f(0.0));
}
