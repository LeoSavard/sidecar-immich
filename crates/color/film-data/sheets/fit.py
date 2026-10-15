# SPDX-License-Identifier: Apache-2.0
# Fit each traced characteristic curve with a toe, a straight line and a shoulder:
#   D = fog + G·(w·sp((E − e0)/w) − c·v·sp((E − e1)/v)),  sp(u) = ln(1 + eᵘ)
# by Nelder–Mead on the squared density error. Then: the speed point (fog + 0.1), middle grey 1.2 log E above
# it, and the toe and shoulder in stops from grey, as crates/color/src/stock.rs takes them.
import math
import re


def sp(u):
    return u + math.log1p(math.exp(-u)) if u > 0 else math.log1p(math.exp(u))


def model(p, e):
    fog, g, e0, w, e1, v, c = p
    return fog + g * (w * sp((e - e0) / w) - c * v * sp((e - e1) / v))


def nelder_mead(f, x0, steps, iters=6000):
    pts = [list(x0)] + [[x + (s if i == j else 0) for j, x in enumerate(x0)] for i, s in enumerate(steps)]
    vals = [f(p) for p in pts]
    for _ in range(iters):
        order = sorted(range(len(pts)), key=lambda i: vals[i])
        pts, vals = [pts[i] for i in order], [vals[i] for i in order]
        n = len(x0)
        centroid = [sum(p[j] for p in pts[:-1]) / n for j in range(n)]
        refl = [centroid[j] + (centroid[j] - pts[-1][j]) for j in range(n)]
        fr = f(refl)
        if fr < vals[0]:
            exp_ = [centroid[j] + 2 * (centroid[j] - pts[-1][j]) for j in range(n)]
            fe = f(exp_)
            pts[-1], vals[-1] = (exp_, fe) if fe < fr else (refl, fr)
        elif fr < vals[-2]:
            pts[-1], vals[-1] = refl, fr
        else:
            con = [centroid[j] + 0.5 * (pts[-1][j] - centroid[j]) for j in range(n)]
            fc = f(con)
            if fc < vals[-1]:
                pts[-1], vals[-1] = con, fc
            else:
                pts = [pts[0]] + [[pts[0][j] + 0.5 * (p[j] - pts[0][j]) for j in range(n)] for p in pts[1:]]
                vals = [vals[0]] + [f(p) for p in pts[1:]]
    i = min(range(len(pts)), key=lambda i: vals[i])
    return pts[i], vals[i]


def fit(name, data):
    def err(p):
        fog, g, e0, w, e1, v, c = p
        if w < 0.05 or v < 0.05 or not (0 <= c <= 1) or g <= 0:
            return 1e9
        return sum((model(p, e) - d) ** 2 for e, d in data)

    es = [e for e, _ in data]
    x0 = [min(d for _, d in data), 0.6, es[0] + 0.3 * (es[-1] - es[0]), 0.25, es[-1] - 0.3, 0.3, 0.3]
    best = None
    for c0 in (0.0, 0.3, 0.6):
        for e1 in (es[-1] - 0.8, es[-1] - 0.3, es[-1] + 0.3):
            x0[4], x0[6] = e1, c0
            p, v = nelder_mead(err, x0, [0.02, 0.1, 0.2, 0.1, 0.3, 0.1, 0.2])
            if best is None or v < best[1]:
                best = (p, v)
    p, v = best
    fog, g, e0, w, e1, vv, c = p
    worst = max(abs(model(p, e) - d) for e, d in data)
    # Speed point: the log exposure where density is fog + 0.1, by bisection.
    lo, hi = es[0] - 2, es[-1]
    for _ in range(100):
        mid = (lo + hi) / 2
        lo, hi = (mid, hi) if model(p, mid) < fog + 0.1 else (lo, mid)
    speed = lo
    grey = speed + 1.2
    stop = math.log10(2)
    print(f"{name}: fog {fog:.3f} G {g:.3f} e0 {e0:.3f} w {w:.3f} e1 {e1:.3f} v {vv:.3f} c {c:.3f}  rms {math.sqrt(v / len(data)):.4f} worst {worst:.4f}")
    print(f"{name}:   speed point {speed:.3f}, grey {grey:.3f}; toe at {(e0 - grey) / stop:.2f} stops, width {w / stop:.2f}; shoulder at {(e1 - grey) / stop:.2f} stops, width {vv / stop:.2f}, strength {c:.3f}")
    return p


def parse(line):
    return [(float(a), float(b)) for a, b in re.findall(r"\((-?[\d.]+), (-?[\d.]+)\)", line)]


lines = {l.split(":")[0]: l for l in open("kodak.txt") if "min:" in l}
sets = {
    "HP5 Plus (ILFOTEC HC 1+31)": [(round(0.1 * i, 1), d) for i, d in enumerate([None, 0.179, 0.179, 0.179, 0.179, 0.179, 0.188, 0.199, 0.210, 0.230, 0.253, 0.285, 0.319, 0.361, 0.410, 0.464, 0.518, 0.588, 0.656, 0.724, 0.791, 0.858, 0.924, 0.987, 1.050, 1.116, 1.182, 1.246, 1.308, 1.374, 1.437, 1.499, 1.564, 1.627, 1.690, 1.754, 1.818, 1.880, 1.941, 2.002, 2.067]) if d is not None],
    "Tri-X 400 (D-76 8 min)": parse(lines["TRIX400 D-76 8min"].split(":", 1)[1]),
    "Tri-X 320 (D-76 8 min)": parse(lines["TRIX320 D-76 8min"].split(":", 1)[1]),
    "Double-X (D-96 6.5 min)": [(-3.0, 0.225)] + parse(open("doublex.txt").read().split("6.5min:")[1]),
    "SFX 200 (ID-11 10 min)": [(round(0.1 * i, 1), 0.287) for i in range(1, 6)] + parse(open("sfx_curve.txt").read().split(":", 1)[1]),
}
for name, data in sets.items():
    fit(name, data)
