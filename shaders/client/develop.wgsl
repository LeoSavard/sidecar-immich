// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/develop_params.wgsl develop/wb.wgsl develop/exposure.wgsl develop/gamut.wgsl develop/tone.wgsl develop/masks.wgsl develop/mixer.wgsl develop/mono.wgsl develop/developer.wgsl develop/grain.wgsl develop/stock.wgsl develop/slide.wgsl develop/grading.wgsl develop/local.wgsl develop/haze.wgsl develop/output.wgsl
// Develop pass: full-screen triangle into developedTex (rgba16float, proxy size). Output is display-linear.

@group(0) @binding(0) var<uniform> P: DevelopParams;

// `sidecar_color::params::NEGATIVE_SCALE`: an inverted negative's grey to about 0.18.
const NEGATIVE_SCALE = 0.0018;
@group(0) @binding(1) var source_tex: texture_2d<f32>;
@group(0) @binding(2) var tone_lut: texture_1d<f32>;
// Black and white's table over the colour cube; 1×1×1 in colour.
// `HALATION_GREEN` in `sidecar_color::params`: the share of the halo the green layer sees.
const HALATION_GREEN = 0.15;

@group(0) @binding(3) var mono_lut: texture_3d<f32>;
// Black and white's adjacency map (`sidecar_color::developer`): the film's log exposure around each place, over
// the whole frame at low resolution; 1×1 when there is none.
@group(0) @binding(4) var density_map: texture_2d<f32>;
// A spectral colour stock's two tables (`develop/slide.wgsl`): each colour's layer responses over the colour
// cube, and the slide's colour over the layers' log H. Both 1×1×1 unless a stock is being rendered.
@group(0) @binding(5) var slide_exposures: texture_3d<f32>;
@group(0) @binding(6) var slide_responses: texture_3d<f32>;
// The whole-frame maps at low resolution, rgba32float (not filterable everywhere, so `map_texel` interpolates by
// hand), one array because develop is at the fragment stage's sixteen textures: layer 0 Texture and Clarity's two
// bands' guided-filter coefficients (`develop/local.wgsl`), layer 1 Dehaze's transmission coefficients, and the
// airlight in texel (0, 0) of layer 2 (`develop/haze.wgsl`, working space before exposure). 1×1 when unused.
@group(0) @binding(7) var maps: texture_2d_array<f32>;

// The local-contrast map at frame position `at` of a frame `frame` pixels across: the map covers the whole frame,
// one texel per `ceil(long edge / 1024)` pixels square (`developer::map_scale`).
// Layer `layer` of the whole-frame maps at frame position `at` of a frame `frame` pixels across: the maps cover the
// whole frame, one texel per `ceil(long edge / 1024)` pixels square (`developer::map_scale`).
fn map_texel(layer: i32, at: vec2f, frame: vec2f) -> vec4f {
  let k = max(ceil(max(frame.x, frame.y) / 1024.0), 1.0);
  let size = vec2i(textureDimensions(maps));
  let f = at / k - 0.5;
  let base = vec2i(floor(f));
  let t = f - floor(f);
  let last = size - 1;
  let c00 = textureLoad(maps, clamp(base, vec2i(0), last), layer, 0);
  let c10 = textureLoad(maps, clamp(base + vec2i(1, 0), vec2i(0), last), layer, 0);
  let c01 = textureLoad(maps, clamp(base + vec2i(0, 1), vec2i(0), last), layer, 0);
  let c11 = textureLoad(maps, clamp(base + vec2i(1, 1), vec2i(0), last), layer, 0);
  return mix(mix(c00, c10, t.x), mix(c01, c11, t.x), t.y);
}

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
  var src = textureLoad(source_tex, vec2i(pos.xy), 0);
  // A film negative (`Negative`): the scene's light from the film's density above its base, before anything else —
  // develop then treats it as any photo's light. The clip flag means nothing on a negative's dense end.
  if (P.negative.w > 0.0) {
    let base = P.negative.rgb;
    let t = max(src.rgb, base * 1e-6);
    src = vec4f(max(pow(base / t, vec3f(P.negative.w)) - 1.0, vec3f(0.0)) * NEGATIVE_SCALE, 0.0);
  }
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
  // Texture and Clarity, on the scene's light: a gain from the source's own detail, so exposure, white balance and
  // the masks don't move the bands.
  if (P.local.z > 0.5 && textureDimensions(maps).x > 1u) {
    c = local_contrast(c, src.rgb, map_texel(0, vec2f(at) + 0.5, frame), P.local);
  }
  // Dehaze, on the scene's light: the airlight scaled into this pixel's exposure, the transmission at this
  // pixel's own luminance.
  if (P.local.w != 0.0 && textureDimensions(maps).x > 1u) {
    let ab = map_texel(1, vec2f(at) + 0.5, frame).xy;
    let t = clamp(ab.x * guided_guide(src.rgb, 1u) + ab.y, 0.0, 1.0);
    let air = textureLoad(maps, vec2i(0), 2, 0).rgb * P.exposure * exp2(local.ev);
    c = haze_apply(c, air, t, P.local.w);
  }
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
  // Halation (`Film.halation`), colour film's: the red layer's exposure, blurred over the base's reflection, back
  // into the red layer and a little of the green, as light before the grain. The map pass reads the red layer
  // itself. Normalised so a flat field is left as it was, as a print's colour correction would take out the cast
  // a uniform reflection gives: what is left is the glow beside bright things.
  if (P.mono.x < 0.5 && P.film.z > 0.0) {
    if (P.mono.z > 0.5) { return vec4f(vec3f(max(c.r, 0.0)), 1.0); }
    let map = vec2f(textureDimensions(density_map));
    if (map.x > 1.0) {
      let k = max(ceil(max(frame.x, frame.y) / 1024.0), 1.0);
      let halo = textureSampleLevel(density_map, mask_sampler, (vec2f(at) + 0.5) / k / map, 0.0).r;
      let into = P.film.z * vec3f(1.0, HALATION_GREEN, 0.0);
      c = (c + into * halo) / (1.0 + into);
    }
  }
  // Grain, in the exposure the tone map is about to take, placed on the whole frame.
  if (P.grain.x > 0.0) {
    let size = P.grain.y * max(frame.x, frame.y);
    if (P.grain.w > 0.5) {
      let n = vec3f(grain_noise_layer(at, size, 1u), grain_noise_layer(at, size, 2u), grain_noise_layer(at, size, 3u));
      c = grain_apply_layers(c, n, P.grain.x, P.grain.z, P.tone);
    } else {
      c = grain_apply(c, grain_noise(at, size), P.grain.x, P.grain.z, P.tone, developed);
    }
  }
  // A named stock's curve: the shadows, grain and all, bent into its fog, the highlights over its shoulder.
  if (P.stock.x > 0.5) { c = vec3f(stock_print(c.r, P.stock, P.shoulder)); }
  // The light a spectral colour stock is exposed to: the working colour as it stands here, before the neutral
  // path compresses it into the display's gamut and tone-maps it. Grain is already in it, as a film's is.
  let scene = c;
  var e: f32;
  // Film rendering (RAW only; `pack` leaves it at 0 for a developed file): the curves per channel, blended
  // with the neutral rendering by its amount. At 0 none of it runs, so the neutral path is unchanged.
  let film = P.film.x > 0.0;
  var ef = vec3f(0.0);
  if (developed) {
    // Already inside its own display gamut and already tone-mapped: neither step is the file's to take again.
    e = tone_display_curve_in(c, local.tone);                  // 4
  } else {
    c = gamut_compress(c, P.tone_shape.z > 0.5);
    e = tone_map_curve_in(c, local.tone, P.tone_shape);                      // 4
    if (film) { ef = film_curve_in(c, local.tone, P.film.y); }
  }
  e = layer_curves(lut_sample(tone_lut, e), layer_curves_tex, local.curves, M.count);
  c = tone_apply(c, e, developed, P.highlight_bleach);
  if (film) {
    let user = vec3f(
      layer_curves(lut_sample(tone_lut, ef.r), layer_curves_tex, local.curves, M.count),
      layer_curves(lut_sample(tone_lut, ef.g), layer_curves_tex, local.curves, M.count),
      layer_curves(lut_sample(tone_lut, ef.b), layer_curves_tex, local.curves, M.count),
    );
    c = mix(c, vec3f(srgb_decode1(user.r), srgb_decode1(user.g), srgb_decode1(user.b)), P.film.x);
  }
  // Colour film, spectrally: the stock's own chain in place of the neutral rendering, mixed in by its amount.
  // What comes out is a rendered picture — a slide on a light table — so the tone controls and the user's
  // curve grade it the way they grade a developed file, which is the identity where they sit at their
  // defaults. Reaching it through the neutral rendering instead would have left half of them doing nothing
  // at all once Amount was full. `P.slide_tone` carries them in a developed file's terms.
  if (P.slide.x > 0.0) {
    var s = slide_colour(scene, slide_exposures, slide_responses, P.slide, P.slide_log_h);
    let graded = layer_curves(lut_sample(tone_lut, tone_display_curve_in(s, P.slide_tone)), layer_curves_tex, local.curves, M.count);
    c = mix(c, tone_apply(s, graded, true, 0.0), P.slide.x);
  }
  c = mask_saturation(mask_contrast(c, local.contrast), local.saturation);
  c = color_mixer(c, local.mixer);                             // 5
  if (P.grade_split.x > 0.5) { c = color_grade(c, P.grade, P.grade_split); }
  c = output_transform(c, P.work_to_disp, developed);
  return vec4f(c, 1.0);
}
