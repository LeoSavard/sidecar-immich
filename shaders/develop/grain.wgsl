// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: common/transfer.wgsl common/color.wgsl
// Grain (DESIGN.md §7, "Grain"): the exposure each spot got, made to flicker as a negative's grains make it,
// before the tone map takes it. Two octaves of value noise placed on the whole frame — clumps at the grain's
// size, and grains at half of it — from an integer hash, so the browser and the server draw the same grain.
// Where an octave is finer than a pixel its lattice is the pixels themselves, and its strength falls with the
// grain's size over the pixel's (Selwyn's law: granularity times the square root of the area seen is constant).

// pcg2d, from Jarzynski and Olano, "Hash Functions for GPU Rendering", JCGT 9(3), 2020.
fn grain_hash(v: vec2u) -> vec2u {
  var h = v * 1664525u + vec2u(1013904223u);
  h.x += h.y * 1664525u;
  h.y += h.x * 1664525u;
  h = h ^ (h >> vec2u(16u));
  h.x += h.y * 1664525u;
  h.y += h.x * 1664525u;
  h = h ^ (h >> vec2u(16u));
  return h;
}

// A lattice point's value: four 16-bit uniforms summed (Irwin–Hall), close to Gaussian, mean 0, variance 1.
// Integer to float and a multiply are exact or correctly rounded on every GPU.
fn grain_node(i: vec2i, octave: u32) -> f32 {
  let h = grain_hash(bitcast<vec2u>(i) + vec2u(octave * 0x9e3779b9u, 0u));
  let sum = f32(h.x & 0xffffu) + f32(h.x >> 16u) + f32(h.y & 0xffffu) + f32(h.y >> 16u);
  return (sum * (1.0 / 65535.0) - 2.0) * 1.7320508076;
}

// Value noise with lattice points `spacing` pixels apart, the first on pixel 0's centre, so at a spacing of 1
// each pixel is a lattice point. Divided by the interpolation's own spread, which is variance 1 everywhere: no
// pattern of stronger and weaker grain along the lattice.
fn grain_octave(p: vec2i, spacing: f32, octave: u32) -> f32 {
  let q = vec2f(p) * (1.0 / spacing);
  let i = vec2i(floor(q));
  let f = q - floor(q);
  let s = f * f * (3.0 - 2.0 * f);
  let v = mix(
    mix(grain_node(i, octave), grain_node(i + vec2i(1, 0), octave), s.x),
    mix(grain_node(i + vec2i(0, 1), octave), grain_node(i + vec2i(1, 1), octave), s.x),
    s.y,
  );
  let w = ((1.0 - s.x) * (1.0 - s.x) + s.x * s.x) * ((1.0 - s.y) * (1.0 - s.y) + s.y * s.y);
  return v * inverseSqrt(w);
}

// The grain at pixel `p` of the frame, for a grain `size` pixels across at this rendering: variance 1 where
// both octaves are larger than a pixel, less where pixels average them.
fn grain_noise(p: vec2i, size: f32) -> f32 {
  let half = 0.5 * size;
  let clumps = grain_octave(p, max(size, 1.0), 0u) * min(size, 1.0);
  let grains = grain_octave(p, max(half, 1.0), 1u) * min(half, 1.0);
  return (clumps + 0.5 * grains) * 0.8944271910; // 1 / sqrt(1 + 0.5²)
}

const GRAIN_LN2 = 0.6931471806;

// `c` with its exposure moved by `n` grains of `stops` standard deviation, keeping hue. A negative's grain grows
// with its density, so a RAW's is strongest toward the white point and least at the black point (the tone curve
// then flattens both ends); a staining developer's stain hides `stain` of it at the white point, less below. A
// developed file is already a print, so its grain is shaped to fade at black and white. The mean exposure is
// kept: exp2 of a Gaussian would otherwise brighten by σ² ln 2 / 2.
fn grain_apply(c: vec3f, n: f32, stops: f32, stain: f32, tone: vec4f, developed: bool) -> vec3f {
  let y = max(dot(c, LUMA_2020), 0.0);
  var sigma: f32;
  if (developed) {
    let e = srgb_encode1(y);
    sigma = stops * 4.0 * e * (1.0 - e);
  } else {
    let t = clamp((log2(max(y, 1e-10) / 0.18) - tone.x) / (tone.y - tone.x), 0.0, 1.0);
    sigma = stops * sqrt((0.2 + t) / 1.2) * (1.0 - stain * t);
  }
  return c * exp2(sigma * n - 0.5 * sigma * sigma * GRAIN_LN2);
}
