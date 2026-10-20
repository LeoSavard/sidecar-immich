# HDR gain map probe

`sidecar_raw::gain_map` (crates/raw/src/gain_map.rs) can read Apple's HDR gain map out of a HEIC, but
docs/ROADMAP.md's stage 2 — applying it — is blocked on a real question: **what formula actually turns the
SDR base plus the gain map into what an iPhone's screen shows?** Apple hasn't published one, and the
reverse-engineering attempts that exist call their own results "known to be incorrect" or "unverified" (see
the module docs and ROADMAP.md for the sources). Guessing in code would look plausible and be silently wrong.

This directory builds a measurable alternative: real, controlled Ultra HDR JPEGs (the open ISO 21496-1 /
Adobe Gain Map format, which Apple's iOS 18+ claims interoperability with) at known parameters, to view on an
actual HDR screen and measure with a colorimeter.

## What it tests

[Google's `libultrahdr`](https://github.com/google/libultrahdr) (MIT/Apache-2.0, the reference encoder and
decoder Android and Chrome use) documents its reconstruction math in `lib/src/gainmapmath.cpp`:

```
gain = gainMapPixel / 255                                      (after undoing gamma, which we always leave at 1)
logBoost = log2(minContentBoost) * (1 - gain) + log2(maxContentBoost) * gain
HDR = (SDR_linear + offsetSdr) * exp2(logBoost) - offsetHdr
```

Every patch here uses `minContentBoost = 1` and both offsets `= 0`, which reduces this to
`HDR = SDR_linear * headroom^gain` — a grounded version of the "looks good" guesses in ROADMAP.md, using a
real per-photo headroom instead of a made-up constant. Measuring these patches checks two things at once:
whether Safari on iOS actually renders this documented formula faithfully, and — if so — whether treating
Apple's own `XMP-HDRGainMap:HDRGainMapHeadroom` as this format's `maxContentBoost` gives the formula stage 2
needs.

## Running it

```sh
./generate.sh                        # builds libultrahdr in a throwaway container, writes out/*.jpg + out/manifest.json
python3 -m http.server               # from this directory
```

Open `http://<this-computer's-LAN-IP>:8000/probe.html` on the iPhone (not `file://` — browsers block `fetch`
on that scheme). Tap the right half of the screen (or press →) to advance through calibration colours and the
HDR test patches; the current patch's id and parameters are shown at the bottom.

On the computer, with the colorimeter in hand:

```sh
python3 measure.py                   # prompts once per patch, appends out/measurements.csv
python3 analyze.py                   # compares measurements.csv against each patch's predicted value
```

`measure.py` doesn't talk to the colorimeter itself — Spyder X support varies by OS and tool (ArgyllCMS's
`spotread`, the Datacolor app, DisplayCAL, …), so it just prompts for whatever number your own tool gives you.
Anything consistent works; nits is easiest to reason about.

## Reading the result

`analyze.py` anchors everything to the measured `cal-white` patch (falling back to `cal-grey50`) so the
comparison is a ratio, not an absolute number — the colorimeter's calibration and the screen's brightness
setting don't matter, only whether every patch scales the same way relative to white. A consistent ratio
across all patches (even one far from 1.0) means the *shape* of the formula is right; a ratio that varies a
lot between the gain sweep, the headroom sweep, and the SDR-base sweep means it isn't, and which sweep
disagrees is a clue toward what the real one looks like instead.

This only tests the open, documented format. Whether Apple's actual proprietary HEIC mechanism
(`urn:com:apple:photo:2020:aux:hdrgainmap`, what `gain_map.rs` reads) uses the same math is a separate
question this doesn't answer directly — but if the ratio holds, it's the best-supported guess available, and
a much better starting point than the unverified community formulas.
