// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: depth/effects.wgsl
// Depth pass 2 of 3: the lens blur, at low resolution, as a gather that stands in for scattering. Each sample
// on a golden-angle spiral over the largest blur disc counts if its own disc reaches this texel. A sample
// farther than this texel only reaches as far as this texel's own disc does, so a sharp subject doesn't take
// in the blurred background behind it; a nearer one reaches with its whole disc, so a blurred foreground
// spreads over what is behind it. Each sample is weighted by the inverse of its disc's area, which spreads a
// point's light evenly over its disc, as a lens does: a bright point becomes a bright disc, not a smear.
// Alpha is the share of the result that came from nearer, blurred points, which compose uses to let a blurred
// foreground cover a sharp background.

@group(0) @binding(0) var<uniform> D: DepthParams;
@group(0) @binding(1) var small: texture_2d<f32>;
@group(0) @binding(2) var depth: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
@group(0) @binding(4) var dst: texture_storage_2d<rgba16float, write>;

const SAMPLES: u32 = 160u;
const GOLDEN_ANGLE: f32 = 2.39996323;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let k = D.scale.x;
  let dims = vec2f(size);
  let p = vec2f(id.xy) + 0.5;
  let centre = textureLoad(small, id.xy, 0).rgb;
  let n_p = depth_nearness(depth, samp, p * k, D.frame);
  let c_p = depth_coc(n_p, D.blur) / k;
  let radius = depth_max_coc(D.blur) / k;
  if (radius < 0.5) {
    textureStore(dst, id.xy, vec4f(centre, 0.0));
    return;
  }
  // Each sample stands for this much of the disc's area, in texels; the centre for its own texel.
  let area = 3.14159265 * radius * radius / f32(SAMPLES);
  let w0 = 1.0 / max(c_p * c_p, 1.0);
  var sum = centre * w0;
  var total = w0;
  var front = 0.0;
  for (var i = 0u; i < SAMPLES; i++) {
    let r = radius * sqrt((f32(i) + 0.5) / f32(SAMPLES));
    let a = f32(i) * GOLDEN_ANGLE;
    let q = p + r * vec2f(cos(a), sin(a));
    let n_q = depth_nearness(depth, samp, q * k, D.frame);
    let c_q = depth_coc(n_q, D.blur) / k;
    let reach = select(min(c_q, c_p), c_q, n_q >= n_p);
    let cover = clamp(reach - r + 0.5, 0.0, 1.0);
    if (cover <= 0.0) { continue; }
    let w = area * cover / max(c_q * c_q, 1.0);
    sum += textureSampleLevel(small, samp, q / dims, 0.0).rgb * w;
    total += w;
    if (n_q > n_p + 0.02 && c_q > c_p) { front += w; }
  }
  textureStore(dst, id.xy, vec4f(sum / total, front / total));
}
