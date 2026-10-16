# Handoff — where the work stands

For whoever picks this up next, person or agent. [`ARCHITECTURE.md`](ARCHITECTURE.md) says where the code is,
[`DESIGN.md`](DESIGN.md) why it is that way (its build log records what each feature does and how it was
checked), [`ROADMAP.md`](ROADMAP.md) what's known to be missing. This page is the state of play; update it when
you stop.

*Last updated 2026-10-15.*

## Open pull requests

**The stack has landed.** #3-#20 are merged: `main` now holds everything up to and including colour film from
a data sheet (PROVIA 100F), and CI is green on each merge. What is left open:

| # | Branch | What |
|---|---|---|
| 21 | `worktree-tone-curve` | The log-log straight segment above middle grey, built and measured. **Changes no pixels**: it waits on two decisions (see Open questions) |
| 22 | `worktree-depth-edges` | The depth map upsampled against the photo, so a blur's edge follows the subject; and the roadmap corrected where focus peaking and AF points were already done |

Both were branched off #20 and have had `main` merged back into them, so either can go first; neither depends
on the other. Build the next image from #22 - it is the one with pixels in it.

*What the merge taught, for the next stack.* GitHub judges a pull request by **ancestry, not content**. When
`main` moved under the stack, #8 reported `CONFLICTING` although `git merge-tree` and a real local merge both
said clean, and it stayed that way across repeated polls - not a cache clearing itself. The cure is to record
the merge: merging `main` into the branch produced a **byte-identical tree** and flipped it to mergeable at
once. So when a stack's base moves, merge it back down through every branch before merging the pull requests,
and check that each merge leaves the tree unchanged - if one does not, that is a real conflict worth reading.

Held: #1 `worktree-portrait-depth` (Apple Portrait matte) and #2 `worktree-hdr-gain-map` (HDR, paused until
the owner's Spyder X display measurements).

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

`sidecar:latest-test` holds everything through #19, so it predates #20, #22 and the withdrawal of Double-X.
**Rebuild it before the next hand-over**, from whichever branch holds everything you mean to ship —
`worktree-depth-edges` is the furthest along (it contains #20 and the depth work; `worktree-tone-curve` is its
sibling and changes no pixels yet). Once the stack has landed, build from `main` instead.

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
  a strong look, and Amount is the only way down from it. Worth deciding with pictures: keep it as it is, or
  let the stock's white point meet the display's. See `testdata/cache/slide/` for the tractor NEF rendered
  neutral, generic film, PROVIA, and PROVIA under tungsten.
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
