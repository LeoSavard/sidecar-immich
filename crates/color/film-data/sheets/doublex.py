# SPDX-License-Identifier: Apache-2.0
# Trace EASTMAN DOUBLE-X 5222's curves (Kodak H-1-5222, March 2022, page 3; images from pdfimages -png):
# dx-001.png, spectral sensitivity: 250..750 nm at x = 103..719, log sensitivity 3..-1 at y = 48..513; the
#   lower curve (density 1.0 over gross fog), which starts at 420 nm.
# dx-000.png, sensitometric curves in D-96: log exposure -3..1 at x = 101..668, density 4..0 at y = 100..666;
#   the third of five from the top at the right (6½ minutes, gamma 0.66, the recommended control gamma),
#   followed right to left by continuity, as the curves bunch together in the toe.
from PIL import Image


def load(name):
    im = Image.open(name).convert("L")
    return im.load(), im.size


def runs(px, x, y0, y1, dark=110):
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
    return [(a, b) for a, b in out if b - a >= 1]


def centre(px, x, a, b):
    w = sum(255 - px[x, y] for y in range(a, b + 1))
    return sum(y * (255 - px[x, y]) for y in range(a, b + 1)) / w


def follow(px, xs, y0, y1, start_pick, grid_cols=(), grid_rows=()):
    pts, last = {}, None
    for x in xs:
        if any(abs(x - g) <= 1 for g in grid_cols):
            continue
        # Thin runs on a gridline row are the gridline, not the curve.
        r = [(a, b) for a, b in runs(px, x, y0, y1) if not (b - a <= 3 and any(a - 1 <= g <= b + 1 for g in grid_rows))]
        if not r:
            continue
        if last is None:
            a, b = start_pick(r)
        else:
            a, b = min(r, key=lambda ab: abs(centre(px, x, *ab) - last))
            if abs(centre(px, x, a, b) - last) > 12:
                continue
        last = centre(px, x, a, b)
        pts[x] = last
    return pts


def at(d, v):
    ks = sorted(d)
    for a, b in zip(ks, ks[1:]):
        if a <= v <= b:
            return d[a] + (d[b] - d[a]) * (v - a) / (b - a)
    return None


# Spectral: the lower curve, from 420 nm (x = 315) to where it ends, left to right; gridlines skipped.
px, (w, h) = load("dx-001.png")
grid = (165, 227, 288, 350, 412, 473, 535, 597, 658)
spec = follow(px, range(318, 640), 282, 500, lambda r: min(r, key=lambda ab: ab[0]), grid, (280, 397))
nm = {250 + (x - 103) / (719 - 103) * 500: 3.0 - (y - 48) / (513 - 48) * 4.0 for x, y in spec.items()}
print("DOUBLEX SPECTRAL log sensitivity 380-730:", ", ".join(f"{at(nm, 380 + 10 * i) if at(nm, 380 + 10 * i) is not None else float('nan'):.3f}" for i in range(36)))
print("DOUBLEX SPECTRAL range", min(nm), max(nm))

# Sensitometric: the 6½-minute curve, right to left.
px, (w, h) = load("dx-000.png")
curve = follow(px, range(462, 103, -1), 250, 664, lambda r: sorted(r)[2] if len(r) >= 5 else sorted(r)[min(2, len(r) - 1)], (243, 385), (242, 383, 524))
pts = {-3.0 + (x - 101) / (668 - 101) * 4.0: 4.0 - (y - 100) / (666 - 100) * 4.0 for x, y in curve.items()}
grid_e = [round(-3.0 + 0.1 * i, 1) for i in range(41)]
print("DOUBLEX D-96 6.5min:", ", ".join(f"({e}, {at(pts, e):.3f})" for e in grid_e if at(pts, e) is not None))
