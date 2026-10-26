# Third-party notices

Generated sections (Rust crates via `cargo about`, npm packages via a licence report) are added by CI at release.
This hand-kept section covers code and data that Sidecar ports or embeds directly.

## LibRaw — CDDL-1.0 (LibRaw also offers LGPL-2.1; Sidecar uses CDDL-1.0)
Copyright LibRaw LLC. Source: https://github.com/LibRaw/LibRaw, tag 0.22.2, vendored at `third_party/libraw`.
LibRaw uses code from dcraw.c by Dave Coffin; LibRaw does not use dcraw's restricted code.
The full CDDL-1.0 text is in `third_party/libraw/LICENSE.CDDL`.
The mobile module compiles the same sources, unmodified, to WebAssembly (`mobile/libraw/build.sh`); images with
the module carry it under `/usr/share/sidecar/mobile`, under the same licence.

## Emscripten and the runtime it compiles in — MIT OR NCSA; MIT; Apache-2.0 WITH LLVM-exception
The mobile module's LibRaw build (`mobile/libraw/build.sh`) is made with Emscripten (the Emscripten authors;
MIT or the University of Illinois/NCSA licence), whose generated JavaScript loads it. Compiled into that
WebAssembly: musl libc (Rich Felker et al., MIT) and LLVM's libc++, libc++abi and compiler-rt (the LLVM
project, Apache-2.0 WITH LLVM-exception, whose exception waives the attribution requirement for compiled
binaries). Licence texts: Emscripten's `LICENSE`, `system/lib/libc/musl/COPYRIGHT`, `system/lib/libcxx/LICENSE.TXT`.

## vkdt — BSD-2-Clause
Ported shaders: `shaders/ingest/hilite_*.wgsl`, `shaders/ingest/rcd_*.wgsl` (from vkdt @ 13756b3e).

Copyright 2019 Johannes Hanika

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
   disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following
   disclaimer in the documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

## Compact-ICC-Profiles — CC0-1.0
Clinton Ingram. `assets/icc/sRGB-v4.icc`, `assets/icc/DisplayP3-v4.icc`.

## jpeg-encoder — (MIT OR Apache-2.0) AND IJG
This software is based in part on the work of the Independent JPEG Group.

## ts-rs — MIT
Aleph Alpha GmbH and the ts-rs contributors. A build-time code generator: it writes `web/src/lib/generated/` from
`sidecar-color`'s types (`--features ts`) and is not compiled into the published image.

## libgomp — GPL-3.0 with the GCC Runtime Library Exception
Linked for OpenMP in LibRaw; the exception permits distribution with non-GPL software.

## Denoising and sharpening
À-trous wavelet shrinkage after Starck & Murtagh; Richardson (1972) and Lucy (1974) deconvolution. Implemented in
`shaders/detail/` from the published methods.

## ACES Reference Gamut Compression — formulas and constants
Academy Software Foundation, aces-core, Apache-2.0. Implemented in `shaders/develop/gamut.wgsl`.

## Spectra for black and white — colour-science (BSD-3-Clause) data; Jakob and Hanika (2019)
`crates/color/src/spectral_data.rs` holds the CIE 1931 2° colour-matching functions and the D65 illuminant at
380–730 nm in 10 nm steps, exported from colour-science 0.4.7 (Copyright 2013 Colour Developers, BSD-3-Clause,
https://github.com/colour-science/colour), which distributes the CIE's standard data; and a few of its Jakob
2019 spectra as test references, and CIE daylight's basis functions S0, S1, S2. `crates/color/src/spectral.rs` implements the spectral upsampling of W. Jakob
and J. Hanika, "A Low-Dimensional Function Space for Efficient Spectral Upsampling", Computer Graphics Forum
38(2), 2019, from the paper; no code was copied.

## Film stocks — makers' datasheets
Spectral sensitivities and characteristic curves read, by the scripts in `crates/color/film-data/`, from:
HARMAN technology's *ILFORD HP5 Plus technical information* and *ILFORD SFX 200 technical information*
(Nov 2018; https://www.ilfordphoto.com/amfile/file/download/file/1903/product/691/ for HP5 Plus); and Kodak
Alaris's *KODAK PROFESSIONAL TRI-X 320 and 400 Films* (F-4017, Feb 2016). The values are in
`crates/color/src/film_data.rs`; the datasheets are not redistributed. ILFORD, HP5 and SFX are trademarks of
HARMAN technology Ltd; Kodak, Kodak Professional and Tri-X of Eastman Kodak Company — named here to say whose
films the curves describe.

## OKLab
Björn Ottosson, "A perceptual color space for image processing" (2020). Constants in `shaders/common/color.wgsl`.

## PCG hash
Jarzynski & Olano, "Hash Functions for GPU Rendering", JCGT 9(3), 2020. `shaders/common/hash.wgsl`.

## Krystek's Planckian locus approximation; Fritsch–Carlson monotone interpolation
Published algorithms; implemented in `crates/color`.

## Lens correction models and data
- **lensfun database** — the lensfun contributors, CC BY-SA 3.0 (https://github.com/lensfun/lensfun, `data/db`).
  Downloaded by the api at run time into the worker's cache and never shipped in the image. The distortion
  (PTLens, poly3, poly5), lateral CA (poly3, linear) and vignetting (Pablo D'Angelo) models are implemented in
  `crates/raw/src/lensfun.rs` from lensfun's published documentation; the LGPL-3.0 library is not used.
- **PanoTools normalized radius** — the PanoTools wiki's definition of the lens correction model's radius.
- **DNG opcodes** — `WarpRectilinear`, `FixVignetteRadial`, per Adobe's DNG Specification 1.7 (`crates/raw/src/lens.rs`).

## ExifTool — Perl licence (Artistic or GPL)
Copyright Phil Harvey. Runs as a separate program in the image.

## libheif — LGPL-3.0-or-later
Copyright Dirk Farin. Present only in images built with `--build-arg HEIF=1`, which is not how the published
image is built. Sidecar never links a HEIF decoder: `sidecar_raw::decoded` runs whichever one it finds as a
separate program, the same boundary as ExifTool, so no LGPL code is linked into the Sidecar binary. Anyone
redistributing an image built that way also inherits HEVC patent-pool obligations, which are independent of
this copyright licence.

## Machine-learning models — weights are licensed separately from code
`sidecar-ml` runs four models, shipped only in the `-ml` image. Their Rust code in `crates/ml/src/models/` is
generated by burn-onnx from each model's ONNX graph; the weights are downloaded (pinned by SHA-256) and
converted by the Dockerfile's `ml-onnx` stages, for that image and for `dev/ml-models`.

- **YuNet** (face detection, `face_detection_yunet_2023mar.onnx`) — MIT. Copyright Shiqi Yu.
  Source: https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet.
- **BiRefNet lite** (subject mask) — MIT. Copyright Peng Zheng and contributors. Source:
  https://github.com/ZhengPeng7/BiRefNet; ONNX export https://huggingface.co/onnx-community/BiRefNet_lite-ONNX.
  Paper: Zheng et al., "Bilateral Reference for High-Resolution Dichotomous Image Segmentation", CAAI AIR 2024.
- **MediaPipe multi-class selfie segmentation** (hair, face skin, body skin, clothes) — Apache-2.0. Copyright
  Google LLC. Source: https://ai.google.dev/edge/mediapipe/solutions/vision/image_segmenter; converted from
  TFLite to ONNX with tf2onnx (Apache-2.0).
- **Depth Anything V2 Small** (relative depth, for depth masks) — Apache-2.0. Copyright the Depth Anything V2
  authors (Lihe Yang et al.). Source: https://github.com/DepthAnything/Depth-Anything-V2; ONNX export
  https://huggingface.co/onnx-community/depth-anything-v2-small, simplified at 518 × 518 with onnxsim
  (Apache-2.0). Only the Small model is Apache-2.0: the Base and Large weights are CC-BY-NC-4.0 and not used.
  Paper: Yang et al., "Depth Anything V2", NeurIPS 2024.

## MPL-2.0 crates: buildid, colored
Used unmodified from crates.io through burn and CubeCL. MPL-2.0 applies to these crates' own files; their source
is at https://crates.io/crates/buildid and https://crates.io/crates/colored.

## Independent JPEG Group
This software is based in part on the work of the Independent JPEG Group.
