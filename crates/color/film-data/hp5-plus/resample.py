# SPDX-License-Identifier: Apache-2.0
# Resample traced.txt: the spectrogram at 380-730 nm in 10 nm steps (0 beyond the trace), and the characteristic
# curve at log E 0.0-4.1 in 0.1 steps; then fit the curve's toe with fog + G·w·ln(1 + exp((logE − e0)/w)).
import math

spec, curve, mode = [], [], None
for line in open("traced.txt"):
    if line.startswith("CURVE"):
        mode = curve
        continue
    if line.startswith("SPECTRUM"):
        mode = spec
        continue
    a, b = map(float, line.split())
    mode.append((a, b))


def at(pts, x):
    if x < pts[0][0] or x > pts[-1][0]:
        return None
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        if x0 <= x <= x1:
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0)


nm = [380 + 10 * i for i in range(36)]
w = [max(at(spec, l) or 0.0, 0.0) for l in nm]
print("SPECTROGRAM", ", ".join(f"{v:.3f}" for v in w))
logE = [round(0.1 * i, 1) for i in range(42)]
d = [at(curve, e) for e in logE]
print("CURVE", ", ".join(f"({e}, {v:.3f})" for e, v in zip(logE, d) if v is not None))

pts = [(e, v) for e, v in zip(logE, d) if v is not None]
best = None
for fog in [0.170 + 0.002 * i for i in range(10)]:
    for g in [0.60 + 0.005 * i for i in range(12)]:
        for e0 in [1.0 + 0.02 * i for i in range(40)]:
            for ww in [0.1 + 0.02 * i for i in range(30)]:
                err = sum((fog + g * ww * math.log1p(math.exp((e - e0) / ww)) - v) ** 2 for e, v in pts)
                if best is None or err < best[0]:
                    best = (err, fog, g, e0, ww)
err, fog, g, e0, ww = best
print(f"FIT fog {fog:.3f} G {g:.3f} e0 {e0:.2f} w {ww:.2f} rms {math.sqrt(err / len(pts)):.4f}")
worst = max(abs(fog + g * ww * math.log1p(math.exp((e - e0) / ww)) - v) for e, v in pts)
print(f"FIT worst {worst:.4f}")
