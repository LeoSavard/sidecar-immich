// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: develop/params.wgsl develop/wb.wgsl develop/exposure.wgsl develop/gamut.wgsl develop/tone.wgsl develop/masks.wgsl develop/mixer.wgsl develop/output.wgsl
// Develop pass: full-screen triangle into developedTex (rgba16float, proxy size). Output is display-linear.

@group(0) @binding(0) var<uniform> P: DevelopParams;
@group(0) @binding(1) var source_tex: texture_2d<f32>;
@group(0) @binding(2) var tone_lut: texture_1d<f32>;

// Masks: one texture per adjustment, in the frame `M.frame` describes. Unused slots hold a 1×1 texture.
@group(1) @binding(0) var<uniform> M: MaskParams;
@group(1) @binding(1) var mask_sampler: sampler;
@group(1) @binding(2) var mask0: texture_2d<f32>;
@group(1) @binding(3) var mask1: texture_2d<f32>;
@group(1) @binding(4) var mask2: texture_2d<f32>;
@group(1) @binding(5) var mask3: texture_2d<f32>;
@group(1) @binding(6) var mask4: texture_2d<f32>;
@group(1) @binding(7) var mask5: texture_2d<f32>;
@group(1) @binding(8) var mask6: texture_2d<f32>;
@group(1) @binding(9) var mask7: texture_2d<f32>;
// Each layer's own curve, a row of 4,096 entries per layer (r32float), the identity for a layer without one.
@group(1) @binding(10) var layer_curves_tex: texture_2d<f32>;

fn mask_weights(pixel: vec2f) -> array<f32, 8> {
  var w = array<f32, 8>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
  if (M.count == 0u) { return w; }
  let size = select(vec2f(textureDimensions(source_tex)), M.frame.zw, M.frame.z > 0.0);
  let uv = (pixel + M.frame.xy) / size;
  w[0] = textureSampleLevel(mask0, mask_sampler, uv, 0.0).r;
  w[1] = textureSampleLevel(mask1, mask_sampler, uv, 0.0).r;
  w[2] = textureSampleLevel(mask2, mask_sampler, uv, 0.0).r;
  w[3] = textureSampleLevel(mask3, mask_sampler, uv, 0.0).r;
  w[4] = textureSampleLevel(mask4, mask_sampler, uv, 0.0).r;
  w[5] = textureSampleLevel(mask5, mask_sampler, uv, 0.0).r;
  w[6] = textureSampleLevel(mask6, mask_sampler, uv, 0.0).r;
  w[7] = textureSampleLevel(mask7, mask_sampler, uv, 0.0).r;
  return w;
}

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let src = textureLoad(source_tex, vec2i(pos.xy), 0);
  let developed = P.display_referred > 0.5;
  var local = mask_start(P.tone, P.mixer);
  let weights = mask_weights(pos.xy);
  for (var i = 0u; i < min(M.count, 8u); i++) { mask_add(&local, M.layers[i], i, weights[i], P.tone); }
  var c = white_balance(src.rgb, P.wb_mul * local.gain);      // 1
  c = exposure(c, P.exposure * exp2(local.ev));                // 2
  c = highlight_blend(c, src.a, P.hl_strength);
  c = P.cam_to_work * c;                                       // 3
  var e: f32;
  if (developed) {
    // Already inside its own display gamut and already tone-mapped: neither step is the file's to take again.
    e = tone_display_curve_in(c, local.tone);                  // 4
  } else {
    c = gamut_compress(c);
    e = tone_map_curve_in(c, local.tone);                      // 4
  }
  e = layer_curves(lut_sample(tone_lut, e), layer_curves_tex, local.curves, M.count);
  c = tone_apply(c, e, developed);
  c = mask_saturation(mask_contrast(c, local.contrast), local.saturation);
  c = color_mixer(c, local.mixer);                             // 5
  c = output_transform(c, P.work_to_disp, developed);
  return vec4f(c, 1.0);
}
