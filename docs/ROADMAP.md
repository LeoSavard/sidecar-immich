# Roadmap

What's known to be missing or wrong, roughly in the order it's worth doing. The specification is
[`DESIGN.md`](DESIGN.md); its build log records what the code actually does today.

## Correctness and quality

**Tone curve above middle grey.** The logistic base curve begins shouldering immediately above grey, so
raising exposure flattens upper-midtone texture — measured log-log slope 0.80 at +1 EV, 0.57 at +2 EV, 0.40
at +2.5 EV. The real constraint is headroom (sRGB holds 2.47 stops above grey; the default `whiteEv` asks for
4.5), so the shape can only decide *where* the compression is spent, not avoid it. A straight segment in
encoded space was built and measured: it raises +2 EV to 0.63 but pushes shadow contrast from 1.29 to 2.37 at
−2 EV, which is worse. A straight segment in *log-log* space holds ~0.98 at +1 EV with neutral shadows and
costs highlights above +2 EV. Either is a `ProcessVersion` bump. Decide with pictures, not slopes.

**Focus peaking measures the wrong thing.** It runs in the view pass over the proxy, where downsampling has
already averaged the high frequencies away, so it reports edge *contrast* rather than sharpness: a soft
high-contrast edge keeps a large gradient across two texels while crisp fine texture has no signal left.
Peaking has to be measured at full resolution.

The plan: the worker computes a focus measure on the full-resolution demosaiced luminance — high-frequency
energy (a Laplacian or difference-of-Gaussians at one to two pixels) normalized by the local standard
deviation over roughly sixteen, so crisp detail beats a blurred edge of the same contrast. Because it reads
the demosaiced image and not the developed one, it does not change when a slider moves: cache it per
`assetKey + ProcessVersion`, like the proxy. Reduce to proxy resolution by **max**-pooling, never averaging —
a single sharp pixel in a cell has to survive. Ship it as an 8-bit single-channel layer (mostly flat, so PNG
carries it cheaply), upload as `r8unorm`, and sample it as a third binding in the view pass, with the
threshold applied client-side so it stays live. `GET /api/assets/{key}/focus?edge=…` relaying a worker
`/v1/focus`.

**AF points from the RAW** are a separate, complementary overlay, not a driver for that mask. Most makers
record them in MakerNotes and ExifTool reads them where LibRaw does not: Sony `FocusLocation` (width, height,
x, y — a ZV-1 file here reads `5472 3648 4420 2409`), Canon `AFInfo2` with `AFPointsInFocus`, Nikon
`AFInfo2`, Fujifilm `FocusPixel`. Caveats worth respecting: the coordinates are in sensor space and need the
orientation and any future crop applied; the tag says where the camera *aimed*, not whether it succeeded; and
manual focus, focus-and-recompose, or an adapted lens leave nothing useful. So draw it as a marker, and make
it a place the loupe can jump to for a critical check — but let the peaking mask keep measuring actual pixels.

**Highlight reconstruction** leaves faint bluish blotches at the edges of reconstructed areas.

**X-Trans at full resolution** still runs LibRaw's Markesteijn on the CPU; only Bayer has a GPU demosaic for
loupe and export. Porting LibRaw's algorithm to WGSL (CDDL-1.0, so the port keeps that header) is the
remaining piece of milestone 5. `DSCF7611.RAF` is the test case.

**iPhone parity is unverified.** The ΔE2000 harness compares Chromium against the server; Safari on Metal has
never been in the loop. It needs a way to run the browser side on a phone and post results back.

## Reach

**Browsers without WebGPU (Firefox, older Safari).** The live editor needs WebGPU — that is the architecture,
and it's what makes the preview match the export. The way to cover other browsers is a server-rendered
fallback: the worker already renders proxies and loupe tiles, so a fallback client can send edit parameters
and display returned JPEGs, with sliders that commit on release instead of live. Slower, but the same
pipeline and the same output. Until then the app should say which browsers work rather than "no WebGPU".

**Wide gamut and HDR.** These are separate: Rec.2020 is a gamut, HDR is a transfer function plus headroom.

- *Rec.2020 SDR export* is close — the 16-bit TIFF path exists and needs the matrix and an ICC profile.
- *True HDR* needs an HDR canvas (WebGPU `toneMapping: { mode: 'extended' }`, not something to count on in
  Safari yet) and an HDR container: 10-bit AVIF with PQ or HLG (rav1e is BSD-2, so licence-compatible), or an
  ISO 21496-1 gain map as Apple writes. Settle the encoder question before the UI.

## Features after V1

Lens correction, then crop and straighten, then local masks — in that order, and for the reason given in
[§15 of the design](DESIGN.md#15-after-v1-lens-correction-crop-local-masks): lens correction redefines the
geometry that crops and masks are stored in.

## Housekeeping

- The Rust sources are hand-wrapped well past rustfmt's comfort at any `max_width`; either adopt a formatter
  and take the one-time diff, or leave it deliberately and say so.
- `deploy/remote-proxy/` is one machine's configuration, kept in the repo as a worked example.
