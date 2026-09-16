// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Module 1 — white balance: diagonal Von Kries scaling in camera space.

fn white_balance(c: vec3f, mul: vec3f) -> vec3f {
  return c * mul;
}
