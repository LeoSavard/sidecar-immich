// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/profile_params.wgsl
// Camera profile pass (DESIGN.md §7, "Camera profiles"; `sidecar_color::dcp`): a DCP's HueSatMap and LookTable on
// develop's input, so develop itself is unchanged. The input is white-balanced as develop will, taken into linear
// ProPhoto RGB through the profile's forward matrix, the HueSatMap applied, then the LookTable at develop's
// exposure the profile expects — the DNG specification's order — and the result brought back to the camera RGB
// develop expects.

@group(0) @binding(0) var<uniform> P: ProfileParams;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var hue_sat: texture_3d<f32>;
@group(0) @binding(3) var look: texture_3d<f32>;
@group(0) @binding(4) var dst: texture_storage_2d<rgba16float, write>;

fn srgb_encode(v: f32) -> f32 {
  return select(1.055 * pow(v, 1.0 / 2.4) - 0.055, 12.92 * v, v <= 0.0031308);
}

// One table (hue, saturation, value divisions in `dims.xyz`; sRGB-spaced values when `dims.w` is 1) applied to a
// linear ProPhoto colour: `sidecar_color::dcp::apply`.
fn apply_table(t: texture_3d<f32>, dims: vec4u, rgb: vec3f) -> vec3f {
  let mx = max(rgb.r, max(rgb.g, rgb.b));
  let mn = min(rgb.r, min(rgb.g, rgb.b));
  if (mx <= 0.0 || mn < 0.0) { return rgb; }
  let delta = mx - mn;
  let s = delta / mx;
  var h = 0.0;
  if (delta > 0.0) {
    if (rgb.r == mx) { h = (rgb.g - rgb.b) / delta; }
    else if (rgb.g == mx) { h = 2.0 + (rgb.b - rgb.r) / delta; }
    else { h = 4.0 + (rgb.r - rgb.g) / delta; }
  }
  if (h < 0.0) { h += 6.0; }
  let hd = i32(dims.x);
  let sd = i32(dims.y);
  let vd = i32(dims.z);
  let hf = h * f32(hd) / 6.0;
  let sf = s * f32(sd - 1);
  var vv = min(mx, 1.0);
  if (dims.w == 1u) { vv = srgb_encode(vv); }
  let vf = vv * f32(max(vd, 1) - 1);
  let h0 = i32(floor(hf));
  let s0 = min(i32(floor(sf)), sd - 2);
  let v0 = min(i32(floor(vf)), max(vd - 2, 0));
  let th = hf - f32(h0);
  let ts = sf - f32(s0);
  let tv = select(0.0, vf - f32(v0), vd > 1);
  var acc = vec3f(0.0);
  for (var dv = 0; dv < 2; dv++) {
    let wv = select(1.0 - tv, tv, dv == 1);
    if (wv == 0.0 || (vd == 1 && dv == 1)) { continue; }
    for (var dh = 0; dh < 2; dh++) {
      let wh = select(1.0 - th, th, dh == 1);
      for (var ds = 0; ds < 2; ds++) {
        let ws = select(1.0 - ts, ts, ds == 1);
        let e = textureLoad(t, vec3i(s0 + ds, (h0 + dh) % hd, min(v0 + dv, vd - 1)), 0).xyz;
        acc += wv * wh * ws * e;
      }
    }
  }
  let h2 = (h + acc.x * 6.0 / 360.0) - 6.0 * floor((h + acc.x * 6.0 / 360.0) / 6.0);
  let s2 = clamp(s * acc.y, 0.0, 1.0);
  let v2 = mx * acc.z;
  let i = floor(h2);
  let f = h2 - i;
  let p = v2 * (1.0 - s2);
  let q = v2 * (1.0 - s2 * f);
  let u = v2 * (1.0 - s2 * (1.0 - f));
  switch (i32(i)) {
    case 0: { return vec3f(v2, u, p); }
    case 1: { return vec3f(q, v2, p); }
    case 2: { return vec3f(p, v2, u); }
    case 3: { return vec3f(p, q, v2); }
    case 4: { return vec3f(u, p, v2); }
    default: { return vec3f(v2, p, q); }
  }
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let c = textureLoad(src, id.xy, 0);
  var pp = P.to_prophoto * (c.rgb * P.wb.rgb);
  if (P.hue_sat.x > 0u) { pp = apply_table(hue_sat, P.hue_sat, pp); }
  if (P.look.x > 0u) {
    // The input is scene light at the sensor's own scale (develop applies its exposure later): the table is read
    // at the exposure the profile is meant for.
    let e = max(P.exposure.x, 1e-6);
    pp = apply_table(look, P.look, pp * e) / e;
  }
  textureStore(dst, id.xy, vec4f((P.from_prophoto * pp) / P.wb.rgb, c.a));
}
