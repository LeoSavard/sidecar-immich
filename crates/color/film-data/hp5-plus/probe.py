# SPDX-License-Identifier: Apache-2.0
# Find gridlines and tick marks in the HP5 Plus datasheet graphs: rows and columns with long runs of darkish
# pixels, and the darkness of the curve itself, to calibrate the tracer.
import sys
from PIL import Image

for name in ("p5-000.jpg", "p1-002.jpg"):
    im = Image.open(name).convert("L")
    w, h = im.size
    px = im.load()
    print(name, w, h)
    cols = [sum(1 for y in range(h) if px[x, y] < 160) for x in range(w)]
    rows = [sum(1 for x in range(w) if px[x, y] < 160) for y in range(h)]
    print(" columns with > 40% dark:", [x for x in range(w) if cols[x] > 0.4 * h])
    print(" rows with > 40% dark:", [y for y in range(h) if rows[y] > 0.4 * w])
    # Short vertical tick marks: columns where a few pixels just below/right of axes are dark.
    hist = {}
    for y in range(h):
        for x in range(w):
            v = px[x, y] // 20
            hist[v] = hist.get(v, 0) + 1
    print(" grey histogram (/20):", sorted(hist.items()))
