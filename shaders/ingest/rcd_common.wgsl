// SPDX-License-Identifier: BSD-2-Clause
// Copyright 2019 Johannes Hanika (vkdt). WGSL port copyright Sidecar contributors.
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that
// the conditions of the BSD 2-Clause licence are met; see NOTICE and THIRD_PARTY_NOTICES.md for the full text.
// Ratio Corrected Demosaicing as implemented in vkdt src/pipe/modules/demosaic/rcd_conv.comp and rcd_fill.comp
// @ 13756b3e18143f0f468b4a0cf34d6e787fcf2819. vkdt fills one shared-memory tile per workgroup; this port splits the same steps into full-resolution
// passes (conv → green → red/blue at blue/red → red/blue at green) so no workgroup memory is needed.
//! requires: ingest/common.wgsl

/// Colour of a pixel on a mosaic cropped to RGGB: 0 R, 1 G, 2 B.
fn rgGB(p: vec2i) -> i32 {
  if (((p.x + p.y) & 1) == 1) { return 1; }
  if ((p.y & 1) == 0) { return 0; }
  return 2;
}

fn discriminator(c: f32, n: f32) -> f32 {
  return select(c, n, abs(0.5 - c) < abs(0.5 - n));
}
