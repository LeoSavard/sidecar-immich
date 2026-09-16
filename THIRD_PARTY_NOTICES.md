# Third-party notices

Generated sections (Rust crates via `cargo about`, npm packages via a licence report) are added by CI at release.
This hand-kept section covers code and data that Sidecar ports or embeds directly.

## LibRaw — CDDL-1.0 (LibRaw also offers LGPL-2.1; Sidecar uses CDDL-1.0)
Copyright LibRaw LLC. Source: https://github.com/LibRaw/LibRaw, tag 0.22.2, vendored at `third_party/libraw`.
LibRaw uses code from dcraw.c by Dave Coffin; LibRaw does not use dcraw's restricted code.
The full CDDL-1.0 text is in `third_party/libraw/LICENSE.CDDL`.

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

## libgomp — GPL-3.0 with the GCC Runtime Library Exception
Linked for OpenMP in LibRaw; the exception permits distribution with non-GPL software.

## Denoising and sharpening
À-trous wavelet shrinkage after Starck & Murtagh; Richardson (1972) and Lucy (1974) deconvolution. Implemented in
`shaders/detail/` from the published methods.

## ACES Reference Gamut Compression — formulas and constants
Academy Software Foundation, aces-core, Apache-2.0. Implemented in `shaders/develop/gamut.wgsl`.

## OKLab
Björn Ottosson, "A perceptual color space for image processing" (2020). Constants in `shaders/common/color.wgsl`.

## PCG hash
Jarzynski & Olano, "Hash Functions for GPU Rendering", JCGT 9(3), 2020. `shaders/common/hash.wgsl`.

## Krystek's Planckian locus approximation; Fritsch–Carlson monotone interpolation
Published algorithms; implemented in `crates/color`.

## ExifTool — Perl licence (Artistic or GPL)
Copyright Phil Harvey. Runs as a separate program in the image.

## Independent JPEG Group
This software is based in part on the work of the Independent JPEG Group.
