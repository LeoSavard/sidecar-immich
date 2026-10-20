#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Interactive measurement logger for the HDR gain map probe.

Deliberately has no colorimeter driver built in: Spyder X support varies by OS and by which tool you have
working (ArgyllCMS's spotread, the Datacolor app, DisplayCAL, …), and guessing at one would be more likely to
break than to save time. Instead this just prompts once per patch and appends whatever number you read off
your own measurement tool, in whatever units you're set up to show (nits is easiest to reason about with
analyze.py, but anything consistent across all your readings works — pick one and stick to it for a session).

Usage:
    python3 measure.py [out-dir]      default: ./out (next to manifest.json from generate.sh)

For each patch, in order: hold the colorimeter on the phone screen where probe.html is showing that patch,
take a reading with whatever tool you're using, then type the luminance (or "x,y,Y", if your tool gives
chromaticity too — only Y is used downstream) here and press Enter. Press Enter alone to skip a patch; Ctrl-C
to stop early — everything already typed is already on disk, one line at a time.
"""
import csv
import datetime
import json
import sys
from pathlib import Path


def main() -> None:
    out_dir = Path(sys.argv[1] if len(sys.argv) > 1 else "out")
    manifest_path = out_dir / "manifest.json"
    if not manifest_path.is_file():
        sys.exit(f"{manifest_path} not found — run ./generate.sh first")
    patches = json.loads(manifest_path.read_text())

    csv_path = out_dir / "measurements.csv"
    is_new = not csv_path.is_file()
    with csv_path.open("a", newline="") as f:
        writer = csv.writer(f)
        if is_new:
            writer.writerow(["timestamp", "id", "label", "reading"])
        print(f"{len(patches)} patches. Advance probe.html to match each prompt before reading.")
        print(f"Appending to {csv_path}\n")
        for n, p in enumerate(patches, start=1):
            try:
                reading = input(f"[{n}/{len(patches)}] {p['id']} — {p['label']}\n  reading (blank to skip): ").strip()
            except KeyboardInterrupt:
                print("\nStopped early; what you've already entered is saved.")
                return
            if reading:
                writer.writerow([datetime.datetime.now().isoformat(timespec="seconds"), p["id"], p["label"], reading])
                f.flush()


if __name__ == "__main__":
    main()
