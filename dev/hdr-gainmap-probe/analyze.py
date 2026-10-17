#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Compares measurements.csv (from measure.py) against each patch's predicted linear HDR value, to check
whether the open ISO 21496-1 / Adobe Gain Map formula (docs/ROADMAP.md, "HDR originals keep their gain map")
is actually what an iPhone's Safari renders — and, if so, whether Apple's own per-photo
`XMP-HDRGainMap:HDRGainMapHeadroom` behaves like this format's `maxContentBoost`.

Usage:
    python3 analyze.py [out-dir]      default: ./out

Every reading is converted to a multiple of the measured SDR white level (the `cal-white` calibration patch,
or `cal-grey50` if that's missing) rather than trusted in absolute nits — the colorimeter's own calibration
and the screen's brightness setting cancel out of a ratio, but not out of an absolute number, so this is the
more robust comparison to make first.
"""
import csv
import json
import sys
from pathlib import Path


def mean_reading(rows: list[dict], patch_id: str) -> float | None:
    values = [float(r["reading"].split(",")[-1]) for r in rows if r["id"] == patch_id]
    return sum(values) / len(values) if values else None


def main() -> None:
    out_dir = Path(sys.argv[1] if len(sys.argv) > 1 else "out")
    patches = {p["id"]: p for p in json.loads((out_dir / "manifest.json").read_text())}
    csv_path = out_dir / "measurements.csv"
    if not csv_path.is_file():
        sys.exit(f"{csv_path} not found — run measure.py first")
    rows = list(csv.DictReader(csv_path.open()))

    white = mean_reading(rows, "cal-white")
    anchor_id = "cal-white"
    if white is None or white <= 0:
        white = mean_reading(rows, "cal-grey50")
        anchor_id = "cal-grey50"
        # cal-grey50 is encoded 128/255 sRGB, i.e. ~0.216 linear (see generate.sh) of true SDR white.
        if white:
            white /= 0.215861
    if not white:
        sys.exit("No usable cal-white or cal-grey50 reading to anchor the scale — measure at least one.")
    print(f"Anchoring 1.0 linear = {white:.4g} (from {anchor_id}, scaled to a full-white equivalent)\n")

    print(f"{'patch':<28} {'predicted':>10} {'measured':>10} {'ratio':>8}")
    ratios = []
    for pid, p in patches.items():
        if "predictedLinearHdr" not in p:
            continue
        measured_raw = mean_reading(rows, pid)
        if measured_raw is None:
            continue
        measured = measured_raw / white
        predicted = p["predictedLinearHdr"]
        ratio = measured / predicted if predicted else float("nan")
        ratios.append(ratio)
        print(f"{pid:<28} {predicted:>10.4f} {measured:>10.4f} {ratio:>8.3f}")

    if not ratios:
        print("\nNo measured patches matched yet — run measure.py for at least the gain/headroom sweeps.")
        return
    mean_ratio = sum(ratios) / len(ratios)
    spread = max(ratios) - min(ratios)
    print(f"\nmean measured/predicted ratio: {mean_ratio:.3f} (spread across patches: {spread:.3f})")
    if spread < 0.15:
        print("Consistent ratio across patches: the formula's *shape* (the log2/exp2 interpolation) looks")
        print("right, even if the overall scale is off by a constant (display calibration, likely) — that's")
        print("what stage 2 needs. A mean ratio far from 1.0 just means the display isn't calibrated to the")
        print("colorimeter's absolute scale, which doesn't matter for this check.")
    else:
        print("Ratio varies a lot across patches: the formula's shape doesn't match what's actually")
        print("rendered. Worth checking per sweep (gain sweep vs. headroom sweep vs. SDR-base sweep,")
        print("printed in generate.sh's patches.tsv order) to see which dimension disagrees.")


if __name__ == "__main__":
    main()
