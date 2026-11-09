# The light sliders, measured (2026-11-08)

The owner, after using the integration build: *bringing Highlights down brings a lot of the midtones down as well
and the contrast in the image gets lost; any adjustment to Contrast, Highlights and Shadows feels like it removes
saturation.* This review measures what each light slider does beyond what it is aimed at, maps every side effect
to the code that causes it, and proposes a redesign whose aim is both a physically grounded rendering of light
and a picture that looks right. Nothing here has been built; the redesign waits on the owner.

The tone code measured is `main`'s (the integration branch does not touch it): `shaders/develop/tone.wgsl`,
`crates/color/src/spline.rs` (`tone_curve_lut`), `crates/color/src/params.rs` (`tone_params`,
`highlight_reach`).

## The tool

`web/scripts/tone-audit.mjs <url> <photo>…` renders a photo through the worker at its original edit and with one
slider moved, and compares the two **per tonal band of the original** (OKLab lightness: shadows < 0.4, lower
mids 0.4–0.55, upper mids 0.55–0.7, highlights 0.7–0.85, near white 0.85–0.97). Per band:

- **ΔL** — the mean OKLab lightness shift, ×100 (1 is about a just-noticeable step).
- **detail** — local contrast kept: mean |L − its 9 × 9 mean|, after over before. 1 keeps every texture.
- **chroma** — OKLab chroma kept (colourfulness). **saturation** — chroma over lightness kept.

`VARIANTS='{"name":{"tone.highlights":-100}}'` replaces the default set of variants.

## What the sliders do

Four D5 frames (doorway, overcast, `_D5N3075`, `survey_5285`) agree to within a few hundredths; the overcast
frame is shown.

| | shadows | lower mids | upper mids | highlights | near white |
|---|---|---|---|---|---|
| **Highlights −100** ΔL / detail / chroma | +3.1 / 0.97 / 1.12 | −1.1 / **0.69** / 0.98 | **−7.2** / **0.63** / 0.88 | −12.6 / 0.71 / 0.85 | −14.7 / 0.62 / 1.63 |
| **Shadows +100** | +7.3 / 1.47 / 1.28 | **+11.2** / 0.87 / 1.24 | **+5.6** / **0.69** / 1.09 | +0.6 / 0.75 / 1.01 | 0 / 0.84 / 1.00 |
| **Contrast +50** | −6.0 / 1.00 / **0.75** | −2.9 / 1.26 / 0.93 | +2.1 / 1.31 / 1.04 | +5.9 / 1.28 / **0.94** | +4.9 / 1.14 / **0.46** |
| **Exposure −1 EV** | −6.1 / 0.71 / 0.75 | −12.7 / 0.80 / 0.74 | −14.1 / 0.89 / 0.77 | −15.7 / 0.87 / 0.81 | −17.9 / 0.67 / 1.60 |

Saturation (chroma over lightness) stays within 0.97–1.03 everywhere except near white and under Contrast in the
highlights (0.88): the sliders keep saturation and let chroma follow lightness.

## Why

**1. Highlights is aimed at the midtones.** It is two mechanisms felt as one (`highlight_reach`'s doc):

| Highlights −100, split | shadows | lower mids | upper mids | highlights |
|---|---|---|---|---|
| the curve bump alone | 0 / 0.98 | −2.1 / 0.81 | **−7.2 / 0.74** | −11.6 / 0.92 |
| the white-point reach alone (+2 EV) | **+3.1** / 1.00 | +1.4 / 0.89 | 0 / 0.90 | −0.9 / 0.93 |

- *The bump* (`tone_curve_lut`) adds `h · w(x)` to the display-encoded curve, where `w` rises from 0 at
  encoded 0.25 to 1 at 0.75. Middle grey is encoded 0.46, where `w` is already 0.38: grey itself moves. The upper
  mids take nearly the full −0.15 offset — that is the darkening the owner sees.
- *Detail is lost because the control is a global curve.* Pulling a band down while white stays pinned
  flattens the curve's slope across that band, and a flatter slope is less local contrast for every texture in
  it (0.63–0.74 kept). This is inherent to doing Highlights and Shadows with one curve for the whole frame.
- *The reach lifts the shadows.* Below grey the tone map is a logistic normalised between the black and white
  points, so moving white 2 EV up re-spreads the shadows: +3 in lightness, +12 % chroma, from a control that
  should not touch them.

**2. Shadows mirrors it**: +11 in the lower mids and +5.6 in the upper mids, detail there down to 0.69.

**3. Colour follows lightness, so darker looks duller.** `tone_apply` scales linear RGB by the luminance ratio.
That keeps chromaticity — the physically right thing for *light*: half the light is the same colour, dimmer — but
in OKLab it keeps saturation, so chroma falls with lightness (Highlights −100: upper mids −7 in L, chroma 0.88;
Contrast +50: shadows chroma 0.75). On a display, colourfulness is what the eye reads as "saturation", so every
darkening reads as desaturation, and Contrast — which per-channel curves would make more colourful — makes the
shadows less so.

**4. The bleach takes colour from bright, legitimate colours.** The fade to neutral (`BLEACH_FROM` = 0.5,
display-linear) begins about a stop under white, by the *output's* luminance. Anything that raises a bright
colour — Contrast +50 — pushes it into the fade: highlights chroma 0.94 → 1.07 and near white 0.46 → 1.60 with the
bleach off. It is a stand-in for "this colour was not measured" keyed on the wrong quantity, and it is the same
mechanism behind the magenta of 2026-11-07: a clipped, reconstructed colour pulled below the fade keeps its cast.

## Proposal

The principle: **a scene is illumination times reflectance.** A rendering compresses the range of the
illumination, which a display can't hold, and keeps the reflectance — the textures and colours of surfaces — that
it can. Light controls should move the illumination; surfaces should stay as they were. That is both the
physically grounded reading of "pull the highlights down" (less light there) and what keeps the picture's
contrast and colour (Durand and Dorsey 2002; Paris, Hasinoff and Kautz 2011, local Laplacian filters).

1. **Highlights, Shadows, Whites and Blacks on a base layer.** Split log luminance into a base (the guided filter,
   #59, edge-aware on a log guide — already used for Clarity) and detail. The four controls act on the base only,
   in **stops from middle grey** with smooth weights (Highlights from about +1 stop up, Shadows from about −1 stop
   down; grey fixed), and the detail is added back unchanged. Targets: grey moves < 1 ΔL, detail kept ≥ 0.95 in
   every band, nothing outside a control's range moves by more than 1 ΔL.
2. **The white-point reach no longer reshapes the shadows**: below grey, the curve is anchored to grey and black,
   not to white.
3. **Colour by appearance, not by ratio.** Tone changes move lightness in a perceptual space (OKLab, or CAM16 as
   ACES 2.0 does, Apache-2.0) and keep **chroma** where light is removed, so a darkened sky stays as blue as it
   was. Contrast gains chroma with slope, as film and per-channel curves do; how much is a look, set by A/B.
4. **The bleach keyed on measurement, not brightness**: fade toward neutral only where the ingest marked samples
   clipped (alpha), by how much was reconstructed — which also ends the magenta. A bright blue sky that was
   measured keeps its colour at any output level.

The default rendering of a new edit — exposure, the straight segment through grey ("a stop in, a stop out"), the
shoulder to white — is unchanged by 1, 2 and 4; 3 changes how colours look whenever a tone control moves. It
would be `PROCESS_VERSION` 10, with older edits rendering as they did.

**What the owner decides:**
- Whether to build 1–4 as proposed, and in what order (1 and 2 fix the midtones and the lost contrast; 3 the
  colour; 4 the bleach and the magenta).
- For 3, chroma kept (darker colours stay as colourful) against saturation kept (today) — an A/B on two ports.
- How much colour Contrast adds — an A/B.
