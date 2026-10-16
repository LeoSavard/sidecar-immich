// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: develop/params.wgsl develop/wb.wgsl develop/exposure.wgsl develop/gamut.wgsl develop/tone.wgsl develop/masks.wgsl develop/mixer.wgsl develop/mono.wgsl develop/developer.wgsl develop/grain.wgsl develop/stock.wgsl develop/output.wgsl
// Develop pass: full-screen triangle into developedTex (rgba16float, proxy size). Output is display-linear.

@group(0) @binding(0) var<uniform> P: DevelopParams;
@group(0) @binding(1) var source_tex: texture_2d<f32>;
@group(0) @binding(2) var tone_lut: texture_1d<f32>;
// Black and white's table over the colour cube; 1×1×1 in colour.
@group(0) @binding(3) var mono_lut: texture_3d<f32>;
// Black and white's adjacency map (`sidecar_color::developer`): the film's log exposure around each place, over
// the whole frame at low resolution; 1×1 when there is none.
@group(0) @binding(4) var density_map: texture_2d<f32>;

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
  var local = mask_start(P.tone, P.mixer, P.saturation);
  let weights = mask_weights(pos.xy);
  for (var i = 0u; i < min(M.count, 8u); i++) { mask_add(&local, M.layers[i], i, weights[i], P.tone); }
  var c = white_balance(src.rgb, P.wb_mul * local.gain);      // 1
  c = exposure(c, P.exposure * exp2(local.ev));                // 2
  c = highlight_blend(c, src.a, P.hl_strength);
  c = P.cam_to_work * c;                                       // 3
  // Where this pixel is in the whole frame — `M.frame` says where a loupe's tile sits in it — so a tile's grain
  // and developer are the export's.
  let frame = select(vec2f(textureDimensions(source_tex)), M.frame.zw, M.frame.z > 0.0);
  let at = vec2i(pos.xy) + vec2i(M.frame.xy);
  // Black and white: the film's exposure in place of the colour, before the tone map, as film is exposed; then
  // what the developer makes of it.
  if (P.mono.x > 0.5) {
    let e = mono_exposure(c, mono_lut, P.mono.y);
    // The adjacency map's own pass: the film's log exposure, nothing after it.
    if (P.mono.z > 0.5) { return vec4f(vec3f(log2(max(e, 1e-6) / 0.18)), 1.0); }
    var local = 0.0;
    var adjacency = 0.0;
    let map = vec2f(textureDimensions(density_map));
    if (P.developer.x != 0.0 && map.x > 1.0) {
      // The map averages `k` × `k` of the frame's pixels per texel (`developer::map_scale`).
      let k = max(ceil(max(frame.x, frame.y) / 1024.0), 1.0);
      local = textureSampleLevel(density_map, mask_sampler, (vec2f(at) + 0.5) / k / map, 0.0).r;
      adjacency = P.developer.x;
    }
    c = vec3f(developer_apply(e, local, adjacency, P.developer));
  }
  // Grain, in the exposure the tone map is about to take, placed on the whole frame.
  if (P.grain.x > 0.0) {
    c = grain_apply(c, grain_noise(at, P.grain.y * max(frame.x, frame.y)), P.grain.x, P.grain.z, P.tone, developed);
  }
  // A named stock's curve: the shadows, grain and all, bent into its fog, the highlights over its shoulder.
  if (P.stock.x > 0.5) { c = vec3f(stock_print(c.r, P.stock, P.shoulder)); }
  var e: f32;
  // Film rendering (RAW only; `pack` leaves it at 0 for a developed file): the curves per channel, blended
  // with the neutral rendering by its amount. At 0 none of it runs, so the neutral path is unchanged.
  let film = P.film.x > 0.0;
  var ef = vec3f(0.0);
  if (developed) {
    // Already inside its own display gamut and already tone-mapped: neither step is the file's to take again.
    e = tone_display_curve_in(c, local.tone);                  // 4
  } else {
    c = gamut_compress(c);
    e = tone_map_curve_in(c, local.tone);                      // 4
    if (film) { ef = film_curve_in(c, local.tone, P.film.y); }
  }
  e = layer_curves(lut_sample(tone_lut, e), layer_curves_tex, local.curves, M.count);
  c = tone_apply(c, e, developed);
  if (film) {
    let user = vec3f(
      layer_curves(lut_sample(tone_lut, ef.r), layer_curves_tex, local.curves, M.count),
      layer_curves(lut_sample(tone_lut, ef.g), layer_curves_tex, local.curves, M.count),
      layer_curves(lut_sample(tone_lut, ef.b), layer_curves_tex, local.curves, M.count),
    );
    c = mix(c, vec3f(srgb_decode1(user.r), srgb_decode1(user.g), srgb_decode1(user.b)), P.film.x);
  }
  c = mask_saturation(mask_contrast(c, local.contrast), local.saturation);
  c = color_mixer(c, local.mixer);                             // 5
  c = output_transform(c, P.work_to_disp, developed);
  return vec4f(c, 1.0);
}
