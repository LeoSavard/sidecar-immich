# SPDX-License-Identifier: Apache-2.0
# Trace ILFORD SFX 200's wedge spectrogram to tungsten light (technical information, Nov 2018, page 1; image
# sfx-001.jpg from pdfimages): two curves, SFX 200 and "conventional film". Calibration from the frame's ticks:
# 400..800 nm at x = 101..728; sensitivity 1.0 at y = 119, 0.5 at y = 212. Where both curves cross a column,
# SFX 200 is the lower one and conventional film the upper; the labels' leader lines are skipped.
from PIL import Image

im = Image.open("sfx-001.jpg").convert("L")
px = im.load()


def runs(x, y0, y1, dark=110):
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
    return [(a, b) for a, b in out if b - a >= 2]


def centre(a, b):
    w = sum(255 - px[x_, y] for y in range(a, b + 1) for x_ in [cx])
    return sum(y * (255 - px[cx, y]) for y in range(a, b + 1)) / w


nm = lambda x: 400 + (x - 101) / (728 - 101) * 400
sens = lambda y: 1.0 - (y - 119) / (212 - 119) * 0.5
sfx, conv = {}, {}
for cx in range(12, 747):
    if 366 <= cx <= 378 or 598 <= cx <= 612:
        continue
    r = runs(cx, 70, 319)
    if not r:
        continue
    lo = max(r, key=lambda ab: ab[1])
    hi = min(r, key=lambda ab: ab[0])
    if len(r) >= 2 or nm(cx) > 660:
        # Follow SFX 200 by continuity where conventional film's steep fall crosses it.
        if sfx and nm(cx) > 600:
            last = sfx[max(sfx)]
            lo = min(r, key=lambda ab: abs(sens(centre(*ab)) - last))
        sfx[nm(cx)] = max(sens(centre(*lo)), 0.0)
    if len(r) >= 2 and nm(cx) < 660:
        conv[nm(cx)] = max(sens(centre(*hi)), 0.0)
    elif len(r) == 1 and nm(cx) < 470:
        # Where the two run together, both are this one.
        sfx[nm(cx)] = conv[nm(cx)] = max(sens(centre(*lo)), 0.0)


def at(d, l):
    ks = sorted(d)
    for a, b in zip(ks, ks[1:]):
        if a <= l <= b:
            return d[a] + (d[b] - d[a]) * (l - a) / (b - a)
    return 0.0


print("SFX200 380-730:", ", ".join(f"{at(sfx, 380 + 10 * i):.3f}" for i in range(36)))
print("CONVENTIONAL 380-730:", ", ".join(f"{at(conv, 380 + 10 * i):.3f}" for i in range(36)))
