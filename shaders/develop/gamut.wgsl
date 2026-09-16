// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Module 3b — gamut compression. Implements the published formulas of the ACES Reference Gamut
// Compression (Academy Software Foundation, aces-core, Apache-2.0): distance from the achromatic axis is
// compressed beyond a threshold toward a limit, so saturated lights don't produce negative channels.

const GC_THRESHOLD = vec3f(0.815, 0.803, 0.880);
const GC_LIMIT = vec3f(1.147, 1.264, 1.312);
const GC_POWER = 1.2;

fn gc_curve(d: f32, lim: f32, thr: f32) -> f32 {
  if (d < thr) { return d; }
  let scl = (lim - thr) / pow(pow((1.0 - thr) / (lim - thr), -GC_POWER) - 1.0, 1.0 / GC_POWER);
  let nd = (d - thr) / scl;
  return thr + scl * nd / pow(1.0 + pow(nd, GC_POWER), 1.0 / GC_POWER);
}

fn gamut_compress(c: vec3f) -> vec3f {
  let ach = max(c.r, max(c.g, c.b));
  if (ach <= 0.0) { return vec3f(0.0); }
  let d = (vec3f(ach) - c) / ach;
  let cd = vec3f(
    gc_curve(d.r, GC_LIMIT.r, GC_THRESHOLD.r),
    gc_curve(d.g, GC_LIMIT.g, GC_THRESHOLD.g),
    gc_curve(d.b, GC_LIMIT.b, GC_THRESHOLD.b),
  );
  return vec3f(ach) - cd * ach;
}
