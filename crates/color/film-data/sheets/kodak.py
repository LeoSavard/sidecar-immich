# SPDX-License-Identifier: Apache-2.0
# Kodak's datasheets draw their graphs as vector paths: read the curves straight from them (pdftocairo -svg), a
# graph's frame giving its axes. Tri-X 320 and 400 (F-4017, Feb 2016): spectral sensitivity on page 7, and
# characteristic curves in D-76 at 6, 8, 10 and 12 minutes (400: page 8, top left; 320: page 9, bottom left).
# Curves are told apart by dash pattern, as the graphs' legends draw them.
from vectors import paths


def frame_and_curves(svg, frame_index, curve_indices):
    ps = list(paths(svg))
    fx = [p[0] for p in ps[frame_index][0]]
    fy = [p[1] for p in ps[frame_index][0]]
    frame = (min(fx), max(fx), min(fy), max(fy))
    return frame, {name: ps[i][0] for name, i in curve_indices.items()}


def to_axes(pts, frame, x_range, y_range):
    x0, x1, y0, y1 = frame
    return sorted(((x_range[0] + (x - x0) / (x1 - x0) * (x_range[1] - x_range[0]), y_range[0] + (y1 - y) / (y1 - y0) * (y_range[1] - y_range[0])) for x, y in pts))


def at(pts, x):
    for (xa, ya), (xb, yb) in zip(pts, pts[1:]):
        if xa <= x <= xb and xb > xa:
            return ya + (yb - ya) * (x - xa) / (xb - xa)
    return None


def curves(svg, frame_index, named, label):
    frame, cs = frame_and_curves(svg, frame_index, named)
    for name, pts in cs.items():
        p = to_axes(pts, frame, (-4.0, 1.0), (0.0, 4.0))
        grid = [round(-4.0 + 0.1 * i, 1) for i in range(51)]
        row = [(e, at(p, e)) for e in grid]
        print(f"{label} {name}:", ", ".join(f"({e}, {d:.3f})" for e, d in row if d is not None))


# Spectral sensitivity, page 7: the solid curve (density 1.0 over gross fog), 250-750 nm, log sensitivity 0-4.
frame, cs = frame_and_curves("trix-p7.svg", 423, {"d1": 435})
spec = to_axes(cs["d1"], frame, (250.0, 750.0), (0.0, 4.0))
print("TRIX SPECTRAL log sensitivity 380-730:", ", ".join(f"{(at(spec, 380 + 10 * i) if at(spec, 380 + 10 * i) is not None else float('nan')):.3f}" for i in range(36)))
print("TRIX SPECTRAL range", spec[0], spec[-1])
curves("trix-p8.svg", 252, {"6min": 270, "8min": 271, "10min": 272, "12min": 269}, "TRIX400 D-76")
curves("trix-p9.svg", 368, {"6min": 386, "8min": 388, "10min": 385, "12min": 387}, "TRIX320 D-76")
