// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
//! requires: common/transfer.wgsl
// Histogram of the developed image as the screen shows it (sRGB-encoded), computed in VRAM: the output's
// pixels — inside the crop, through the same geometry the view applies — read from the developed texture.
// 4 × 256 bins (R, G, B, luma) plus clipped-pixel counts; only these 1,026 integers are read back.

struct HistParams {
  dims: vec2u,           // the output's size in pixels
  stride: u32,
  _pad: u32,
  hi_t: f32,             // display-linear threshold for "at white"
  lo_t: f32,             // display-linear threshold for "at black"
  _pad2: vec2f,
  geometry: mat3x3f,     // output (0…1) → developed texture uv
}

@group(0) @binding(0) var developed: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> bins: array<atomic<u32>, 1026>;
@group(0) @binding(2) var<uniform> H: HistParams;

fn bin(v: f32) -> u32 {
  return min(u32(max(v, 0.0) * 256.0), 255u);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let p = id.xy * H.stride;
  if (p.x >= H.dims.x || p.y >= H.dims.y) { return; }
  let h = H.geometry * vec3f((vec2f(p) + 0.5) / vec2f(H.dims), 1.0);
  let uv = h.xy / h.z;
  if (h.z <= 0.0 || uv.x < 0.0 || uv.y < 0.0 || uv.x >= 1.0 || uv.y >= 1.0) { return; }
  let size = textureDimensions(developed);
  let c = textureLoad(developed, min(vec2u(uv * vec2f(size)), size - 1u), 0).rgb;
  let e = srgb_encode(c);
  atomicAdd(&bins[bin(e.r)], 1u);
  atomicAdd(&bins[256u + bin(e.g)], 1u);
  atomicAdd(&bins[512u + bin(e.b)], 1u);
  atomicAdd(&bins[768u + bin(dot(e, vec3f(0.2126, 0.7152, 0.0722)))], 1u);
  let m = max(c.r, max(c.g, c.b));
  if (m >= H.hi_t) { atomicAdd(&bins[1024u], 1u); }
  if (m <= H.lo_t) { atomicAdd(&bins[1025u], 1u); }
}
