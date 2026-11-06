// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Module 3b — gamut compression. Implements the published formulas of the ACES Reference Gamut
// Compression (Academy Software Foundation, aces-core, Apache-2.0): distance from the achromatic axis is
// compressed beyond a threshold toward a limit, so saturated lights don't produce negative channels. The
// thresholds and limits were derived in ACEScg, so since `PROCESS_VERSION` 9 the working Rec.2020 colour is
// taken there for it (`Tone::gamut_ap1`) and brought back; an older edit compresses in Rec.2020, as it did.

const GC_THRESHOLD = vec3f(0.815, 0.803, 0.880);
const GC_LIMIT = vec3f(1.147, 1.264, 1.312);
const GC_POWER = 1.2;

// Linear Rec.2020 (D65) → ACEScg, with Bradford from D65 to the ACES white so neutrals stay neutral, and back:
// `sidecar_color::mat::rec2020_to_ap1`, which a test checks these against.
const REC2020_TO_AP1 = mat3x3f(
  vec3f(0.9748949779, 0.0021795628, 0.0047972397),
  vec3f(0.0195991086, 0.9955354689, 0.0245320167),
  vec3f(0.0055059135, 0.0022849683, 0.9706707437)
);
const AP1_TO_REC2020 = mat3x3f(
  vec3f(1.0258247477, -0.0022343695, -0.0050133515),
  vec3f(-0.0200531909, 1.0045865019, -0.0252900718),
  vec3f(-0.0057715569, -0.0023521324, 1.0303034232)
);

fn gc_curve(d: f32, lim: f32, thr: f32) -> f32 {
  if (d < thr) { return d; }
  let scl = (lim - thr) / pow(pow((1.0 - thr) / (lim - thr), -GC_POWER) - 1.0, 1.0 / GC_POWER);
  let nd = (d - thr) / scl;
  return thr + scl * nd / pow(1.0 + pow(nd, GC_POWER), 1.0 / GC_POWER);
}

fn gamut_compress_in(c: vec3f) -> vec3f {
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

// Compress `c` (linear Rec.2020): in ACEScg when `ap1`, in Rec.2020 itself otherwise.
fn gamut_compress(c: vec3f, ap1: bool) -> vec3f {
  if (!ap1) { return gamut_compress_in(c); }
  return AP1_TO_REC2020 * gamut_compress_in(REC2020_TO_AP1 * c);
}
