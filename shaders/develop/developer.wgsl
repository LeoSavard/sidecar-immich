// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Black and white's developer (DESIGN.md §7, "Developers"; `sidecar_color::developer`): adjacency, development
// and compensation, on the film's exposure before the tone map. Works in log exposure, stops from middle grey.

/// Where development starts to tell, in stops from middle grey: the shadows a negative's speed is read at.
const DEV_SPEED_POINT = -2.5;
/// Where a compensating developer starts to hold back, in stops from middle grey: the light midtones.
const DEV_KNEE = 1.5;

// ln(1 + eᵘ): a hinge, flat below 0 and rising with slope 1 above, rounded over about a stop.
fn dev_softplus(u: f32) -> f32 {
  return select(log(1.0 + exp(u)), u + log(1.0 + exp(-u)), u > 0.0);
}

// The film's exposure `e` as the developer leaves it. `local` is the exposure around it, log2 over middle grey
// (the adjacency map); `adjacency` is 0 when there is no map. `d` is `DevelopParams.developer`.
fn developer_apply(e: f32, local: f32, adjacency: f32, d: vec4f) -> f32 {
  // The standard developer at normal development does nothing, exactly.
  if (e <= 1e-10 || (adjacency == 0.0 && d.z == 1.0 && d.w == 0.0)) { return e; }
  var x = log2(e / 0.18);
  // Adjacency: away from the surroundings, as fresh developer at an edge's thin side and exhausted at its dense
  // one push the two apart.
  x = x + adjacency * (x - local);
  // Development: contrast times `d.z` above the speed point, rounded into the shadows below it.
  x = x + (d.z - 1.0) * dev_softplus(x - DEV_SPEED_POINT);
  // Compensation: density grows by `1 − d.w` as much above the knee.
  x = x - d.w * dev_softplus(x - DEV_KNEE);
  return 0.18 * exp2(x);
}
