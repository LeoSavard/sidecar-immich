// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Composite one stroke's coverage onto a mask: paint (b + (1 − b)·c) or erase (b·(1 − c)), or copy the base
// alone to start a mask from the one the models made. The base may be any size; it is read through a linear
// sampler at this target's pixel centres.

struct CompositeParams {
  size: vec2f,           // the target, pixels
  mode: u32,             // 0 paint, 1 erase, 2 copy the base
  _pad: u32,
}

@group(0) @binding(0) var<uniform> C: CompositeParams;
@group(0) @binding(1) var base: texture_2d<f32>;
@group(0) @binding(2) var coverage: texture_2d<f32>;
@group(0) @binding(3) var linear_sampler: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
  return vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let uv = pos.xy / C.size;
  let b = textureSampleLevel(base, linear_sampler, uv, 0.0).r;
  if (C.mode == 2u) { return vec4f(b, 0.0, 0.0, 1.0); }
  let c = textureSampleLevel(coverage, linear_sampler, uv, 0.0).r;
  let painted = select(b + (1.0 - b) * c, b * (1.0 - c), C.mode == 1u);
  return vec4f(painted, 0.0, 0.0, 1.0);
}
