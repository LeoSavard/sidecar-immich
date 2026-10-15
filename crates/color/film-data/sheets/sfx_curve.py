# SPDX-License-Identifier: Apache-2.0
# Trace ILFORD SFX 200's characteristic curve (technical information, Nov 2018, page 2; ID-11 stock, 10 min,
# 20 °C): relative log E 0..4.5 at x = 10..440 (96 px per unit), density 0..3 at y = 293..10 (94 px per unit).
# For each column the curve is the thickest dark run; gridline columns are skipped.
from PIL import Image

im = Image.open("sfx-002.png").convert("L")
px = im.load()
skip = (10, 55, 56, 104, 152, 200, 248, 296, 344, 392, 440)
pts = {}
for x in range(12, 438):
    if any(abs(x - g) <= 1 for g in skip):
        continue
    out, start = [], None
    for y in range(12, 291):
        d = px[x, y] < 110
        if d and start is None:
            start = y
        if not d and start is not None:
            out.append((start, y - 1))
            start = None
    out = [(a, b) for a, b in out if b - a >= 3]
    if not out:
        continue
    a, b = max(out, key=lambda ab: ab[1] - ab[0])
    w = sum(255 - px[x, y] for y in range(a, b + 1))
    pts[(x - 10) / 96] = (293 - sum(y * (255 - px[x, y]) for y in range(a, b + 1)) / w) / 94


def at(v):
    ks = sorted(pts)
    for p, q in zip(ks, ks[1:]):
        if p <= v <= q:
            return pts[p] + (pts[q] - pts[p]) * (v - p) / (q - p)


print("SFX200 ID-11 10min:", ", ".join(f"({e / 10}, {at(e / 10):.3f})" for e in range(1, 44) if at(e / 10) is not None))
