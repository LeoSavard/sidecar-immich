# SPDX-License-Identifier: Apache-2.0
# Gridlines of the Provia sheet's stencil graphs (page 6), in page coordinates: each stencil's own placement
# (pdftocairo -svg) maps its pixels onto the page, so frames and curves drawn as separate stencils line up.
from PIL import Image

# Placements from p6.svg: (sx, sy, tx, ty), page = (sx * px + tx, sy * py + ty).
PLACE = {
    "st-000.png": (0.240159, 0.240135, 52.9807, 162.7304),  # characteristic curves' frame
    "st-001.png": (0.240159, 0.240135, 52.9099, 162.4745),
    "st-002.png": (0.240159, 0.240135, 52.9099, 162.4745),
    "st-003.png": (0.240159, 0.240135, 52.9099, 162.4745),
    "st-005.png": (0.240124, 0.24, 307.043, 437.41),  # dye densities' frame
    "st-006.png": (0.240124, 0.24, 306.71, 437.3279),
    "st-007.png": (0.240124, 0.24, 306.71, 437.3279),
    "st-008.png": (0.240124, 0.24, 306.71, 437.3279),
}


def lines(name):
    im = Image.open(name).convert("L")
    px = im.load()
    w, h = im.size
    sx, sy, tx, ty = PLACE[name]
    cols = [x for x in range(w) if sum(px[x, y] > 128 for y in range(h)) > 0.5 * h]
    rows = [y for y in range(h) if sum(px[x, y] > 128 for x in range(w)) > 0.5 * w]
    return [round(sx * x + tx, 2) for x in cols], [round(sy * y + ty, 2) for y in rows]


if __name__ == "__main__":
    for n in ("st-000.png", "st-005.png"):
        c, r = lines(n)
        print(n, "columns", c)
        print(n, "rows", r)
