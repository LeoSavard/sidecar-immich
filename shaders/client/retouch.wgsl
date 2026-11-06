// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/retouch_params.wgsl
// Retouch, one spot (DESIGN.md §15, "Retouch"), over its box: the photo's camera-native linear light, before the
// lens pass. `src` is the photo as the spots before this one left it; this writes the box into `dst`, which the
// host copies back.
//
// Heal is gradient-domain cloning (Pérez, Gangnet and Blake, "Poisson image editing", SIGGRAPH 2003): keep the
// source's gradients, meet the target's surroundings at the edge. The solution is the source plus the harmonic
// function that takes the edge's mismatch inside; on a disc that function is the Poisson integral of the edge
// values — exact, one pass, no iteration. The mismatch is taken in log light, so a source lit differently from the
// target is corrected by a ratio, as light differs.

@group(0) @binding(0) var<uniform> R: RetouchParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;

// Edge samples around the disc.
const EDGE = 64u;

fn at(p: vec2f) -> vec4f {
  return textureSampleLevel(src, samp, p / vec2f(textureDimensions(src)), 0.0);
}

fn log_light(c: vec3f) -> vec3f {
  return log2(max(c, vec3f(1e-6)));
}

// The light just outside the disc around `c` in direction `u`: three samples across the edge's first tenth of a
// radius, so a single noisy pixel doesn't decide the membrane.
fn rim(c: vec2f, u: vec2f) -> vec3f {
  return (log_light(at(c + u * R.radius * 1.02).rgb) + log_light(at(c + u * R.radius * 1.06).rgb)
    + log_light(at(c + u * R.radius * 1.10).rgb)) / 3.0;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  if (id.x >= R.size.x || id.y >= R.size.y) { return; }
  let pix = R.origin + id.xy;
  if (any(pix >= textureDimensions(src))) { return; }
  let p = vec2f(pix) + 0.5;
  let here = at(p);
  let d = p - R.centre;
  let r = length(d);
  if (r >= R.radius) {
    textureStore(dst, pix, here);
    return;
  }
  let edge = R.radius * clamp(1.0 - R.feather, 0.0, 1.0);
  let w = R.opacity * (1.0 - smoothstep(edge, R.radius + 1e-3, r));
  var out = here.rgb;
  if (R.kind == 2u) {
    // Red eye: in white-balanced light, where red stands well above green and blue, take it down to their mean.
    let b = here.rgb * R.wb.rgb;
    let t = smoothstep(1.2, 1.8, b.r / max(max(b.g, b.b), 1e-6));
    let fixed = vec3f(mix(b.r, 0.5 * (b.g + b.b), t), b.g, b.b) / R.wb.rgb;
    out = fixed;
  } else {
    let source = at(p - R.centre + R.take_from).rgb;
    if (R.kind == 1u) {
      out = source;
    } else {
      // The Poisson kernel at (r, θ) for edge angle φ: (ρ² − r²) / (ρ² − 2ρr·cos(θ − φ) + r²), normalised.
      let theta = atan2(d.y, d.x);
      var m = vec3f(0.0);
      var total = 0.0;
      for (var k = 0u; k < EDGE; k++) {
        let phi = 6.283185307 * (f32(k) + 0.5) / f32(EDGE);
        let u = vec2f(cos(phi), sin(phi));
        let mismatch = rim(R.centre, u) - rim(R.take_from, u);
        let rho = R.radius;
        let kernel = (rho * rho - r * r) / max(rho * rho - 2.0 * rho * r * cos(theta - phi) + r * r, 1e-6);
        m += kernel * mismatch;
        total += kernel;
      }
      out = max(source, vec3f(0.0)) * exp2(m / total);
    }
  }
  textureStore(dst, pix, vec4f(mix(here.rgb, out, w), here.a));
}
