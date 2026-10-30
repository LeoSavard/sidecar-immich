# Handoff — where the work stands

For whoever picks this up next, person or agent. [`ARCHITECTURE.md`](ARCHITECTURE.md) says where the code is,
[`DESIGN.md`](DESIGN.md) why it is that way (its build log records what each feature does and how it was
checked), [`ROADMAP.md`](ROADMAP.md) what's known to be missing. This page is the state of play; update it when
you stop.

*Last updated 2026-10-27.*

## Where it stands, 2026-10-22

Two features were asked for and built that day (the feature pause was the owner's to lift, and they lifted
it), each on its own branch off `main`, neither depending on the other. **#38 is merged; #39 is this
branch.**

| # | What | Checked by |
|---|---|---|
| 38 | **Measure** (DESIGN.md §8): a fold under the histogram giving what the rendering holds — at white, at black, and a probed place in 0…255, in display-linear luminance and in stops from middle grey — with a column per side of the flip, so two renderings are read side by side instead of judged by eye. *Copy* writes the table as text. | `web/scripts/measure-smoke.mjs <url> <photo>` |
| 39 | **The editing folder** (DESIGN.md §3): a root with no `immich_prefix`, which Immich can neither resolve into nor be told about. `POST /api/intake?to=folder` writes an imported photo there, filed by its capture date through the root's template, and the home page offers the choice only where both destinations exist. | unit tests, plus an end-to-end import against a server configured with one |

Neither changes a pixel — no shader, no uniform, no packing — and parity passed on both, which is what says
so. Both added a build-log entry at the head of `DESIGN.md`, so the second to land conflicted there; the
resolution was to keep both entries, which is what that conflict always means. Worth expecting from any two
branches written the same day: the build log is append-at-the-top, so it collides by construction.

`deploy/sidecar.dev.toml` gained an editing folder at `testdata/cache/editing`, so `./dev/serve` exercises
the folder path out of the box (it has no Immich, so that is the only destination it offers).

**Remote branches, 2026-10-27:** most of the 37 merged ones are gone. Four merged ones are left
(`highlight-reach`, `local-intake`, `measure-readout`, `standalone-folder`), plus `claude/pensive-edison-xnk4wl`,
#43's branch, closed for #42's version of the same fix. Kept on purpose: `worktree-depth-spike` (the
Depth Anything spike) and `library-name-search` (#28's code and tests, closed under the Immich-stays-the-library
decision).

## 2026-10-26: a review of the decode pipeline

The whole day's session — both pull requests, the owner's decisions, what is left and how to check it again — is
[`research/session-2026-10-26.md`](research/session-2026-10-26.md). The rest of this section is the detail as it
happened.

[`research/pipeline-review-2026-10.md`](research/pipeline-review-2026-10.md) reads the pipeline from file to
proxy. It covers where a decode can go wrong without an error (the shim's black pattern and DNG calibration, X-Trans
preview vs export, colour spaces of developed files), why the top of the tone range drew ten PRs, duplication worth
folding, and a staged, licence-clean plan for decoding on the phone. Branch `claude/sweet-maxwell-s67it6` carries
its only two code changes, neither of which changes a pixel for a file that opened before: no colour matrix
no longer panics (`camera.rs`), and the mosaic copy uses `raw_pitch` (`shim.cpp`, built and its tests passing, though
not yet run on a RAW file or in the dev container). Everything else in it is a proposal waiting on the owner and on test files.

Later the same day, on the same branch: **the share target keeps a photo nothing matched** and the pick page
offers to import it; the review gained **a path to reconcile the highlights** (six links, an owner and a state
each, and a fixed highlight set to measure them on) and **what the file already says** (fields LibRaw parses
that the pipeline estimates instead — among them `BaselineExposure`, which the LibRaw path drops); and **the
mobile module** started ([`MOBILE.md`](MOBILE.md)): `mobile/`, served at `/m` by `-mobile` images, developing
a JPEG or HEIC on the phone's GPU with the editor's own code. It has run against a real server in headless
Chromium, never on a phone: the next step is an iPhone and an Android phone, with the three questions at the
end of MOBILE.md.

**Stage 2 followed the same day: a RAW decoded and developed on the phone** (MOBILE.md), matching the server's
proxy for the same file to half-float rounding. Its loose ends: bin-first proxies on both sides (the 26 MP phone
limit comes from ingest memory), zlib/libjpeg in the wasm build, Emscripten in the dev container, and a real phone.
It also turned up **D14** in the review: highlight reconstruction makes a truncated RAW a quarter NaN, on the
server as on the phone — a server bug, fixed next (below).

**Then the whole highlight stack was reviewed before D14 was fixed** (the owner suspected they were linked; they
were), and the fix went with it, on branch `claude/sweet-maxwell-s67it6` again, from `main` after #42. The
review's §3, "The highlight stack, end to end", walks every stage from `normalize` to the bleach. The step that
made the NaN was the pyramid's rescale, which also decided every reconstructed pixel's colour; measured against
ground truth with a new tool (`sidecar-dev hilite-truth`: a frame clipped on purpose, compared with itself
unclipped) the old pass was worse than no reconstruction in the partly clipped band. `hilite_assemble` and
`hilite_doub` are now a bounded fill that keeps measured samples (`INGEST_GENERATION` 4, so every proxy rebuilds
once); it beats both the old pass and plain clipping on three frames at two exposures, the damaged file has no
NaN, and the phone still matches the server. **Still worth doing:** a look at real blown frames (`_D5N3075.NEF`,
`DSC01899.ARW`) against generation 3, since a synthetic clip is honest for the transition band but a sun has no
truth to compare with.

**Then two more asks, same branch:** **the phone runs the one editor** — `/m` imports into the editing folder and
opens `/edit/<key>`, which develops the phone's own copy on the phone and says so; the module's small editor is
gone (MOBILE.md) — and **Apple's ProRAW white balance** (`AnalogBalance`, the review's D2), which read 2,786 K and
tint −158 on a daylight frame and now reads 6,880 K +32. The owner has since run the editor on an iPhone and
confirmed ProRAW's white balance fixed.

**And then the interface:** the page no longer zooms itself on a phone, the camera's rendering follows the edit's
zoom and position (and drives it), and the loupe reaches 150 % and 200 %. A wider look at the interface, with
seventeen suggestions in order of how often they are met, is
[`research/ui-review-2026-10.md`](research/ui-review-2026-10.md). The owner answered nine of them and those are
built (the table at its top); **on 2026-10-27 they answered the rest**, and six more are built on this branch: the
hide-tools button's panel glyph in place of the ⓘ (1), a zoom readout in the photo's corner that fits on a desktop
and only reads the number on a phone (2), a **Loupe** toggle beside Focus in both quick-access rows (8), the
"developed on this device" note only in the installed app (11), a pipette instead of a pencil for the white-balance
picker (15), and "Reset to as shot" in the normal text colour while it can do something (16). Item 17 is left as it
is. **Item 7 is the one still open:** the owner thought the phone header's truncation might have been an artefact of
the cloud session, and it was not — it reproduces on the current local build at 390 × 844, where `IMG_1768.CR2`
truncates mid-extension and the four buttons take 230 of the 390 px. On an iPhone (2026-10-27) the owner confirmed the page-zoom guard,
the panel handle and the camera's rendering following the edit, and found two faults, fixed since #44 merged on
`claude/sweet-maxwell-s67it6` and not yet in a pull request: hiding the tools left the chosen panel height as an
empty grey panel, and "Bring it up to date now" left Highlights' reach behind, so the offer came back (the session
record's §10).

**Before those, on the owner's word, three more on the same branch** (DESIGN.md's build log has each):
- **D11:** a camera's Adobe RGB JPEG, marked only in EXIF, decodes in Adobe RGB on the server and on the phone
  (`INGEST_GENERATION` 5, `META_REVISION` 2, so cached metadata and proxies are read again once).
- **Highlight recovery is gone as a control** (2026-10-30): the pyramid prevents the magenta cast the blend
  was covering, so the slider answered a problem the shipped app no longer has. The field and the shader path
  stay, so an edit that stored 0.5 renders as it did — no `PROCESS_VERSION` bump, no migration.
- **A half-float view canvas** where the display reports more than 8 bits a channel, with the dither scaled to
  a 10-bit step; `?canvas=8` / `?canvas=16` compares the two. It can't be checked headless (Chromium can't present
  a WebGPU canvas there). On the owner's iPhone it has made no difference they have noticed, which an already
  dithered 8-bit canvas makes likely; `?canvas=8` against `?canvas=16` on a smooth gradient is the comparison.

**Stage 1 of the mobile module followed** (a RAW's embedded preview on the phone; MOBILE.md), with real RAW
files for the first time in this work: CR2s and NEFs from rawpy's test set, under `testdata/cache/samples/`
(not committed). They also ran the shim's `raw_pitch` change through the GPU ingest, so that caveat is gone.

Found in passing, not fixed: **an import into the editing folder pre-builds its proxy from the staging path**.
The worker's ingest endpoint starts a background proxy build, and `place` ingests the staged copy before
renaming it into the folder, so that build races the rename. It won in testing; when it loses, the build fails
with a warning and the editor's own request decodes it again — a wasted decode, never a wrong picture.
`place` could ask for metadata without the pre-build.

Found in passing and fixed on the same branch: **an import into the editing folder kept the staging name in its
metadata** (`meta.fileName` read `138b8c5d1f1ed5a3-IMG_EDITOR.jpg` while the file was `IMG_EDITOR.jpg`), because
`place` ingests the staged file before moving it and the worker's cached metadata kept the first path's name. The
name now follows the path a photo was last read from (DESIGN.md's build log).

## Open pull requests

**Everything up to #32 is merged.** `main` holds the whole first iteration — colour film from a data sheet,
the depth pipeline, Apple's depth/matte aux image (#1) and the HDR gain map read and carried, unapplied (#2) —
and, on 2026-10-20: the highlight-reconstruction measurement (#26), **rustfmt at 100 columns with a CI gate**
(#29), `V2.md` and the desktop survey (#27, #30), and two fixes reported from the editor (#31, #32).

**New features are paused** from 2026-10-16 (owner's call). The work is maintainability and standardization;
[`V2.md`](V2.md) is the programme. **Immich stays the library manager** (owner's call, 2026-10-20), so what
was "widen past a single Immich deployment" is now one item: a configuration with no Immich in it, which is
what a filesystem *editor* needs. The filesystem flavour itself stays, for use cases that are not the owner's —
an editor, never a second library. #28 (`find_by_name`, searching a folder library) was closed under that
decision; its branch keeps the code and its six tests.

**As of 2026-10-27 there are two open pull requests, and they are split on one rule:** what is merged and
deployed is what has been tested. **This branch (#45)** carries the two fixes after #44, the six interface items
from the owner's answers, the decode review's stage 1 (D4, D12 and `device.lost` recovery), the histogram that
defines where a channel saturates (D6/D7, built and not yet wired in) and `sidecar-dev levels`. It changes no
pixel any cache holds, so `INGEST_GENERATION` stays at 5 and the deployment that follows reuses every cached
file. **#46 holds what is written but not demonstrated:** D5, the X-Trans highlight gate, waiting on the
owner's re-evaluation of highlight reconstruction itself; and D1, the black-level repeat pattern mapped per CFA
position, which nothing in the library can exercise because every camera model here writes a uniform black in
the raw IFD. Both were taken out of this line on 2026-10-27 so #45 could be merged and deployed clean.
#21 (the straight segment above middle grey, order 3) merged on 2026-10-22 and #25 (this page) on 2026-10-20;
#43, a first take on an imported photo's staging name, was closed for #42's version of the same fix.

**The formatter is a gate now.** `cargo fmt --all --check` runs in CI beside clippy and `rustfmt.toml` fixes
`max_width = 100`. A branch open when #29 landed pays for it once: `git merge main`, keep the branch's own
side on any Rust conflict, `cargo fmt --all`, commit. #21 crossed it that way and the merge itself was clean.
The reflow's SHA is in `.git-blame-ignore-revs` — `git config blame.ignoreRevsFile .git-blame-ignore-revs`
makes local blame step over it, and GitHub already does.

*What the reformat cost, for the next time one is proposed.* #29 sat open three days and had to be merged down
**twice in one hour** — once for #31's export work, once for #26's measurement tool — because every Rust
branch that lands while a reformat is open collides with it. The resolution is mechanical on a formatting-only
branch (take `main`'s version of the conflicted file, re-run `cargo fmt --all`, and it is `rustfmt(main)` by
construction), but it is paid per landing. A formatting change is cheap the day it is written and compounds
daily after.

**The branches were tidied on 2026-10-20.** 24 merged local branches and the 12 worktrees for merged work are
gone; `.claude/worktrees` went from 5.0 GB to 673 MB. What is left is `main`, the open branches, and two kept
on purpose: `worktree-depth-spike` (one commit, the Depth Anything V2 / burn-onnx spike, also on GitHub) and
`worktree-lens-correction` (local only, 41 commits from September, superseded by `main` but the one branch
whose history `main` does not contain). The matching remote branches are deleted separately, by hand.

**What changed under #1 while it sat.** It was parked in favour of the gain map because a depth map's best use
needed a blur renderer nobody had written. That renderer shipped — lens blur, haze by distance, range layers
and the joint-bilateral refinement all run off Depth Anything V2's map, for every photo rather than the ~16%
carrying Apple's. So #1's value was never a feature but a **second source** for a finished pipeline: Apple's
disparity is the same quantity the model produces (relative inverse depth, comparable resolution) measured by
stereo instead of guessed from one image. Its roadmap section says so, with the remaining work and the open
`MASK_REVISION` question written down. Nothing blocks picking it up; it needs deciding whether it is worth a
pass, ideally with a Portrait capture blurred both ways side by side.

Held, and genuinely blocked: **#2's stage 2** — carrying the gain map through an edit — cannot be guessed at.
No reconstruction formula is published, the reverse-engineered ones call themselves wrong, and the answer
needs an HDR display and a colorimeter. `dev/hdr-gainmap-probe` is ready for whenever that can be done.

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
edges, the pinned curve endpoints, a stock that the tone controls can grade, the withdrawal of Double-X, and
the export fixes of 2026-10-20 (a version is the edit; its files say in their names where they differ; an
Immich link copied from the People tab opens). **Rebuild it before the next hand-over, from `main`:** the
stack has landed and neither open pull request changes a pixel, so `main` is the whole of what there is to
ship.

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

**Features are paused**, so don't start one. [`V2.md`](V2.md) is the agreed direction and says what order to
work in; the queue below it in ROADMAP keeps its places but is not what is being worked through.

The live work is **Workstream A**. A1 and A2 — rustfmt and its CI gate — are done; what is left is:

1. **The three silent traps**, the maintainability target because no test catches them: a
   `shaders/client/*.wgsl` missing from `WEB_ENTRIES`, a wasm request struct without
   `#[serde(rename_all = "camelCase")]`, and `npm run check` passing against stale wasm and stale WGSL.
2. **The generated uniform mirrors** — fifteen `#[repr(C)]` structs hand-mirrored in WGSL. The naga check is
   good and stays; the hand-mirroring is the part worth deleting.
3. **A5: replace `immich.enabled()`** — a stringly-typed "is the URL non-empty" test read in a dozen places —
   with the presence of an adapter. It is what stops the next feature re-introducing the coupling by accident,
   and it is worth doing whether or not a second flavour ever ships.

**Workstream B is one item**, after 2026-10-20: a configuration with no `[immich]` section and a `Root`
without `immich_prefix`, because that is what a filesystem editor needs and Workstream C cannot start without
it. The rest of the seam turned out to be already answered — `OpenQuery` takes a plain path, a single-user
desktop app has no accounts, `Destination::Device` already writes locally. The survey's wider finding, that
the coupling is three concerns rather than one, is kept in [`V2.md`](V2.md) for whoever asks the bigger
question later.

If something smaller and self-contained is wanted instead, ROADMAP's **Rec.2020 SDR export** needs no
decision from anyone: the 16-bit TIFF path exists and wants the matrix and an ICC profile. It is a *gamut*,
not HDR — the HDR half is #2's, and blocked.

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
  `THIRD_PARTY_NOTICES.md`. A correction from the desktop survey, since it is the shape of mistake to expect:
  Real-ESRGAN's *code* is BSD-3-Clause but its distributed **weight files carry no licence statement at all**,
  which grants nothing. Check the weights separately from the code, every time.
- **`cargo fmt --all` before pushing**, now that CI gates it, with the rest of ARCHITECTURE's "Before pushing,
  run what CI runs".
