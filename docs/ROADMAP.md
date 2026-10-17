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

**X-Trans at full resolution** still runs LibRaw's Markesteijn on the CPU; only Bayer has a GPU demosaic for
loupe and export. Porting LibRaw's algorithm to WGSL (CDDL-1.0, so the port keeps that header) is the
remaining piece of milestone 5. `DSCF7611.RAF` is the test case.

**iPhone parity is unverified.** The ΔE2000 harness compares Chromium against the server; Safari on Metal has
never been in the loop. It needs a way to run the browser side on a phone and post results back.

## Reach

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

- *Rec.2020 SDR export* is close — the 16-bit TIFF path exists and needs the matrix and an ICC profile.
- *True HDR* needs an HDR canvas (WebGPU `toneMapping: { mode: 'extended' }`, not something to count on in
  Safari yet) and an HDR container: 10-bit AVIF with PQ or HLG (rav1e is BSD-2, so licence-compatible), or an
  ISO 21496-1 gain map as Apple writes. Settle the encoder question before the UI.

**HDR originals keep their gain map.** An iPhone HEIC is an SDR picture plus a gain map — a small auxiliary
image telling an HDR screen how far past SDR white to lift each area — and Photos shows it HDR. Sidecar reads
only the SDR picture (ImageMagick and `heif-convert` decode the primary image and drop the auxiliary one) and
exports SDR, so an edit looks flat beside its original. In order:

1. *Read the gain map* at decode, with its metadata (headroom, min/max boost, gamma). `heif-convert` can write
   auxiliary images out; check which it names the gain map, and whether Apple's pre-21496 maps (HEIC
   `urn:com:apple:photo:2020:aux:hdrgainmap`) need their own handling. Cache it beside the proxy.
2. *Carry it through the edit.* Edit the SDR picture as now and reapply the original map to the result, as
   the gain-map standard intends; exposure changes the base, so the map's headroom may need scaling with it.
   Geometry (crop, straighten, orientation) must transform the map exactly as it does the picture.
3. *Export with it*: an Ultra HDR JPEG (ISO 21496-1 with the Adobe/Google XMP fallback) is what iOS and
   Android both display as HDR. Google's `libultrahdr` is Apache-2.0 — check it through `cargo deny`
   (or its C build) and add it to NOTICE before depending on it. HEIC output stays off the table (HEVC).
4. *Show it in the editor* — last, and least certain: it needs an HDR-capable WebGPU canvas in Safari. Until
   then the editor stays SDR and says the export will be HDR.

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

## Features after V1

Lens correction, crop and straighten (with keystone and turns), and mask layers — model masks, brush strokes,
and every light and colour control per layer — have all shipped, in the order
[§15 of the design](DESIGN.md#15-after-v1-lens-correction-crop-local-masks) gives. What §15 still lists and
hasn't been built:

- **Gradients.** Linear and radial masks, analytic in the shader: a third kind of layer beside model masks and
  brushes, with handles drawn over the photo like the crop's.
- **Sky.** A sky model, to join the subject, skin and hair ones.
- **Depth, beyond what shipped.** Range layers, lens blur and haze by distance have shipped, and the map's
  edges are now the photo's: `client/depth_refine.wgsl` upsamples the model's 518² map against the photo
  (joint bilateral upsampling, §7 "Depth"), so a blur's edge follows hair or a thin branch instead of drifting
  into a halo. What is left here: **depth-weighted focus peaking**, and haze's strength curve, which may want
  tuning with pictures. The refinement itself could go further — it is one pass, so it takes most of the error
  out of an edge rather than all of it (the texel being repaired votes for itself); a second pass, or He, Sun
  and Tang's guided filter with its box means, would finish the job if pictures ask for it.
- **Detail per layer.** Sharpening and noise reduction through a mask: they run on the worker at full
  resolution, so a layer's value has to reach the detail passes, not just the develop pass.
- **Upright.** Keystone found automatically from the photo's lines; today Vertical and Horizontal are set by eye.

## Specialised tools worth integrating

Licences below were checked against the projects' own LICENSE files, not recalled — but re-check before
adopting, since licences change. The pattern to watch: for anything model-based, the **weights** carry their
own licence, separate from the code's, and are often the more restrictive of the two.

**AI upscaling — viable.** Real-ESRGAN is BSD-3-Clause for both code and pretrained weights; its
`Real-ESRGAN-ncnn-vulkan` port is MIT, and NCNN itself is BSD-3-Clause throughout. The ncnn/Vulkan route
needs no PyTorch runtime, which suits the image — and the worker already has a Vulkan device. The two tools
most people reach for first, chaiNNer (GPL-3.0) and Upscayl (AGPL-3.0), are both hard excludes.

**Panorama stitching — viable, with a patent caveat.** OpenCV has been Apache-2.0 since 4.5, `stitching`
module included. Hugin and Enblend/Enfuse — the traditional open-source stitching stack — are GPL, excluded.
The caveat is patents, not copyright: OpenCV's SURF feature finder sits behind `OPENCV_ENABLE_NONFREE`; use
ORB or AKAZE instead. No mature pure-Rust stitching crate exists, so this means an OpenCV dependency.

**Context-aware auto-edit — thin.** OpenCV's `xphoto` auto white balance (gray-world, simple, learning-based)
is Apache-2.0 and genuinely shippable. Beyond that it's mostly dead-but-permissive research code (Google's
HDRNet is Apache-2.0 but archived and TensorFlow-1.x) or non-commercial licences (Zero-DCE is CC BY-NC 4.0,
which blocks redistribution just as firmly as copyleft does, for a different reason). Realistic split:
auto-WB off the shelf, auto-tone built here — published methods are always fair game, their code isn't.

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

## Interface

**Hide the editing tools — done.** A circled-i toggle in the canvas's top-left corner (or `H`) puts the tools
away: on a phone the bottom sheet shrinks to its tab row, on the desktop the sidebar gives way to a strip of
section tabs over the photo. A tab brings the tools back open at that section. Crop handles and the brush are
off while hidden. Remembered per device (`localStorage`), no sidecar involved.

**Output space in the editor — done.** sRGB / P3 sits where "Viewing and exporting in …" was (desktop) and
under Focus (phone). Switching reconfigures the canvas (`ViewPass.setDisplayP3`), repacks the develop uniforms
for the new `DisplaySpace` and saves it as the setting, so `/settings` and the next photo agree.

## Housekeeping

- The Rust sources are hand-wrapped well past rustfmt's comfort at any `max_width`; either adopt a formatter
  and take the one-time diff, or leave it deliberately and say so.
- `deploy/remote-proxy/` is a worked example of one setup: a reverse proxy on another host reaching this one
  over a VPN. A real deployment's copy lives in the git-ignored `deploy/local/`.
