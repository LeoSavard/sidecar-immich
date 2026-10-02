// SPDX-License-Identifier: Apache-2.0
// Copyright Sidecar contributors
// Brush strokes onto a mask (DESIGN.md §15), one stroke at a time: each segment of the path is drawn as a
// capsule — a quad around it, the fragment measuring its distance to the segment — into a coverage target
// blended with `max`, so a stroke lays down its flow once however its segments overlap. The composite pass
// (brush_composite.wgsl) then paints that coverage onto the mask, or erases with it. The browser draws at the
// proxy's size and the worker at the export's, with this same file.

struct BrushParams {
  size: vec2f,           // the target, pixels
  radius: f32,           // pixels
  feather: f32,          // 0 hard … 1 soft from the centre
  flow: f32,             // 0…1
  _pad0: f32,
  _pad1: vec2f,
}

@group(0) @binding(0) var<uniform> B: BrushParams;

struct Capsule {
  @builtin(position) pos: vec4f,
  @location(0) @interpolate(flat) a: vec2f,
  @location(1) @interpolate(flat) b: vec2f,
}

// `seg` is one segment, both ends in target pixels; a dab is a segment of length zero.
@vertex
fn vs(@builtin(vertex_index) v: u32, @location(0) seg: vec4f) -> Capsule {
  var corners = array<vec2f, 6>(vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(-1.0, 1.0), vec2f(-1.0, 1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0));
  let corner = corners[v];
  let a = seg.xy;
  let b = seg.zw;
  let d = b - a;
  let len = length(d);
  let dir = select(vec2f(1.0, 0.0), d / max(len, 1e-6), len > 1e-4);
  let normal = vec2f(-dir.y, dir.x);
  let r = B.radius + 1.0;
  let along = select(a - dir * r, b + dir * r, corner.x > 0.0);
  let p = along + normal * (r * corner.y);
  var out: Capsule;
  out.pos = vec4f(p / B.size * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0), 0.0, 1.0);
  out.a = a;
  out.b = b;
  return out;
}

@fragment
fn fs(in: Capsule) -> @location(0) vec4f {
  let ab = in.b - in.a;
  let t = clamp(dot(in.pos.xy - in.a, ab) / max(dot(ab, ab), 1e-8), 0.0, 1.0);
  let d = length(in.pos.xy - (in.a + ab * t));
  // At least a pixel of edge however hard the brush, so its outline is antialiased at every size.
  let inner = min(B.radius * (1.0 - B.feather), B.radius - 1.0);
  return vec4f(B.flow * (1.0 - smoothstep(inner, B.radius, d)), 0.0, 0.0, 1.0);
}
