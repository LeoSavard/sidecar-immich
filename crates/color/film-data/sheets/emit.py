# SPDX-License-Identifier: Apache-2.0
# Write the traced data as Rust: each stock's characteristic curve as (log E, density) pairs, and the spectral
# sensitivities resampled at 380-730 nm in 10 nm steps.
import math
import re


def parse(text):
    return [(float(a), float(b)) for a, b in re.findall(r"\((-?[\d.]+), (-?[\d.]+)\)", text)]


def rust_pairs(name, doc, pts):
    rows = ", ".join(f"({e:.1f}, {d:.3f})" for e, d in pts)
    return f"/// {doc}\npub const {name}: [(f64, f64); {len(pts)}] = [{rows}];\n"


def rust_array(name, doc, vals):
    return f"/// {doc}\npub const {name}: [f64; 36] = [{', '.join(f'{v:.3f}' for v in vals)}];\n"


kodak = {l.split(":")[0]: l.split(":", 1)[1] for l in open("kodak.txt") if ":" in l}
trix400 = parse(kodak["TRIX400 D-76 8min"])
trix320 = parse(kodak["TRIX320 D-76 8min"])
doublex = [(-3.0, 0.225)] + parse(open("doublex.txt").read().split("6.5min:")[1])
sfx_curve = [(round(0.1 * i, 1), 0.287) for i in range(1, 6)] + parse(open("sfx_curve.txt").read().split(":", 1)[1])

trix_log = [float(v) for v in kodak["TRIX SPECTRAL log sensitivity 380-730"].split(",")]
# Beyond the curve's end (668 nm) the film sees nothing.
trix_spec = [v if not math.isnan(v) else -9.0 for v in trix_log]
dx_log = [float(v) for v in open("doublex.txt").read().split("380-730:")[1].split("\n")[0].split(",")]
# Kodak's curve starts at 420 nm: held flat below. Its steep end, where the trace jumped to the other curve,
# read from the graph: -0.65 at 660 nm, nothing beyond.
dx_spec = []
for i, v in enumerate(dx_log):
    l = 380 + 10 * i
    if l < 430:
        v = dx_log[5]
    if l == 660:
        v = -0.65
    if l > 660 or math.isnan(v):
        v = -9.0
    dx_spec.append(v)
sfx_spec = [float(v) for v in open("sfx.txt").read().split("SFX200 380-730:")[1].split("\n")[0].split(",")]

out = []
out.append(rust_pairs("TRI_X_400_CURVE", "KODAK PROFESSIONAL TRI-X 400, D-76, 8 minutes, large tank, 20 °C (Kodak Alaris F-4017, Feb 2016, page 8): (log exposure in lux·s, density), read from the sheet's vector graph.", trix400))
out.append(rust_pairs("TRI_X_320_CURVE", "KODAK PROFESSIONAL TRI-X 320, sheets, D-76, 8 minutes, large tank, 20 °C (F-4017, page 9): (log exposure in lux·s, density), read from the sheet's vector graph.", trix320))
out.append(rust_pairs("DOUBLE_X_CURVE", "EASTMAN DOUBLE-X 5222, D-96, 6½ minutes, 21 °C, gamma 0.66 (Kodak H-1-5222, March 2022, page 3): (log exposure in lux·s, density), traced from the sheet's graph.", doublex))
out.append(rust_pairs("SFX_200_CURVE", "ILFORD SFX 200, ID-11 stock, 10 minutes, 20 °C (HARMAN, Nov 2018, page 2): (relative log exposure, density), traced from the sheet's graph.", sfx_curve))
out.append(rust_array("TRI_X_LOG_SENSITIVITY", "KODAK TRI-X's spectral sensitivity (F-4017, page 7): log₁₀ of the reciprocal of the exposure in erg/cm² giving density 1.0 over gross fog, read from the vector graph; −9 where the film sees nothing.", trix_spec))
out.append(rust_array("DOUBLE_X_LOG_SENSITIVITY", "EASTMAN DOUBLE-X's spectral sensitivity (H-1-5222, page 3), as Tri-X's; the sheet starts at 420 nm, held flat below.", dx_spec))
out.append(rust_array("SFX_200_SPECTROGRAM", "ILFORD SFX 200's wedge spectrogram (HARMAN, Nov 2018, page 1), traced and resampled, 0 to 1 as drawn: sensitivity out to 740 nm.", sfx_spec))
open("emitted.rs", "w").write("\n".join(out))
print("\n".join(o[:200] for o in out))
