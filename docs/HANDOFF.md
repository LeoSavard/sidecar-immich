# Handoff — where the work stands

For whoever picks this up next, person or agent. [`ARCHITECTURE.md`](ARCHITECTURE.md) says where the code is,
[`DESIGN.md`](DESIGN.md) why it is that way (its build log records what each feature does and how it was
checked), [`ROADMAP.md`](ROADMAP.md) what's known to be missing. This page is the state of play; update it when
you stop.

*Last updated 2026-10-15.*

## Open pull requests and the order to merge them

All are drafts on GitHub (`LeoSavard/sidecar-immich`). From #8 up they are *stacked*: each branch is built on
the one before, and its PR's base is that branch, not `main`. Merge in this order:

| # | Branch | What |
|---|---|---|
| 14 | `worktree-xmp-defaults` | Sidecar and history: a field left out at its default is removed, not kept. **First.** |
| 6 | `worktree-dev-loop` | Fast dev profile, per-worktree builds, `dev/serve`, `dev/bootstrap`, ARCHITECTURE, Dockerfile fix |
| 4 | `worktree-global-saturation` | Global Saturation slider |
| 3 | `worktree-curve-highlights-shadows` | Highlights/Shadows as curve bumps (ProcessVersion 2) |
| 5 | `worktree-look-clipboard` | Copy and paste a look |
| 7 | `worktree-source-sha1` | Notice when a sidecar was saved for a different file |
| 8 | `worktree-edit-types` | TypeScript edit types generated from Rust (merges 6, 4, 3, 5, 7, 14) |
| 9 | `worktree-editor-split` | Editor page split into section components and a session module |
| 10 | `worktree-film-rendering` | Film rendering (colour): per-channel curves, interlayer effects |
| 11 | `worktree-wb-eyedropper` | White balance eyedropper |
| 12 | `worktree-hide-tools` | Hide the editing tools; sRGB/P3 in the editor |
| 13 | `worktree-depth-masks` | Depth Anything V2 Small, depth masks, mask range layers |
| 15 | `worktree-depth-effects` | Lens blur and haze by distance; mask lock fix (slow proxies); Saturation labels; tool groups; mask edges; camera rendering beside the edit |
| 16 | `worktree-bw-film` | Black and white from estimated spectra: film types and filters |
| 17 | `worktree-film-grain` | Film grain: procedural, in exposure, sized on 35 mm; browser = server |
| 18 | `worktree-film-developer` | Black and white: the scene's light, developers and development (adjacency map) |
| 19 | `worktree-film-hp5` | Named stocks from datasheets: HP5 Plus, Tri-X 400 and 320, SFX 200 (spectral sensitivity, toe and shoulder) |
| 20 | `worktree-film-provia` | Colour film, spectrally: FUJICHROME PROVIA 100F from its data sheet, end to end |

Held: #1 `worktree-portrait-depth` (Apple Portrait matte) and #2 `worktree-hdr-gain-map` (HDR, paused until
the owner's Spyder X display measurements).

**A fix to a lower branch** goes on that branch, then is merged upward through every branch above it, in
order, and each is pushed (`git merge --no-edit <base>`; new commits only, never rebase or force-push, never
push to `main`). #14 and #6's Dockerfile fix were carried up this way.

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

`sidecar:latest-test` currently holds everything through #19, so it predates #20 and the withdrawal of
Double-X: rebuild it from the top of the stack before the next hand-over.

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
  ROADMAP's "Tone curve above middle grey"; decide with pictures, against the camera rendering.
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
