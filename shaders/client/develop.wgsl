// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: develop/params.wgsl develop/wb.wgsl develop/exposure.wgsl develop/gamut.wgsl develop/tone.wgsl develop/mixer.wgsl develop/output.wgsl
// Develop pass: full-screen triangle into developedTex (rgba16float, proxy size). Output is display-linear.

@group(0) @binding(0) var<uniform> P: DevelopParams;
@group(0) @binding(1) var source_tex: texture_2d<f32>;
@group(0) @binding(2) var tone_lut: texture_1d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let src = textureLoad(source_tex, vec2i(pos.xy), 0);
  var c = white_balance(src.rgb, P.wb_mul);                   // 1
  c = exposure(c, P.exposure);                                 // 2
  c = highlight_blend(c, src.a, P.hl_strength);
  c = P.cam_to_work * c;                                       // 3
  c = gamut_compress(c);
  c = tone_map(c, P.tone, tone_lut);                           // 4
  c = color_mixer(c, P.mixer);                                 // 5
  c = output_transform(c, P.work_to_disp);
  return vec4f(c, 1.0);
}
