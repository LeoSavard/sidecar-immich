# SPDX-License-Identifier: Apache-2.0
# Trace the HP5 Plus datasheet's curves (Ilford, Nov 2018) from its embedded graph images.
# Calibration from the gridlines and tick marks found by probe.py:
#   characteristic curve (p5-000.jpg): log E 0..4 at x = 8 + 90.5 * logE; density 0 at y = 276, 89.6 px per unit
#   spectrogram (p1-002.jpg): 400..650 nm at x = 72 .. 359; sensitivity 0 at y = 186, 1.0 at y = 72
# For each column, the curve is the thickest run of dark pixels (thin gridlines and the frame are skipped).
from PIL import Image


def runs(px, x, y0, y1, dark):
    out, start = [], None
    for y in range(y0, y1):
        d = px[x, y] < dark
        if d and start is None:
            start = y
        if not d and start is not None:
            out.append((start, y - 1))
            start = None
    if start is not None:
        out.append((start, y1 - 1))
    return out


def trace(name, x0, x1, y0, y1, dark=110, min_run=3, skip=()):
    im = Image.open(name).convert("L")
    px = im.load()
    pts = {}
    for x in range(x0, x1):
        if any(abs(x - g) <= 1 for g in skip):
            continue
        r = [(a, b) for a, b in runs(px, x, y0, y1, dark) if b - a + 1 >= min_run]
        if r:
            a, b = max(r, key=lambda ab: ab[1] - ab[0])
            # Weighted centre of the run, by darkness.
            wsum = sum(255 - px[x, y] for y in range(a, b + 1))
            pts[x] = sum(y * (255 - px[x, y]) for y in range(a, b + 1)) / wsum
    return pts


# Characteristic curve.
curve = trace("p5-000.jpg", 10, 412, 9, 275, skip=(53, 98, 144, 189, 234, 279, 324, 370))
print("CURVE logE density")
for x in range(10, 412, 9):
    if x in curve:
        print(f"{(x - 8) / 90.5:.3f} {(276 - curve[x]) / 89.6:.3f}")

# Spectrogram: wavelength from the ticks (piecewise linear between them), sensitivity linear.
ticks = [(72, 400), (129, 450), (187, 500), (244, 550), (301, 600), (359, 650)]


def nm(x):
    (xa, la), (xb, lb) = (ticks[0], ticks[1]) if x < ticks[1][0] else next((t, u) for t, u in zip(ticks, ticks[1:]) if x <= u[0]) if x <= ticks[-1][0] else (ticks[-2], ticks[-1])
    return la + (lb - la) * (x - xa) / (xb - xa)


spec = trace("p1-002.jpg", 15, 416, 13, 185)
print("SPECTRUM nm sensitivity")
for x in sorted(spec):
    if x % 3 == 0:
        print(f"{nm(x):.1f} {(186 - spec[x]) / 114.0:.3f}")
