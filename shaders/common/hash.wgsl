// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// PCG hash, from Jarzynski & Olano, "Hash Functions for GPU Rendering", JCGT 9(3), 2020.

fn pcg_hash(v: u32) -> u32 {
  let s = v * 747796405u + 2891336453u;
  let w = ((s >> ((s >> 28u) + 4u)) ^ s) * 277803737u;
  return (w >> 22u) ^ w;
}

fn hash_unit(p: vec2u, salt: u32) -> f32 {
  return f32(pcg_hash(p.x ^ pcg_hash(p.y ^ pcg_hash(salt)))) / 4294967295.0;
}
