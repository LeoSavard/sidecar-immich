# Handoff — where the work stands

For whoever picks this up next, person or agent. [`ARCHITECTURE.md`](ARCHITECTURE.md) says where the code is,
[`DESIGN.md`](DESIGN.md) why it is that way (its build log records what each feature does and how it was
checked), [`ROADMAP.md`](ROADMAP.md) what's known to be missing. This page is the state of play; update it when
you stop.

*Last updated 2026-10-16.*

## Open pull requests

**The stack has landed, and so has everything built on it.** #3-#24 are merged: `main` now holds colour film
from a data sheet (PROVIA 100F), the depth map upsampled against the photo (#22), the curve's pinned
endpoints (#23) and the tone controls grading a stock (#24), with CI green on each merge.

Three pull requests are open. **Every one of them waits on the owner, not on work** — each is built, tested
and merge-clean, and each stops at a judgement that wants pictures or a measurement:

| # | Branch | What | Waiting on |
|---|---|---|---|
| 21 | `worktree-tone-curve` | The log-log straight segment above middle grey, built and measured. **Changes no pixels** | Which shoulder order, and what Contrast should mean above grey (see Open questions) |
| 1 | `worktree-portrait-depth` | Apple's depth/matte aux image read from a HEIC, plus the surveyed findings. **Changes no pixels** | Nothing external — see below |
| 2 | `worktree-hdr-gain-map` | The HDR gain map read and carried, unapplied. **Changes no pixels** | An HDR display and a colorimeter: no reconstruction formula is published and the guesses are wrong |

All three were brought up to date with `main` on 2026-10-16 and all three report `MERGEABLE`/`CLEAN`; CI's own
checks (clippy with warnings denied, the workspace tests, the generated-TypeScript gate) were run locally on
each before pushing. #1 and #2 are independent of each other despite sharing `crates/raw/src/heif_aux.rs`
**byte for byte**: they collide only on the two adjacent `pub mod` lines in `crates/raw/src/lib.rs`, so they
can merge in either order, and neither's docs assume the other is present any more.

**What changed under #1 while it sat.** It was parked in favour of the gain map because a depth map's best use
needed a blur renderer nobody had written. That renderer shipped — lens blur, haze by distance, range layers
and the joint-bilateral refinement all run off Depth Anything V2's map, for every photo rather than the ~16%
carrying Apple's. So #1's value is no longer a feature but a **second source** for a finished pipeline:
Apple's disparity is the same quantity the model produces (relative inverse depth, comparable resolution)
measured by stereo instead of guessed from one image. Its roadmap section now says so, with the remaining work
and the open `MASK_REVISION` question written down. Nothing blocks picking it up; it simply needs deciding
whether it is worth a pass, ideally with a Portrait capture blurred both ways side by side.

Held, and genuinely blocked: #2's stage 2 cannot be guessed at without the display measurement above.

*What the merge taught, for the next stack.* GitHub judges a pull request by **ancestry, not content**. When
`main` moved under the stack, #8 reported `CONFLICTING` although `git merge-tree` and a real local merge both
said clean, and it stayed that way across repeated polls - not a cache clearing itself. The cure is to record
the merge: merging `main` into the branch produced a **byte-identical tree** and flipped it to mergeable at
once. So when a stack's base moves, merge it back down through every branch before merging the pull requests,
and check that each merge leaves the tree unchanged - if one does not, that is a real conflict worth reading.

**A fix to a lower branch** goes on that branch, then is merged upward through every branch above it, in
order, and each is pushed (`git merge --no-edit <base>`; new commits only, never rebase or force-push, never
push to `main`). #14 and #6's Dockerfile fix were carried up this way, and so was the clippy fix that turned
the whole stack red: the lint was in #20's code, so it was fixed there and merged into #21 and #22. One cause,
one fix, nine red builds — which is the shape of every mistake in a stack this deep, and the reason to run
CI's own checks before pushing (ARCHITECTURE, "Before pushing, run what CI runs").

## Deploying a test build

The owner's server runs `deploy/local/compose.yaml` (git-ignored, in the main checkout) with the image tag
`sidecar:local`, built with `--target ml --build-arg HEIF=1`. Port 8090 is the real deployment: never stop,
restart or reuse it during development (`./dev/serve` picks 8091 and up). To hand over a test build:

```bash
docker build --target ml --build-arg HEIF=1 -t sidecar:latest-test .     # from the top branch's worktree
# the owner then:
docker tag sidecar:local sidecar:previous && docker tag sidecar:latest-test sidecar:local
cd ~/Claude/sidecar/deploy/local && docker compose up -d
```

`sidecar:latest-test` holds everything through #19, so it predates #20 and everything after it — the depth
edges, the pinned curve endpoints, a stock that the tone controls can grade, and the withdrawal of Double-X.
**Rebuild it before the next hand-over, from `main`:** the stack has landed, and none of the three open pull
requests changes a pixel, so `main` is the whole of what there is to ship.

**To get the colour stocks in a real deployment**, the image needs the owner's two ISO tables mounted and
named, since Sidecar carries neither and will not render a stock without them. In `deploy/local/compose.yaml`,
read-only, outside the repository:

```yaml
    volumes:
      - /path/to/standards:/standards:ro
    environment:
      SIDECAR_STATUS_A: /standards/status-a.csv
      SIDECAR_ISO_DAYLIGHT: /standards/iso-sensitometric-daylight.csv
```

Without them everything else works and the Film section simply offers no stock; the log says which file is
missing. The worker measures each stock's profile once (about three seconds) and caches it under
`cache/stocks`.

## Open questions

- **How dark a slide should be.** PROVIA renders with a slide's own density, because viewing puts the light
  table at the display's white — the chain is faithful and the pictures look like slides, but on a screen it is
  a strong look. **Less pressing since #24:** Amount is no longer the only way down from it, because Contrast,
  Blacks, Whites, Highlights, Shadows and the curve now grade the stock (identity at their defaults, so the
  chain's own colour is still what appears untouched). The question that remains is only whether the *default*
  should be this dark, or the stock's white point should meet the display's. See `testdata/cache/slide/` for
  the tractor NEF rendered neutral, generic film, PROVIA, and PROVIA under tungsten.
- **Sidecar's default rendering is darker than the camera's JPEG.** Visible with the Camera toggle. This is
  ROADMAP's "Tone curve above middle grey", and the candidate curve is now built and measured on #21 — but it
  renders nothing until two things are settled, both with pictures rather than slopes: **which shoulder order**
  (how far the straight segment runs against how hard the top is squeezed; the table is in ROADMAP), and
  **what Contrast should mean above grey**, since it is the logistic's own steepness today and a straight
  segment has none. Then it is a `ProcessVersion` bump, the WGSL mirror, a migration and a parity case.
- **Film defaults.** Colour film's interlayer default is 45 (the owner's choice). Black and white has no
  per-stock tone curve yet.

## What comes next (agreed with the owner)

Film, in three stages:

1. **Black and white** — done in #16: film types (class sensitivities), filters; grain in #17 (colour too).
   The scene's light (daylight, tungsten, from white balance) and developers (adjacency, compensation, staining,
   N±2) in #18. Named stocks from datasheets in #19: HP5 Plus, Tri-X 400 and 320, SFX 200; more are data
   (`crates/color/film-data/README.md` shows how each sheet was read). The owner also supplied colour sheets
   for stage 3: Fujifilm ETERNA 250D, Superia X-TRA 400 and Fujifilm's motion picture film manual.
2. **Alternative processes** — tintype (collodion: the blue-sensitive type plus its short tonal scale) and
   cyanotype simulation; then cyanotype *digital negatives*, calibrated from a printed and scanned step tablet.
3. **Colour film, spectrally** — **done for reversal film in #20**: FUJICHROME PROVIA 100F renders from its
   data sheet end to end (layer sensitivities → characteristic curves → dye densities → the light table),
   replacing the per-channel approximation wherever the stock is named. What is left of the stage: a **print
   stock**, so a colour *negative* can be rendered the way it is actually seen — the owner's ETERNA 250D and
   Superia X-TRA 400 sheets are negatives, and a negative without a print is only an orange mask; then
   **halation**, and **grain per dye layer** instead of the one luminance grain colour uses now. A new
   reversal stock, meanwhile, is data: its sheet traced into `film_data.rs` and an entry in `slide::STOCKS`.

The spectral method is Jakob and Hanika's (`crates/color/src/spectral.rs`); a new stock or filter is data, not
code.

### If you are picking this up and the owner is away

Every open pull request is waiting on them, so don't start by trying to finish one. The film stages above are
the agreed direction, and stage 3's **print stock** is the largest honest piece of it: the owner's two
negative data sheets are in hand, and a negative without a print is only an orange mask, so nothing else
unlocks those sheets. If you want something smaller and self-contained first, ROADMAP has three that need no
decision from anyone:

- **Depth-weighted focus peaking** (ROADMAP, "Depth, beyond what shipped" and "Subject-aware tools"). Every
  piece exists — the focus measure, the cached `depth` and `subject` masks, the view-pass binding — so this is
  weighting one measure by a mask that is already there, and it answers "is the subject sharp?" rather than
  "is anything sharp?".
- **Highlight reconstruction's bluish blotches** at the edges of reconstructed areas (ROADMAP, first section).
  A real, visible defect with a reproducible test case, as against a feature nobody has asked for.
- **Rec.2020 SDR export**, which ROADMAP calls close: the 16-bit TIFF path exists and wants the matrix and an
  ICC profile. Note this is a *gamut*, not HDR — the HDR half is #2's, and blocked.

## Working conventions that aren't obvious from the code

- **Build and test only in the dev container** (`./dev/run …`). Each worktree has its own target directory
  and server state; sccache shares the compiled dependencies.
- **Verify UI changes in a browser** before calling them done: Playwright scripts in `web/scripts/`
  (`editor-smoke.mjs` at wide, `--narrow` and `--fallback`), screenshots looked at. Headless Chromium gets
  WebGPU with the flags those scripts pass.
- **Parity** (`testdata/cache/parity.sh`-style: fixtures → native → browser → compare) whenever pixels change;
  add a case for a new stage.
- **Library photos** are the owner's: read them, copy them under `testdata/cache/library/` with a *new* file
  name, never write under `/mnt/storage/`.
- **Licensing** rules are in `CLAUDE.md`; every new third-party source goes in `NOTICE` and
  `THIRD_PARTY_NOTICES.md`.
