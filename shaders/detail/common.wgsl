// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: uniforms/detail_params.wgsl
// What the detail passes (denoise and deconvolution) share. Their uniform, DetailParams, is generated from
// crates/engine/src/detail.rs.

fn clamp_px(p: vec2i, size: vec2u) -> vec2i {
  return clamp(p, vec2i(0), vec2i(size) - vec2i(1));
}

const STAB_EPS = 0.0005;
