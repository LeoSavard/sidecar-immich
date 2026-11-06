// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/defringe_params.wgsl
// Defringe (DESIGN.md §7, "Defringe"; `sidecar_color::params::Defringe`): axial chromatic aberration's purple and
// green fringes, on develop's input. A fringe is a fringe hue beside a steep luminance edge: each pixel's hue is
// judged in OKLab (white-balanced, in the working space), the edge as the largest contrast in log luminance across
// the reach along either axis, and a pixel with both is taken toward neutral at its own luminance.

@group(0) @binding(0) var<uniform> D: DefringeParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;

const LUMA_2020 = vec3f(0.2627, 0.6780, 0.0593);
const REC2020_TO_SRGB = mat3x3f(
  vec3f(1.6604910021, -0.1245504745, -0.0181507634),
  vec3f(-0.5876411388, 1.1328998971, -0.100578898),
  vec3f(-0.0728498633, -0.0083494226, 1.1187296614),
);

fn work_at(p: vec2i) -> vec3f {
  let size = vec2i(textureDimensions(src));
  return D.cam_to_work * (textureLoad(src, clamp(p, vec2i(0), size - 1), 0).rgb * D.wb.rgb);
}

fn log_y(c: vec3f) -> f32 {
  return log2(max(dot(c, LUMA_2020), 1e-6));
}

// OKLab hue (radians, 0…2π) and chroma of a linear Rec.2020 colour (Ottosson, through linear sRGB).
fn hue_chroma(c: vec3f) -> vec2f {
  let s = REC2020_TO_SRGB * c;
  let l = 0.4122214708 * s.r + 0.5363325363 * s.g + 0.0514459929 * s.b;
  let m = 0.2119034982 * s.r + 0.6806995451 * s.g + 0.1073969566 * s.b;
  let q = 0.0883024619 * s.r + 0.2817188376 * s.g + 0.6299787005 * s.b;
  let lms = sign(vec3f(l, m, q)) * pow(abs(vec3f(l, m, q)), vec3f(1.0 / 3.0));
  let a = 1.9779984951 * lms.x - 2.4285922050 * lms.y + 0.4505937099 * lms.z;
  let b = 0.0259040371 * lms.x + 0.7827717662 * lms.y - 0.8086757660 * lms.z;
  var h = atan2(b, a);
  if (h < 0.0) { h += 6.283185307; }
  return vec2f(h, sqrt(a * a + b * b));
}

// How far inside a hue band (radians, from and to, wrapping round) a hue is: 1 inside, falling to 0 over 10°.
fn in_band(h: f32, band: vec4f) -> f32 {
  let soft = 0.1745;
  let width = (band.z - band.y + 6.283185307) % 6.283185307;
  let from_start = (h - band.y + 6.283185307) % 6.283185307;
  let inside = from_start <= width;
  let before = (band.y - h + 6.283185307) % 6.283185307;
  let after = (h - band.z + 6.283185307) % 6.283185307;
  if (inside) { return 1.0; }
  return max(1.0 - before / soft, 0.0) + max(1.0 - after / soft, 0.0);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let p = vec2i(id.xy);
  let here = textureLoad(src, p, 0);
  let c = work_at(p);
  let hc = hue_chroma(c);
  let weight = D.purple.x * in_band(hc.x, D.purple) + D.green.x * in_band(hc.x, D.green);
  if (weight <= 0.0 || hc.y < 0.01) {
    textureStore(dst, p, here);
    return;
  }
  // The steepest edge in reach: the range of log luminance along either axis.
  let long = select(f32(max(size.x, size.y)), D.reach.w, D.reach.w > 0.0);
  let r = max(i32(round(D.reach.x * long)), 1);
  var lo = vec2f(1e9);
  var hi = vec2f(-1e9);
  for (var k = -r; k <= r; k++) {
    let a = log_y(work_at(p + vec2i(k, 0)));
    let b = log_y(work_at(p + vec2i(0, k)));
    lo = min(lo, vec2f(a, b));
    hi = max(hi, vec2f(a, b));
  }
  let contrast = max(hi.x - lo.x, hi.y - lo.y);
  let w = clamp(weight * smoothstep(D.reach.y, D.reach.z, contrast), 0.0, 1.0);
  let neutral = vec3f(dot(c, LUMA_2020));
  let out = D.work_to_cam * mix(c, neutral, w) / D.wb.rgb;
  textureStore(dst, p, vec4f(out, here.a));
}
