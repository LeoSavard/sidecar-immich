// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/edge_params.wgsl
// A mask layer's edge (DESIGN.md §15, "Mask edges"): blurred over the edge's width, then thresholded again, so
// the threshold moves the edge out or in and the ramp around it sets how soft it is. The blur runs at a
// resolution where its radius is at most 16 texels: 0 averages the mask down, 1 blurs along `dir` (a Gaussian,
// run across then down), 2 reads the blurred mask back at full size through the ramp. Render passes, as the
// brush's, since r8unorm can't be a storage texture in WebGPU.

@group(0) @binding(0) var<uniform> E: EdgeParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var linear_sampler: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  if (E.mode == 0u) {
    let k = u32(E.scale);
    let full = textureDimensions(src);
    let origin = vec2u(pos.xy - 0.5) * k;
    var sum = 0.0;
    var n = 0.0;
    for (var j = 0u; j < k; j++) {
      for (var i = 0u; i < k; i++) {
        let p = origin + vec2u(i, j);
        if (p.x < full.x && p.y < full.y) {
          sum += textureLoad(src, p, 0).r;
          n += 1.0;
        }
      }
    }
    return vec4f(sum / max(n, 1.0), 0.0, 0.0, 1.0);
  }
  if (E.mode == 1u) {
    let reach = i32(ceil(E.radius));
    let sigma = max(E.radius * 0.5, 0.5);
    let full = vec2i(textureDimensions(src));
    var sum = 0.0;
    var total = 0.0;
    for (var t = -reach; t <= reach; t++) {
      let w = exp(-0.5 * f32(t * t) / (sigma * sigma));
      let p = clamp(vec2i(pos.xy) + vec2i(E.dir) * t, vec2i(0), full - 1);
      sum += textureLoad(src, p, 0).r * w;
      total += w;
    }
    return vec4f(sum / total, 0.0, 0.0, 1.0);
  }
  let v = textureSampleLevel(src, linear_sampler, pos.xy / E.size, 0.0).r;
  let half = max(E.ramp.y * 0.5, 0.001);
  return vec4f(smoothstep(E.ramp.x - half, E.ramp.x + half, v), 0.0, 0.0, 1.0);
}
