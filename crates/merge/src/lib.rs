// SPDX-License-Identifier: Apache-2.0
//! Multi-frame work (docs/research/multi-frame-and-denoise.md): aligning frames first; HDR merge, panoramas,
//! HDRIs and focus stacks build on it, and each writes its result as a linear DNG.

pub mod align;
// SPDX-License-Identifier: Apache-2.0
//! Multi-frame work (docs/research/multi-frame-and-denoise.md): aligning frames first; HDR merge, panoramas,
//! HDRIs and focus stacks build on it, and each writes its result as a linear DNG.

pub mod align;
pub mod hdr;
pub mod hdri;
pub mod linalg;
pub mod pano;
