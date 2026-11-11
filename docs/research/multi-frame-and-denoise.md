# Multi-frame (HDR, panoramas, HDRIs, focus stacks) and a raw denoiser of our own

*Desk research, 2026-11-06, for the owner's request to build these. Licences were read from the projects'
own repositories that day; every algorithm named is from a published paper. Nothing here has been run yet.*

## The design that ties them together: a merge produces a new original

Lightroom's answer is the right shape for Sidecar too: **an HDR merge, a panorama, a focus stack and a
denoised frame each produce a new linear raw file — a DNG — beside the frames it came from**, and that file
is then edited like any other original. Nothing in the editor, the sidecar, history, masks or export has to
learn about multi-frame; the merge is a tool that makes an input, not a pipeline mode. Concretely:

- **Output:** a DNG with `PhotometricInterpretation = LinearRaw` (demosaiced, three channels), 16-bit float
  samples, the source camera's colour matrices and as-shot neutral copied across, and an extended white level
  so an HDR merge keeps its range. LibRaw already opens linear DNGs (Sidecar reads them today, including
  JPEG XL-compressed ones), so the editor needs no new decoder. Writing one needs a small TIFF/DNG writer on
  the `tiff` crate (MIT).
- **Where it lands:** next to the sources when their root is writable (the same rule as the sidecar), or in
  the editing folder; the sources each get nothing written.
- **Provenance:** the new file's own sidecar records which frames made it and with what settings, so a merge
  can be redone.

**Memory is the architectural constraint the roadmap warned about.** A 45 MP frame as RGBA16F is ~360 MB; a
five-frame bracket is ~1.8 GB of VRAM, a twenty-frame panorama far more. The merge work therefore runs on the
worker, tile by tile, holding only the tiles in flight — the export path already tiles; this generalises it.

## Shared infrastructure, built once

| Piece | Used by | From |
|---|---|---|
| Linear float DNG writer | all | DNG 1.7 specification |
| Frame alignment: features + RANSAC homography, then sub-pixel refinement (ECC or optical-flow on luminance) | HDR, focus stack, handheld panorama | Brown & Lowe 2007; Evangelidis & Psarakis 2008 (ECC) |
| Feature detector and descriptor | panorama, alignment | SIFT (patent expired 2020) or ORB/AKAZE; implemented here, on the GPU over a downscaled luminance |
| Gaussian/Laplacian pyramid on the GPU | multi-band blending, exposure fusion, focus stacking, local Laplacian (clarity) | Burt & Adelson 1983 |
| Noise model per camera and ISO (Poisson–Gaussian) | HDR merge weights, the VST, the denoiser | Foi et al. 2008; Granados et al. 2010 |
| Tiled worker job with progress over SSE | all | the export job's machinery |

## HDR merge (bracketed raws)

**Raw makes this simpler than most guides suggest:** a raw file's response is linear, so no camera response
curve has to be estimated (Debevec–Malik is for JPEGs). Each frame is scaled by its exposure (shutter ×
aperture² × ISO, from EXIF, checked against the overlapping pixels) into one radiance scale, and merged
per pixel with noise-optimal weights — inverse variance from the noise model, so the cleanest exposure wins
where it is valid (Granados et al., "Optimal HDR reconstruction with linear digital cameras", CVPR 2010;
Hasinoff et al. 2010). A sample within a margin of its channel's saturation (the clip threshold Sidecar
already measures per frame, `sidecar-dev levels`) gets weight zero, which is what keeps clipped highlights
out without blending tricks.

- **Alignment:** handheld brackets need it; tripod brackets want a check that finds nothing to do. Align on
  luminance normalised by exposure.
- **Ghosts:** where frames disagree by more than their noise allows (a moving branch, a person), take that
  region from one reference exposure only (Granados et al. 2013, "Automatic noise modeling for ghost-free HDR
  reconstruction"). A user-chosen reference and a ghost-strength control are what every tool converges on.
- **Output:** the linear float DNG above, with its real range. The existing Highlights, Whites, curve and —
  once built — local Laplacian tone mapping are what bring it into a display; exposure fusion (Mertens et al.
  2007) is the alternative "natural look" path and reuses the pyramid.
- **Reference:** colour-hdri (BSD-3-Clause) is an allowed implementation to read. `hdrmerge`
  (jcelaya, GPL-3.0) is not; its approach is in a published paper.

## Panoramas

The pipeline is Brown & Lowe's ("Automatic Panoramic Image Stitching using Invariant Features", IJCV 2007),
which every open stitcher descends from:

1. **Lens correction first** — Sidecar already has it (the file's own profile, or lensfun's).
2. **Features and matches** on downscaled luminance; RANSAC per image pair; the image-match graph decides
   which frames belong to one panorama (so a folder of frames can be offered as "these 12 make a pano").
3. **Camera model:** rotation-only (a handheld pano pivots), focal length from EXIF; **bundle adjustment**
   (Levenberg–Marquardt over rotations and focal) across all pairs; **wave correction** so the horizon is
   straight.
4. **Projection:** rectilinear (few frames), cylindrical (a row), spherical/equirectangular (a full sphere —
   which is the HDRI case).
5. **Exposure:** in linear raw a constant gain per frame is exact, so compensation is a solved scalar per
   frame, far better behaved than the gamma-domain version.
6. **Seams** (graph cut, or a cheaper dynamic-programming seam) to route around moving things, then
   **multi-band blending** (Burt & Adelson) on the shared pyramid.
7. **Output:** the linear float DNG, with an optional crop to the largest rectangle.

Licences: OpenCV's `stitching` module is Apache-2.0 and an allowed reference (but a heavy C++ dependency, and
this server's CPU has no AVX2); Hugin and Enblend/Enfuse are GPL — excluded. The one Rust stitcher found
(`ilia3101/Panorama`) carries an "Anti-Capitalist Software Licence" — not permissive, excluded. Building it
here is the plan; the GPU does detection, warping and blending, the CPU only the small solvers.

## HDRIs

An HDRI is a **full-sphere (360°×180°) equirectangular HDR image used to light 3D scenes**, so it is the
intersection of the two above with stricter rules:

- **Capture:** several rows of brackets around a nodal point (or a fisheye in 3–6 directions), so the merge
  is HDR *per direction*, then a spherical stitch of the merged frames. Merge first, stitch second: alignment
  is better on noise-optimal merged frames, and a moving cloud ghosts once instead of per exposure.
- **Accuracy over looks:** no tone curve, no local contrast, a neutral fixed white balance, every frame
  developed identically. What a 3D renderer wants is radiance, so the output is **scene-linear** and
  optionally **absolute** — scaled to real luminance from the exposure metadata, which Sidecar has.
- **The sun clips** at any practical shutter speed; the honest options are an ND filter frame, or flagging it
  and letting the user add an analytic sun in their renderer. Worth stating in the UI, not hiding.
- **Poles:** zenith and nadir need care — local tone mapping makes artefacts there (Hugin's own guide), and
  the nadir usually shows the tripod, which wants a patch (the heal tool, once it exists).
- **Formats:** OpenEXR (half or float, ZIP/PIZ) via the `exr` crate (BSD-3-Clause), and Radiance `.hdr`
  (the `image` crate, MIT/Apache-2.0); a tone-mapped equirectangular JPEG preview beside it.

## Focus stacking

Alignment must include **scale**, because focusing changes the field of view (focal breathing) — so a
similarity transform per frame, not a translation. Then a per-pixel focus measure (Sidecar's focus-peaking
measure already exists), a depth/index map smoothed so the choice doesn't flicker between frames, and a
pyramid blend of the chosen samples (the same pyramid again).

## A raw denoiser of our own

The roadmap's plan stands and the ingredients are now confirmed from their own repositories:

- **Noise synthesis:** Sony Research's "Noise Modeling in One Hour" (Li, Jiang, Iso, CVPR 2025) — code at
  `SonyResearch/raw_image_denoising`, **MIT**. It needs only **dark frames** per camera (lens cap on, a set of
  ISOs and shutter speeds, fully automatable in about an hour), dropping the gain-calibration and
  signal-independent profiling steps, and reports beating the previous best synthesis.
- **Architecture:** NAFNet (`megvii-research/NAFNet`, **MIT**) — convolution-only, small and fast, which suits
  both training here and inference through burn on wgpu. Restormer (**MIT**) and SCUNet (**Apache-2.0**) are
  the heavier alternatives; BasicSR (**Apache-2.0**) is the reference training loop.
- **Training without CUDA.** The owner's rule rules out PyTorch on CUDA here. burn (Apache-2.0, 0.21 released
  with up to 8× lower framework overhead) trains on wgpu with autodiff, on this server's RTX A4000 through
  Vulkan. No independent figure for burn-wgpu training speed exists, so **the first step is a benchmark**:
  NAFNet-width-32 on 256² packed-Bayer crops, iterations per second.
- **What the model sees:** the raw mosaic packed to four half-resolution planes (RGGB), black-subtracted and
  normalised, plus a per-pixel noise-level map from the camera's noise model, so one network serves every
  ISO. Raw-domain, before demosaic, is the published consensus. X-Trans needs a different packing and is out,
  as X-Trans full resolution already is.
- **Where it runs:** an explicit, progress-barred, tiled export-time operation on the worker — "Denoise" makes
  a denoised linear DNG beside the original (the multi-frame design above), so the editor previews the result
  at full speed afterwards instead of running a network per slider move.
- **Weights we own, under Apache-2.0** — trained here, from data we are allowed to use.

**What it needs from the owner** (it cannot be built from the repository alone):

1. **Dark frames** from each camera to support: lens cap on, each whole-stop ISO, a few shutter speeds.
2. **Clean training targets.** Low-ISO, well-exposed raws, as varied as possible. The obvious source is the
   owner's own library — but weights trained on it would then be **published under Apache-2.0**. A network
   doesn't store its training photos in recoverable form, yet whether personal photos may train published
   weights is the owner's call. Public raw datasets mostly carry research-only terms (the "NTIRE tradition"
   the upscaling entry describes) and would need checking one by one.
3. **Its eye on real high-ISO frames**, against the existing wavelet noise reduction — PSNR on synthetic noise
   is the training signal, not the verdict.
