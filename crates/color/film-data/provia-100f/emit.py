# SPDX-License-Identifier: Apache-2.0
# Write provia.txt (provia.py's output) as Rust for crates/color/src/film_data.rs.
import math
import re

text = open("provia.txt").read()


def row(prefix):
    return text.split(prefix, 1)[1].split("\n", 1)[0]


out = ["// FUJICHROME PROVIA 100F Professional [RDP III] (Fujifilm data sheet AF3-036E, page 6).\n"]
for name, key in (("RED", "CURVE red:"), ("GREEN", "CURVE green:"), ("BLUE", "CURVE blue:")):
    pts = re.findall(r"\((-?[\d.]+), (-?[\d.]+)\)", row(key))
    out.append(f"/// PROVIA 100F's {name.lower()} characteristic curve: (log H in lux·s, Status A density), E-6, daylight 1/50 s.\n"
               f"pub const PROVIA_100F_{name}_CURVE: [(f64, f64); {len(pts)}] = [{', '.join(f'({float(a):.2f}, {float(b):.3f})' for a, b in pts)}];\n")
for name in ("yellow", "magenta", "cyan"):
    vals = [float(v) for v in row(f"DYE {name} range").split(":", 1)[1].split(",")]
    # Outside the sheet's plotted range (388-719 nm) each end's value is held.
    known = [i for i, v in enumerate(vals) if not math.isnan(v)]
    vals = [vals[known[0]] if i < known[0] else vals[known[-1]] if i > known[-1] else v for i, v in enumerate(vals)]
    out.append(f"/// PROVIA 100F's {name} dye: spectral diffuse density, peak 1, 380-730 nm (the sheet's 388-719 nm; held at its edge value beyond).\n"
               f"pub const PROVIA_100F_{name.upper()}_DYE: [f64; 36] = [{', '.join(f'{v:.4f}' for v in vals)}];\n")
for name in ("blue", "green", "red"):
    vals = [float(v) for v in row(f"SENS {name} range").split(":", 1)[1].split(",")]
    out.append(f"/// PROVIA 100F's {name}-sensitive layer: log₁₀ sensitivity (cm²/J for density 1.0 above D-min), 380-730 nm; NaN where\n"
               f"/// the sheet draws nothing (below its plotted floor, about 1/30 of the peak).\n"
               f"pub const PROVIA_100F_{name.upper()}_SENSITIVITY: [f64; 36] = [{', '.join('f64::NAN' if math.isnan(v) else f'{v:.4f}' for v in vals)}];\n")
open("emitted.rs", "w").write("\n".join(out))
print("\n".join(o[:160] for o in out))
