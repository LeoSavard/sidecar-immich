# Roadmap

What's known to be missing or wrong, roughly in the order it's worth doing. The specification is
[`DESIGN.md`](DESIGN.md); its build log records what the code actually does today.

> **New features are paused** (owner's call, 2026-10-16). The current work is maintainability and
> standardization: see [`V2.md`](V2.md), which reorders what is below. Items here keep their place in the
> queue; the queue is just not what is being worked through at the moment. Immich stays the library manager
> (2026-10-20); V2's library seam narrows to what a filesystem *editor* needs, which is a configuration with
> no Immich in it.

## Correctness and quality

**Middle grey sat a stop below where the curve assumes — fixed, `PROCESS_VERSION` 3.** `normalize.wgsl`
scales sensor saturation to 1.0 and the tone curve reads middle grey at 0.18 of that, i.e. 2.47 stops of
headroom. A correctly exposed capture does not put grey there: cameras meter lower to keep highlight room.
**Measured against the cameras' own JPEGs: 1.15 stops lower (median), over 196 files sampled at random from
the library and stratified by body, lens and exposure — 11 bodies, 14 lenses, four brands.** The consequence
was this page's old "default rendering is darker than the camera's JPEG": Sidecar rendered the camera's
middle grey at **−1.31 EV** (IQR −1.53…−1.13). The method is a *ranked* comparison — the rank of middle grey
in the camera's rendering, read at the same rank in Sidecar's — so crop, scale and orientation cannot
corrupt it.

What it is not: the deficit is flat across scene brightness (darkest third −1.14, middle −1.19, brightest
−1.11), so it is not a metering artifact; and every lens group tracks its body, so optics play no part. It is
a constant, and it is now `Exposure::baseline_ev` (`BASELINE_EV`, 1.15), applied to a RAW before the person's
own Exposure and not at all to a developed file, which its own maker already rendered. Re-rendering all 196
puts grey at **−0.01 EV** (IQR −0.21…+0.15). An edit from before the bump keeps the picture it had: `migrate`
gives it no baseline.

*One number for every camera, decided rather than defaulted to* (owner's call, 2026-10-20). A table looks
tempting: the per-body medians are D3 1.20, X100F 1.19, D7000 1.13, D5 1.52, EOS M3 1.10, ZV-1 0.86, D3200
1.32, D3000 0.91, A7RM4 1.14, so one constant leaves −0.45 EV (D5) to +0.32 EV (ZV-1). But the spread
*within* a body is 0.17…0.45 EV — nearly the size of the spread between them — and it does not follow the
exposure program: pooled, manual 0.39, aperture-priority 0.35, program AE 0.44, so a camera shot by hand is
no more variable than one left to decide. A per-model number would therefore be fitting one photographer's
habits as much as a camera's behaviour, from one library. It is what the DNG spec's `BaselineExposure` is
for, and the moment to build it is when evidence from more than one library asks for it. Worth knowing
meanwhile that Nikon's Active D-Lighting is baked into the previews this was measured against, the likeliest
reason the D5 is the outlier. **The white point** — next.

**Nothing reached display white — fixed, `PROCESS_VERSION` 4.** Saturation sits 2.47 stops over the curve's grey while the default
`whiteEv` puts display white at 4.5 — `0.18 × 2^4.5`, four times saturation — so the top two stops of the
curve's range describe light the sensor cannot produce. Measured: **0 of 196 frames had a single pixel at
white**, brightest pixel median 0.611 linear; on the 66 frames where the camera itself clips, 0.689. A blown
window renders as light grey, which is also why highlight-reconstruction artifacts are *visible* there — the
camera's rendering clips them to white and they vanish. The measurement says `whiteEv` should default to
**3.63** (IQR 3.48…3.81), which with the baseline put the brightest pixel at 0.965 linear on clipping frames.

`DEFAULT_WHITE_EV` is now 3.63, and on the clipping frames the brightest pixel went from 0.689 linear to
**0.965** against the camera's 1.000. Grey stayed where the baseline put it, −0.01 EV, so the two changes do
not disturb each other.

The collision it had to get past, since the shape of it will recur: for a *developed* file `white_ev` is not
a white point but the Whites slider, read against the default in `display_tone`, and `EditParams` cannot tell
a RAW's edit from a JPEG's — so moving the constant would have put Whites at −43 on every existing edit of a
developed file, and the editor hardcoded `4.5` in two more places. The answer is the one the baseline
established: **store the reference with the edit**. `tone.white_zero_ev` is the `whiteEv` an edit calls
Whites 0; a RAW reads `white_ev` absolutely, a developed file reads the difference, and the editor reads the
same difference. An edit from before the bump keeps 4.5 and so keeps both its rendering and its slider.
A look pasted between photos now carries slider positions rather than references, for the same reason.

**What a blown highlight does — `PROCESS_VERSION` 5.** Two faults sat on top of each other. The output
transform rolled the top tenth off with a tanh shoulder that *never reached white*: display white came out at
252 of 255, and a channel had to reach 1.2 — twenty per cent over white — to encode as 255. And
`highlight_blend` neutralised a clipped pixel toward the **mean** of its three channels, which dims a blown
area while it corrects it, pulling it further from white still.

The owner's call, 2026-10-21: a detector saturates, so clipping should hit a wall. `soft_clip1` is now a plain
clamp — which also bleaches a bright colour toward white the way film does, one channel reaching the ceiling
before the next. And where every sample under a pixel saturated, no channel measured its colour: each reports
only "at least this much", so the honest value is neutral at the **brightest** of the three. That is
`Exposure::neutral_highlights`, its own control (**Neutral highlights**, default 25%), rather than more work
for a slider named recovery.

Measured on the window frame: the cast over near-white pixels falls linearly with the control — 19.0 at 0%,
16.0 at 25%, 13.5 at 50%, 11.2 at 75%, 9.2 at 100% — against the camera's 4.1. Pure white appears only at
100%, and that is not a second behaviour lagging behind: white *is* where the cast reaches zero, since a pixel
reads white only when its three channels are equal. The control is honest in the quantity it moves, which is
why it is named for neutrality rather than for white. Raising the destination above the brightest channel was
tried and changes nothing — the tone map scales RGB by the luminance ratio, so the absolute level is
discarded and only the chromaticity survives (0.86% white at 75% of the control, against 0.02%).
Frames whose highlights are bright but *not* sensor-clipped are untouched — the waterfall moved 0.00% to
0.08% — because coverage is what drives it.

What this still does not reach: the remaining gap to the camera, 9.2 against 4.1 even at full, is in
*partially* clipped pixels, which this control deliberately leaves alone and which the reconstruction owns.
That is the same place the hard edge comes from, below. `highlight_blend`'s own default (0.5, toward the mean)
is now the odd one out and wants re-measuring beside the new control.

**Tone curve above middle grey.** *Read this against the two entries above: the shoulder is not where the
darkness was.* The candidate curve at order 3 moved the median brightness by 0.02 EV. The logistic base
curve begins shouldering immediately above grey, so
raising exposure flattens upper-midtone texture — measured log-log slope 0.80 at +1 EV, 0.57 at +2 EV, 0.40
at +2.5 EV. The real constraint is headroom (sRGB holds 2.47 stops above grey; the default `whiteEv` asks for
4.5), so the shape can only decide *where* the compression is spent, not avoid it. A straight segment in
encoded space was built and measured: it raises +2 EV to 0.63 but pushes shadow contrast from 1.29 to 2.37 at
−2 EV, which is worse. A straight segment in *log-log* space holds ~0.98 at +1 EV with neutral shadows and
costs highlights above +2 EV (built on #21, `tone::straight_stops`). Either is a `ProcessVersion` bump.
Decide with pictures, not slopes.

Note what the white point does to this question: `straight_stops` returns the identity whenever `whiteEv` is
at or under the display's 2.47 stops of headroom, so if the white point comes down to what the sensor
delivers, a stop of scene light is a stop of picture all the way to white and there is no shoulder to choose.
The shoulder survives only as the price of headroom deliberately kept above saturation for reconstructed
highlights — and then the order is exactly the knob for it.

**Focus peaking at full resolution — done**, in `sidecar_engine::focus`. It used to run in the view pass over
the proxy, where downsampling had already averaged the high frequencies away, so it reported edge *contrast*
rather than sharpness. The worker now measures it on the full-resolution demosaiced luminance: the Laplacian
of a lightly blurred luminance over the local standard deviation of the unblurred one, so a crisp edge
outscores a blurred edge of the same amplitude, and uncorrelated per-pixel noise — whose Laplacian and
deviation scale together — does not light up. Because it reads the demosaiced image rather than the developed
one it does not move when a slider does, so it is cached beside the proxy under the same generation marker and
dropped with it. It reduces to each proxy's size by **max**-pooling, never averaging, so a single sharp pixel
survives its cell; it ships as an 8-bit PNG, uploads as `r8unorm`, and is sampled as a third binding in the
view pass with the threshold applied client-side, so the slider stays live. `GET /api/assets/{key}/focus?edge=…`
relays the worker. Five tests cover it, including the two that matter: sharp beating blurred at equal contrast,
and max-pooling keeping a pixel that averaging would lose.

**AF points from the RAW — done**, in `sidecar_raw::af`: a complementary overlay, not a driver for that mask.
ExifTool reads them from MakerNotes where LibRaw does not (Sony `FocusLocation`, Canon and Nikon `AFInfo2`,
Fujifilm `FocusPixel`), the coordinates are brought out of sensor space through the orientation, and the
editor draws a marker the loupe can jump to for a critical check. The caveats stand and are why it stays a
marker: the tag says where the camera *aimed*, not whether it succeeded, and manual focus, focus-and-recompose
or an adapted lens leave nothing useful.

What is left of peaking is in "Subject-aware tools" below: weighting the measure by the subject mask, and
ranking eyes and faces first. Noise is already handled.

**Highlight reconstruction — low priority, and it is not what this entry used to claim.** The item said
"leaves faint bluish blotches at the edges of reconstructed areas", and no such artifact could be reproduced:
the blotches in the obvious test photo are the sky and the trees beyond a window, confirmed by the owner and
by switching reconstruction off. What the pass *is* for is narrower and worth stating correctly, because it
is not detail recovery.

A raw file clips per channel, and not at the same scene luminance: a neutral white puts its largest raw
response in green, which is exactly why green's white-balance multiplier is 1 and red's is above it, so green
pins first and red keeps recording for roughly log₂(red multiplier) — about a stop — afterwards. Measured on
this repository's own fixtures: on `_D5N3075.NEF` green clips on 12.5 % of samples, blue on 10.0 % and **red
on only 2.3 %**, so about a tenth of that frame has green pinned while red is still a live measurement; on
`DSC01899.ARW` green clips on 0.14 % of samples and **red and blue on none at all**. In that band
reconstruction is solving for the pinned channel from two measured ones, not inventing. Above it — every
channel pinned, which is the core of a blown window — there is nothing left and anything drawn is inpainting
from the neighbours.

**So the value is avoiding false colour, not recovering texture**, and it matters most to exactly the
workflow that pulls highlights down with the curve: in the transition band green is frozen while red and blue
climb, the white-balance gains turn that into a magenta drift, and a tone curve lifts the tinted near-white
into visible range where it reads as a pink patch. Left clipped to white it would never be seen. The develop
pass's `highlight_blend` is a second net for the hopeless case.

The measurements below stand, and `sidecar-dev hilite <raw> <dir> [--crop]` is how to repeat them: it buckets
the proxy by its alpha — the fraction of source samples that clipped under each output pixel, so 0 is
untouched picture and 1 the heart of a blown area — and prints the mean chromaticity `r/(r+g+b)`, `b/(r+g+b)` of each bucket in white-balanced
camera RGB, where a neutral reads 0.333. It also counts, on the mosaic itself, how often each colour actually
reaches saturation, which is what decides whether a blown area still holds an honest measurement.

On `_D5N3075.NEF` (Nikon D5, a blown window, as shot 4,640 K; R clips on 2.3 % of samples, G 12.5 %, B 10.0 %)
the picture outside blown areas sits at r 0.344, b 0.337, and the fully-clipped core at **r 0.279, b 0.396**.
Turning reconstruction off (`SIDECAR_NO_HILITE=1`, the diagnostic switch in `ingest.rs`) puts that same core at
**r 0.353, b 0.400** — so reconstruction lifts green, which is its job, but takes red down with it, and that
red deficit is the cast. On `DSC01899.ARW` (Sony ZV-1, only green clipping, 0.14 % of samples) it behaves:
green comes back and red and blue fall proportionally.

*Ruled out, by experiment rather than by reading:*

- **The `wgt == 0` fallback in `hilite_reduce.wgsl`** (every tap in a 5×5 clipped, so the level has no colour
  to average and falls back to "white"). It is a real latent bug — raw `(1, 1, 1)` is not neutral, since white
  balance still multiplies it, so it paints lilac, and neutral would be `1/wb`, the convention the
  desaturation target four lines above already uses. But painting the fallback pure green leaves the picture
  visually identical and moves the core's chromaticity by 0.002: the path is reached, then diluted to nothing
  as `assemble` renormalizes each finer level against real data. Worth fixing for tidiness, not for this.
- **The desaturation in the same pass** (`P.desat`, 0.3). Setting it to 0 changes nothing visible.
- **The `scale` that rescales the coarse estimate**, in `hilite_doub.wgsl`. A clipped channel's ratio
  `sample / estimate` is an underestimate — the sample stopped at saturation — so averaging it in drags the
  scale down and steals from channels that did not clip. That reasoning is sound and dropping clipped channels
  from the average is probably right on its own merits, but **it cannot be this artifact**: `doub` replaces the
  pixel with `upsm * scale`, and chromaticity is scale-invariant, so `scale` sets brightness and the *hue comes
  entirely from the pyramid estimate*. Measured: no chromaticity change at all.
- **The blotches in the obvious test crop.** `--crop` picks the densest patch of partly-clipped pixels, and on
  this photo it lands on soft lilac and grey blobs in the window that look exactly like the artifact — and
  they are still there with reconstruction switched off. They are the scene (glare and whatever is outside the
  glass), not the pass. Anyone chasing this should run that A/B before trusting their eyes, as should have
  happened here first.

*Where to look next, if anyone returns to this.* The hue of a reconstructed pixel is the pyramid estimate's
hue, so that is the only place a shift can come from. In a fully-blown core there are no unclipped taps
nearby, so the estimate is carried in from coarse levels — the surroundings — and a window's surroundings are
the room, which is far from the window's own colour. That fits the measured red deficit; the next experiment
is to dump each pyramid level as an image and find the level where the hue departs, rather than guess again.
But note what the per-channel census above implies: the fully-blown core is the case with no data in it, and
the band that *does* carry data is the one where the pass already measures correctly. So this is a small
refinement to a stage that is working, which is why the entry now sits low rather than at the top.

**X-Trans at full resolution — parked, and out of milestone 5** (owner's call, 2026-10-16: the Fuji body is
gone, and nobody else has asked). It still runs LibRaw's Markesteijn on the CPU; only Bayer has a GPU demosaic
for loupe and export, so an X-Trans file is slower, not broken. Porting LibRaw's algorithm to WGSL (CDDL-1.0,
so the port keeps that header) is what it would take, and `DSCF7611.RAF` is the test case if demand ever
appears. Milestone 5 is otherwise complete; this is no longer what is holding it open.

**iPhone parity is unverified.** The ΔE2000 harness compares Chromium against the server; Safari on Metal has
never been in the loop. It needs a way to run the browser side on a phone and post results back.

## Reach

**Someone else's folder structure.** The storage template is tuned to one library — this one. `render_dir`
understands date tokens only (`{{yyyy}}`, `{{yy}}`, `{{y}}`, `{{MMMM}}`, `{{MMM}}`, `{{MM}}`, `{{M}}`,
`{{dd}}`, `{{d}}`) with month names in English or French, and a `Root` carries an `immich_prefix` and a
`per_user` flag shaped by Immich's own internal layout. Note what this does *not* affect: the sidecar is
written beside the original, so **writing is already layout-independent**. What the template constrains is
*finding* the original from a share — hash, name and capture date — which is the step that fails on a
library laid out any other way, leaving only Immich's own search and the picker.

Worth doing, in order of value: fall back to the name walk (`sidecar_template::find_by_name`) when the date
path misses, which makes an unrecognised layout degrade instead of failing; more tokens and more month
locales; a configured layout that need not be Immich's, including the plain case of "everything in one
folder" and the common `{{yyyy}}/{{event}}/` shape where a folder name is not a date at all. The measure of
success is that someone whose photos sit in `2019/Holidays/` rather than `2019/05.May/2019-05-02/` can point
Sidecar at them and have sidecars land correctly. See [`V2.md`](V2.md), Workstream B.

**Browsers without WebGPU (Firefox, older Safari) — done.** `POST /api/assets/{key}/preview` develops the
cached proxy with the same shaders and parameters the browser would have used and returns a JPEG, so the
picture is the same; it just arrives when a slider is released rather than while it moves. One request is in
flight at a time and changes arriving mid-render coalesce into a single follow-up, so a drag costs one render,
not one per frame. The colour packer is wasm, so the white-balance readout and the curve stay live without a
GPU. The loupe and export already ran on the server and are unaffected.

What's hidden in fallback, because it lives in the view shader rather than the develop pass: the histogram,
the clipping overlays, focus peaking and the AF marker. Moving those server-side would mean returning the
histogram alongside the JPEG and drawing the overlays into it.

**Wide gamut and HDR.** These are separate: Rec.2020 is a gamut, HDR is a transfer function plus headroom.

- *Rec.2020 SDR export* is close — the 16-bit TIFF path exists and needs the matrix and an ICC profile. The
  profile need not be shipped as a third-party binary: it can be synthesised from primaries plus transfer
  function with lcms2 (MIT) or moxcms (Apache-2.0 OR BSD-3, pure Rust).
- *True HDR.* **The container question is settled; the display question moved to the desktop.** ISO 21496-1
  was published in July 2025, and both Apple's Adaptive HDR and Google's Ultra HDR build on it; Android 15
  writes the Ultra HDR XMP in APP1 *and* the ISO metadata in APP2 by default, so a file carries both and
  legacy readers still work — **write both rather than choosing**. Ranking: gain-map JPEG first (displayed as
  HDR on Apple, Android and Chrome, degrades to a correct SDR JPEG everywhere, and libultrahdr is Apache-2.0),
  AVIF with PQ or HLG second (libavif, dav1d and rav1e are permissive, but Chrome gates AVIF *gain maps*
  behind a flag), JPEG XL third (the format supports gain maps; no browser decodes them). Firefox is the
  consistent hole on desktop and Android.
  *This entry used to gate HDR display on "an HDR-capable WebGPU canvas in Safari". That framing was wrong
  for a desktop build and was corrected on 2026-10-17:* the browser HDR canvas is Chrome-led and partly
  proposal, while **native wgpu is the right target**. One internal representation serves every platform —
  linear float plus BT.2100, which is what both WebGPU's `rgba16float` + `rec2100-display-linear` and
  Wayland's `windows_scrgb`/`windows_bt2100` descriptions amount to. Whether wgpu actually exposes HDR
  swapchain formats is unverified and is on the measurement list in [`V2.md`](V2.md).

**HDR originals keep their gain map.** An iPhone HEIC is an SDR picture plus a gain map — a small auxiliary
image telling an HDR screen how far past SDR white to lift each area — and Photos shows it HDR. Sidecar reads
only the SDR picture (ImageMagick and `heif-convert` decode the primary image and drop the auxiliary one) and
exports SDR, so an edit looks flat beside its original. In order:

1. *Read the gain map* at decode, with its metadata — **done**, in `sidecar_raw::gain_map`, through a shared
   `sidecar_raw::heif_aux` module: one `heif-convert --with-aux` run per photo, every auxiliary image written
   out, each reader picking the one it wants, so adding a second reader costs no second decode —
   `portrait_depth` is exactly such a reader and shares this module unchanged. Checked against a real
   iPhone 17 Pro HEIC rather than assumed: the aux image (`urn:com:apple:photo:2020:aux:hdrgainmap`) is named
   `<stem>-urn_com_apple_photo_2020_aux_hdrgainmap.png`, 8-bit grey at exactly half the main image's linear
   size — the same resolution convention `portrait_depth` already found for the Portrait Effects Matte.
   `exiftool` carries two tags in the gain map's own group, `HDRGainMapVersion`/`HDRGainMapHeadroom`, plus two
   MakerNotes scalars that sound related (`Apple:HDRHeadroom`, `Apple:HDRGain`) but aren't confirmed to mean
   what their names suggest; all four are read and carried, unapplied.

   **No reconstruction formula is implemented, and that's deliberate, not an oversight.** Apple hasn't
   published one, and the independent reverse-engineering attempts that exist
   ([heif-hdrgainmap-decode](https://github.com/m13253/heif-hdrgainmap-decode),
   [AppleJPEGGainMap](https://github.com/grapeot/AppleJPEGGainMap)) call their own results "known to be
   incorrect" or "unverified" — one guesses `hdr = sdr × 8^gain` "because it looks good," the other doesn't
   claim a formula at all. A community decode (not verified here) gives a linear form instead:
   `hdr = sdr × (1 + (headroom − 1) × gain)`. Even Photos on iPhone and on Mac are reported to render the same
   file differently. A wrong formula looks plausible and is silently wrong — worse than not rendering HDR —
   so stage 2 needs an actual HDR display to check against, not another guess from documentation that doesn't
   exist. (Both reverse-engineering projects are GPL-licensed or unlicensed; nothing from either is usable
   here regardless — CLAUDE.md — this was read for background, not ported.)

   `dev/hdr-gainmap-probe` builds that check: real, controlled Ultra HDR JPEGs (ISO 21496-1, via Google's
   `libultrahdr` — MIT/Apache-2.0) at known gain and headroom values, to view on an iPhone and measure with a
   colorimeter against that format's own documented formula (`HDR = SDR × headroom^gain`, the grounded form
   of the guesses above). Measuring tells us whether Safari actually renders the open standard faithfully,
   and if Apple's own `HDRGainMapHeadroom` behaves the way this format's `maxContentBoost` does — the best
   starting point available for stage 2's formula, once measured rather than assumed.

   **A second, newer mechanism exists and isn't read yet either.** Some files also carry an ICC profile
   (`Display P3 Primaries; PQ (Adaptive Gain Curve …)`) with a binary `HD Gain Map Info` tag — Apple's iOS
   18+ "Adaptive HDR", introduced alongside ISO 21496-1 at WWDC24 and still not interchangeable with
   Adobe/Google's ISO files in practice. Whether this is the mechanism actually driving HDR display for a
   given photo, with the auxiliary-image gain map as a compatibility fallback, or something else, is open.
2. *Carry it through the edit.* Edit the SDR picture as now and reapply the original map to the result, as
   the gain-map standard intends; exposure changes the base, so the map's headroom may need scaling with it.
   Geometry (crop, straighten, orientation) must transform the map exactly as it does the picture. Blocked on
   deciding stage 1's open formula question first — there's nothing to carry through correctly otherwise.
3. *Export with it*: an Ultra HDR JPEG (ISO 21496-1 with the Adobe/Google XMP fallback) is what iOS and
   Android both display as HDR. Google's `libultrahdr` is Apache-2.0 — check it through `cargo deny`
   (or its C build) and add it to NOTICE before depending on it. HEIC output stays off the table (HEVC).
4. *Show it in the editor* — last, and least certain. On the desktop this is a native wgpu HDR swapchain
   rather than a Safari canvas (see "Wide gamut and HDR" above); until it works, the editor stays SDR and
   says the export will be HDR.

**Portrait depth maps (background blur, background replacement).** An iPhone Portrait-mode HEIC carries a
depth-correlated auxiliary image too, read by `sidecar_raw::portrait_depth` through the same external-decoder
boundary as the gain map above (`heif-convert --with-aux`, no libheif link). This belongs on the standard
branch, not `sidecar-ml`: the signal is already in the file, not a model's guess from pixels, and needs no
GPU, weights, or the `-ml` image.

*Confirmed, not assumed, against a stratified sample of this user's own library* (205 of 7,588 HEICs,
`heif-convert` 1.23.4 + `exiftool` 13.25, iPhone 14 Pro through 17 Pro, iOS/software ~18 through ~27): 91% of
HEICs carry an HDR gain map (near-universal, unrelated to depth); **~16% (33/205) carry a literal raw
disparity aux image** (`urn:mpeg:hevc:2015:auxid:2`, named by Apple's own `XMP-apdi:AuxiliaryImageType =
disparity`); **~16% (32/205) carry the derived Portrait Effects Matte**
(`urn:com:apple:photo:2018:aux:portraiteffectsmatte`). These overlap but aren't the same set: 18 of the 33
disparity-bearing photos have *no* effects matte at all — segmentation apparently needs a recognized subject
(person or pet) to produce a matte, while the raw stereo capture doesn't, so the raw map is the more universal
of the two whenever Portrait mode was used. `ExifIFD:CustomRendered = "Portrait"` is a cheap, already-present
flag for "the Camera app rendered this as Portrait," present on files from both groups — worth checking before
paying for an aux-image decode. The two are confirmed as the same Apple API Apple documents publicly:
`CGImageSourceCopyAuxiliaryDataInfoAtIndex` plus `AVDepthData`/`AVPortraitEffectsMatte`
`.init(fromDictionaryRepresentation:)` — see
[Image Depth Maps Tutorial for iOS](https://www.kodeco.com/8246240-image-depth-maps-tutorial-for-ios-getting-started),
which also notes HEIC keeps depth as metadata where a JPEG would store it as a second image (not checked here:
this library has no Portrait JPEGs to confirm against).

*The two are not interchangeable, and neither is what it might look like at first.* The effects matte is a
continuous 8-bit grey image at exactly half the main picture's linear size (1512×2016 of 3024×4032), spanning
the full 0–255 range rather than two flat plateaus — Apple's *Portrait Effects Matte*: how much of the
baked-in blur Photos already applied at each pixel, not a distance. The raw disparity image is a real
geometric signal, carrying its own `XMP-depthData` camera-calibration block (focal length, principal point,
radial lens distortion — `AVCameraCalibrationData`, serialized), but at a **fixed** low resolution regardless
of the main photo's size: measured 576×768 on both a 3024×4032 and a 4284×5712 capture from the same iPhone 17
Pro — a twentieth to a thirtieth of the main image's pixel count, always, because the stereo correspondence
search that produces it runs at a constant working resolution, not the sensor's. Every example found reports
`XMP-depthData:Accuracy = relative`, **never `absolute`, even on these LiDAR-equipped Pro models** — Apple's
Camera app evidently publishes dual/triple-camera stereo disparity into the still photo, not the LiDAR's
metric range. **Metric depth is representable in Apple's own format** (`AVDepthData.DepthDataAccuracy`'s
other case is `.absolute`) **— the format allows it, but ordinary Portrait captures on this hardware don't use
that case**; whether some other capture path (a LiDAR-scanning app, Cinematic video) ever does is still open.
The raw samples are quantized per shot, and the file carries what's needed to undo that:
`XMP-apdi:IntMinValue`/`IntMaxValue` (0/255 in every example) rescale linearly onto that photo's own
`FloatMinValue`/`FloatMaxValue` (e.g. 0.0616–0.571 on one frame, consistent with inverse metres) — reading
those two tags and applying the rescale is the next step for `portrait_depth::read`, which today returns the
raw 0–1 samples, not Apple's own disparity units.

*DNG (ProRAW) gets a reduced, structurally different subset.* The sampled library's DNGs (80 of 11,617, all
iPhone 17 Pro) carry the *derived* mattes — the same URNs (`portraiteffectsmatte`, `semanticskinmatte`,
`semanticskymatte`) turned up directly in about a third of them — but as extra TIFF SubIFDs tagged `Semantic
Name` (DNG 1.6's semantic-mask extension, added alongside ProRAW; Adobe and Apple co-developed it, though
Adobe's own tools don't read it back) rather than a HEIF auxiliary-image box. **None of the 80 DNGs sampled
carried the raw disparity array or its `XMP-depthData` calibration block at all** — only the mattes. So a DNG
needs its own TIFF-SubIFD reader; `heif-convert` (a HEIF-only tool) finds nothing in one, and
`portrait_depth::read` doesn't attempt it.

Checked and ruled out, rather than left open: these ProRAW DNGs carry no `NoiseProfile`, `GainMap`,
`OpcodeList`, or `CameraCalibration` tag at all (grepped 5–15 files directly), so there's no free per-camera
noise model sitting unused for the wavelet denoiser (roadmap, "Detail") to pick up, and no shading/vignette
table beyond what lensfun already supplies. They also carry no HDR gain map — expected, since a gain map
describes how an *already-rendered* SDR picture should be lifted for HDR display, and a DNG hasn't been
rendered yet; the gain map work below is a HEIC/JPEG-export concern, not something a DNG path would also
unlock. `AsShotNeutral` is present but always `1 1 1` on every file checked — Apple doesn't put the as-shot
white balance there; LibRaw already gets it from `cam_mul` instead (DESIGN.md §5), so this isn't a gap.

*Resolution and registration.* The effects matte is half the main image's edge, same aspect, the same HEIF
rotation libheif already applies to the main image — close enough to the proxy's frame that it could slot in
beside the existing model masks (DESIGN.md §15) once resized. The raw disparity image, being fixed-resolution
rather than scaled to the photo, needs more work: upsampling naively would blur edges (hair, fingers) well
past what the effects matte already does at half-res, so anything using the raw map for a *sharp* mask (as
opposed to a coarse depth ordering) wants edge-aware upsampling. **That upsampling now exists** —
`client/depth_refine.wgsl` (joint bilateral, DESIGN.md §7 "Depth") was built for the model's own 518² map and
takes an arbitrary low-resolution depth map against the photo's luminance, which is exactly this problem; the
disparity map's 576×768 is the same order as 518². Either map still arrives in the *original's* frame, not the
proxy's downsampled one, so it needs its own resize rather than reusing `mask_images`' assumption that every
mask already matches the proxy.

*What it's good for, roughly most to least ready.* **Most of this list has been overtaken by the depth work
that shipped since** (#13, #15, #22): a `depth` mask from Depth Anything V2 Small, range layers, lens blur and
haze by distance, and the joint-bilateral refinement all exist now, for *every* photo rather than the ~16%
that carry Apple's map. What is left for Apple's own depth is therefore not a feature of its own but a
**second source** for a pipeline that already works — see the dated note at the end of this section.

- **Depth-aware grading — shipped**, off the model's map: a `depth` mask with a `range` band is a layer like
  any other, carrying the same exposure, white-balance, contrast and curve blending. Apple's map would change
  where the mask's values come from, not what can be done with them.
- **A cheaper, possibly better subject mask for the ~16% of photos that have one.** The raw disparity (or the
  effects matte, where present) is real multi-view geometry, not a single-image model's guess — for exactly
  the photos that carry it, it could stand in for `sidecar-ml`'s BiRefNet `subject` mask entirely, skipping a
  GPU inference pass. It needs the edge-aware upsampling above first, and BiRefNet still has to run for the
  other ~84% of photos, so this is a conditional win, not a replacement.
- **Weak supervision or a sanity check for the ML masks.** Short of replacing BiRefNet outright, the same
  signal can validate it: where BiRefNet's `subject` mask disagrees sharply with the depth-derived rough split,
  that's a cheap low-confidence flag, cheaper than a second model. The thousands of effects-mattes and semantic
  mattes (sky, skin, hair, teeth, glasses) already sitting in this library are also free, human-unlabelled
  ground truth if `sidecar-ml`'s own models are ever fine-tuned — Apple's segmentation running for free on
  photos this library already has.
- **Background blur ("portrait mode" after the fact) — shipped**, and it was indeed new rendering work rather
  than a slot into the mask weights, as this section predicted: `EditParams.lensBlur` (amount, focus, range)
  gathers with a spatially-varying radius from the depth map, weighting each tap by the inverse of its disc's
  area so a bright point becomes a bright disc (DESIGN.md §7). It runs off the model's map, so it is not
  limited to Portrait captures.
- **Background replacement.** A mask is already most of a green-screen-free cutout; combined with `sidecar-ml`'s
  `subject` mask (on the `-ml` image), the two could cross-check each other — but replacement itself
  (compositing a new background, relighting the cut edge) is also new work.
- **Haze by distance — shipped**, and it did not need metric depth after all: `EditParams.haze` grades by the
  map's relative ordering, which is enough for an aerial-perspective look even though it is not metres.
  *Relighting* by true distance would still need the metric case Apple's format allows but these captures
  don't use.
- **Depth-aware noise reduction or sharpening** is still open, and for a reason unrelated to depth: per-layer
  detail (roadmap, "Detail per layer") needs a layer's value to reach the detail passes on the worker, which
  run at full resolution outside the develop pass.
- **AF point corroboration.** ExifTool's `Apple:AFMeasuredDepth` (a single scalar, present on nearly every
  file regardless of Portrait mode, units unconfirmed — not the same thing as `Apple:FocusDistanceRange`,
  which is the lens's focus-position bracket, not a scene reading) is a second, independent depth-ish signal
  with no aux-image work at all, worth cross-checking a disparity map against rather than trusting either
  blind.
- **Parallax / "spatial" effects** need a true per-pixel metric map at usable resolution; disparity here is
  relative and fixed-resolution, so this is the least ready use.

*Licensing.* No new exposure: the gain map section above already establishes that reading any HEIC auxiliary
image goes through the same opt-in external decoder (`HEIF=1`, `heif-convert`), never a linked libheif.
`heif_aux::AUX_DECODER_ENV` is a separate override from `decoded::HEIF_DECODER_ENV` because
ImageMagick's `magick`/`convert` HEIF delegates have no equivalent of `--with-aux` — only `heif-convert` is
known to expose auxiliary images at all, so there is exactly one built-in command to try, not a choice of
three like the main decode. `sidecar_raw::heif_aux` runs it once per photo and hands every auxiliary image to
whichever readers want one (this, and the gain map), so a photo isn't decoded once per reader.

*Status.* `sidecar_raw::portrait_depth::read` extracts and classifies the aux image and decodes it to a plain
0–1 buffer, pinned against both the repo's committed fixture and the real-library numbers above in tests.
Still open: undoing the per-shot `IntMinValue`/`FloatMinValue` quantization described above, a DNG reader,
calling any of this from the worker or exposing it over HTTP, and the resize into the proxy's frame. The first
real use is no longer an open question — it is to stand in for the model's `depth` mask where the file carries
a map, which is what the dated note below spells out.

*Decision (2026-10-13): parked behind the HDR gain map work below.* The depth signal only ever appears on
~16% of photos and its best-understood use (a depth mask for grading) is a small, well-scoped follow-up
whenever it's picked back up — but it doesn't unlock the headline ask (a blurred background) by itself, since
that needs a new spatially-varying blur renderer that would work just as well off `sidecar-ml`'s `subject`
mask, for all photos, once it exists. The gain map is universal (91% of HEICs) and fixes a visible regression
(an edited photo looking duller than its original) rather than adding a new capability, so it's the correct
next step. This section stays as the record of what was verified, for whenever the depth mask or the blur
renderer comes back up.

*Where that leaves it (2026-10-16).* The blur renderer came back up and shipped, off a model's map, for every
photo — so the prediction above held, and this branch's remaining value is no longer a feature. It is a
**second source for a pipeline that is now complete**: `Ml::depth` runs Depth Anything V2 Small at 518² and
stretches its relative inverse depth between the photo's 1st and 99th percentiles into the cached `depth`
mask, and Apple's disparity is the same kind of quantity — relative inverse depth, no unit, no origin, at a
comparable 576×768 — measured by stereo rather than guessed from one image. Substituting it where the file
carries one is a contained job with a real payoff (no GPU inference pass, and genuine geometry instead of a
single-image estimate, which should be better exactly where the model is weakest: a mirror's reflection, a
photograph of a photograph, a flat textured wall). What it needs: the `IntMinValue`/`FloatMinValue` rescale
above so the values mean what the model's do, the same percentile stretch so a `range` band set on one source
means the same on the other, the resize into the proxy's frame, and a decision about whether swapping a cached
mask's source needs a `MASK_REVISION` bump (it does, if a photo's existing layers are to keep their meaning).
Worth doing with pictures: the honest test is whether an Apple-sourced `depth` mask blurs a Portrait capture's
background better than the model's does, on the same photo.

**Editing non-RAW images — done, in `sidecar_raw::decoded`.** JPEG, PNG and TIFF decode in-process; HEIC runs
an external decoder. They skip the RAW-only ingest stages (hot pixels, demosaic, highlight reconstruction all
assume a mosaic) and join the pipeline where the RAW path does — linear light, own primaries — so
downsampling, the focus measure, the develop pass and export are unchanged. The colour model needed no
special case: `SourceSpace::camera_color` hands the pipeline a `CameraColor` whose "camera" space *is* sRGB
or Display P3 with unity as-shot multipliers, so `camera::resolve` returns unity multipliers and the right
primaries matrix, and temperature and tint become relative shifts around D65.

The tone curve needs its own path, though. The develop pass renders scene-referred light, and a JPEG or
HEIC is already rendered for display; fed through the RAW curve it was tone-mapped twice — white at 82 %, the
top fifth of the range pressed into a tenth. Developed files now take a display-referred path (DESIGN.md §7,
"Developed files"): identity at default settings, controls that act on the picture as it is, and none of the
RAW stages that promise recovery the file can't give (highlight blend, gamut compression, the soft clip).

What's left, if it's ever wanted: Adobe RGB is treated as sRGB (its primaries would be a third matrix);
the white-balance slider still reads in kelvin (a D65 file shows about 6,500 K, +10 tint) rather than as an
explicit relative shift from zero.

The leeway varies a lot by format, and that shapes what the editor should offer. *TIFF* is the easy case when
it's what it usually is (uncompressed or losslessly compressed, sometimes 16-bit): no artifact concerns, the
only loss is recovery headroom. *JPEG* is 8-bit, already gamma-encoded, and typically 4:2:0
chroma-subsampled, so aggressive tone or white-balance moves posterize and show colour-selective (mixer)
fringing much sooner than the same move on a RAW — modest global adjustments are fine, recovery-style ones
aren't. *HEIC* is 8- or 10-bit and usually Display P3, so it has meaningfully more room than JPEG.

*The HEIC licensing trap, checked against LICENSE files rather than recalled.* libheif — the de facto
decoder — is LGPL-3.0-or-later; linking it, even dynamically, is off limits (CLAUDE.md). HEVC also carries
patent-pool royalty exposure (MPEG LA, HEVC Advance, …) independent of any copyright licence. So Sidecar
links no HEIF decoder at all: it runs one as a separate program if the operator installed one, the boundary
NOTICE already documents for ExifTool, and the published image ships none (`--build-arg HEIF=1` opts in).
For any format Sidecar might one day *write*, **prefer AVIF**: `libavif` and `dav1d` are both BSD-2-Clause
and AV1 is royalty-free, avoiding both problems — but note AVIF decodes AV1, not HEVC, so it is not a way to
open the HEIC files a phone actually produces.

**Non-NVIDIA GPUs (iGPUs, AMD and Intel dGPUs).** Nothing in the renderer itself is NVIDIA-specific:
`sidecar_engine::Gpu::new_headless()` opens whatever Vulkan adapter wgpu finds, and no WGSL shader or Rust
pipeline stage reaches for an NVIDIA-only feature — the "cooperative matrix configurations" line wgpu logs on
startup is naga's own capability enumeration, not something Sidecar's code depends on. What's actually
NVIDIA-specific today is the container plumbing, not the rendering: `deploy/compose.yaml` and
`deploy/remote-proxy/compose.yaml` both request `devices: [nvidia.com/gpu=all]` (the NVIDIA Container Toolkit's
CDI device), the Dockerfile sets `NVIDIA_DRIVER_CAPABILITIES` and installs only the libraries NVIDIA's Vulkan
ICD needs (`libegl1`, `libxext6`, `libx11-6`), and DESIGN.md's GPU-sharing section assumes Immich's own ML
and transcoding share the same NVIDIA card, which doesn't hold on an Intel or AMD box — Immich's ML runs on
CUDA or OpenVINO, not ROCm.

An AMD or Intel deployment needs a compose profile passing through `/dev/dri` (render nodes) instead of the
CDI device, confirmation that the base image's Mesa Vulkan ICD (RADV for AMD, ANV for Intel) is present
instead of relying on NVIDIA's proprietary one, and dropping the shared-VRAM-budget assumption where Immich
isn't also on that card. Worth testing on real AMD and Intel hardware before documenting it as supported —
Mesa's compute-shader support has historically lagged NVIDIA's on some corner cases the ingest and develop
passes might hit, and that needs checking with pictures, not assumed from the driver's feature flags.

## Image processing: primitives first

Surveyed on 2026-10-17 (see [`research/desktop-port-survey.md`](research/desktop-port-survey.md) for the
sources behind every claim here). The organising insight is that **a small number of primitives unlock many
features, so build the primitives rather than the features.** One Gaussian/Laplacian pyramid with coefficient
remapping buys clarity, texture, local contrast, shadow/highlight recovery and tone mapping at once; one
guided filter buys dehaze refinement, mask feathering and better depth upsampling. Everything below is
implementable from published papers — none of it requires reading GPL code.

**The guided filter** (He, Sun and Tang, ECCV 2010 / TPAMI 2013) is the highest-leverage single item in the
survey: four box filters over I, p, I·p and I², then per-pixel algebra, in linear time regardless of kernel
size, which is an ideal WGSL shape. It is edge-preserving with better behaviour near edges than the
bilateral, and the fast subsampled variant reports better than 10× with almost no visible degradation. It
would also **supersede the joint-bilateral depth upsampling already shipped** — which §7 "Depth" already
notes is one pass that leaves some edge error behind — with something cheaper and more standard.

**ACES Reference Gamut Compression** is the best cost/benefit item: a closed-form, invertible function of
each pixel's RGB ratios to the achromatic axis with seven scalar parameters, no LUTs and no iteration, on the
order of thirty lines of WGSL. It was introduced in ACES 1.3 as a technical correction rather than a look,
and ACES 2.0's display gamut compression reuses the same curve. It fixes the blue and magenta breakdown of
saturated LED and neon that neither white balance nor the mixer can reach. OpenColorIO (BSD-3-Clause) is an
allowed reference. Two things to settle first: no licence statement was found for the RGC CTL code itself,
and the transform ID differs between ACES documents.

**Hue-preserving tone mapping.** Applying a curve per channel distorts hue; the fix is to apply it to a
luminance norm and handle chroma separately. Sidecar's base curve is already a logistic, so this is a change
to an existing stage rather than a new one — and it is half of what ROADMAP's "Tone curve above middle grey"
is really asking for, the other half being the gamut compression above.

**A variance-stabilising transform around the existing wavelet noise reduction.** Raw noise is
Poisson-Gaussian per CFA plane, and the wavelet stage implicitly assumes uniform-variance Gaussian — which is
exactly why it is habitually too strong in shadows and too weak in highlights. Foi's pipeline (generalised
Anscombe transform → Gaussian-assumption denoiser → *exact unbiased inverse*, where the choice of inverse is
what controls bias) is two per-pixel scalar functions plus a small LUT wrapped around a stage that already
exists. Caveats worth carrying: the transform is inaccurate below about one count per pixel, and vkdt's own
noise-profile fitting has had integer-overflow failures, so guard rails are required.

**A highlight-aware demosaic.** Standard interpolators mix clipped and unclipped neighbours, which makes
magenta and green fringes at highlight edges *before* highlight reconstruction ever runs. The fix is a
per-channel clip mask and a renormalised weight sum inside the existing RCD kernel — cheap, and no new
algorithm. Note the related finding: **no measured benchmark showing anything beats RCD is in circulation**,
so the useful demosaic work is not a new interpolator but noise-aware switching, since colour overshoot on
noisy files is what a fitted noise profile predicts.

Then, in rough order of cost: **local Laplacian filters** (Paris, Hasinoff and Kautz, SIGGRAPH 2011) for
halo-free clarity and texture at large radius, taking the fast variant's fixed-remap-level discretisation
because it is the GPU-friendly shape; **single-image exposure fusion** (Mertens et al.) over synthetic
exposures from one RAW, reusing the same pyramid; **blind dehaze** by dark channel prior with guided-filter
refinement, for which the 2024 IPOL article is a peer-reviewed implementation target — noting that Sidecar's
existing depth-based haze is the *better-conditioned* problem, since a depth map gives transmission directly
and skips the fragile estimation step, so the gap is only haze without depth; and **lens-aware
Richardson-Lucy deconvolution**, which is unusually GPU-friendly (two separable convolutions and two
elementwise passes per iteration) and is a genuine differentiator here because lens correction already means
the lens is identified, so the PSF can come from a radius-and-aperture table rather than a guess. Use TV
regularisation with automatically estimated strength; bilateral-RL is patent-encumbered.

**Multi-frame is where the remaining quality lives, and it forces a decision during the port.** Handheld
multi-frame super-resolution (Wronski et al., SIGGRAPH 2019) runs at about 100 ms per 12 MP frame on a phone,
and its structure is gather-only and GPU-native. Pixel-shift sensor modes are the easier first increment,
since the sub-pixel offsets are known by design and only the merge is needed. The same alignment-and-merge
infrastructure serves HDR merge, averaging denoise and focus stacking. **The architectural point outranks any
one feature: multi-frame needs several full-resolution raw frames resident on the GPU at once, and a
single-image memory model may not accommodate that — decide it during the desktop port rather than
retrofitting it.**

## Features after V1

Lens correction, crop and straighten (with keystone and turns), and mask layers — model masks, brush strokes,
and every light and colour control per layer — have all shipped, in the order
[§15 of the design](DESIGN.md#15-after-v1-lens-correction-crop-local-masks) gives.

**What the category expects and Sidecar does not have** (surveyed 2026-10-17 against Lightroom Classic,
Capture One, DxO PhotoLab, ON1, darktable and RawTherapee; sources in
[`research/desktop-port-survey.md`](research/desktop-port-survey.md)). Sidecar is ahead on the unusual —
nothing else renders film spectrally from manufacturer data sheets — and behind on the boring. In priority
order:

- **Heal, clone and spot removal — the one universal gap, and the only one that is architecturally
  different in kind.** All six have it; it is the first thing anyone reaches for on a dust spot; and it is
  not another WGSL stage with a scalar parameter set, because it needs **per-patch records in the XMP
  history** rather than sliders. Build exactly one non-destructive parametric heal/clone — source offset,
  feathered edge, wavelet or Poisson blend — and stop there. Sidecar already has the hardest prerequisite
  for portrait work, since the skin, hair and face masks can constrain a source region automatically. Red-eye
  is a small adjacent tool. *Generative* fill is a trap for a self-hosted Apache-2.0 project: every
  commercial one is a paid cloud service needing an internet connection.
- **Clarity, texture and local contrast.** Front-panel sliders everywhere; their absence is immediately
  visible. Falls out of the local Laplacian or guided filter above, which is why those come first.
- **Dehaze.** Present in all of Lightroom, darktable and RawTherapee.
- **A scene-to-display tone transform.** This is becoming the consensus default rather than an option:
  darktable made `sigmoid` the default for new installs in 5.2 and added AgX in 5.4; RawTherapee added a
  sigmoid tone mapper in 5.12. An editor shipping only exposure, contrast and a curve clips highlights in a
  way that reads as broken to anyone arriving from those. This is the same question as "Tone curve above
  middle grey" at the top of this file — worth noting that the field has converged while that decision waits.
- **Camera profile selection — a correctness issue, not a feature.** Everyone exposes an input-profile
  choice; without one, Sidecar's colours differ from every other tool on the same file and users read that
  as a bug. The incremental piece is DCP's forward matrix plus HueSatMap/LookTable — data plumbing and a
  small 3D-LUT sampler, not an algorithm, since camera matrices are already interpolated by illuminant.
- **Colour grading wheels.** The OKLCh mixer does *not* substitute: wheels act on tonal ranges, the mixer on
  hue bands, and they are orthogonal. A shadows/midtones/highlights lift-gamma-gain operator is a handful of
  lines of WGSL on top of what is already computed — the best cost/benefit of any "pro" feature surveyed.
- **Presets and styles, copy-paste, and sync across a selection.** Presets are not a feature so much as a
  distribution channel, and the spectral film simulation is exactly the kind of thing that wants a style
  container. The gap most likely to bite in practice is **"apply this to the other 200 frames"** —
  settings-subset copy/paste, sync across a selection, auto-sync. Unglamorous, and the difference between a
  developer tool and something usable after a shoot.
- **Snapshots and virtual copies.** The category splits versions into two primitives and users expect both:
  virtual copies (a second entry sharing one RAW, for two deliverable looks) and snapshots (named states
  inside one entry, for A/B while editing). The XMP already carries history and versions, so this is mostly
  UI plus a side-by-side compare.
- **Soft proofing with a gamut check** — see the desktop section of [`V2.md`](V2.md); it is nearly pure
  rendering-path work and sits next to the Rec.2020 export already listed above. Note it needs no knowledge
  of the viewer's own display: the image is transformed to the proof profile and back, and the result is
  shown in whatever space the editor already renders. Compose the whole chain into one 3D LUT and compute
  the gamut warning from that same chain, since a parallel path is how gamut warnings end up disagreeing
  with the preview. *Display calibration is not part of this and is not Sidecar's job* — editors consume a
  profile, they do not measure one.
- **3D LUT loading**, **defringe** as a post-demosaic tool distinct from raw CA correction, and
  **film-negative inversion** for scans.
- **HDR merge and focus stacking.** These sit with panorama below: not table stakes for a RAW *developer*
  (darktable and RawTherapee have none of the three) but expected of anything calling itself all-in-one. All
  three ride on the multi-frame alignment infrastructure above, and focus stacking is cheap given that focus
  peaking already computes a focus measure — the hard part is that focal breathing changes the field of view
  between slices, so sharpness cannot be compared at fixed coordinates.

One completion bar worth adopting from Adobe: Lens Blur was not considered finished until it worked in
presets and in batch. For Sidecar that means **ML and depth masks must re-derive deterministically per
image** to survive batch application — a design constraint on features already shipped, not a UI task.

What §15 still lists and hasn't been built:

- **Gradients.** Linear and radial masks, analytic in the shader: a third kind of layer beside model masks and
  brushes, with handles drawn over the photo like the crop's. Universal across the category — treat as table
  stakes rather than an enhancement.
- **Sky — and it should not be a model.** The obvious route is closed: SegFormer's code is Apache-2.0 but its
  **weights are restricted to non-commercial research or evaluation**, and mmsegmentation's zoo inherits each
  method's own licence, so the trap re-enters there. The better route uses infrastructure already shipped:
  **sky is the far plane of the existing Depth Anything V2 map**, combined with a low-texture and
  chromaticity prior, the BiRefNet subject mask as a negative constraint, and a guided filter to finish the
  edge. Zero licence risk, no new model to download, no extra inference pass.
- **Depth, beyond what shipped.** Range layers, lens blur and haze by distance have shipped, and the map's
  edges are now the photo's: `client/depth_refine.wgsl` upsamples the model's 518² map against the photo
  (joint bilateral upsampling, §7 "Depth"), so a blur's edge follows hair or a thin branch instead of drifting
  into a halo. What is left here: **depth-weighted focus peaking**, and haze's strength curve, which may want
  tuning with pictures. The refinement itself could go further — it is one pass, so it takes most of the error
  out of an edge rather than all of it (the texel being repaired votes for itself); a second pass, or He, Sun
  and Tang's guided filter with its box means, would finish the job if pictures ask for it. A **second source
  for the map itself** — Apple's own stereo disparity, where a Portrait-mode HEIC carries one — is surveyed
  under "Portrait depth maps" above.
- **Detail per layer.** Sharpening and noise reduction through a mask: they run on the worker at full
  resolution, so a layer's value has to reach the detail passes, not just the develop pass.
- **Upright.** Keystone found automatically from the photo's lines; today Vertical and Horizontal are set by eye.

## Specialised tools worth integrating

Licences below were checked against the projects' own LICENSE files, not recalled — but re-check before
adopting, since licences change. The pattern to watch: for anything model-based, the **weights** carry their
own licence, separate from the code's, and are often the more restrictive of the two.

**AI upscaling — the code is viable, the weights are not cleared.** *This entry used to say "Real-ESRGAN is
BSD-3-Clause for both code and pretrained weights". The second half of that is wrong and was corrected on
2026-10-17.* The code is BSD-3-Clause, its `Real-ESRGAN-ncnn-vulkan` port is MIT and NCNN itself is
BSD-3-Clause throughout, all verified from the projects' own LICENSE files — and the ncnn/Vulkan route still
needs no PyTorch runtime, which suits the image. But **the distributed weight files carry no licence
statement at all**, and a weights file with no licence grants nothing; it is in practice worse than a stated
non-commercial licence, because there is no permission to point at. The checkpoints are named for their
training sets (`ESRGAN_SRx4_DF2KOST_official` = DIV2K + Flickr2K + OST), whose terms are unverified and are
in the NTIRE tradition of research-only. Three ways out: an explicit grant from the author, a checkpoint
whose card states a permissive licence, or retraining on BasicSR (Apache-2.0) over imagery Sidecar may use.
chaiNNer (GPL-3.0) and Upscayl (AGPL-3.0) remain hard excludes.

This is the general pattern and it is worth stating once: **weights are a dependency with their own licence,
and "the code is Apache-2.0" is never sufficient.** Track them in `NOTICE` and `THIRD_PARTY_NOTICES.md` like
source, with three acceptable statuses — an explicit permissive licence in the weights' own repository or
model card, trained in-house, or not shipped. The dataset underneath is the least examined layer of all.

**Panorama stitching — viable, with a patent caveat.** OpenCV has been Apache-2.0 since 4.5, `stitching`
module included. Hugin and Enblend/Enfuse — the traditional open-source stitching stack — are GPL, excluded.
The caveat is patents, not copyright: OpenCV's SURF feature finder sits behind `OPENCV_ENABLE_NONFREE`; use
ORB or AKAZE instead. No mature pure-Rust stitching crate exists, so this means an OpenCV dependency.

**Raw denoise — the one big ML feature where the licensing solves itself, and the most wanted by revealed
preference** (DxO built a product line on denoise alone; Adobe put it in the Detail panel). There is no
permissive raw denoiser with permissive weights to pick up off the shelf. But the ingredients for owning one
outright are all verified permissive: MIT noise-synthesis code (Sony Research's CVPR 2025 work, which needs
only dark-frame capture and cuts preparation from days to hours; and the ICCV 2021 repository), Apache-2.0 or
MIT architectures (NAFNet, SCUNet, SwinIR, Restormer — the last is MIT, contrary to a widespread belief that
it is academic-only), and BasicSR (Apache-2.0) for training. Add Sidecar's own measured sensor noise profiles
— which it needs anyway — and the result is **Apache-2.0 weights the project owns**. Raw-domain, before
demosaic, is the published consensus, and the VST above matters here too: networks trained on synthetic
Gaussian noise do well on real data once the variance is stabilised. Shape it as an explicit, progress-barred,
tiled export-time operation rather than a live pipeline stage. **No published runtime figure exists for any
open raw denoiser at full resolution on a consumer or integrated GPU — measure, don't assume.** Intel's OIDN
is a useful precedent (Apache-2.0 *including its weights repository*) but is trained for render noise, needs
auxiliary buffers and has no Vulkan backend, so it does not transfer.

**Context-aware auto-edit — thin, and implement the papers rather than the repos.** OpenCV's `xphoto` auto
white balance (gray-world, simple, learning-based) is Apache-2.0 and genuinely shippable, and grey-world,
white-patch and grey-edge are textbook. The obvious learned route is closed: Afifi and Brown's Deep White-
Balance Editing says it is for research purposes and cannot be used commercially, and has no LICENSE file at
all, while WB_sRGB restricts the *dataset* too — so retraining on it is blocked as well. Beyond that it's
mostly dead-but-permissive research code (Google's HDRNet is Apache-2.0 but archived and TensorFlow-1.x) or
non-commercial licences (Zero-DCE is CC BY-NC 4.0, which blocks redistribution just as firmly as copyleft
does, for a different reason). **No permissively licensed learned auto-tone model surfaced at all**, so
auto-tone is better framed as a parametric fit — percentile anchoring into Sidecar's own tone operator, plus
face-aware exposure from the YuNet detections already present — than as a learned regressor.

*On runtimes:* burn (Apache-2.0), candle, tract, ONNX Runtime (MIT) and ncnn (BSD-3) are all permissive, so
the choice is engineering, not licensing. Stay on burn-wgpu — one GPU abstraction, no shipped native blob —
and reach for ncnn's hand-tuned Vulkan kernels only if a specific model's throughput proves unacceptable.
Note that nearly every published inference benchmark assumes CUDA and says nothing about Vulkan on an
integrated GPU.

**Subject-aware tools: skin, colour ranges, subject masks and sharper peaking.** One model family, three
uses, built together because they share the hard part — knowing what in the frame is a person, a face, skin,
sky, the subject.

- *Colour ranges, with skin as the first preset* (Capture One's Skin Tone tool is the reference): a hue sector
  on a colour wheel with chroma and lightness limits and a soft edge, an eyedropper that centres it on a
  tapped patch, hue/chroma/lightness shifts, and **uniformity** — pulling hue, chroma and lightness inside the
  range toward its centre, which evens out blotches and redness while keeping the colour's depth. Measured in
  OKLCh like the mixer, so evening out redness doesn't move perceived brightness. Several ranges per photo
  (two people, skin and sky). A live mask overlay shows what's affected. A develop module after the mixer, in
  the shared WGSL; neutral settings are the identity, and an edit that uses it bumps `ProcessVersion`.
- *Subject and skin masks from a model* — **done**: `sidecar-ml` (BiRefNet lite for the subject, MediaPipe's
  multi-class model for hair and skin, YuNet for faces) runs on the worker's GPU on demand; masks are cached
  per photo and served by name, and the editor's Masks section applies exposure, contrast, saturation and
  white balance through any of them, inverted or not, identically in the browser and on export (a parity
  case covers it). What builds on it: the skin colour range (range × skin mask), focus peaking weighted by the
  subject, faces for eye-first peaking — each asks `$lib/masks` (browser) or `Worker::mask_images` (server)
  for a mask by name.
- *Hair-accurate edges*: the part masks come from a 256² model, so they're right about what is hair but
  blocky about where it ends. Refine at full resolution rather than tiling the model, which would lose the
  whole-image context segmentation needs: a guided filter or a matting model, driven by the coarse mask and
  the photo, tile by tile.
- *Focus peaking, refined*: weight the focus measure by the subject mask so peaking answers "is the subject
  sharp?" rather than "where is anything sharp?", and rank eyes and faces first, next to the AF point overlay.
  Peaking itself is done (above) and is already noise-aware — it divides by the local standard deviation, which
  for uncorrelated per-pixel noise scales with the noise's own amplitude, so high-ISO grain does not light up
  without needing the detail module's estimated σ.

## What not to build

Negative evidence is worth as much as the positive kind, and it is rarer. The most useful artifact the 2026
survey turned up is **darktable's list of thirteen formally deprecated modules** — a mature project publicly
declaring which tool concepts turned out to be dead ends: basic adjustments, channel mixer, contrast
brightness saturation, crop and rotate, defringe, fill light, global tonemap, invert, levels, spot removal,
tone mapping, vibrance, zone system. So: no `global tonemap`, no `fill light`, no `levels`, no
`contrast brightness saturation`, no `zone system`, no legacy channel mixer. The replacements are a
scene-referred display transform, a zonal tone control, grading wheels and proper colour calibration — all
listed above. **Read that list carefully, though:** `defringe` and `spot removal` were superseded by better
implementations, not abandoned as capabilities, which is why both appear above as things to build.

Beyond a single heal/clone, **frequency separation, liquify, layer compositing and content-aware or
generative fill are not table stakes for a RAW developer.** The counter-evidence is direct: darktable and
RawTherapee, the two tools with the deepest processing, have essentially none of them and are not criticised
for it — the criticism they get is about asset management. The honest nice-to-have tail nobody will miss:
liquify, watermarking, time-lapse, sky replacement, double exposure, texture and border packs, posterize,
bloom.

Three technical dead ends, so nobody spends a week finding out: **BM3D in WGSL** (block matching plus a 3D
collaborative transform with scatter-add aggregation and real divergence — possible, but a project rather
than a feature; non-local means with a VST gets most of the way with none of the pain); **the domain
transform**, whose recursive 1D sweeps are the worst GPU fit surveyed and whose authors warn that robust
derivative estimates are the real problem on noisy images — use the guided filter instead; and **moving the
editing controls from OKLCh to Jzazbz**, where the gain is modest, the PQ-like nonlinearity costs
transcendentals per pixel, and perfectly hue-preserving compression is probably not achievable anyway, since
holding hue constant in a perceptual approximation does not hold perceived hue constant.

One strategic note: darktable crossed into ML in June 2026 with an optional AI subsystem, an AI object mask
and a `neural restore` module covering raw denoise and upscale. **"Open source has no AI" is no longer a
differentiator** — ML masks are matched in kind, though not in breadth. What is still differentiating is that
Sidecar runs models *locally*, against Adobe's cloud approach.

## Interface

**Hide the editing tools — done.** A circled-i toggle in the canvas's top-left corner (or `H`) puts the tools
away: on a phone the bottom sheet shrinks to its tab row, on the desktop the sidebar gives way to a strip of
section tabs over the photo. A tab brings the tools back open at that section. Crop handles and the brush are
off while hidden. Remembered per device (`localStorage`), no sidecar involved.

**Output space in the editor — done.** sRGB / P3 sits where "Viewing and exporting in …" was (desktop) and
under Focus (phone). Switching reconfigures the canvas (`ViewPass.setDisplayP3`), repacks the develop uniforms
for the new `DisplaySpace` and saves it as the setting, so `/settings` and the next photo agree.

## Housekeeping

- **Adopt rustfmt — now a priority**, not housekeeping (owner's call, 2026-10-16: maintainability over new
  features). The sources are hand-wrapped past what rustfmt will reproduce, so the diff never reaches zero and
  the only question is how large a one-time reflow to accept: measured at **20,365** diff lines at
  `max_width = 100`, **12,583** at 140, **9,688** at 160 and **7,012** at 200, against ~41,000 lines of Rust.
  It goes in one commit that changes nothing else, whose SHA is then added to `.git-blame-ignore-revs` so
  `git blame` steps over it, and a `cargo fmt --check` gate joins clippy in CI. See [`V2.md`](V2.md).
- `deploy/remote-proxy/` is a worked example of one setup: a reverse proxy on another host reaching this one
  over a VPN. A real deployment's copy lives in the git-ignored `deploy/local/`.
