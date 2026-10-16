# SPDX-License-Identifier: Apache-2.0
# Read FUJICHROME PROVIA 100F's curves from its data sheet (Fujifilm AF3-036E, page 6):
#  - spectral sensitivity (19): vector paths in p6.svg (pdftocairo -svg -f 6 -l 6), coloured blue, green, red;
#    calibrated from the graph's grid path: 400..700 nm at x = 349.63..527.86, log sensitivity 1, 0, -1 at
#    y = 203.74, 263.13, 322.52;
#  - characteristic curves (18) and spectral dye densities (21): stencil images (pdfimages -png -f 6 -l 6), one per
#    curve, placed on the page by p6.svg's transforms (grid.py); calibrated from each graph's frame stencil.
# The characteristic curves' green and blue are drawn only where they part from red; elsewhere the sheet shows them
# on the red curve, and they are taken from it there.
import re
import sys

from PIL import Image

sys.path.insert(0, "../sheets")  # vectors.py; run from the directory holding the extracted images
from grid import PLACE  # noqa: E402
from vectors import paths  # noqa: E402


def interp(pts, x):
    ks = sorted(pts)
    for a, b in zip(ks, ks[1:]):
        if a <= x <= b:
            return pts[a] + (pts[b] - pts[a]) * (x - a) / (b - a)
    return None


def piecewise(ticks, v):
    """Map page coordinate v by piecewise-linear ticks [(page, value)], extrapolating at the ends."""
    ticks = sorted(ticks)
    for (p0, v0), (p1, v1) in zip(ticks, ticks[1:]):
        if v <= p1 or (p1, v1) == ticks[-1]:
            if v >= p0 or (p0, v0) == ticks[0]:
                return v0 + (v1 - v0) * (v - p0) / (p1 - p0)
    raise ValueError(v)


def stencil_curve(name):
    """Page-coordinate (x, y) of the curve in a stencil: per column, the centre of the topmost white run."""
    im = Image.open(name).convert("L")
    px = im.load()
    w, h = im.size
    sx, sy, tx, ty = PLACE[name]
    out = {}
    for x in range(w):
        y, run = 0, None
        while y < h:
            if px[x, y] > 128:
                a = y
                while y < h and px[x, y] > 128:
                    y += 1
                run = (a, y - 1)
                break
            y += 1
        if run:
            out[sx * x + tx] = sy * (run[0] + run[1]) / 2 + ty
    return out


# Characteristic curves: log H = -3.5 at x = 83.48, 44.7 page units per log unit; density 4.0 at y = 171.62 down to
# 0.0 at 350.28.
logh = lambda x: -3.5 + (x - 83.48) / ((285.21 - 83.48) / 4.5)
dens = lambda y: 4.0 - (y - 171.62) / ((350.28 - 171.62) / 4.0)
curves = {}
for name, stencil in (("red", "st-002.png"), ("green", "st-001.png"), ("blue", "st-003.png")):
    raw = sorted((logh(x), dens(y)) for x, y in stencil_curve(stencil).items() if logh(x) > -3.48)
    # The legend's sample lines sit at density 0.6-0.9 left of log H -2: not the curve, which is at 3.3 there.
    raw = [(e, d) for e, d in raw if not (e < -2.0 and d < 2.0)]
    # Dash ends leave stray fragments: keep points within 0.04 of the median of their neighbours.
    kept = []
    for i, (e, d) in enumerate(raw):
        near = sorted(dd for _, dd in raw[max(0, i - 6):i + 7])
        if abs(d - near[len(near) // 2]) < 0.04:
            kept.append((e, d))
    curves[name] = dict(kept)
grid = [round(-3.4 + 0.05 * i, 2) for i in range(87)]
for name in ("red", "green", "blue"):
    row = []
    for e in grid:
        d = interp(curves[name], e)
        # Gaps in a dashed curve are bridged; past where a curve is drawn, the sheet shows it on the red one.
        lo, hi = min(curves[name]), max(curves[name])
        if d is None or not lo <= e <= hi:
            d = interp(curves["red"], e)
        row.append((e, d))
    print(f"CURVE {name}:", ", ".join(f"({e:.2f}, {d:.3f})" for e, d in row if d is not None))

# Spectral dye densities: wavelength from the frame's ticks, density 1.0, 0.5, 0.0 at y = 467.89, 528.85, 588.25.
nm_ticks = [(346.18, 400), (406.57, 500), (464.56, 600), (524.0, 700)]
d_ticks = [(467.89, 1.0), (528.85, 0.5), (588.25, 0.0)]
for name, stencil in (("yellow", "st-006.png"), ("magenta", "st-007.png"), ("cyan", "st-008.png")):
    c = {piecewise(nm_ticks, x): piecewise(d_ticks, y) for x, y in stencil_curve(stencil).items()}
    print(f"DYE {name} range {min(c):.1f}..{max(c):.1f}:", ", ".join(f"{(interp(c, l) if interp(c, l) is not None else float('nan')):.4f}" for l in range(380, 740, 10)))

# Spectral sensitivity, log: vector paths 293 (blue), 292 (green), 291 (red).
ps = list(paths("p6.svg"))
nm = lambda x: 400 + (x - 349.63) / ((527.86 - 349.63) / 300)
logs = lambda y: (263.13 - y) / ((322.52 - 203.74) / 2)
for name, i in (("blue", 293), ("green", 292), ("red", 291)):
    c = {}
    for x, y in ps[i][0]:
        c[nm(x)] = logs(y)
    print(f"SENS {name} range {min(c):.1f}..{max(c):.1f}:", ", ".join(f"{(interp(c, l) if interp(c, l) is not None else float('nan')):.4f}" for l in range(380, 740, 10)))
