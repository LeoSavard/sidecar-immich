// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Depth pass 0 of 3: make the depth map's edges the photo's edges (ROADMAP, "Depth, beyond what shipped").
//
// Depth Anything gives a 518² map of relative depth. Stretched to the frame it is smooth where the photo is
// sharp, so a lens blur's edge follows the map rather than the subject: hair, a thin branch or a railing gets
// a halo of the wrong distance. This pass re-sharpens the stretched map against the photo — joint bilateral
// upsampling (Kopf, Cohen, Lischinski and Uyttendaele, 2007), implemented from the paper: each output texel
// is a weighted mean of nearby depths, weighted by distance *and* by how closely the photo's luminance there
// matches the photo's luminance here. Across an edge the luminance differs, so the far side barely counts and
// depth steps where the photo steps; within a flat region every tap counts and the map stays smooth.
//
// The taps are spaced a coarse texel apart, because that is the scale of what has to be repaired: finer taps
// would only re-read the same interpolated values. `D.spacing` carries that spacing in this texture's pixels
// (the frame's long edge over the model's 518), so the cost is a fixed 25 taps whatever the resolution.
//
// It is not He, Sun and Tang's guided filter, which the roadmap named: that needs box means of the guide, the
// input and their products — several passes and intermediate textures — for a result that differs mainly in
// how it handles gradients *within* a region, which a depth map does not have much of. This is one pass, it
// is the same arithmetic in both engines, and it is where the halo actually comes from.

struct DepthRefineParams {
  spacing: f32,      // distance between taps, in this texture's pixels
  sigma_range: f32,  // luminance difference that halves a tap's weight, in the photo's linear light
  _pad: vec2f,
}

@group(0) @binding(0) var<uniform> D: DepthRefineParams;
@group(0) @binding(1) var photo: texture_2d<f32>;
@group(0) @binding(2) var depth: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
@group(0) @binding(4) var dst: texture_storage_2d<rgba16float, write>;

// Rec.2020 luminance, as the rest of the pipeline weighs it.
const REFINE_LUMA = vec3f(0.2627, 0.6780, 0.0593);

// Taps per side: 5 × 5, two coarse texels either way.
const REFINE_RADIUS: i32 = 2;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let size = textureDimensions(dst);
  if (id.x >= size.x || id.y >= size.y) { return; }
  let dims = vec2f(size);
  let here = vec2f(id.xy) + 0.5;
  let centre = dot(textureSampleLevel(photo, samp, here / dims, 0.0).rgb, REFINE_LUMA);

  // A spacing under a pixel means the map is already at the photo's resolution: nothing to repair.
  let step = max(D.spacing, 0.0);
  if (step < 1.0) {
    textureStore(dst, id.xy, textureSampleLevel(depth, samp, here / dims, 0.0));
    return;
  }
  // The spatial term spans the taps rather than falling off within them: the texel being repaired is the one
  // the centre sits in, so its own value is no more trustworthy than its neighbours' and must not drown them.
  let spatial = 1.5 * step;
  let inv_spatial = 1.0 / (2.0 * spatial * spatial);
  let inv_range = 1.0 / (2.0 * max(D.sigma_range, 1e-4) * max(D.sigma_range, 1e-4));

  var sum = 0.0;
  var weight = 0.0;
  for (var dy = -REFINE_RADIUS; dy <= REFINE_RADIUS; dy++) {
    for (var dx = -REFINE_RADIUS; dx <= REFINE_RADIUS; dx++) {
      let offset = vec2f(f32(dx), f32(dy)) * step;
      let at = clamp(here + offset, vec2f(0.5), dims - 0.5) / dims;
      let luma = dot(textureSampleLevel(photo, samp, at, 0.0).rgb, REFINE_LUMA);
      let d = luma - centre;
      let w = exp(-dot(offset, offset) * inv_spatial - d * d * inv_range);
      sum += w * textureSampleLevel(depth, samp, at, 0.0).r;
      weight += w;
    }
  }
  // The centre tap always weighs 1, so `weight` can never be zero.
  let out = sum / weight;
  textureStore(dst, id.xy, vec4f(out, out, out, 1.0));
}
