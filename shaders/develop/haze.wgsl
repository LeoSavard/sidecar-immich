// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Dehaze without depth (DESIGN.md §7, "Dehaze"), applied: Koschmieder's I = J·t + A·(1 − t) with the
// transmission t refined from the dark channel prior, and the amount as a change of the air's extinction — the
// transmission raised to a power e (t = e^(−βd), so t^e is e times the optical depth). e = 1 is the picture as
// it is; +100 takes e to 0, the haze the prior found removed; −100 takes it to 2, twice the haze. Rewritten so
// no division by t is left: I' = A + (I − A)·t^(e − 1).

// The smallest transmission the inversion trusts (He et al.'s t₀): below it, noise would be amplified past use.
const HAZE_T_MIN = 0.1;

// The pixel's own bound on its transmission (Meng, Wang, Duan, Xiang and Pan, "Efficient image dehazing with
// boundary constraint and contextual regularization", ICCV 2013): the scene's radiance can't be negative, so
// I ≥ A·(1 − t) in every channel and t ≥ 1 − min(I / A). It never exceeds the true transmission — a scene point
// with a black channel meets it exactly — so it only corrects a map that came out too low, as the guided map does
// on fine dark detail against a bright sky, where inverting too far turns small channel differences into a tint.
fn haze_bound(c: vec3f, air: vec3f) -> f32 {
  let n = c / max(air, vec3f(1e-6));
  return 1.0 - min(n.r, min(n.g, n.b));
}

fn haze_apply(c: vec3f, air: vec3f, t: f32, amount: f32) -> vec3f {
  let e = 1.0 - amount;
  let gain = pow(max(max(t, haze_bound(c, air)), HAZE_T_MIN), e - 1.0);
  return max(air + (c - air) * gain, vec3f(0.0));
}
