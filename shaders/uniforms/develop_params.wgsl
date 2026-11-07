// SPDX-License-Identifier: Apache-2.0
// Generated from `sidecar_color::params::DevelopParams` by #[derive(Uniform)].
// Do not edit: change the Rust struct, then run `npm run uniforms` in web/.

struct DevelopParams {
  // Camera-space white balance multipliers, green = 1.
  wb_mul: vec3f,
  // 2^EV.
  exposure: f32,
  // Camera RGB → linear Rec.2020, recomputed on every white balance change. A `mat3x3f` is three columns,
  // each padded to 16 bytes.
  cam_to_work: mat3x3f,
  // Linear Rec.2020 → the display's primaries (sRGB or Display P3).
  work_to_disp: mat3x3f,
  // A RAW: black EV, white EV, steepness, pivot. A developed file: blacks, whites, contrast, knee.
  tone: vec4f,
  // Highlight blend strength.
  hl_strength: f32,
  // 0 for a RAW, 1 for a developed file — see `SourceKind`.
  display_referred: f32,
  // Global chroma gain − 1 (OKLCh) — the mask-saturation baseline every layer's own saturation adds to.
  saturation: f32,
  // How completely colour fades to neutral as a rendered value approaches display white.
  highlight_bleach: f32,
  // Per band: hue shift (radians), chroma gain − 1, lightness shift, unused.
  mixer: array<vec4f, 8>,
  // Film rendering: amount 0…1 (0 is the neutral rendering), interlayer spread in log exposure, unused ×2.
  film: vec4f,
  // Black and white: x 1 when on, y the side of `Packed::mono_lut`'s cube, z 1 to output the adjacency
  // map's log exposure, w unused.
  mono: vec4f,
  // Grain (`Grain::uniform`): standard deviation in stops (0 none), size as a share of the long edge, the
  // share a staining developer hides at the white point, 1 for a grain per dye layer.
  grain: vec4f,
  // Black and white's developer (`crate::developer`): adjacency, its reach as a share of the long edge,
  // development's contrast factor, compensation. `[0, 0, 1, 0]` does nothing. In colour all zero but the reach,
  // which carries halation's (`HALATION_MM`) when it is on: the map's blur reads it either way.
  developer: vec4f,
  // A named stock's curve (`crate::stock::Curve`), from a RAW: 1 on, the toe in stops from grey, its width,
  // the shoulder's strength.
  stock: vec4f,
  // The stock's shoulder in stops from grey, its width, unused ×2.
  shoulder: vec4f,
  // Colour film, spectrally (`crate::slide`): the amount 0…1 (0 off), the side of `Packed::slide_exposure`'s
  // cube, the side of the profile's response table, unused.
  slide: vec4f,
  // Where a layer response of 1 lands in the response table, per layer, and a decade of log H, as fractions
  // of its domain (`slide::SlideProfile`).
  slide_log_h: vec4f,
  // The tone controls as a *developed file's* (`display_tone`): what comes out of a stock's chain is a
  // rendered picture, as a scanned slide is, so Blacks, Whites, Contrast and the curve grade it rather than
  // reaching it through the neutral rendering it replaced. All zero without a stock.
  //
  // The knee is held at 1 — no shoulder. It exists to recover headroom when a developed file's exposure is
  // raised; here Exposure has already moved the film along its own characteristic curve, whose shoulder is
  // the sheet's, and a second one would be counted twice.
  slide_tone: vec4f,
  // The base curve's shape above middle grey: x the shoulder's order (0 = the logistic, as before
  // `PROCESS_VERSION` 6), y Contrast as an exponent for the S-curve that grades the straight segment,
  // since a fixed segment has no steepness of its own for Contrast to set. z 1 to compress out-of-gamut
  // colour in ACEScg (`Tone::gamut_ap1`), 0 in Rec.2020 as before `PROCESS_VERSION` 9. w unused.
  tone_shape: vec4f,
  // Colour grading (`ColorGrading::uniform`): per wheel — shadows, midtones, highlights, global — the OKLab a
  // and b to add and the lightness shift.
  grade: array<vec4f, 4>,
  // Colour grading: x 1 on, y the shadows/highlights crossover in OKLab lightness, z the overlap exponent.
  grade_split: vec4f,
  // Texture and Clarity (`Presence::uniform`): the fine and middle bands' change in contrast, −1…1; z 1 when
  // either is on, so develop reads the local-contrast map; w Dehaze, −1…1 (0: no haze map is read).
  local: vec4f,
  // A film negative inverted (`Negative::uniform`): the film base's camera RGB, and 1/γ; all zero for a positive.
  negative: vec4f,
  // An alternative process (`process::uniform`): x 1 cyanotype, 2 tintype, 0 none; y the cyanotype's peak
  // density, z its luminance absorbance per unit density. `process_dark`: the cyanotype's absorbance per channel
  // per unit density, or the tintype's black japan; w their luminance. `process_light`: the paper or the
  // silver, w its luminance. Linear Rec.2020, as shown.
  process: vec4f,
  process_dark: vec4f,
  process_light: vec4f,
}
