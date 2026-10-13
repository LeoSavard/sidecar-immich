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

1. *Read the gain map* at decode, with its metadata — **done**, in `sidecar_raw::gain_map`, through the same
   `heif-convert --with-aux` boundary as `portrait_depth` (docs above). Checked against a real iPhone 17 Pro
   HEIC rather than assumed: the aux image (`urn:com:apple:photo:2020:aux:hdrgainmap`) comes out named
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
  sharp?" rather than "where is anything sharp?"; make it noise-aware (normalise by the estimated noise σ the
  detail module already computes, so high-ISO grain stops lighting up); and rank eyes and faces first, next to
  the AF point overlay.

## Interface

**Hide the editing tools.** A circled-i toggle in the canvas's top-left corner (a different corner from the
existing clip/peaking/AF-point toggles, top-right) collapses the sidebar — file/camera details, the develop
sliders, Export/Save — into a compact center bar: the section tabs (WB · Light · Curve · Color · Detail) stay
reachable, so switching tools still works, but everything else shrinks to its smallest useful form and the
photo gets the rest of the viewport. Client-only state, no sidecar involved.

The same panel is the right place to put output space (sRGB / Display P3) inline, instead of only reachable
by leaving the editor for `/settings`. This is wiring, not an architecture change: `ViewPass.create`'s
`context.configure({ colorSpace })` in `web/src/lib/gpu/view.ts` only runs once today, but a
`GPUCanvasContext` can be reconfigured live — switching mid-edit means calling `configure()` again with the
new colour space and re-packing the develop uniforms (`packer.pack(source, meta.info.color, display)`) for
the new `DisplaySpace`, then `schedule(true)`.

## Housekeeping

- The Rust sources are hand-wrapped well past rustfmt's comfort at any `max_width`; either adopt a formatter
  and take the one-time diff, or leave it deliberately and say so.
- `deploy/remote-proxy/` is a worked example of one setup: a reverse proxy on another host reaching this one
  over a VPN. A real deployment's copy lives in the git-ignored `deploy/local/`.
