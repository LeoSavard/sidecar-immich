# Sidecar — system design

*Sidecar* is a working name. Status: draft 2, 2026-09-14, with a build log kept current (last entry 2026-09-16); Immich API details checked against the source of Immich 3.1.0, the pinned version. Revised for iPhone as the primary platform, an external library with a strict date layout, versioned exports with the JPEG leading the stack, and crop / lens correction / local masks after V1.
Source: the architecture plan (PWA entry, XMP sidecars, server-side demosaic, WebGPU preview, SSE export telemetry, Carbon/shadcn/HIG interface references).

## Implementation notes (build log)

Where the code differs from the text below, the code and these notes win.

- **The mobile module, stage 0** (docs/MOBILE.md): an optional SvelteKit app in `mobile/`, served at `/m` by
  images built with the new `mobile` and `ml-mobile` targets (`-mobile` tags), and only where
  `server.mobile_root` holds a build. A developed photo chosen on the phone is decoded by the browser into the
  proxy shape the server sends (linear RGBA16F, Display P3 where the canvas allows), developed by the editor's
  own `DevelopPass`/`ViewPass`/packer and sections, saved as a JPEG on the phone, or sent to the server with the
  phone's edit as its first version; a RAW goes to the server the way a share does. Its `$lib` is the main
  app's (`files.lib`), so there is no second pipeline; for it, `SourceSpace` moved into `sidecar-color` with a
  wasm export (`sc_source_color`), and the TypeScript sRGB curve, half floats and extension lists became one
  module each (the extension list checked against Rust by a test, after the picker and share target had drifted
  to 10 and 11 of 21). Checked: `mobile/scripts/mobile-smoke.mjs` against a real `sidecar dev` (no GPU on the
  worker, Chromium on SwiftShader) — the photo drawn, Exposure +1 EV brightening it 164 → 207 of 255, the
  shared Colour controls, a saved JPEG, and an import whose asset key is the photo's SHA-1 with the phone's
  edit stored on it; and the phone's render at default settings against the original JPEG, **mean 0.5 / 255
  per channel, p99 2 / 255** (re-encoding and dither). SwiftShader loses the device when it presents a WebGPU
  canvas headless, so under `--swiftshader` the script measures through the page's own readback instead of
  the screen; the module has not yet been on a phone.

- **A shared photo nothing matched can be imported** (§3): the Android share target hashed the file, opened
  `/open?sha1=…` and dropped the bytes, so a photo the server couldn't see — not backed up to Immich yet, or
  any photo in a deployment with no Immich — ended on an empty pick page with no way forward but finding the
  file some other way. The service worker now keeps the shared file in Cache Storage under its checksum
  (`web/src/lib/shared.ts`: the last three shares, none older than a day), the pick record carries the
  checksum `/open` was given, and the pick page offers **Import this photo** — first when nothing matched,
  under the matches when a name matched, since a name alone may be a different photo — through the same
  `POST /api/intake` the home page uses (`web/src/lib/intake.ts`), with the same choice of destination and the
  same consent before a full editing folder gives a photo up. The file leaves the phone only on that tap, and
  is let go once imported or found. Checked: two api tests (only a real checksum is carried; a pick saved
  before the field still reads) and a Playwright run of a real multipart POST to `/share` through the built
  service worker against a stand-in api, for both the no-match and the name-match case — the upload carried
  the shared bytes unchanged and the phone's copy was gone afterwards.

- **Highlights could not reach a highlight** (§7): reported from the editor — bringing Exposure down
  recovered a blown sky, so the data was plainly there, but Highlights at −100 left it exactly as it was.
  It was structural. The tone map sends everything at or above `white_ev` to display white before the LUT
  sees it, and the LUT's ends are pinned, so a control that only bends the curve below white can reach
  nothing already at it. Measured on a frame with a blown window: **15.4 % of it at white, and Highlights
  −100 changed that by 0.0** — while Whites −100 and Exposure −2 each took it to 0 %, because they act on
  where white is. The same photo under the old rendering clipped **0.03 %**: clipping begins at
  `white_ev − baseline_ev` scene stops above grey, and the baseline (1.15) with the lower white point (0.87)
  moved that from +4.50 to +2.48 stops, so the flaw had been there all along and only now had 2.02 stops of
  picture to show it with. Negative Highlights now adds up to `highlight_reach_ev` (2 stops, `PROCESS_VERSION`
  7) to the white point the curve is built on. Checked: four unit tests (the white point moves with the
  slider and only downward from 0; the scene level that clips; an older edit's white point does not move; a
  developed file's tone parameters do not move at all), and `web/scripts/highlight-recovery-smoke.mjs` on the
  real render — **15.4 % → 7.7 % → 0 %** across Highlights 0, −50, −100, with an edit from before the bump
  unchanged. The uniform did not change size; this is all in `pack`, which the browser and the server share.
  *Still open:* whether `DEFAULT_WHITE_EV` should move back up is a question for a survey, not for two
  photographs — see the roadmap.

- **An old rendering no longer spreads** (§4): two consequences of stored references that nobody chose.
  *The original is not an edit* — a history begun under an older version stored its own defaults in the
  root, and migrating them meant "open the original" handed back a rendering no one had ever asked for, on
  the one node that stands for the absence of an edit; `version_of` now returns today's defaults for it
  outright, while `edit_of` still rebuilds the stored root as written so that versions built on it do not
  move. *And an old rendering used to reproduce itself*: the references are inherited by whatever is edited
  from them, so a version saved today out of a version-2 sidecar came out marked 6 and rendering by 2's
  rules — measured directly, `baselineEv 0.0 | whiteZeroEv 4.5 | shoulderOrder 0.0` on a node created on the
  spot. So editing an old edit now brings it up to date first, said plainly while the photo is still
  untouched, with the way back offered for the session afterwards. This does not let the version-specific
  code go — a ported edit still has ancestors to render, and reading them still needs `migrate` — but it
  makes the set of edits on old references closed and shrinking instead of self-renewing, which is the
  precondition for ever removing any of it. Checked: two history tests (the original is today's rendering
  however old the history; an old root is still unchanged for the versions built on it) and
  `web/scripts/rendering-update-smoke.mjs`, which now sets the photo to an old rendering itself rather than
  leaning on a fixture's history — the standing notice, one ordinary change carrying the port (**+1.21 stops
  measured at the probe**, against 1.15 expected plus the triggering change's own small effect), the way
  back, that a further change then leaves it where they asked, and that a later session asks again.

- **An old edit can take up today's rendering** (§4): `migrate` keeps an old edit's picture by pinning the
  references the pipeline has moved on from, which is right — the person approved *that* picture — but it
  left no way back: a photo edited before `PROCESS_VERSION` 3 rendered 1.15 stops below where the
  measurement puts its middle grey, for as long as it existed, and checking out the original did not escape
  it (the two `the_original_of_…` tests say why). `adopt_current_rendering` is the other direction. It moves
  only the references, keeps every setting, and keeps Whites reading what it read by carrying its offset to
  the new white point; a developed file takes up the white point alone, since it has no baseline, shoulder
  or bleach of its own. The editor offers it where the other notices about a photo are, since the History
  section is not somewhere anyone would look for it, and taking it is an ordinary edit — the picture moves
  at once, Undo takes it back, saving files a version. Checked: four unit tests, including that packing the
  result lifts the exposure by exactly `BASELINE_EV`; and `web/scripts/rendering-update-smoke.mjs` in
  Chromium on a fixture whose sidecar really does predate the baseline, which reads the change off the
  Measure probe rather than describing it — **−0.37 stops from grey before, +1.17 stops after, against the
  1.15 expected** — then Undo back, Redo forward, and a save that leaves 18 versions as 19 with Exposure
  still at the +0.30 EV the person set.

- **The home page shows photographs, not grey boxes** (§3, §11): four changes to what it lists. The imported
  photos are their own row — **one thumbnail row high**, scrolling sideways rather than wrapping, with the
  count beside the heading and size, the exported mark and Remove kept behind *More info*; what is in that
  folder is left out of Recent, so no photograph appears twice. Recent itself keeps **40**
  (`RECENT_KEPT`): it is a way back to what you were working on, not a library view, and every row costs a
  thumbnail. Thumbnails now have a second source — `GET /api/assets/{key}/thumb` renders the photo as the
  edit currently renders it, small, from the cached proxy, so it costs a develop pass over a 480-pixel image
  rather than a decode. Immich's own thumbnail stays the fast path where there is one; Sidecar's answers both
  halves of the gap, a photo Immich has never seen *and* one it holds but will not thumbnail, the iPhone DNGs
  among them, through an `onerror` that falls through to the same place. And the editor has **a way back**:
  until now the only way out on a phone was the browser's own back gesture, which a photo opened from a share
  or a pasted link does not always have. Checked: `web/scripts/folder-limit-smoke.mjs` (25 assertions,
  including that the row is one tile tall at both widths and that every imported photo's thumbnail actually
  decoded), the thumb endpoint returning a 480×321 JPEG, and the back button at phone size — present, clear
  of the tools toggle, and landing on `/`.

- **The editing folder is bounded** (§3): a folder that lives inside the image cannot be allowed to grow
  until the host fills up, so it holds **four** photographs by default (`intake_limit`). What goes when it is
  full is the oldest one that has been *exported* — a render of it exists elsewhere, so the folder is no
  longer the only copy of that work. A photograph nobody has exported is never chosen however old it is, and
  an import that would need one is refused instead: the alternative is an editor that deletes someone's
  negative to make room for the next import. The home page asks before the file picker opens rather than
  after a photo has been uploaded, and names what would go; the server asks again when the bytes arrive, and
  asks it *after* the duplicate check, so re-opening something already in the folder never costs a
  photograph. Removing by hand takes two taps. Checked: `web/scripts/folder-limit-smoke.mjs` in Chromium
  (Playwright's filechooser event is what proves the picker did and did not open) — twenty assertions over
  both branches of the message; and against the API, four imports fill it, the fifth is refused `422` with
  nothing deleted and the staging folder clean, exporting one makes it `nextOut`, the next import takes
  exactly that one and logs it, a delete prunes the template's empty directories back to the root, and an
  unknown key is a `404`. Unit tests for the eviction order, for an unexported photo never being chosen, and
  for the default of four.

- **Sidecar without Immich, as a deployment rather than a possibility** (§3): the editing folder made a
  no-Immich configuration expressible; this makes it something you can run. `/photos` is created in the image
  and owned by the user Sidecar runs as, because Docker takes a fresh named volume's ownership from the
  image's directory at that path — a path the image does not have becomes root-owned, and the uid-1000
  process cannot write to it, which would have failed every import on a permission error. Verified with a
  two-line image: the chowned path writes, a root-owned one is refused. `deploy/standalone/` is the worked
  example — one root, no `immich_prefix`, no `[immich]` url, the folder a named volume, bound to loopback.
  Checked end to end on that exact config: session reports no Immich and no sign-in, an import lands at
  `2019/01.January/2019-01-02/IMG_1768.CR2`, an edit saves its XMP beside the original, `/open?path=` reopens
  it while a path outside the root is refused, and an export comes back as a 6024×4022 JPEG with
  `immichAssetId: null` — the upload is skipped and the file is always offered as a download. What is still
  missing is a way to *browse* the folder: import, Recent and a known path are the only ways in.

- **A photo can be imported without putting it in Immich** (§3): `POST /api/intake` only ever uploaded to the
  library, because an edit needs an original in a root and the library was the only kind of root there was. A
  root declared with no `immich_prefix` is now an *editing folder* — Immich can neither resolve into it nor
  be told about it — and `?to=folder` writes the photo straight there. The capture date still decides where
  it sits, since that is what makes a folder navigable, so the bytes go to a hidden `.incoming/` name inside
  the root, get ingested to read `capturedUnix`, and are renamed into the template's folder for that date;
  the same photo imported twice is one file, a different photo under a taken name gets `-1`. The home page
  offers the choice only where both exist, and where there is no Immich it no longer talks about sharing from
  Immich or pasting Immich links. Checked: against a server with an editing folder and no Immich — an import
  lands at `2019/01.January/2019-01-02/Imported_1768.CR2` (the file's own `DateTimeOriginal`, confirmed with
  exiftool, not the folder the fixture had been hand-placed in), importing it again returns the same asset
  key and leaves one file, a different photo of the same name becomes `-1`, nothing is left in `.incoming`,
  and an edit saved on the import writes its XMP beside the original in the editing folder; unit tests for
  the naming rule, for a prefix-less root being invisible to `mount_path` and `immich_path`, and for
  `Date::from_unix`, which is new in `sidecar-template`.

- **The numbers behind a rendering** (§8): the histogram's corner triangles already knew how much of the
  picture was at white, but only as a tooltip on whichever rendering was on screen, which is no way to compare
  two of them. A **Measure** panel under the histogram now states it — at white, at black, and, once a place
  is probed, what that place holds in 0…255, in display-linear luminance and in stops from middle grey. The
  probe is kept in the picture's own frame, so a crop, a turn and the flip all read the same spot, and each
  side of the flip keeps its own reading: flipping to the compared version and back leaves **both columns on
  screen together**. *Copy* puts the table on the clipboard as text, which is how a measurement leaves the
  editor and reaches a bug report or a conversation. Nothing new is computed for the clipping figures — they
  are the counts the histogram pass already reads back — and the probe is one small `copyTextureToBuffer` on
  developedTex, queued at the histogram's own gate so it reads the picture as it then stood and costs nothing
  per frame. Checked: `web/scripts/measure-smoke.mjs` in Chromium — the probe reads a triplet and a stop
  figure, lifting the exposure raises both the probe and "at white", the flip brings a named second column
  that differs at the same place, and *Copy* yields the whole table. The first run of it also explained
  itself: the fixture's stored edit is an *old* one (`baselineEv 0`, `whiteEv 4.5`, `shoulderOrder 0`, kept
  renderable by the stored references of §7), so "As shot" — which uses today's defaults — sits 1.15 EV
  brighter and clips 14.5 % of the frame against the edit's 1.0 %. That difference was always there; this is
  the first time it could be read off rather than guessed at.

- **A version is the edit, wherever its files go** (§9): exporting to this device and then to Immich used to
  answer "already exported" and upload nothing — the device-only record matched on settings and was read as
  proof the file was in the library. An entry now counts as *in Immich* only when it names an asset there, and
  an export that finds its own record without one renders again under that version and uploads it, filling the
  entry in instead of making a second one. What tells two renders apart is the edit and the settings, which is
  also what the file name says: `-2048px`, `-p3`, `-q80` as they differ from a full-size sRGB JPEG at 92, so
  one version's files sit side by side instead of overwriting each other (quality is dropped for a TIFF, which
  has none, before it can split one file in two). Checked: the sheet driven in Chromium against a stub Immich —
  to the device, then *Also save to Immich* (one upload, one entry, `immichAssetId` filled in, the render time
  kept), then the same request again (answered from the record, nothing rendered or uploaded), then quality 80
  (a second file, `IMG_1768-v1-q80.jpg`, still version 1); unit tests for the version rule, the naming and the
  "in the library" test, and a sidecar round trip for recording one render twice.

- **Colour film, spectrally** (§7, "Colour film, spectrally"): FUJICHROME PROVIA 100F renders through its own
  data sheet — layer sensitivities, characteristic curves, dyes, light table — in the browser and on the
  server, from two tables the develop pass reads (`develop/slide.wgsl`). The licensed half of the chain is
  measured on the server from the operator's own copies of ISO 5-3 and ISO 7589 and served as a per-stock
  profile; a deployment without those files offers no stock. Checked: the pass agrees with `Slide::render` on
  the CPU within 0.06 stop over greys and saturated colours, and at amount 0 renders bit-identically to no
  stock; the uniform grew to 416 bytes, verified against the WGSL by naga; parity cases `slide-provia` and
  `slide-provia-mixed`. The stock replaces the neutral tone map; what it renders is then graded by the tone
  controls as a developed file is (`slide_tone`), the identity at their defaults. `Film.interlayer` is not
  part of a sheet's chain. *The first arrangement left those controls reaching the picture only through the
  neutral rendering, so at a full Amount they moved and did nothing — reported from the editor, and fixed.*
- **Named stocks** (§7, "Stocks"): ILFORD HP5 Plus and SFX 200, KODAK TRI-X 400 and 320, from
  the datasheets the owner supplied — spectral sensitivity and the characteristic curve's toe and shoulder.
  EASTMAN DOUBLE-X was traced and then dropped: its sheet's spectral curve stops at 420 nm and the owner found
  sources describing a strong response below that, so the held-flat trace would misreport the film. A named
  stock stands on its own sheet; where the sheet runs out, the stock is not offered.
  HP5's spectrogram was first read divided by its tungsten light; SFX 200's filter factors showed that wrong
  (×9 against the sheet's ×2.8 behind red) and it is now read as drawn. Checked: every fit within 0.015 density
  of its trace; Tri-X 400's speed point on ISO 400's (−2.701 against −2.699 log lux·s); each stock's red filter
  factor within 0.3 stop of its sheet; packing tests; parity `stock-hp5` and `stock-sfx-red` (21/21 pass);
  the six renderings of the tractor and the portrait, and SFX 200 against generic panchromatic behind red. In
  the editor a stock is a choice under its film type; the edit still names the stock.

- **Developers** (§7, "Developers"): adjacency through a low-resolution map of the whole frame, development N±2,
  compensation, grain per developer and a staining developer's grain masking. Checked: packing tests; an engine
  test of an edge (bright side 0.588 → 0.634, dark side 0.138 → 0.107, display-linear) and of a loupe tile given
  the whole frame's map (within 0.001); parity `dev-acutance`, `dev-staining-layers` and `dev-compensating`
  (19/19 pass); the tractor in each developer and at N±1, whole and in 100 % crops; the three editor smoke runs
  choose a developer, tungsten and N+1 and save them. Choosing the rendering already chosen no longer resets its
  settings.

- **Black and white under other light** (§7, "The scene's light"): daylight, tungsten, or the white balance's
  temperature. Checked: illuminants against colour-science's A and D50; tungsten's effect on a red and a sky;
  old edits without `light` read as daylight; the tractor in both film types under each light.

- **Grain** (§7, "Grain"): procedural, in exposure before the tone map, sized on a 35 mm frame and averaged by
  Selwyn's law where finer than a pixel. Checked: packing tests; parity `grain-mono`, `grain-film`, `grain-heic`
  identical browser to server (16/16 pass); a portrait developed at 1600 px and 5584 px with grain 0/40/80
  (midtones grainy, deep shadows and the bright fleece less so; the proxy softer, as expected); the three editor
  smoke runs set Amount 30 and Size 40 and save them. `sidecar-dev develop` now passes black and white's table and
  demosaics at full size when asked for more than half the sensor.

- **Immich checksums:** Immich hashes file contents only for its internal library. External-library assets get `sha1("path:" + originalPath)` as their checksum, so checksum search can't find them. Resolution therefore relies on the date layout and filename search for external RAWs, confirms candidates by hashing the local files, and finds the Immich asset by `originalPath`.
- **Immich is 3.2.0** on the server (floating `v3` tag). The API behaviour relied on is unchanged from 3.1.0. `GET /api/users/me` needs `user.read`, so the sign-in key needs `user.read, asset.read, asset.view, asset.upload, asset.update, stack.create`.
- **Uniform layouts** are mirrored by hand in `#[repr(C)]` Rust structs and checked against the WGSL by parsing it with naga in tests. `encase` 0.12 dropped glam support and `webgpu-utils` isn't needed: the browser packs uniforms with the same Rust code compiled to WebAssembly (`crates/color-wasm`).
- **Vulkan in containers** also needs `libegl1`, `libxext6` and `libx11-6` in the image; NVIDIA's ICD fails to initialise without libEGL.
- **Proxies (milestone 2)** use LibRaw's half-size mode (2×2 binning, no interpolation): 0.3–0.5 s for a 45 MP NEF, ~2 s for a compressed RAF. The GPU demosaic of milestone 5 replaces this.
- **XMP field names** follow the JSON parameters in PascalCase inside `sc:Sidecar/sc:Edit` (`sc:Ev`, `sc:TemperatureK`, …). The namespace URI is `https://sidecar.ls-tech.ca/ns/1.0/` and must never change.
- **Library root:** the external library's template starts at `/mnt/storage/Photography/Library`.
- **Lightroom-merged panorama DNGs** in JPEG XL, which LibRaw 0.22 can't decode, are read by `sidecar_raw::linear_dng` (see below).
- **GPU ingest (milestone 5):** vkdt's highlight pyramid and RCD are ported to WGSL; RCD is split into five full-texture passes instead of vkdt's shared-memory tiles. X-Trans proxies use 3×3 binning on the GPU; full-resolution X-Trans (export, loupe) still uses LibRaw's Markesteijn on the CPU — a WGSL port of LibRaw's code is the remaining step. LibRaw is built with OpenMP (a Fuji RAF opens in 0.2 s instead of 1.7 s).
- **Export (milestone 6):** JPEG through `jpeg-encoder` (pure Rust, MIT/Apache + IJG) rather than mozjpeg. The worker streams NDJSON progress to the api, which relays it as SSE with `Last-Event-ID` replay. The outbox is reached through worker endpoints, so the api keeps `/cache` read-only.
- **Detail (milestone 7):** denoise and sharpening are implemented from the published methods (à-trous wavelets with square-root variance stabilization and a per-image noise estimate; Richardson–Lucy on luminance), not ported — vkdt's denoise depends on per-camera noise profiles. The worker keeps the last two full-resolution buffers for loupe tiles.
- **Internal library** roots can set `per_user = true`: Immich keeps a folder per user (storage label or id) before the template.
- **Entry points (milestone 8):** Android share target hashes in the service worker and opens `/open?sha1=…` (keeping the file, so a photo nothing matches can be imported from the pick page); desktop pastes an Immich link, resolved through `GET /api/assets/{id}`.
- **Development:** `./dev/serve` runs api + worker on the first free port from `127.0.0.1:8091`, the real library read-only, and a writable test copy under `testdata/cache/library`. It builds the `dev-serve` cargo profile (dependencies optimized, Sidecar's crates at `opt-level = 1`, incremental): touching `sidecar-color` rebuilds in 2 s instead of 22 s. Each git worktree has its own target directory, server state and container; sccache shares compiled dependencies between them. `--web` adds Vite's dev server for live reload. See `docs/ARCHITECTURE.md`.
- **First live run (2026-09-15):** share → resolve → develop → sidecar → export → upload → stack, against the real external library. The Immich *admin* key is scoped without `asset.read`, so stacks can't be checked from the CLI — only in the Immich app.
- **Output space is a user setting** (§11), not a guess from `matchMedia`. It drives the develop uniform, the canvas, the loupe request and the export default together; the export sheet still overrides it for one export. The editor previewing in P3 while exports defaulted to sRGB was a real trap: saturated reds fit on screen and clipped in the file.
- **Tint direction:** the slider names what happens to the *image*, as every editor does. Positive tint moves the assumed illuminant toward green so the photo goes magenta. It shipped inverted and is fixed; `positive_tint_moves_the_illuminant_toward_green` pins it.
- **Where middle grey and white sit** (`PROCESS_VERSION` 3 and 4): `normalize.wgsl` puts sensor saturation at 1.0 linear, and the curve read middle grey at 0.18 of that and white at `0.18 × 2^4.5`. Both were wrong, and measurably: against the cameras' own JPEGs over **196 files, 11 bodies, 14 lenses, four brands**, a correctly exposed capture puts middle grey **1.15 stops lower** (so Sidecar rendered the camera's grey at −1.31 EV), and saturation sits **3.63 stops** over that grey, not 4.5 — so **no render ever reached display white**: 0 of 196 frames held a pixel there, and a blown window came out light grey. `Exposure::baseline_ev` (1.15) and `DEFAULT_WHITE_EV` (3.63) fix them; re-rendering all 196 puts grey at −0.01 EV and the brightest pixel on a clipping frame at 0.965 linear against 0.689. One constant for every camera deliberately — the per-body spread is barely larger than the spread within a body, and does not follow the exposure program. Both references are **stored per edit** (`baseline_ev`, `tone.white_zero_ev`) so an edit made before each bump renders exactly as it did and its sliders read where they were left.
- **What a blown highlight does** (`PROCESS_VERSION` 5): the output transform clips at display white instead of rolling the top tenth off with a tanh shoulder — that shoulder never reached white (a neutral blown pixel came out at 252 of 255, and a channel had to reach 1.2 to encode 255), and clipping per channel bleaches a bright colour toward white as film does, one channel reaching the ceiling before the next. Above that, **colour fades to neutral over the last stop** (`Tone::highlight_bleach`, from 0.5 display-linear, complete at white): a detector's receptors saturate one after another so a bright colour loses its cast before it gets there, where `tone_apply`'s luminance-ratio scale holds chromaticity to the ceiling and leaves a coloured band beside anything that reached white. Measured: a blown window goes from no white at all to 12.2% pure white, its bright-but-coloured pixels 34.7% → 23.4%, and **midtone saturation unchanged on every frame tested** — only the top stop moves. A coverage-driven control for neutralising fully clipped pixels was built first and withdrawn: once the white point is right those pixels render at white, where the bleach neutralises them anyway. An edit from before the bump keeps the highlights it was approved with.
- **The curve above middle grey** (`PROCESS_VERSION` 6): a straight segment in log-log — a stop of scene light for a stop of picture — meeting a shoulder `1 − (1 − t)ⁿ` that reaches white at `whiteEv` with no slope left over. Without a kink the knee is fixed at `(nH − W)/(n − 1)`, so the order is the whole choice, and it is **3** (`Tone::shoulder_order`), chosen from pictures. Against the logistic it replaces: a stop in is a stop out to +1.5 EV where the logistic held 0.74, paid for above +2.5 (0.42 against 0.50); on rendered frames local contrast in that band is 8–17% higher. Shadows are the logistic's throughout and are bit-identical. A fixed segment has no steepness for Contrast to set, so Contrast grades it as it grades a developed file, a power S-curve through encoded grey — the identity at 1, so grey stays where the segment puts it. An edit from before the bump keeps the logistic above grey. The uniform carries the shape in a new `tone_shape` lane and so grows to 432 bytes, verified against the WGSL by naga.
- **Lens correction (§15), from the file first:** a survey of this library found who writes what. Sony ARW
  (ZV-1, ILCE-7RM4) carries distortion, CA and vignetting tables; Fujifilm RAF (X100F) carries CA and
  vignetting; Lightroom's DNGs carry `WarpRectilinear`/`FixVignetteRadial` opcodes; Nikon DSLRs, Canon, Olympus
  and Panasonic carry only on/off flags or unverified tables, and phones only `GainMap` shading. Each reading
  was checked against a second source rather than assumed — Sony distortion and CA against Lightroom's opcodes
  for the same exposures (within a pixel) and against the camera's own JPEG (`sidecar-dev camera-jpeg`: 36 of
  36 frames align better through the table), Fujifilm vignetting against Lightroom's to 0.3 %. Sony's
  vignetting table is left unused: the ZV-1 writes shading-corrected sensor data (its JPEG adds 0.00 EV at the
  corners where the table asks for 0.3–0.8), so applying it would correct twice. Everything converts into
  one radial model (`sidecar_color::lens`), sampled into 64 knots for a separate lens pass
  (`shaders/client/lens.wgsl`, bicubic reads) that runs before develop — in the browser only when a lens
  setting changes, on the server after denoise and sharpening, with loupe tiles cropping only the source the
  tile reads. A zoom about the optical centre keeps blank edges out of the frame. ExifTool reads the tags in
  the same run as the AF point; `META_REVISION` re-reads cached metadata without rebuilding proxies.
- **lensfun for the rest:** the api fetches the database files from lensfun's repository on first start and
  monthly (`[lensfun]` in `sidecar.toml`) and hands them to the worker, which has no internet route; cached
  metadata is re-matched when a newer database arrives. Lenses are matched on ExifTool's `LensID` among
  lenses that fit the body's mount, requiring the same focal range and maximum aperture. The crop factor is
  lensfun's for the body, not ExifTool's: a ZV-1 writes a rounded "24 mm" equivalent, and that 6 % showed up
  as the profile's radius being off against the camera's own JPEG. Only sensor mosaics get lensfun: a linear
  DNG a program made (Lightroom's merges) was corrected by it already.
- **JPEG XL linear DNGs:** Lightroom's panoramas since 2023 store linear RGB as JPEG XL tiles, which LibRaw
  0.22 can't open; `sidecar_raw::linear_dng` reads the DNG structure and decodes them with `jxl-oxide`, a row
  of tiles at a time in parallel, averaged down to the size asked for (a 40 MP merge: 2.7 s). The tiles are
  tagged BT.2100 with the sRGB curve and must be rendered with a linear transfer to get the DNG's values back.
  Every other Lightroom-made DNG in the library (deflate float HDR merges, lossy-JPEG panoramas, Enhanced
  CFA) already decodes through LibRaw. Exports of merges wider than the GPU's texture limit are rendered at
  that limit.
- **Machine learning (`sidecar-ml`, in progress):** its own program, not part of the worker, so a
  machine without the GPU for it never loads it and GPU memory comes back when it exits — burn only releases
  memory with its process. The worker will start it on demand with one end of a Unix socket pair as its
  stdin (framed JSON plus raw pixels, documented in `crates/ml/src/main.rs`); it exits after five idle
  minutes. burn on Vulkan (or Metal) through wgpu: no CUDA or other vendor runtime. f16 where the GPU has it
  (BiRefNet 1.0 s instead of 1.9 s on the A4000; 3.9 GB of GPU memory instead of 13), with LayerNorm weights
  kept f32. The model code in `crates/ml/src/models/` is generated by burn-onnx and committed, so builds need
  no Python. The weights ship only in the `-ml` image (`--target ml`): the Dockerfile's `ml-onnx` stages
  fetch them pinned by SHA-256 and convert two, and `dev/ml-models` uses the same stages to regenerate the
  code. Weights are found by the names the generated code gives them, so code and weights must come from the
  same conversion: the conversions are made reproducible (tf2onnx's constant names canonicalised, burn's
  random parameter ids stripped from the weight files) and pinned by hash, and the image compiles sidecar-ml
  after importing. burn is a pre-release, pinned exactly. Tuned kernels and the driver's shader cache must live on a
  volume (`--cache`): without them each start recompiles for 30–90 s; with them it is ready in 1–6 s.
- **Masks (§15), the model-made kind:** the worker starts `sidecar-ml` when a photo's masks are first asked
  for (`crates/worker/src/ml.rs`), develops the proxy at default settings — a normal-looking photo, what the
  models were trained on — and caches `subject`, `skin`, `face-skin`, `body-skin`, `hair`, `clothes` and the
  faces beside the photo's proxies, in the proxy's frame (oriented, before lens correction), keyed like the
  focus mask. `GET /api/assets/{key}/masks` lists them; `…/masks/{name}` is one as a PNG. In the browser,
  `$lib/masks` loads one as a texture and the editor lens-corrects it as it does the focus mask; the worker
  un-orients and corrects it to match the frame export, preview and loupe develop in. An adjustment names its
  mask (`EditParams.masks`, at most 8): the develop pass samples each mask and adds `wₖ ×` its exposure (in
  stops, before the tone map), white balance (camera-space gains), contrast and saturation (after the tone
  map). An edit without masks serializes as before, so no ETag, history id or export record moves, and no
  `ProcessVersion` bump was needed: an older Sidecar keeps masks it can't render, and entries a newer one adds
  (a `kind`) are carried and skipped here. The first masks on a new install take minutes while burn tunes
  itself to the GPU; after that a photo's masks take seconds, once.
- **The camera's rendering beside the edit:** the worker extracts the largest JPEG the camera embedded in a RAW
  (`JpgFromRaw`, `PreviewImage` or `OtherImage`, by ExifTool), copies the RAW's orientation tag onto it and
  caches it (`GET /api/assets/{key}/camera`). The editor's Camera toggle splits the view — the canvas keeps the
  left half (top on a phone) so everything drawn over it keeps its place — and is off while cropping or
  painting. Developed files have none: they are their own rendering.
- **Mask edges (§15):** a layer's optional `edge` (width, feather, shift) blurs its finished mask — after the
  range and the strokes — over up to 5 % of the long edge and thresholds it again: the threshold moves the edge
  out or in (0.5 ± 0.45), the ramp around it sets how soft it is (feather 0 keeps it crisp, only moved and
  smoothed). Averaged down so the Gaussian's radius is at most 16 texels, in 16-bit textures so a moved
  threshold doesn't band; render passes (`client/mask_edge.wgsl`), since r8unorm can't be a storage texture.
  The browser's painter and the worker run the same passes from one plan (`MaskEdge::plan`, through wasm).
- **Depth effects (§15): lens blur and haze by distance.** A pass between the lens pass and develop, on
  camera-native linear light, so a blurred highlight spreads as the light it is and haze is light in the scene
  before white balance sees it (`shaders/depth/effects.wgsl` and three `client/depth_*.wgsl` entries, the same
  WGSL in the browser and the worker; uniform packed by `sidecar_color::depth::pack`, 64 bytes). *Haze*:
  transmittance `t = exp(−τ·(1 − nearness))`, τ up to 3 at ±100; adding mixes toward the airlight, removing
  inverts that with t floored at 0.2. The airlight is the mean camera RGB of the farthest 3 % of the photo,
  estimated by the worker with the depth mask (`depth.json`, listed with the masks; `MASK_REVISION` 3).
  *Lens blur*: the circle of confusion is linear in the difference in nearness from the focus (Depth Anything
  gives inverse depth, which a thin lens's blur follows), minus half the depth of field, up to 2.5 % of the long
  edge. It is gathered at a scale where the largest disc is ≤ 16 texels (downsample with the haze, a
  160-sample golden-angle gather, then compose at full resolution), so a wide blur costs the same at any size.
  A sample counts if its own disc reaches the texel; a farther one only as far as the texel's own disc, so a
  sharp subject doesn't take in the blurred background behind it, while a nearer blurred one spreads over what
  is behind it (its share is kept in alpha and lets compose blur a sharp pixel it covers). Samples weigh by the
  inverse of their disc's area, so a bright point becomes a bright disc. A loupe tile is developed with the
  blur's reach of margin, its corner on the gather grid, and matches the whole (engine test). Tap to focus
  reads the depth mask under the tap. `EditParams.lensBlur` (amount, focus, range) and `haze` are left out when
  off. Measured on the tractor photo's server previews (edge energy, higher is sharper): focus near → fence,
  cab and trees 2.3, 2.5, 2.6; focus on the cab → 3.4, 10.8, 3.8; unblurred 5.3, 11.1, 13.8.
- **The depth map's edges are the photo's** (§7, "Depth"): the model's map is 518², so stretching it to the
  frame leaves a step smeared over a couple of its texels and a blur's edge drifts off the subject into a halo
  — visible around hair and thin branches. `client/depth_refine.wgsl` upsamples it against the photo instead
  (joint bilateral upsampling, Kopf et al. 2007, from the paper): 25 taps a texel apart, each weighted by
  distance and by how closely the photo's luminance there matches the photo's here, so depth steps where the
  photo steps and stays smooth where it does not. It depends on the photo and the map alone, not on any
  slider, so both engines render it once and keep it; it is capped at 2,048 on the long edge, four times the
  model's own. One pass takes most of an edge's error out rather than all of it — the texel being repaired is
  itself a tap — which is what the engine test holds it to, along with leaving flat regions and fine texture
  alone. Checked in the browser with `web/scripts/depth-edges.mjs`.
- **Depth masks (§15):** Depth Anything V2 *Small* (Apache-2.0; Base and Large are CC-BY-NC and excluded),
  simplified at 518² like BiRefNet and imported by burn-onnx, gives relative inverse depth: no unit, no fixed
  origin. `Ml::depth` stretches it between the photo's own 1st and 99th percentiles to 0–255, near white, and
  the worker caches it as `depth` with the other masks (`MASK_REVISION` 2, so cached sets are made again once
  with it). f16 stays within 2.4 % of f32's span at worst, 0.2 % on average, and runs in about 100 ms on the
  A4000. On its own the mask is a far-to-near gradient; a layer on it usually carries a `range`
  (`MaskAdjustment.range`: from, to, feather, 0–100 of the mask's value), which the brush composite applies
  when it seeds the layer from the model's mask (`in_range` in `client/brush_composite.wgsl`, the same pass
  in the browser and on export), so strokes still add to or take from the band it selects, and the develop
  pass is unchanged. A new depth layer starts as the nearer half (50–100, feather 15). A mirror's reflection
  sits at the mirror's distance, where Apple's Portrait matte counts it as subject.
- **Crop and straighten (§15):** `sidecar_color::geometry` turns `EditParams.geometry` — quarter turns, a
  mirror, straighten, Vertical and Horizontal keystone (a virtual camera tilt of up to 30° with a focal length of
  0.8 picture diagonals, Aspect for the stretch that leaves), and a crop — into one projective map from the
  output to the lens-corrected picture. It is applied *after* develop, which is per pixel: the editor's view
  pass samples the developed texture through it (and the histogram counts the output's pixels through it); the
  worker's warp pass (`shaders/server/warp.wgsl`, bicubic) resamples the developed texture for export, the
  fallback preview and loupe tiles, with the pending orientation folded into the same matrix. So nothing
  upstream of develop changed and no parity case moved. The crop is stored in the straightened plane, in
  fractions of the picture, and is fitted by the same Rust code on every change (through wasm in the
  browser): a crop nobody drew is the largest of the photo's shape (or the locked ratio) the picture fills,
  and follows the straighten back and forth; a drawn one keeps its place and only shrinks as it must. The
  loupe's centre is in the output when the edit has geometry. Absent `geometry` means none and isn't
  serialized, so no ETag moved; a 0.4 app ignores the field (shows the photo uncropped) and its merge-on-save
  keeps it.
- **Mask layers (§15):** a layer now reaches every light and colour control: Whites and Blacks blend the
  tone map's parameters toward the layer's at its weight, the colour mixer's values add, and a layer curve
  (`curve`; Highlights and Shadows are its 25 % and 75 % points, as globally) is applied after the global
  curve from a row of an 8 × 4,096 `r32float` texture — `MaskParams` grew to 1,568 bytes. The develop
  shader's tone stage is split (`tone_*_curve_in` → curves → `tone_apply`) so the curves sit between;
  results without layers are bit-identical. New fields are left out at their defaults, so a layer using
  only 0.4's fields is stored as 0.4 stored it and still renders there; one using new fields is carried and
  skipped by 0.4. Two parity cases cover layers on a RAW and on a developed file.
- **Brushes (§15):** `strokes` on a layer (radius as a fraction of the long edge, feather, flow, points),
  over a model's mask or on a `brush` layer that starts empty. Points are stored in the photo before lens
  correction — the models' frame — so a stroke stays on what it covered whatever the lens, crop or
  straighten later do. Rasterized by `client/brush.wgsl` (each segment a capsule, blended `max` into a
  coverage target, so one stroke lays its flow down once) and `client/brush_composite.wgsl` (paint
  `b + (1 − b)c`, erase `b(1 − c)`), into `r8unorm` at the proxy's size in the browser and up to 4,096 px on
  the worker, then moved by `client/lens_mask.wgsl` — the lens pass as a render pass for one-channel targets.
  While a stroke is painted the editor doesn't develop the photo per move: at its first point it develops
  the photo twice — without the layer, and with it at full weight everywhere (an empty mask, inverted) — and
  each frame draws only what the finger added since the last, lens-corrects only that area of the mask, and
  blends the two renders by it there (`client/mask_preview.wgsl`). That is exact where the mask is 0 or 1;
  in a soft edge it blends results rather than parameters, which measured 0.04 levels from the exact render.
  The histogram, whose readback costs a phone frames, waits until the brush is off. On the A4000 the GPU had
  run a median 200 ms behind the finger; now it keeps up. When the finger lifts, the editor stores the stroke simplified (Ramer–Douglas–Peucker at a tenth of
  the radius) and rounded to 1e-4, and repaints from what was stored, so the editor shows what export renders.
- **Interaction:** slider rows allow vertical panning and claim a touch only after 4 px of sideways travel, so the tool panel scrolls. Holding a slider no longer hides the editor chrome — the histogram is exactly what you want to watch while dragging.

## 0. Summary

Three services beside an existing Immich install, shipped as one Apache-2.0 Docker image on GitHub Container Registry:

- **sidecar-web** — SvelteKit web app. Opens from an iOS Shortcut in Safari (primary) or the Android share sheet, renders a 16-bit linear proxy through a WGSL develop pipeline in WebGPU, computes the histogram on the GPU.
- **sidecar-api** — Rust (axum). Resolves a shared photo to its original on disk, reads and writes XMP sidecars (the only process that can write into the libraries), schedules jobs, streams export progress over SSE, uploads versions to Immich and stacks them.
- **sidecar-worker** — Rust. LibRaw decode plus wgpu on Vulkan on the RTX card: ingest (demosaic, highlight reconstruction, proxy), 1:1 loupe tiles, full-resolution export. Mounts the libraries read-only.

One rule holds the design together: **every develop module is written once in WGSL** and runs both in the browser (WebGPU — Metal underneath on iPhone) and on the server (wgpu → Vulkan). Preview and export are the same code, so what the user approved on the phone is what gets rendered.

## 1. Where the plan changes

Everything the plan selected stays. These points either can't work as written or leave a hole.

| Kind | Plan says | Problem | Design | § |
|---|---|---|---|---|
| Breaks | Client `POST`s; backend answers with `text/event-stream`; `EventSource` reconnects | `EventSource` only issues `GET` — it can't send a body, so it can't open or resume a `POST` stream. | `POST /api/exports` creates a job (`202`). `GET /api/exports/{id}/events` streams it. The job lives server-side; reconnects replay via `Last-Event-ID`. | 9 |
| Breaks | RAW directory `:ro`, "parallel overlay" `:rw`, `.xmp` written next to the RAW | A file can't be created beside a RAW through a read-only mount; an overlay's upper directory puts the `.xmp` where Immich and darktable never look. | Split privilege by process: the worker (which parses RAW files) mounts `:ro`; the api mounts the same host paths `:rw` and its writer can only create `*.xmp` beside an existing original, atomically. Filesystem snapshots are the backstop. | 2, 4 |
| Breaks | Web Share Target is the entry vector | No support on iOS or iPadOS — the primary platform. Android only (plus ChromeOS). A share also delivers file bytes, not a path or an ID. | On iPhone, a Shortcut in the share sheet sends the file's hash, name and capture date to Safari; the api finds the original through the library's date layout. Android keeps the share target; desktop pastes an Immich link. Nothing is uploaded. | 3 |
| Breaks | Proxy as "16-bit half-float OpenEXR or PNG" | PNG has no half-float type; browsers decode 16-bit PNG to 8 bits and can't decode EXR natively. | Raw little-endian RGBA16F bytes with HTTP compression, handed to `writeTexture` into an `rgba16float` texture unchanged. | 5 |
| Breaks | Highlight recovery kicks in when exposure pushes values above 1.0 | In a float pipeline ×2^EV loses nothing; clipping happened at the sensor. Clamping at 1.0 before tone mapping throws away the range that recovery needs. | No clamp in linear stages. Reconstruction runs on the server against the sensor saturation mask; per-pixel clip coverage ships in the proxy's alpha channel for a cheap client-side blend. | 5, 7 |
| Breaks | Client WebGPU shaders; server "headless compute shaders" | Two implementations of the same math drift; the export stops matching the preview. | WGSL single source, wgpu on the server, parity checks in CI and on the iPhone itself. | 7, 12 |
| Refine | AMaZE on the GPU via LibRaw | LibRaw demosaics on the CPU only; AMaZE has no production GPU implementation (darktable runs it on CPU). | RCD on the GPU for Bayer (close to AMaZE, GPU-friendly); Markesteijn 3-pass on the GPU for X-Trans. Port from vkdt's GLSL compute shaders (BSD-2-Clause) and LibRaw's X-Trans code (LGPL-2.1 or CDDL-1.0): both sit closer to WGSL than darktable's OpenCL and fit Sidecar's Apache-2.0 image (§12). AMaZE exists only as GPL-3.0 code and is left out. | 5, 12 |
| Refine | Static camera → Rec.2020 matrix; temperature-only WB | DNG dual-illuminant matrices are interpolated by white balance. Temperature alone can't neutralize green/magenta casts. | CPU recomputes the matrix on every WB change; add a Tint control. | 7 |
| Refine | HSL with piecewise-linear bands | HSL isn't perceptual (lightness moves with hue); piecewise bands tear at band edges. | Same 8-band Hue/Saturation/Luminance controls, evaluated in OKLCh with overlapping raised-cosine weights that sum to 1. | 7 |
| Refine | Cubic splines for curves | Natural cubic splines overshoot and can invert tones between control points. | Monotone cubic (Fritsch–Carlson), baked to a 4,096-entry LUT on each edit. | 7 |
| Refine | γ = 2.2 before sRGB / Display P3 | Both spaces use the piecewise sRGB transfer; 2.2 won't match the exported file. | sRGB transfer function in preview and export alike. | 8 |
| Gap | Spatial passes bypassed on the client | At proxy scale (a third to a quarter of sensor width) noise and fine detail are invisible, so Sharpen and Denoise would be set blind. | Server-rendered 1:1 loupe tiles from a full-resolution buffer kept in VRAM for the session. | 5 |
| Gap | Histogram compute evaluates "the pipeline output" | A compute shader can't read back a canvas texture after it's presented. | Develop into an offscreen texture that feeds both the view pass and the histogram. | 7, 8 |

## 2. Topology

```
iPhone: Immich app ──share──▶ Shortcut (hash · name · date) ──Open URL──▶ Safari editor ──HTTPS──▶ Caddy ──▶ sidecar-api ──jobs──▶ sidecar-worker
                                                                                                             │   ▲ lookup/upload/stack   │
                                                                                                             │   └──── immich-server     │
                                                                                               rw, *.xmp only▼                           ▼ ro
                                                                                     external + internal libraries (same date layout)
```

| Container | Runs | Library mounts | Other mounts | Network |
|---|---|---|---|---|
| sidecar-api | HTTP, auth, `/open` and resolve, XMP read/write, job scheduler, SSE, Immich client, static web build | `/library/external`, `/library/internal` rw — writes `*.xmp` only | `/cache` ro, `/state` rw (sessions, recent list, outbox manifests), `sidecar.toml` ro | Caddy, immich-server, worker |
| sidecar-worker | LibRaw + wgpu (Vulkan): ingest, loupe, export | `/library/external`, `/library/internal` ro | `/cache` rw | internal only |
| caddy (existing) | TLS, reverse proxy | — | — | public |
| immich-server (existing) | unchanged — pinned at 3.1.0 | — | — | — |

**The XMP writer** is the only code path that writes into a library:

1. `realpath(target)` resolves under a configured root; no symlink leaves it.
2. The target ends in `.xmp` and its original exists as a regular file.
3. Write `.<name>.xmp.tmp-<random>` with `O_EXCL` in the same directory, `fsync`, `rename()` over the target, `fsync` the directory.
4. The writer exposes no delete, truncate, or chmod.

It still holds write access to directories, so btrfs/ZFS snapshots of both libraries are the real safety net.

**GPU sharing.** RTX A4000 (16 GB) and RTX 4000 Ada (20 GB) both fit; budget for 16 GB, because Immich's ML and transcoding use the same card. The worker enforces a configurable VRAM budget (default 8 GB) and runs two queues: *interactive* (proxy, loupe) and *export*. Exports yield at tile boundaries when interactive work arrives.

## 3. Opening a photo

| Platform | User action | Mechanism |
|---|---|---|
| iPhone / iPad — primary | Share → *Edit in Sidecar*, from the Immich app | Shortcut → Safari `GET /open?…` |
| Android | Share → Sidecar, from the Immich app | Web Share Target → service worker → `/open` |
| Desktop | Paste an Immich photo link | `/photos/<uuid>` → Immich API → checksum → `/open` |
| Any | Choose or drag a file on the device being edited on | `POST /api/intake` → Immich upload → `/edit/{assetKey}` |

### The iOS Shortcut

*Edit in Sidecar* — shown in the share sheet, accepts Images and Files:

1. **Get Details of Images** → Date Taken → **Format Date** (Date Format: ISO 8601, Include ISO 8601 Time on) → `taken`, e.g. `2018-03-27T13:11:29-04:00`
2. **Get Details of Files** → Name → **URL Encode** → `name`
3. **Get Details of Files** → File Size → **Format File Size** (bytes, no units) → `size`
4. **Generate Hash** (SHA1) of the Shortcut input → `sha1`
5. **Open URLs** → `https://edit.example.com/open?sha1=…&name=…&taken=…&size=…`

Use ISO 8601 rather than the Long formats. Long output (`March 27, 2018 at 13:11:29 EDT`) follows the phone's language and region settings — month names, the word "at", 12- or 24-hour clock, and a time-zone abbreviation instead of an offset — so a settings change would silently break parsing. The api still accepts the English Long form as a fallback.

It reads the file the Immich app already downloaded in order to share it; nothing is uploaded. Distributed as an iCloud link with an Import Question for the server address, linked from the editor's `/setup` page, with a copy kept in `ios/`.

**Why Safari and not the Home Screen app.** iOS can't route a URL into a Home Screen web app, so a Shortcut always lands in Safari. Sign in once there. The Home Screen app, if installed, shows *Recent* — the last 50 photos opened from any device — so a photo can be reopened without sharing it again.

### `GET /open`

Cookie-authenticated (a signed-out visitor goes to sign-in and comes back). It resolves the photo, records it in *Recent*, starts ingest in the background so the proxy is usually decoding before the page loads, and answers `303 → /edit/{assetKey}`. Several candidates go to `/pick`, which shows Immich thumbnails; no match goes to `/pick` with search.

The recorded path can stop being true while the record stays valid — Immich moves a file in its internal
library whenever the storage template's inputs change. So a stored path that no longer exists is re-resolved
through Immich by asset id, once, and written back, rather than every route failing until the photo is opened
from Immich again. Reopening from *Recent* is the case this matters for; `/open` always resolves afresh.

### Intake from the editing device

Every route above finds a photo that is already on the server. `POST /api/intake` (multipart, one `file`
field) is for one that isn't: a photo picked on the phone or laptop doing the editing, or dropped on the home
page.

It is not a separate kind of asset. An edit lives in an XMP beside an original, so the original has to become
a file in a library root before there is anywhere to put one — the photo is uploaded to Immich with the
user's own key, and from there it is opened exactly like anything else. Nothing downstream of `/edit/{key}`
knows it arrived this way.

The one wrinkle is timing. `POST /api/assets` returns as soon as the bytes land, but the capture date the
storage template is keyed on comes from metadata extraction, which runs after — so for a moment the asset's
`originalPath` names somewhere Sidecar either can't see or would open only to have the file move out from
under it. Intake polls the asset until its path maps to a file under a configured root, for up to 20 seconds,
and otherwise says the photo is safely in the library but not openable from here yet. An exact duplicate is
already filed and resolves on the first try.

**`?to=folder`: the editing folder.** The library is one destination, not the only one. A root declared with
no `immich_prefix` is a folder Immich knows nothing about — an *editing* folder — and marking one `intake =
true` makes it where imported photos go. `POST /api/intake?to=folder` writes the bytes there itself: no
upload, no key, nothing to tell Immich, and nothing to wait for a library to file.

What a library does for free then has to be done here. Immich settles a photo's path by its capture date
through the storage template; this does the same, with the date read from the file — which only the worker
can do. So the bytes are written to a staging name inside the root (`.incoming/`, hidden, and on the same
filesystem so what follows is a rename rather than a copy), ingested to learn `capturedUnix`, and renamed
into the folder the root's template gives for that date. A file with no `DateTimeOriginal` is filed under the
day it was imported, as a library with nothing else to go on would file it. A failure anywhere after the
write takes the staged file away again.

A name already taken in that folder is stepped past — `IMG_1768-1.CR2`, `-2` — unless the file already there
holds *exactly* these bytes, in which case that is the same photo and its path is what comes back: importing
one twice gives one file and one asset key, as it would in a library. From there it is opened, edited and
sidecar-written exactly like anything else; the XMP lands beside the original in the editing folder.

**The folder is bounded, and that is the point.** A library is a disk someone chose; the editing folder is
usually a volume inside the image, so left alone it grows until the host fills up — one 30 MB original at a
time, for as long as anyone keeps editing. `intake_limit` (default **4**) is what stops that. It is
deliberately small: the folder is a workbench, not a shelf, and the components are all here for anyone who
wants a bigger one.

What goes when the folder is full is **the oldest photograph that has been exported**. A render of it exists
somewhere else, so the folder is no longer the only copy of that work. A photograph nobody has exported is
never chosen, however old — losing it to make room would be the editor deleting someone's negative — and an
import that would need one is **refused**, with the person deciding what to let go of instead.

The question is asked before the file picker opens, not after a photo has been chosen and uploaded: the home
page reads `GET /api/intake/folder` and, when the folder is full, puts a message between the import button
and the picker naming exactly what would go. `make_room` asks the same question again when the bytes arrive,
because a client that skipped the message still meets the rule. A re-import of something already there is a
duplicate and adds nothing, so the check sits *after* the dedupe — being full never costs you a photograph
you were only reopening.

`DELETE /api/intake/folder/{key}` takes one out by hand: the original, both spellings of its sidecar, and the
record it was ever opened, after which the template's now-empty directories are pruned back to the root. This
is the one place Sidecar deletes a photograph, so it is confined to the intake root, keyed by content hash
rather than path — a key is not a path, and nothing outside the folder is nameable through it. In the
interface it takes **two taps**: the first arms the button and says `Remove?`, the second does it, and an
armed button disarms itself after four seconds.

`GET /api/session` reports `intakeFolder` (the root's name, or `null`), so the home page offers the choice
only where there is one. `?to=immich` without an Immich library answers `501`, and so does `?to=folder`
without an editing folder; with no `?to=` at all the destination is the library, which is what it always was.

### Library roots

```toml
# sidecar.toml
[immich]
admin_api_key_file = "/run/secrets/immich_admin_key"   # job.create only; internal-library sidecar discovery (§4)

[[roots]]
name          = "external"
immich_prefix = "/mnt/photos"                     # originalPath prefix inside immich-server
mount         = "/library/external"
template      = "{{y}}/{{MM}}.{{MMMM}}/{{y}}-{{MM}}-{{dd}}/{{filename}}"
month_locale  = "en"                              # 09.September
moves_files   = false                             # Immich never moves external files

[[roots]]
name          = "internal"
immich_prefix = "/data/library/<storage label>"
mount         = "/library/internal"
template      = "{{y}}/{{MM}}.{{MMMM}}/{{y}}-{{MM}}-{{dd}}/{{filename}}"
month_locale  = "en"
moves_files   = true                              # storage template applies

[[roots]]
name          = "Editing folder"                  # shown on the home page
# No immich_prefix: Immich knows nothing about this folder, so no lookup can resolve into it and no file in
# it is ever given an originalPath. It is an editor's folder, not a second library.
mount         = "/library/editing"
template      = "{{y}}/{{MM}}.{{MMMM}}/{{y}}-{{MM}}-{{dd}}/{{filename}}"
month_locale  = "en"
intake        = true                              # photos imported from the device land here
```

A root without `immich_prefix` drops out of `mount_path` and `immich_path`, which is the whole of what makes
it invisible to Immich: the checksum and `originalPath` steps of the cascade below can never name it, and the
template step skips it whenever Immich is configured. Where Immich is *not* configured, its files resolve by
date and name like any other root's — which is the filesystem editor's configuration, a deployment with no
Immich in it at all. At most one root is the intake; the first one marked wins.

### Resolution cascade

sidecar-api tries each in turn and stops at the first hit:

1. **Checksum** (when `sha1` is present) — `POST /api/search/metadata { checksum }` with the requesting user's API key. Immich 3.1.0 accepts the checksum as 40-character hex or 28-character base64, so the Shortcut's hex goes through unchanged → `originalPath` → matching root by prefix → mount path. This also proves the user may open the photo.
2. **Date and name through the template** (when `taken` and `name` are present) — take the calendar date exactly as written in `taken`, in the phone's time zone and never converted to UTC (`13:11:29-04:00` on 27 March stays 27 March). Render each root's template for that date, e.g. `2026/09.September/2026-09-14/`, and look for `DSC_1234.*` and `DSC_1234+N.*` there, then in the previous and next day's folders (captures near midnight, time zones). Among stem matches prefer RAW extensions over a camera JPEG, then an exact name, then matching size, then matching SHA-1. Confirm Immich lists an asset at that path for this user with `POST /api/search/metadata { originalPath }` — a case-insensitive substring match in 3.1.0, so compare the returned path exactly. Works without the hash, and even if a share delivers a converted file, because only the stem has to match.
3. **Filename search** — `POST /api/search/metadata { originalFileName }`, also a substring match, filtered to exact stem matches. One match opens; several go to the picker.
4. **Picker** with search.

Draft 1's local hash index is gone: the strict layout makes step 2 two or three directory listings, with no scan and no dependency on Immich's database.

`assetKey` is the hex SHA-1 of the original's bytes — content-derived, identical to what Immich computes, unchanged by any database rebuild. When resolution succeeded without a hash, the api computes it from the NVMe copy (well under a second for a 60 MB RAW). Immich UUIDs appear only transiently (step 1, desktop links); nothing stores them.

**Confirmed:** the Immich iOS app pulls the original asset from the server before sharing, so the Shortcut receives the RAW's exact bytes under its original name with a readable Date Taken. *Generate Hash* runs from the share sheet on the largest RAW in the library, and its result matches `sha1sum` of the file on the server — so step 1 resolves every share directly, and step 2 only runs when Immich can't answer (unreachable, or mid-rescan after a database rebuild).

**Also confirmed:** folder month names are English (`09.September`), matching `month_locale = "en"`.

**Immich's own edits.** Immich 3.x can crop, rotate and mirror an asset itself. Those edits live in Immich's database and change its thumbnails, not the file. Sidecar develops the untouched original and ignores them; when the RAW's asset reports `isEdited`, the editor says so. If a share ever delivers Immich's edited rendering instead of the original, the hash won't match and step 2 still finds the RAW by its stem.

## 4. Edit state on disk

- **File.** Reuse whichever sidecar exists (`DSC_1234.NEF.xmp`, then `DSC_1234.xmp`); otherwise create `DSC_1234.NEF.xmp` — the name Immich and darktable both look for.
- **How Immich 3.1.0 finds sidecars.** Discovery checks `<original>.xmp`, then `<stem>.xmp`. It runs when a library scan imports or changes an external asset, and for every asset without a known sidecar when an admin starts the Sidecar job's *Missing* run. The `refresh-metadata` asset job only re-reads a sidecar Immich already knows about — it never discovers a new one.
- **External library.** Immich never moves these originals, so a sidecar stays beside its RAW for good, whether or not Immich knows about it. Adding a sidecar doesn't change the RAW, so a library scan won't discover it; only a *Missing* run would.
- **Internal library** (an iPhone ProRAW DNG, say). The storage template moves a sidecar with its original only if Immich knows about it. After creating a sidecar there, the api starts the Sidecar job's *Missing* run with an admin API key holding only `job.create`, read from a Docker secret. Runs are batched to at most one an hour and happen only after an internal-library sidecar was created. In 3.1.0 the only way to start that run is `PUT /api/jobs/sidecar { "command": "start", "force": false }`, which is marked deprecated. The api logs a failure and the setup page shows a warning, so an Immich upgrade that removes it doesn't go unnoticed. Each run queues a check for every asset Immich knows no sidecar for, in both libraries — a couple of file lookups each, but many queue jobs on a large library, hence the batching. It also makes Immich aware of Sidecar's external-library sidecars, which the `sc:Sidecar` wrapper makes harmless.
- **Internal-library integrity report.** In 3.1.0 the untracked-files check walks the internal library and only counts originals and encoded videos as tracked, so `.xmp` files there — Sidecar's and Immich's own — can appear in that report, which offers deletion. Don't bulk-delete from it. The external library isn't scanned.
- **One wrapper element.** Everything Sidecar writes sits inside a single `sc:Sidecar` struct. Once Immich knows about a sidecar, metadata extraction merges every tag read from it over the RAW's own tags, and prefers its dates. Top-level `sc:ModifyDate` or `sc:WhiteBalance` would surface as the standard `ModifyDate` and `WhiteBalance` tags; inside the wrapper, exiftool reports them as `SidecarModified`, `SidecarWhiteBalanceTemperature` and so on, which collide with nothing. Every `sc:` name elsewhere in this document lives inside the wrapper.
- **Namespace.** Sidecar's own (`sc:`). Never Adobe `crs:` — the math differs, and Lightroom would misrender those values.
- **Read–modify–write**, through Adobe's `xmp_toolkit` crate (MIT OR Apache-2.0, built on the BSD-3-Clause XMP Toolkit SDK), which already preserves unknown namespaces. Parse the whole packet, replace only the `sc:` elements Sidecar knows, keep everything else — other namespaces (Immich ratings and descriptions, darktable history) and any `sc:` elements written by a newer Sidecar.
- **Concurrency.** ETag = SHA-256 of the canonicalized edit parameters. It excludes `sc:Exports`, so a finishing export never makes the editor's next save conflict. A save names the history node it was edited from (`base`); when that is no longer the head — another device saved in between — the save forks instead of failing, and the editor says both were kept (see *Edit history*). A bare edit without `base`, from a client that predates history, still needs `If-Match` and gets `412` with the current state.
- **`sc:ProcessVersion`** pins the math. A pipeline change that alters output bumps it; older sidecars keep rendering with the older path until the user updates them. A sidecar with a newer ProcessVersion than the running app opens read-only with "Update Sidecar to edit this photo" — a phone can hold an old cached copy of the app.
- **`sc:SourceSHA1`** ties the sidecar to its original and catches a sidecar left behind by a rename.
- **Immich writes too.** When a description, date, location, rating or tag changes in Immich, it writes those fields into `<original>.xmp` — the same file — through exiftool, which preserves unknown namespaces. Tagging alone creates the file, so many RAWs will already have one. Sidecar's writer re-checks the file's size and modification time just before its rename and starts over if Immich wrote in between. A test edits a rating in Immich and asserts `sc:Sidecar` survives.
- **Drafts.** Slider state autosaves to IndexedDB (per device, 500 ms debounce). It commits to XMP four seconds after a gesture ends (a checkpoint), on *Save*, on *Export*, and on `visibilitychange → hidden` via `fetch(…, { keepalive: true })`. Drafts also cover Safari's edge-swipe back gesture: swiping forward returns to the edit intact.

### Edit history

Every save is a version of the photo's edit, kept as a tree in `sc:Sidecar/sc:History` (`sidecar_xmp::history`),
so it travels with the file, survives a reinstall, moves with it under Immich's storage template, and is the
same on every device. `sc:Edit` stays the full current edit — the history's head — so anything that reads
only it is unaffected.

- **Nodes.** `id` (12 hex), `parent`, `etag` (of its full edit), `created`, `kind` (`original`, `imported`,
  `save`, `auto`), optional `label`, `user` and `session`, and `delta`: the root (`id = original`) holds the
  complete default parameters it was created with; every other node only the fields that differ from its
  parent (objects recurse, arrays and scalars are replaced whole). XMP has no null, so fields a node turns
  off are listed in `removed`. A few hundred bytes per node; five versions of an iPhone HEIC's edit came to
  12 KB of sidecar. Ids, ETags and labels are in the XMP reader's string list — hex like `12e456…` would
  otherwise come back as a float.
- **Saving.** `PUT …/edit { edit, base, checkpoint, session }`. From the head, a save adds a child. From an
  older node, it adds a child there: editing an old version branches and discards nothing. A save identical
  to its base just moves the head there.
- **Checkpoints.** The editor saves four seconds after a gesture ends. A checkpoint folds into the head
  instead of adding a node when the head is this session's own checkpoint, under three minutes old,
  unlabelled, not exported, childless, and the new changes go on adjusting a setting that checkpoint already
  adjusted (Temperature, Tint and as-shot count as one white balance; the lens strengths as one lens
  correction). Whatever else changed in the same pause folds in with it: white balance tweaked three times
  with exposure nudged in between is one step, while exposure then contrast alone is two. The three minutes
  run from the step's first checkpoint, so a step can't grow forever. A fold that leaves the node equal to
  its parent (an undo) removes the step.
- **Other operations.** `GET …/history` returns every node with its full edit and the exports made from it;
  `POST …/history/checkout { node }` makes a version (or the original) current; `PUT …/history/{node}/label`
  names one. Opening another version saves unsaved changes first.
- **Pruning.** Past 300 nodes, the oldest node that is not the original, the head, labelled, exported or a
  fork point is squashed into its only child. Exports record the ETag they were made from, so the version
  behind every export stays reachable.
- **Editor.** A History tool shows the tree, git-style — the current version's ancestry in the first lane,
  each branch in its own — with a plain-language summary of each step ("Exposure +0.30 EV · Whites −12",
  read through the same slider mapping the controls use), export badges, and per version: open, compare
  (press and hold the photo to see it), name, and *use its* white balance, light, curve, colour mixer or
  detail in the current edit. Undo and redo are on screen too; they stay per session and per gesture.

```xml
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:sc="https://sidecar.ls-tech.ca/ns/1.0/"
    xmp:Rating="4">
   <sc:Sidecar rdf:parseType="Resource">
    <sc:ProcessVersion>1</sc:ProcessVersion>
    <sc:SourceSHA1>9b2f0c4e8d1a7f3365b0e2c4d9a8f1e07c6b5a43</sc:SourceSHA1>
    <sc:Modified>2026-09-14T10:02:11Z</sc:Modified>
    <sc:WhiteBalance rdf:parseType="Resource">
     <sc:Mode>custom</sc:Mode>
     <sc:Temperature>5150</sc:Temperature>
     <sc:Tint>6</sc:Tint>
    </sc:WhiteBalance>
    <sc:Exposure rdf:parseType="Resource">
     <sc:EV>0.65</sc:EV>
     <sc:HighlightBlend>0.4</sc:HighlightBlend>
    </sc:Exposure>
    <sc:Tone rdf:parseType="Resource">
     <sc:BlackEV>-10.0</sc:BlackEV>
     <sc:WhiteEV>6.5</sc:WhiteEV>
     <sc:Contrast>1.15</sc:Contrast>
     <sc:Curve>
      <rdf:Seq>
       <rdf:li>0.000, 0.000</rdf:li>
       <rdf:li>0.250, 0.215</rdf:li>
       <rdf:li>0.750, 0.790</rdf:li>
       <rdf:li>1.000, 1.000</rdf:li>
      </rdf:Seq>
     </sc:Curve>
    </sc:Tone>
    <sc:MixerLuminance>
     <rdf:Seq>
      <rdf:li>0</rdf:li><rdf:li>0</rdf:li><rdf:li>0</rdf:li><rdf:li>0</rdf:li>
      <rdf:li>0</rdf:li><rdf:li>-35</rdf:li><rdf:li>0</rdf:li><rdf:li>0</rdf:li>
     </rdf:Seq>
    </sc:MixerLuminance>
    <sc:Exports>
     <rdf:Seq>
      <rdf:li rdf:parseType="Resource">
       <sc:Version>1</sc:Version>
       <sc:EditETag>41c07be2</sc:EditETag>
       <sc:File>DSC_1234-v1.jpg</sc:File>
       <sc:Exported>2026-09-12T21:40:03Z</sc:Exported>
       <sc:ImmichAssetId>…</sc:ImmichAssetId>
      </rdf:li>
      <rdf:li rdf:parseType="Resource">
       <sc:Version>2</sc:Version>
       <sc:EditETag>3b9e0c71</sc:EditETag>
       <sc:File>DSC_1234-v2.jpg</sc:File>
       <sc:Exported>2026-09-14T10:03:40Z</sc:Exported>
       <sc:ImmichAssetId>…</sc:ImmichAssetId>
      </rdf:li>
     </rdf:Seq>
    </sc:Exports>
   </sc:Sidecar>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
```

(`sc:MixerHue` and `sc:MixerSaturation` follow the same 8-entry order: red, orange, yellow, green, aqua, blue, purple, magenta. ETags are shortened here. The namespace URI, `https://sidecar.ls-tech.ca/ns/1.0/`, must never change.)

## 5. Ingest and proxy (sidecar-worker)

On first open, cached by `assetKey + ProcessVersion + IngestGeneration`:

1. **Open** — LibRaw `open_file` + `unpack`. Read `black`/`cblack`, `maximum`, `cam_mul`, `pre_mul`, `rgb_cam`, DNG ColorMatrix1/2 with illuminants, CFA `filters`/`xtrans`, `flip`, ISO, lens.
2. **Upload CFA** to the GPU as `r16uint`.
3. **Normalize** — subtract per-site black level, divide by (white − black).
4. **Hot and dead pixels** — replace a pixel with the median of same-colour neighbours when it deviates by more than k·σ. (LibRaw only maps bad pixels from a supplied list.)
5. **Saturation mask** — per CFA site, raw ≥ 0.99 × white level.
6. **Pre-WB** — multiply by as-shot `cam_mul`; demosaic algorithms assume roughly neutral input.
7. **Demosaic** — RCD for Bayer, ported from vkdt's compute shaders; Markesteijn 3-pass for X-Trans, ported from LibRaw. Both WGSL compute (§12).
8. **Highlight reconstruction** — rebuild clipped channels from unclipped neighbours using the mask (multi-scale inpainting, after vkdt's `hilite` module), at as-shot WB.
9. **Undo pre-WB** — divide `cam_mul` back out, leaving camera-native linear RGB — what the client's WB module expects.
10. **Orientation** — apply EXIF `flip`.
11. **Session buffer** — keep the full-resolution result in VRAM (45 MP × RGBA16F ≈ 360 MB) for loupe tiles; LRU-evicted against the VRAM budget.
12. **Downsample** — area filter in linear light to the requested long edge; alpha = fraction of clipped source pixels under each output pixel.
13. **Pack** — RGBA16F little-endian into `/cache`, stored pre-compressed as brotli (Safari) and zstd (Chromium) so nothing is compressed per request.

Wire format:

```
GET /api/assets/{key}/proxy?long_edge=2560
Accept-Encoding: gzip, deflate, br

200 OK
Content-Type: application/x-sidecar-rgba16f
Content-Encoding: br
X-Width: 2560
X-Height: 1707
X-Process-Version: 1

body: width × height × 8 bytes, row-major, top-left origin, orientation applied
```

The client calls `queue.writeTexture({ texture }, bytes, { bytesPerRow: width * 8 }, [width, height])` — no decode, no conversion. As-shot WB, matrices, levels and EXIF come from `GET /api/assets/{key}/meta`.

**Sizing.** `long_edge = min(maxViewportEdge × devicePixelRatio × 1.25, device.limits.maxTextureDimension2D, classCap)` with `classCap` = 2,560 on iPhone and 4,096 on iPad and desktop. The texture limit is a ceiling, not a target: one 4,096² RGBA16F texture is 128 MB, and the pipeline holds about two.

**Progressive.** A 768 px proxy arrives first (≈ 3 MB before compression); the full proxy swaps in when ready.

**Cache generation.** `ProcessVersion` versions the develop parameters in a sidecar, and raising it marks
every sidecar read-only to an older Sidecar — so it cannot also stand for what the pixels were decoded by.
`INGEST_GENERATION` does that, and is bumped by any change to what ingest produces from the same original:
demosaic, hot pixels, highlight reconstruction, orientation, the downsample. Both are in the file name.
Without the second one, 0.2.1's fix for a HEIC being rotated 180° twice never reached a photo whose proxy had
already been built — the name hadn't changed, so the cache kept answering, upside down. Files from an earlier
generation can never be asked for again, so the worker sweeps them at startup. Cached metadata records the
generation it was read under and is re-read when that differs: it holds the orientation, the oriented size
and the AF point, which is how 0.2.2's support for the mirrored EXIF orientations (2, 4, 5, 7) reaches photos
opened before it.

**One decode per photo.** Opening a photo fans out into four requests that all want the same decode: `/open`'s
background pre-build, the `meta` route's re-ingest pre-build, and the client's own proxy and focus fetches for
the first paint. They are serialized on the asset key, not on key + edge, because one decode writes every size
it was asked for — a caller waiting on a different edge of the same photo still wants that decode to happen
once. Two decoders run at a time overall, so different photos are unaffected.

**Loupe.**

```
POST /api/assets/{key}/loupe
{ "center": [0.552, 0.518], "size": [1024, 1024], "params": { … }, "colorSpace": "display-p3" }

200 OK   image/png   display-encoded, 8-bit, 100% scale
```

`center` is normalized to the oriented image, so it survives proxy-size changes and, later, lens correction and crop (§15). The tile runs denoise, deconvolution and the develop modules over the region plus a margin equal to the filters' support, from the session buffer. The client asks 250 ms after the last change, and only while the loupe is open.

## 6. Where each stage runs

| Stage | Ingest (server, once) | Preview (phone, per change) | Loupe (server, 1:1 tile) | Export (server, full res) |
|---|---|---|---|---|
| Normalize, hot pixels | ● | | reuses ingest | ● |
| Demosaic (RCD / Markesteijn) | ● | | reuses ingest | ● |
| Highlight reconstruction | ● as-shot WB | blend via clip alpha | reuses ingest | ● final WB |
| Denoise (à-trous wavelet) | | | ● | ● |
| 1 White balance | | ● | ● | ● |
| 2 Exposure | | ● | ● | ● |
| 3 Camera → Rec.2020, gamut compress | | ● | ● | ● |
| Deconvolution (Richardson–Lucy) | | | ● | ● |
| 4 Tone map + curve | | ● | ● | ● |
| 5 Color mixer | | ● | ● | ● |
| Output transform | | ● | ● | ● |
| Histogram, clip overlays | | ● | | |

Modules 1–5 and the output transform are the shared WGSL. Lens correction, crop and masks slot in after V1 (§15).

## 7. Client pipeline (WebGPU)

```
proxy bytes → sourceTex (rgba16float, immutable)
                 │
   UBO + LUT ──▶ develop pass (fragment, modules 1–5) ──▶ developedTex (display-linear)
                                                              │                 │
                                   pan · zoom · clip ──▶ view pass ──▶ canvas   histogram compute → bins → mapAsync → UI
```

- **Develop pass** — full-screen triangle into `developedTex` (`rgba16float`, proxy size). At most once per animation frame, only when parameters changed.
- **View pass** — samples `developedTex` through a `geometry: mat3x3f` transform (pan and zoom in V1; crop and straighten compose into it later), mip chain for zoom-out, applies clip overlays, sRGB-encodes, adds dither from Christoph Peters' blue-noise textures (CC0). Runs on view changes and after develop.
- **Histogram compute** — reads `developedTex` with stride 2 on phones, 1 on desktop, after each develop; one readback in flight.

Canvas: `configure({ device, format: navigator.gpu.getPreferredCanvasFormat(), colorSpace, alphaMode: 'opaque' })`, where `colorSpace` is the user's output space (§11), not the display's gamut. Declaring sRGB on a P3 screen makes the viewport a soft proof: the OS maps it to the panel, and what is on screen is what the exported file holds.

```wgsl
// shaders/client/develop.wgsl — composed from shaders/develop/*.wgsl
@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let src = textureLoad(sourceTex, vec2i(pos.xy), 0);
  var c = src.rgb;                          // camera-native linear RGB
  c = whiteBalance(c, P.wbMul);             // 1   diagonal Von Kries, camera space
  c = exposure(c, P.exposure);              // 2   × 2^EV, no clamp
  c = highlightBlend(c, src.a, P.hlStrength);
  c = P.camToWork * c;                      // 3   camera → linear Rec.2020
  c = gamutCompress(c);                     //     pull negatives toward the neutral axis
  c = toneMap(c, P.tone);                   // 4   log encode → sigmoid → curve LUT on luminance
  c = colorMixer(c, P.mixer);               // 5   OKLCh, 8 bands
  c = P.workToDisp * c;                     //     Rec.2020 → sRGB / P3 primaries, soft clip (RAW) or clamp
  return vec4f(c, 1.0);
}
```

Module functions take their values as arguments and never read the uniform buffer themselves, so local masks (§15) can call them with per-pixel blended values. Module bypasses compile as pipeline `override` constants, so disabled modules cost nothing.

1. **White balance.** The `color` crate (compiled to WASM) turns (Temperature K, Tint) into CIE xy on the Planckian locus offset along the isotemperature line → XYZ white → camera neutral through the XYZ→camera matrix interpolated for that temperature → multipliers = 1/neutral, normalized so G = 1. For *As shot* it runs the inverse, iterating on `cam_mul` to place the sliders.
   **Eyedropper.** The same inverse with a picked patch as the neutral (`CameraColor::neutral_xy`, wasm
   `sc_white_from_neutral`): a tap on the photo maps back through crop, straighten and lens correction to the
   source texture, which is still camera RGB before white balance; the editor reads back a square about 9 CSS
   pixels across there, leaves out pixels the ingest flagged as clipped, and sets the temperature and tint under
   which that average is grey. Needs WebGPU (the source lives on the GPU); without it the button is hidden.
2. **Exposure.** `2^EV`, computed on the CPU. Values above 1.0 pass through untouched.
   **Highlight blend.** Where alpha > 0, blend toward the pixel's luminance-preserving neutral by `alpha × strength`, suppressing the magenta cast WB multipliers put on reconstructed highlights.
3. **Color space.** Camera → XYZ (inverse of the interpolated XYZ → camera matrix, normalized so camera white lands on D65) then XYZ → Rec.2020, recomputed whenever WB changes. Gamut compression (distance-based, in the style of the ACES reference gamut compression) keeps saturated lights from producing negative channels.
#### Highlights, and what it can reach

Highlights has two halves, and before `PROCESS_VERSION` 7 it only had the first.

The **LUT's bend** grades what is below white: `tone_curve_lut` lifts or lowers the curve weighted toward
the top, with its ends pinned so nothing walks off the histogram because a slider moved
(`the_ends_are_pinned_whatever_the_sliders_do`). That is right for a grade and useless for a recovery,
because the tone map maps everything at or above `white_ev` to display white *before* the LUT sees it — so
anything blown arrives as exactly 1.0, and `LUT(1) = 1`. Highlights could not move a blown highlight at all.

The **white point's reach** is the other half: negative Highlights adds up to `highlight_reach_ev` (2 stops
at −100) to the `white_ev` the curve is built on, bringing more of the scene's top end into the picture for
the shoulder to compress. Recovery has to act on where white *is*; there is nowhere else to act from.

Two stops because that is what the rendering's own defaults put past white. Clipping begins at
`white_ev − baseline_ev` scene stops above middle grey, and the baseline (1.15) with the lower white point
(4.5 → 3.63) moved that from **+4.50 to +2.48** — 2.02 stops, which on a frame with a blown window is the
difference between **0.03 % and 15.4 %** of it rendering as flat white. At full travel Highlights now reaches
all but a hundredth of a stop of it, and Whites still moves the point itself (±2 EV), so the two compose.

A developed file has none of this: whatever wrote it chose where its white went, there is no scene above it
to recover, and `display_tone` grades what is there. And an edit from before version 7 keeps the Highlights
it was approved with — `migrate` sets its reach to zero, so the slider bends the curve and leaves the white
point alone, exactly as it did.

4. **Tone map.** Luminance Y (Rec.2020 weights) → `log2(Y / 0.18)` mapped from [BlackEV, WhiteEV] to [0, 1] → sigmoid (contrast, pivot) → user curve LUT → decode to Y′ → `rgb × Y′ / Y`, which keeps channel ratios and therefore hue. The curve is Fritsch–Carlson monotone cubic over display-encoded values, baked into 4,096 entries in an `r32float` 1D texture and interpolated manually (`r32float` isn't filterable without an optional feature). Shadows / Midtones / Highlights sliders in the UI move curve control points — the same spline.
5. **Color mixer.** Linear → OKLab → LCh. Bands: red, orange, yellow, green, aqua, blue, purple, magenta. Raised-cosine weights span neighbouring band centres and sum to 1. `h += Σ wᵢ·hueᵢ`, `C ×= 1 + Σ wᵢ·satᵢ`, `L += Σ wᵢ·lumᵢ · smoothstep(0.02, 0.08, C)` so neutrals stay put. Back to linear.

**Developed files take a display-referred path.** A JPEG, HEIC, PNG or TIFF was rendered for display by
whatever wrote it — tone curve, white clip, gamut — and none of that can be undone, so the stages above that
exist to render sensor light don't apply to it. `SourceKind::Developed` (from `RawInfo.source_kind`, passed to
`pack`) sets `display_referred` in the uniform, and the develop pass then skips highlight blending (strength
0), gamut compression and the RAW tone curve, and ends in a plain 0–1 clamp instead of the output soft clip.
Step 4 becomes `tone_display`, on sRGB-encoded luminance, the identity at default settings: a shoulder that
only engages once exposure is positive (knee `1 − 0.3·EV`, floor 0.4) → levels for Blacks (±0.1 encoded)
and Whites (±0.2) → a power S-curve through encoded middle grey for Contrast (exponent = the stored
contrast) → the same user-curve LUT → `rgb × Y′ / Y`, with black lifted to neutral grey. The stored fields
are the RAW ones (`whiteEv`, `blackEv`, `contrast`), read through the slider mapping, so the editor and the
sidecar format need no second meaning; the editor limits Exposure to ±2 EV and hides Highlight recovery.
Whites is read as the difference from `tone.whiteZeroEv`, the white point the edit was made against, rather
than from today's constant — a developed file has no white point to place, so its Whites would otherwise move
whenever the RAW default did. `baselineEv` is a RAW's alone and is not applied here at all.
Measured on an iPhone HEIC at default settings: mean difference from the file 0.03/255, none above 0.12/255.

**Film.** A RAW can be rendered the way colour negative film renders light, as an alternative to step 4's
neutral rendering — the process, not a look imitating any stock. Film has three emulsion layers, each with its
own characteristic curve, so red, green and blue each pass through the curve separately: a bright saturated
light reaches the shoulder in one channel before the others and fades toward white, and midtones gain
saturation from the toe and shoulder pulling channels apart. The neutral path's `rgb × Y′ / Y` holds hue
instead; that is its point, and film's is not to. `EditParams.film` (absent = neutral, so old edits are
unchanged and need no process version) holds:

- **Amount** 0–100 (default 100): a mix between the neutral and the film result, after the user's curve.
- **Interlayer effects** 0–100 (default 45): in film, developer by-products from one layer hold back
  development in its neighbours, which separates colours. Modelled as a spread in log exposure away from the
  pixel's mean over the channels, `x + s·(x − mean)`, with `s = interlayer/100 × 0.6`, before the curves. A
  neutral has no spread, so greys stay grey and white balance keeps its meaning.

`film_curve_in` (shaders/develop/tone.wgsl) applies the same logistic, black and white points as the neutral
path to each channel; the user's curve LUT and mask curves then apply per channel. A channel at zero is held
four stops under the black point, where the curve is flat. Developed files ignore it (`pack` writes 0): their
curve was already applied. Judged on ten RAWs from the library: 30 reads as natural, 70 is strong (skin goes
orange-red); the user chose 45 as the default. Halation, grain and monochrome film are later steps.

**Black and white.** `EditParams.monochrome { film, filter }` renders the light as a black-and-white film type
would see it, instead of taking a weighted sum of RGB. Three numbers can't say which spectrum a colour came
from; Sidecar picks the one of Jakob and Hanika's family (2019: a sigmoid of a quadratic in wavelength) whose
colour under D65 is the pixel's — always a valid reflectance, smooth as surfaces are, and exactly the colour
it came from — implemented from the paper in `sidecar_color::spectral` (Newton's method on the CIELAB
difference, from grey out in steps), with CIE 1931 colour matching functions and D65 at 10 nm taken from
colour-science (BSD-3). colour-science's own implementation was too slow for this (minutes for a coarse table);
ours fits a 17³ table in about 0.16 s, once per process, and matches its spectra within 0.01.

Each colour is fitted at a surface's brightness (luminance at middle grey's, no channel above 0.95), not
scaled until a channel reaches 1: the family's spectrum depends on brightness, and an implausibly bright,
saturated colour comes back as a red that rises again in the violet — unseen by the eye, seen by blue-sensitive
film, which then rendered red paint lighter than panchromatic did. Light is linear, so the scale divides back
out. The film's exposure is the spectrum lit by D65 (the picture is white-balanced), through the filter's
transmission and the film type's sensitivity, white at 1; tabulated over the colour cube for the chosen
film and filter (`mono_lut`, rebuilt per pack in well under a millisecond), read by the develop pass right
after the camera matrix (`develop/mono.wgsl`, eight corners, since r32float isn't filterable), and passed on as a
neutral to the tone map. Film rendering gives way when both are set; developed files can be black and white too.

Film types are the classes' sensitivities — panchromatic (classic), extended-red panchromatic
(modern flat-grain), orthochromatic, blue-sensitive (collodion; the ultraviolet real plates saw is not in any RAW) — not
measured curves of named stocks, which need their datasheets digitised. Filters are generic yellow, orange,
red (long-pass edges at 500, 555, 600 nm) and green (a band). Measured on everyday colours: blue-sensitive film
darkens skin 0.7 stop against panchromatic and lifts the sky; orthochromatic takes a saturated red to near
black; a red filter darkens a blue sky 1.6 stops. Not yet: the stock's own characteristic curve.

**The scene's light.** `Monochrome.light` names what lit the scene: `daylight` (D65, the default, left out of
the serialized edit), `tungsten` (CIE illuminant A: Planck's law at 2,848 K with CIE's older constant 1.435e-2),
or `white-balance` — the temperature the edit's white balance resolves to, as a black body below 5,000 K and CIE
daylight above (the split CIE's colour-rendering index uses; daylight from CIE 15's chromaticity formula and
basis functions S0, S1, S2, taken from colour-science). The white-balanced pixel says what the surface is; film
has no white balance, so it sees that reflectance times the light, and the meter (white at 1) evens out the
total. Under tungsten, panchromatic film renders a saturated red 0.86 stop lighter and a blue sky 0.55 stop
darker than in daylight, against the same grey. Checked against colour-science: A within 0.1 %, D50 within 0.1
of 100.

**Developers.** `Monochrome.developer` (`standard`, `fine-grain`, `acutance`, `compensating`, `staining`; the
default left out of the edit) and `Monochrome.development` (−2 … 2, the zone system's N−2 … N+2) act on the film's
exposure right after `mono_exposure`, before grain and the tone map (`develop/developer.wgsl`,
`sidecar_color::developer`). In log exposure `x`, stops from middle grey:

- *Adjacency:* `x + a·(x − local)`, where `local` is the exposure around the pixel, blurred over the developer's
  reach on a 35 mm negative — developer exhausts in dense areas and stays fresh in thin ones, and diffusion
  carries both across an edge (the Eberhard effect, Mackie lines).
- *Development:* contrast × `5 / (5 − N)` above a speed point 2.5 stops under grey, rounded into the shadows
  with a softplus hinge: N+1 fits four zones of subject into five of negative; shadows barely move.
- *Compensation:* density grows by `1 − c` as much above a knee 1.5 stops over grey.

| developer | adjacency | reach | compensation | grain |
|---|---|---|---|---|
| standard (general purpose) | 0 | — | 0 | × 1 |
| fine grain (solvent) | 0 | — | 0 | × 0.75 |
| high acutance (dilute) | 0.35 | 0.08 mm | 0.1 | × 1.2 |
| compensating (two-bath, stand) | 0.25 | 0.3 mm | 0.45 | × 1 |
| staining (pyro) | 0.2 | 0.15 mm | 0.3 | × 1, stain hides up to half at the white point |

Development scales grain by `1 + 0.15 N`. These are the developers' kinds as the literature describes them, not
fits to one formula's measurements. The standard developer at N returns its input exactly.

`local` comes from an *adjacency map*: the frame averaged down by a whole number to at most 1,024 texels on its
long edge (`client/density_down.wgsl`), developed by the develop pass itself with `mono.z` set — which returns
the film's log exposure right after `mono_exposure`, through the whole frame's masks — then blurred twice
(`client/density_blur.wgsl`, separable Gaussian, σ the reach in map texels). The develop pass samples it
bilinearly at the pixel's place in the frame. A develop of the whole frame builds its own map (browser:
`DevelopPass` in `develop.ts`; server: `DevelopPass::render`), so preview, export and parity need nothing new;
the loupe builds one from the whole session source, lens-corrected at the map's size, and hands it to every
tile, so a tile's edges are the whole picture's (engine test: within 0.001). Depth effects and the detail
filters are not in the loupe's map; at a fraction of a millimetre's reach this doesn't show.

**Stocks.** A named stock is a film type with its maker's measured curves, offered under its type in the
editor (the edit names the stock). Four so far, read from the datasheets the owner supplied by the scripts in
`crates/color/film-data/` (its README lists the sheets and how to rerun them); the data is in
`crates/color/src/film_data.rs`:

| stock | type | spectral sensitivity | characteristic curve |
|---|---|---|---|
| ILFORD HP5 Plus | panchromatic | spectrogram, traced | ILFOTEC HC 1+31, 6½ min |
| KODAK TRI-X 400 | panchromatic | log sensitivity, vector | D-76, 8 min |
| KODAK TRI-X 320 | panchromatic | Tri-X's one curve (the sheet gives no other) | D-76, 8 min, sheets |
| ILFORD SFX 200 | extended red | spectrogram to 740 nm, traced | ID-11, 10 min |

- *Spectral sensitivity.* Kodak plots log₁₀ of the reciprocal of the energy giving density 1.0 over fog — the
  film's own sensitivity, used as it is. ILFORD plots a "wedge spectrogram to tungsten light (2,850 K)", 0 to
  1; it is read as drawn, the film's relative sensitivity. *That corrects the first reading of HP5*, which
  divided the light out: SFX 200's filter-factor table decides it — read as drawn, SFX needs ×2.9 behind a red
  filter where the sheet says ×2.8; divided by tungsten it would need ×9. (The first reading had leaned on the
  sheet's warning that cameras' meters underexpose by up to 1½ stops behind deep red: that is about meters'
  red response, not the film's spectrum.) Every stock's red (Wratten 25) factor through Sidecar's generic red
  filter lands within 0.3 stop of its sheet: SFX 2.9 / 2.8, Tri-X 9.0 / 8 in daylight and 4.1 / 5 in tungsten.
  Our yellow is a long-pass at 500 nm, deeper than a Wratten 8 (Tri-X 2.4 / 2, SFX 1.6 / 2). HP5 read as drawn renders yellow 0.6 stop darker and a blue sky 0.4 stop lighter than
  the eye — the classic panchromatic its sheet describes; its red factor, 6.3, sits with Tri-X's.
- *Characteristic curve.* Each is fitted (`sheets/fit.py`, Nelder–Mead) as
  `fog + G·(w·sp((log E − e₀)/w) − c·v·sp((log E − e₁)/v))`, `sp(u) = ln(1 + eᵘ)`: a toe that becomes the
  straight line, and a shoulder taking `c` of the gradient away — within 0.015 density of every trace. Middle
  grey sits 1.2 log E (Zone V four stops over Zone I) above the speed point at fog + 0.1; Kodak's curves are in
  lux·s, and Tri-X 400's speed point falls at log −2.701, where ISO 400 (0.8 / 400 lux·s) puts it: −2.699. In
  stops from grey the toes sit near −3.7 to −4 (HP5, Tri-X 400, SFX), Tri-X 320's at −1.4 and twice
  as wide — the long toe it is known for; shoulders: SFX 200 rolls fully over around the white point, Tri-X 400
  takes 30 % off above +4.4, HP5 is nearly straight.
- *Rendering.* `stock = [1, toe at, toe width, shoulder strength]` and `shoulder = [at, width]` reach the develop
  pass, which bends exposure after grain (`develop/stock.wgsl`): the curve's shape, scaled to slope 1 at grey,
  so the deepest shadows flatten into fog, grain with them, and the highlights go over the shoulder, while
  grey and Contrast's midtone slope are kept. The print puts the fog half a stop over the tone map's black
  point (`tone_params`): the black point moves to the curve's floor, Blacks still moves it from there, and the
  steepness follows the narrower range. The gradient itself is the print's: a darkroom printer picks the
  paper grade. From a RAW only; a developed file keeps its own tones and takes just the sensitivity.

*Named products use their own data.* The owner's rule: generic items (the film classes, the developer kinds, Film's
interlayer model) may rest on assumptions, but anything carrying a product name must rest on that product's own
data or a published standard — never another product's curves. The generic items name no products. One known gap:
Double-X's sheet plots its sensitivity from 420 nm and the trace is held flat below, which carries 15 % of a
daylight exposure (Tri-X, measured to 300 nm, carries 18 % there): it needs the owner's decision.

**Colour film, spectrally (stage 3).** `crate::slide` renders a colour reversal stock from its data
sheet alone — FUJICHROME PROVIA 100F first, whose sheet (AF3-036E) gives each layer's spectral sensitivity, the
three characteristic curves and each dye's spectral density, all traced (`crates/color/film-data/provia-100f/`):

1. The scene's spectrum: each colour's reflectance (Jakob and Hanika) under the scene's light, at the exposure a
   reflected-light meter gives ISO 100 for Sidecar's middle grey (ISO 2720: q·K/S = 0.65 × 12.5 / 100 lux·s).
2. Each layer's exposure through its sensitivity. FUJIFILM's sensitivities are relative (read as absolute, density
   1.0 would take ~0.1 J/cm², 10⁸ times what an ISO 100 film needs), so each layer is placed on its curve through the
   light the curves were exposed with — the sheet's "Daylight", **ISO 7589's sensitometric daylight**.
3. The characteristic curves give Status A densities, as the sheet measured them (a Fuji FAD-30S densitometer).
4. The dye amounts that read those densities through **ISO 5-3's Status A spectral products**, by Newton's method.
5. The slide on the sheet's ISO 3664 light table (CIE D50), the eye adapted to it (Bradford), on the display.

Both ISO tables are licensed and stay **out of the public repository and image**. The owner found them: Status A
in ANSI/ISO 5-3-1995 (ANSI/NAPM IT2.18-1996, table 3: log₁₀ spectral products at 10 nm, with slopes beyond each
listed range; ISO 5-3:2009 keeps the same 10 nm values), and ISO sensitometric daylight in ISO 7589-1984, table 1
(D55 × the ISO standard lens, 350–690 nm; 0 beyond, where Provia's layers see nothing and the eye under 0.1 % of
a lux) — the current edition is ISO 7589:2002; its daylight table is still to be compared. With both: a metered
middle grey in ISO daylight renders [0.074, 0.080, 0.089] — equal Status A densities of about 1.1, slightly cool
on the D50 light table. They are
read at run time from the operator's own files, named by `SIDECAR_STATUS_A` and `SIDECAR_ISO_DAYLIGHT`
(`Densitometer::parse`, `parse_spectrum`). Tests run the mechanics through a stand-in densitometer and D55
(`slide::stand_in_standards`), which render no real slide.

*What the develop pass reads.* Two tables (`develop/slide.wgsl`, eight corners each, since rgba32float does not
filter): each colour's three layer responses over the colour cube (`exposure_lut`), and the slide's colour over
layer log H, 65³, in log₂ (`response_lut`; within 2 % between lattice points). A pixel's responses are its
largest channel times the table's at its chromaticity, as light is linear; `SlideProfile`'s three offsets put
each layer's response on its own curve, and clamping to the table's domain is what holds every curve flat past
its ends — so an unexposed frame comes out at full density, as a reversal stock does. The stock takes the place
of the neutral tone map, and `Film.amount` mixes the two; `Film.interlayer` has no part in a sheet's chain and
is ignored (the editor hides it for a stock).

*Grading what comes out.* A stock's output is a rendered picture — a slide on a light table — so the tone
controls and the user's curve act on it the way they act on a developed file (`tone_display_curve_in`, with
`DevelopParams.slide_tone` carrying them in that form): the identity where they sit at their defaults, so the
chain's own colour is what appears, and a grade on top when they are moved. The knee is held at 1, because
Exposure has already moved the film along its own characteristic curve and the sheet's shoulder is the one
that counts. This replaced the first arrangement, where those controls reached the picture only through the
neutral rendering — which meant that at a full Amount, Contrast, Blacks, Whites, Highlights, Shadows and the
curve did nothing at all while still moving. Exposure worked throughout, since it is the film's exposure.

*Which side holds what, and why.* `pack` is compiled to wasm so the browser packs the same uniforms, and the
browser must hold no ISO table. So a stock splits in two. The **profile** — the response table and the three
offsets, all a standard touches — is measured on the server from the operator's files, cached per stock
(`sidecar_worker::stocks`, in memory and under `cache/stocks`; about four seconds, a quarter of a million dye
solves) and served by `GET /api/film/stocks/{id}`; the browser fetches it once and uploads the table straight to
the GPU. Neither standard can be recovered from it: it is a table of colours and three numbers. Everything else
— the sheet's sensitivities and the scene's light — is public, so `exposure_lut` is built per edit on whichever
side is rendering. A deployment without the two files renders no stock and offers none: `GET /api/film/stocks`
reports what it has, and the editor shows the choice only for those.

Edges, stated: sensitivities beyond the sheet's plotted floor (about 1/30 of each peak) are 0;
dyes beyond 388–719 nm hold their end values, where the eye sees under 0.1 % of the light. Viewing places the light
table at the display's white, so the slide keeps its density: a slide's middle grey is darker than Sidecar's
neutral rendering of the same exposure, as it is on a light table.

**Grain.** `EditParams.grain { amount, size }` makes each spot's exposure flicker as a negative's grains make
it, before the tone map, in colour or black and white, on a RAW or a developed file (`develop/grain.wgsl`). The
noise is procedural, not a stored map: two octaves of value noise — clumps at the grain's size, grains at half
of it — whose lattice values come from an integer hash of the pixel's place in the whole frame (pcg2d, from
Jarzynski and Olano's 2020 JCGT paper), four 16-bit uniforms summed to a near-Gaussian value. Integer maths and
an exact multiply make it the same on every GPU: parity's grain cases are identical between browser and server.
Interpolated value noise is divided by its own spread at that position, so the lattice leaves no pattern of
stronger and weaker grain. Position is the frame's: the develop pass adds `MaskParams.frame`'s offset, so a
loupe tile's grain is the export's at the same pixels.

Size is a grain on a 35 mm negative, 5–40 µm (`Grain::microns`, doubling every third of the slider), turned into
pixels from the frame's long edge at each rendering, so grain keeps its share of the picture at every size.
Where an octave is finer than a pixel, its lattice is the pixels and its strength falls with grain size over
pixel size — Selwyn's law, granularity × √(area seen) constant — so the proxy shows what a scaled-down export
would, and the full-size export shows the grain itself.

The flicker is in exposure, `exp2(σ·n)`, with its mean kept (`− σ² ln 2 / 2`). Grain is denser where the
negative is, so σ grows with the pixel's place between the black and white points (`√((0.2 + t)/1.2)`); the
tone curve then flattens it in the deep shadows and the clipped highlights, and more contrast shows more of it,
as a harder paper grade does. A developed file is already a print: its σ is shaped `4e(1 − e)` on the encoded
value instead. Amount 100 is a σ of 0.6 stop. Colour grain is one value for all three channels (luminance
grain); separate grain per dye layer belongs to stage 3's spectral colour film.

Uniform buffer:

```wgsl
struct DevelopParams {            // offset  size
  wbMul      : vec3f,             //    0     12   camera-space multipliers, G = 1
  exposure   : f32,               //   12      4   2^EV
  camToWork  : mat3x3f,           //   16     48
  workToDisp : mat3x3f,           //   64     48
  tone       : vec4f,             //  112     16   RAW: blackEv, whiteEv, steepness, pivot
                                  //                developed: blacks, whites, contrast, knee
  hlStrength : f32,               //  128      4
  displayReferred : f32,          //  132      4   0 RAW, 1 developed
  saturation : f32,               //  136      4   global chroma gain − 1
  _pad1      : f32,               //  140      4
  mixer      : array<vec4f, 8>,   //  144    128   per band: hue, sat, lum, unused
  film       : vec4f,             //  272     16   amount 0–1 (0 neutral), interlayer spread, unused ×2
  mono       : vec4f,             //  288     16   black and white: 1 on, the table's side, 1 = output the
                                  //                adjacency map's log exposure, unused
  grain      : vec4f,             //  304     16   σ in stops (0 none), size ÷ long edge, stain masking, unused
  developer  : vec4f,             //  320     16   adjacency, reach ÷ long edge, development contrast, compensation
  stock      : vec4f,             //  336     16   a named stock's curve: 1 on, toe at, toe width, shoulder strength
  shoulder   : vec4f,             //  352     16   the stock's shoulder: at, width (stops from grey), unused ×2
}                                 //  total 368 bytes
```

**Struct layouts.** The 368-byte table above documents the layout; code never hard-codes it. `encase` (MIT-0) derives it on the Rust side and `webgpu-utils` (MIT) reads it from the WGSL source in the browser, so a field added to the struct can't silently shift the offsets.

**Parameter flow.** Slider input → params store (Svelte 5 `$state`) → derived CPU values (WASM: WB → multipliers and matrix; curve → LUT) → `queue.writeBuffer` → dirty flag → next `requestAnimationFrame` runs develop → view → histogram. Undo snapshots on `pointerup`, not per input event.

**Device loss.** Safari drops the GPU device when a tab goes to the background; so do other mobile browsers. On `device.lost`: request a new adapter, re-upload the proxy from Cache Storage, rebuild pipelines. The params store is untouched.

**Memory on an iPhone** (2,560 × 1,707): sourceTex 35 MB, developedTex 35 MB plus 12 MB of mips, LUT/bins/uniforms under 1 MB — about 82 MB, well inside what Safari allows a tab.

## 8. Histogram and clipping

```wgsl
@group(0) @binding(0) var developed : texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> bins : array<atomic<u32>, 1024>; // R, G, B, luma × 256
@group(0) @binding(2) var<uniform> H : HistParams;                              // dims, stride

fn bin(v: f32) -> u32 { return min(u32(max(v, 0.0) * 256.0), 255u); }

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) id: vec3u) {
  let p = id.xy * H.stride;
  if (any(p >= H.dims)) { return; }
  let e = srgbEncode(textureLoad(developed, p, 0).rgb);   // bins match the screen
  atomicAdd(&bins[bin(e.r)], 1u);
  atomicAdd(&bins[256u + bin(e.g)], 1u);
  atomicAdd(&bins[512u + bin(e.b)], 1u);
  atomicAdd(&bins[768u + bin(dot(e, vec3f(0.2126, 0.7152, 0.0722)))], 1u);
}
```

Readback: zero the bins with `writeBuffer`, dispatch `ceil(dims / stride / 16)`, `copyBufferToBuffer` into a `MAP_READ` buffer, `mapAsync`. If the previous readback hasn't resolved, skip this one — never queue. 4,096 bytes cross to the CPU. A 16 × 16 workgroup is 256 invocations, exactly WebGPU's default per-workgroup limit.

Clip overlays, in the view pass before encoding and dither (thresholds precomputed on the CPU as `srgbDecode(254.5/255)` and `srgbDecode(0.5/255)`):

```wgsl
let m = max(c.r, max(c.g, c.b));
if ((V.flags & CLIP_HIGHS) != 0u && m >= V.hiT) { return vec4f(1.0, 0.0, 0.0, 1.0); }  // any channel at white
if ((V.flags & CLIP_LOWS)  != 0u && m <= V.loT) { return vec4f(0.0, 0.0, 1.0, 1.0); }  // every channel at black
```

The test runs after tone mapping, so red means *the output file will clip here*, not "an intermediate value exceeded 1.0". Corner triangles on the histogram show the clipped percentage and toggle each overlay (`J` with a keyboard).

### Measure

The same counts, read rather than hovered. **Measure**, a fold under the histogram on the wide layout, states
what the rendering on screen holds:

| | |
|---|---|
| `at white` / `at black` | the clipped counts the histogram pass already returns, as a percentage of the pixels it counted |
| `probe, 0…255` | the probed place in the output's own encoding — what the exported file would carry there |
| `probe, linear` | its display-linear luminance (Rec. 709 weights) |
| `stops from grey` | `log2(Y / 0.18)`, which is how an exposure reads |
| `highest channel` | the greatest single channel there, which is what "at white" tests |

The probe reads a square about four proxy pixels out from the tapped place, averaged — a single pixel of a
proxy is noise. Its point is stored in the **picture's own frame**, the frame developedTex is in, so a crop, a
straighten, a turn or the flip all read the same place in the photograph. The readback is a
`copyTextureToBuffer` queued at the same gate as the histogram's, so it sees the picture as it stood when the
frame was drawn, whatever renders next, and no readback happens on a frame that would not already have run
one.

Each side of the flip (`\`, or hold on a touchscreen) keeps **its own** reading, named for the version it is:
flip to the compared version and back, and both columns stay on screen, which is the whole point — two
renderings of one photograph, read side by side instead of judged by eye. *Copy* writes the table as plain
text, so a measurement can leave the editor.

**Focus peaking** shares the pass and the same principle — judge what the pixels actually hold:

```wgsl
let step = max(1.0 / vec2f(textureDimensions(developed)), vec2f(V.uv_from_px[0][0], V.uv_from_px[1][1]));
if (focus_energy(uv, step) >= V.peak_t) { return vec4f(PEAK_COLOUR, 1.0); }   // amber; red and blue stay clipping
```

`focus_energy` is the central-difference gradient of encoded luminance. **This is a stopgap:** measured on the proxy it reports edge contrast rather than sharpness, and the real measure belongs at full resolution on the server, shipped as a layer — see the roadmap. The step is the larger of one texel and one canvas pixel, so zooming out averages detail away instead of aliasing downscaling noise into false peaks. It reads the proxy, so at fit zoom it reports composition-scale sharpness; critical focus means zooming in or opening the loupe.

### An edit and the rendering it was made under

Stored references keep an old edit's picture: `migrate` pins `baseline_ev`, `white_zero_ev`,
`shoulder_order` and `highlight_bleach` to what the edit was approved with, so the person gets back the
photograph they chose. That is right, and it is not the whole story, because left alone it has two
consequences nobody chose.

**The original is not an edit.** A history begun under an older version stored *its* defaults in the root,
and `version_of` migrated them like any other node — so "open the original" handed back a rendering no one
had ever asked for, on a node that stands for the absence of an edit. It now returns `EditParams::default()`
outright: the original means this file as Sidecar renders it today. Only what is *handed out* changes;
`edit_of` still rebuilds the stored root as written, because every child's delta was taken against that and
replaying one onto a different base would move versions somebody did approve.

**An old rendering used to spread.** The references are inherited by whatever is edited from them, and
nothing stamped them as old: a version saved today, out of a sidecar from `PROCESS_VERSION` 2, came out
marked version 6 and rendering by version 2's rules — measured, before this changed. So a rendering nobody
could reach any more went on reproducing itself for as long as anyone kept editing, and the version number
said nothing about it.

So **editing an old edit brings it up to date first** (`adopt_current_rendering`, below). The photo says so
while it is still untouched — "changing anything brings it up to date, and the picture will change" — and
the first change carries the update with it. Afterwards the way back is offered for as long as the photo is
open; taking it restores the references and stops the asking **for that session only**, because an edit left
on old references is the thing this is here to stop. None of it is automatic in the background: nothing
changes until the person changes something, and Undo takes all of it back.

What this does **not** do is let the version-specific code go. A frozen or ported edit still has to render
its ancestors, and reading them still needs `migrate` — the version-1 curve anchors above all. What it does
is make the set of edits carrying old references **closed and shrinking** rather than self-renewing, which
is the precondition for ever removing any of it.

### Taking up today's rendering

`EditParams::adopt_current_rendering` is the other direction from `migrate`. It sets the references a new
edit of this file would get and leaves everything the person set alone — Exposure, the curve, masks, film,
crop. Whites needs care, because it is stored as a white point and *read* as the distance from the
rendering's own: the offset is kept, so the slider still says what it said. Only references this file's
rendering actually reads are touched, so a developed file takes up the white point and nothing else — it has
no baseline, and `pack` holds its shoulder and bleach at zero anyway. That is also what makes
`behind_current_rendering` worth asking: nothing is offered, and nothing happens on an edit, unless
something would visibly change.

The rule lives in `sidecar-color` and reaches the browser through `sc_adopt_rendering`, so the two cannot
drift on what "today's rendering" means.

## 9. Export

Every export is kept. Each distinct edit that gets exported becomes a numbered version, and the newest JPEG leads the photo's stack in Immich.

1. **Commit** — `PUT /api/assets/{key}/edit` with `If-Match` → XMP written → new ETag.
2. **Create job** — `POST /api/exports` with `Idempotency-Key` (one UUID per Export tap) and `{ assetKey, editEtag, format, colorSpace, quality, longEdge, destination }`.
   - If `sc:Exports` already holds this ETag with these settings *and that entry names an Immich asset*, the answer is `200 { version, immichAssetId }` — nothing renders twice. Two things make it render anyway, both under the existing version number and the recorded file name: the file being wanted on this device (the copy that was uploaded is deleted, so there are no bytes left to hand back), and the entry having no `immichAssetId` — an export that only ever went to the device, for which this is the upload it never had. What the entry already covers is not done twice: the upload is skipped when it is in the library, and the record is written only to add what this export did.
   - Otherwise the api assigns the version under a per-photo lock and answers `202 { jobId, version }`. The job is pinned to that ETag; later edits don't touch it. Retrying with the same key returns the same job.
3. **Stream** — `new EventSource('/api/exports/' + jobId + '/events')`.
4. **Render** — worker, full resolution, tiled.
5. **Upload and stack** — api.

**Destination.** `destination` is `immich` (the default, and what a client that sends nothing means), `device`
or `both`. `immich` is the original behaviour: upload, stack, drop the local copy. The other two keep the
rendered file in the worker's outbox and put a download link in the `done` event, so the phone or laptop doing
the editing is a destination in its own right rather than only a remote control for the library. The browser
hands it to the share sheet where there is one — that's the path that ends in *Save Image* on a phone — and
falls back to an ordinary download. Kept files are swept after 24 hours; the destination is not part of what
makes two exports the same version, so asking for the same render twice, once for each place, is one version —
one `sc:Exports` entry, which a later upload fills in with its `immichAssetId` rather than duplicating. The
sheet offers the other place as soon as a render finishes: *Also save to Immich* under a file that went to the
device, *Save to this device* under one that is already in the library (that one renders again, since its
bytes are gone, and hands the result straight to the share sheet).

**Versions and file names.** A version is the *edit*: a new edit ETag takes the next number, and the same edit
exported again — another format, size, quality, colour space or destination — reuses it. The file name carries
the rest, so two renders of one version never claim one name. The full-size sRGB JPEG at the default quality
is the plain name; anything else adds its shorthand, in the order size, space, quality. A render recorded
before this rule keeps the name it was recorded under: those are the bytes in Immich.

| Export | File name |
|---|---|
| First exported edit, JPEG | `DSC_1234-v1.jpg` |
| Same edit, 16-bit TIFF | `DSC_1234-v1.tif` |
| Same edit, JPEG at 2,048 px | `DSC_1234-v1-2048px.jpg` |
| Same edit, JPEG in Display P3 | `DSC_1234-v1-p3.jpg` |
| Same edit, JPEG at quality 80 | `DSC_1234-v1-q80.jpg` |
| Same edit, 1,024 px, P3, quality 100 | `DSC_1234-v1-1024px-p3-q100.jpg` |
| Changed edit, JPEG | `DSC_1234-v2.jpg` |

Immich stores uploads in the internal library under the storage template. With `fileCreatedAt` set to the RAW's capture time, `DSC_1234-v2.jpg` lands in `2026/09.September/2026-09-14/` — the same folder path the RAW has in the external library.

```
retry: 3000

id: 14
event: progress
data: {"stage":"denoise","stageProgress":0.85,"overall":0.52}

: keepalive

id: 22
event: done
data: {"version":2,"file":"DSC_1234-v2.jpg","immichAssetId":"…","stackId":"…"}
```

- Events: `queued {position}`, `progress`, `done`, `failed {code, message, retryable}`.
- Each job's events sit in a ring buffer; on reconnect the browser sends `Last-Event-ID` and missed events replay.
- A `: keepalive` comment every 15 s. SSE still needs this against idle timeouts on carriers and proxies — one line, not a ping/pong protocol.
- The client calls `close()` on `done` or `failed`; otherwise `EventSource` reconnects after the server ends the stream.
- Safari suspends a background tab within seconds, which drops the stream. The render carries on server-side; on `visibilitychange → visible` the stream reconnects and replays, or the page reads `GET /api/exports/{id}` if the job already finished. The new version also appears in Immich as the stack's cover.
- Caddy's `reverse_proxy` flushes `text/event-stream` immediately; no extra config.

Full-resolution render, in 4,096² tiles with overlap ≥ the largest filter support, feathered at seams:

| Stage | Space | Progress weight |
|---|---|---|
| Decode, normalize, hot pixels | CFA | 5 % |
| Demosaic | camera RGB, pre-WB | 10 % |
| Highlight reconstruction at final WB | camera RGB | 5 % |
| Denoise: variance-stabilized à-trous wavelet shrinkage | camera RGB, linear | 30 % |
| Develop 1–3 (shared WGSL) | → Rec.2020 linear | 5 % |
| Richardson–Lucy deconvolution on luminance, damped | Rec.2020 linear | 25 % |
| Resample to `longEdge` if set | linear | 5 % |
| Develop 4–5 + output transform (shared WGSL) | display | 5 % |
| Encode, metadata | — | 5 % |
| Upload, stack | — | 5 % |

**Output file.**

- JPEG: 8-bit, 4:4:4, quality from the export sheet (default 92), triangular dither before quantizing, encoded with `mozjpeg`. TIFF: 16-bit, with the `tiff` crate.
- ICC profile embedded: the compact sRGB or Display P3 v4 profile from Compact-ICC-Profiles (CC0).
- `exiftool -tagsFromFile <raw> -all:all -Orientation=1` (pixels are already rotated). ExifTool runs as a separate process inside the image.
- The export's own XMP carries an `sc:Sidecar` wrapper with `SourceSHA1`, `Version`, `EditETag` and the complete edit parameters. Immich reads a JPEG's embedded XMP the same way it reads a sidecar, so the wrapper matters here too. Every version holds the edit that produced it, and the link back to the original lives in the file, not only in Immich's stack.

**Upload and stack** (api, with the user's Immich API key):

1. The worker writes the file to `/cache/outbox/<jobId>/` with a manifest.
2. `POST /api/assets` (multipart): `assetData`, `filename` = `DSC_1234-v2.jpg`, `fileCreatedAt` = RAW capture time, `fileModifiedAt`. Immich 3.1.0 has no `deviceAssetId` or `deviceId` fields; it detects duplicates by checksum per user and answers `{ id, status: "duplicate" }`, which counts as success.
3. Stack. In 3.1.0, `POST /api/stacks { assetIds }` makes the first ID the primary and merges in any existing stack whose *primary* is listed; a non-primary member listed there is pulled out of its stack instead. So the api reads the RAW asset's `stack.primaryAssetId` and sends:

   | RAW's state | JPEG export | TIFF export |
   |---|---|---|
   | Not stacked | `[export, raw]` | `[raw, export]` |
   | Stacked | `[export, currentPrimary]` | `[currentPrimary, export]` |

   Every merge creates a new stack ID, so Sidecar never stores one. No separate call to change the primary is needed.
4. Record the version in `sc:Exports` in the RAW's sidecar — as a new entry, or by filling in the one this
   render already has (the same edit and the same settings, so the same file) with where it has now reached.
5. Delete the outbox entry, unless the device asked for the file too. On restart the api retries leftover outbox entries with backoff, so an Immich outage never loses a finished render.

Step 4 is the one failure that doesn't sink the export. A render that reached Immich is not lost because the
sidecar couldn't be written — the commonest reason being a library folder the api container has no write
access to — so that failure is reported as a `warning` event and stepped over: the photo's version history
misses an entry, and the file the person asked for still arrives.

**Also confirmed** in the 3.1.0 source: stack creation checks `asset.update` on every listed asset, and a merge keeps all members of the absorbed stack.

## 10. API

| Method | Path | Purpose |
|---|---|---|
| GET | `/open` | `?sha1&name&taken&size` → resolve, record in Recent, start ingest, `303` to the editor or picker |
| POST | `/api/resolve` | `{ sha1?, name?, taken?, size?, immichAssetId? }` → `{ assetKey, filename, camera, hasSidecar }` or `{ candidates }` |
| GET | `/api/recent` | Last 50 photos opened by this user, any device |
| GET | `/api/assets/{key}/meta` | Dimensions, orientation, as-shot WB (K, tint), multipliers, matrices, EXIF, export versions |
| GET | `/api/assets/{key}/proxy?long_edge=` | RGBA16F bytes (§5) |
| POST | `/api/assets/{key}/loupe` | 1:1 tile, PNG |
| GET | `/api/assets/{key}/edit` | Params + `ETag` |
| PUT | `/api/assets/{key}/edit` | Commit; `If-Match` required; `412` on conflict |
| POST | `/api/intake` | multipart `file` → upload to Immich, `{ assetKey }`; `501` without a library |
| POST | `/api/exports` | `202` new job, or `200` existing version; `Idempotency-Key`; `destination` |
| GET | `/api/exports/{id}/events` | `text/event-stream` |
| GET | `/api/exports/{id}` | Status snapshot |
| DELETE | `/api/exports/{id}` | Cancel |

**Auth.** The user signs in once per browser (Safari, Home Screen app, desktop) with an Immich API key holding `asset.read`, `asset.view` (thumbnails in the picker), `asset.upload`, `asset.update` and `stack.create` — stack creation in 3.1.0 checks `asset.update` on each asset. The admin key for internal-library sidecar discovery (§4) is separate, holds only `job.create`, and is read from a Docker secret. It's stored server-side, encrypted at rest; the browser gets an HttpOnly, `SameSite=Lax` session cookie — `Lax` so the cookie comes along on the top-level navigation from the Shortcut. Every resolve goes through that key, so nobody opens a photo Immich wouldn't show them.

```ts
type Band = 'red' | 'orange' | 'yellow' | 'green' | 'aqua' | 'blue' | 'purple' | 'magenta';

interface EditParams {
  processVersion: 4;
  whiteBalance: { mode: 'asShot' | 'custom'; temperatureK: number; tint: number };   // 2,000–50,000 K; −150…+150
  exposure:     { ev: number; baselineEv: number; highlightBlend: number };          // −5…+5 EV; the rendering's own, not a control; 0…1
  tone:         { blackEv: number; whiteEv: number; whiteZeroEv: number; contrast: number;
                  curve: Array<[x: number, y: number]> };                            // the whiteEv this edit calls Whites 0
  mixer:        Record<Band, { hue: number; saturation: number; luminance: number }>; // −100…+100
  detail:       { sharpen: { amount: number; radius: number; iterations: number };
                  denoise: { luminance: number; chroma: number } };
  // After V1, optional: lens?, geometry?, masks? — absent means off (§15)
}
```

## 11. Interface

**Stack.** SvelteKit (static adapter) + Tailwind + shadcn-svelte (Bits UI primitives). Immich's web app is SvelteKit + Tailwind, and shadcn/ui itself is React-only. Carbon supplies density and spacing; Apple's HIG supplies touch behaviour.

**Design target.** iPhone in Safari. iPad and desktop get the wide layout, but every interaction is designed for a thumb first.

**Neutral surfaces.** Pure, unbiased greys — a tinted UI shifts perceived white balance. Carbon Gray 100 steps: `#161616` background, `#262626` panel, `#393939` raised, text `#f4f4f4` / `#c6c6c6`. One low-chroma accent for focus and active slider fill; saturated red and blue are reserved for clip overlays.

**iPhone (primary).** Image on top; compact histogram as a toggleable overlay; a chip row of tools above a bottom sheet showing one section at a time. While a slider is dragged, everything except that slider and its value fades out.

**iPad and desktop (≥ 1,024 px).** Canvas on the left; 320 px panel on the right. Histogram pinned at the top of the panel. Accordion sections in pipeline order: White balance · Light · Tone curve · Color mixer · Detail. Export pinned at the bottom. Carbon's 32 px small row height; spacing 4 / 8 / 16 / 24 px.

**Safari on iOS.**

- Layout in `dvh` units with `env(safe-area-inset-*)` padding, so the bottom sheet clears the home indicator and Safari's toolbar.
- `touch-action: none` on the canvas, and `gesturestart` / `gesturechange` prevented there, so pinches zoom the photo rather than the page — Safari ignores `user-scalable=no`.
- `-webkit-touch-callout: none` and `user-select: none` on the editor, so press-and-hold shows the as-shot view instead of the callout menu.
- Controls stay 24 px clear of the left screen edge, away from Safari's back swipe; drafts cover an accidental swipe anyway.
- `theme-color` `#161616`, so Safari's toolbar matches the editor.

**Slider.**

- Visual track 4 px; hit area 44 pt tall on touch (HIG minimum), 32 px with a pointer; thumb 28 pt on touch, 16 px with a pointer.
- Relative drag: touching the row doesn't jump the value; movement adjusts from where it is.
- Scrub gain: moving the finger up off the row lowers gain — full, ½, ¼ — for fine work without a separate precision mode.
- Bipolar sliders (Temperature, Tint, Exposure, mixer) fill from the default tick; Temperature and Tint tracks carry low-chroma gradients.
- Default detent: iOS Safari exposes no haptics to web pages, so the detent is a magnetic zone (± 2 % of the range) where the thumb snaps and the value label flashes. Android adds a short `navigator.vibrate`.
- Double-tap resets; tap the value to type; arrow keys step, Shift ×10.
- Press and hold the image to see the as-shot rendering.
- Floating overlays use backdrop blur (HIG *Materials*), switched off during drags — `backdrop-filter` over a live WebGPU canvas costs phones frames.

**Keyboard (iPad keyboard, desktop).** `\` before/after · `J` clip overlays · `Ctrl/⌘ Z`, `Shift Ctrl/⌘ Z` · `E` export · `Z` loupe.

```
web/src/
  service-worker.ts              app shell + proxy cache; Android share target
  routes/edit/[key]/+page.svelte
  routes/pick/+page.svelte       candidate picker and search
  routes/+page.svelte            Recent
  lib/gpu/     device.ts · proxy.ts · develop.ts · view.ts · histogram.ts · uniforms.ts
  lib/color/   WASM bindings to crates/color (WB ↔ temperature, matrices, spline LUT)
  lib/state/   params.svelte.ts · history.ts · drafts.ts (IndexedDB) · sync.ts (ETag PUT)
  lib/ui/      Slider · Histogram · CurveEditor · ColorMixer · BottomSheet · Loupe · ExportSheet
```

## 12. Repository and deployment

```
sidecar/
  LICENSE                 Apache-2.0
  NOTICE                  headline credits (Apache-2.0 convention)
  THIRD_PARTY_NOTICES.md  every dependency and ported file, generated in CI
  CLAUDE.md               source rules for implementing agents (§12)
  deny.toml               cargo-deny licence allowlist
  about.toml              cargo-about notice generation
  Dockerfile              one image, two commands: sidecar api · sidecar worker
  .github/workflows/      CI, licence checks, image build → ghcr.io
  third_party/libraw/     pinned LibRaw source (CDDL-1.0), git submodule
  shaders/            single WGSL source for browser and worker
    common/           transfer functions, OKLab, luminance
    develop/          wb · exposure · highlight_blend · matrix · gamut · tone · mixer · output
    ingest/           normalize · hotpixel · rcd · markesteijn · hl_reconstruct · downsample
    detail/           atrous_denoise · rl_deconvolve
    client/           develop · view · histogram
  crates/
    sidecar/          the one binary: `sidecar api`, `sidecar worker`, `sidecar dev`
    api/              axum · open/resolve · XMP writer · history · jobs · SSE · Immich client · lensfun sync
    worker/           caches · ingest · loupe · export · preview · outbox
    engine/           wgpu device and passes: ingest, develop, detail, lens, focus
    shaders/          WGSL composition (`//! requires:`) for the worker and the browser
    raw/              LibRaw wrapper, developed files, JPEG XL linear DNGs, lens tables, lensfun, AF point
    libraw-sys/       bindings to third_party/libraw, adapted from libraw-rs (MIT OR Apache-2.0)
    color/            WB/temperature, DNG matrix interpolation, tone curve, lens model, uniform packing
    color-wasm/       `color` for the browser, as wasm32
    xmp/              sc:Sidecar schema on top of xmp_toolkit, and the edit history
    template/         storage-template rendering and date/name lookup
    ml/               sidecar-ml: subject, face and hair/skin models on burn (the `-ml` image)
    devtools/         sidecar-dev: shader composition, parity fixtures, lens checks
  web/                SvelteKit app
  ios/                Edit in Sidecar.shortcut + setup notes
  deploy/             compose.yaml · Caddyfile · sidecar.toml
  testdata/           one RAW per sensor family + reference renders
```

**Parity checks.** Safari's WebGPU runs on Metal and Chromium's on Dawn, and the phone is the one that matters.

- CI: each test RAW × a fixed parameter set renders through the worker (wgpu) and headless Chromium WebGPU (Playwright). Fail if mean ΔE2000 > 0.5 or p99 > 2.
- On the iPhone: `/debug/parity` renders the same fixtures in Safari, reads back `developedTex`, and posts the pixels to the api, which compares them against the worker's renders with the same thresholds. Run before every release.
- Bit-exact output isn't achievable across GPU stacks (fused multiply-add, rounding), so perceptual equality is the contract.

```yaml
services:
  sidecar-api:
    image: ghcr.io/<you>/sidecar:1.0.0
    command: ["sidecar", "api"]
    user: "1000:1000"                                  # owner of the library files
    environment:
      IMMICH_URL: http://immich-server:2283
      WORKER_URL: http://sidecar-worker:8081
    volumes:
      - ./sidecar.toml:/etc/sidecar.toml:ro
      - /srv/nvme/photos:/library/external:rw          # XMP writer only
      - /srv/nvme/immich/library/<storage label>:/library/internal:rw
      - sidecar-cache:/cache:ro
      - sidecar-state:/state
    secrets: [immich_admin_key]                        # job.create only
    networks: [immich, sidecar]

  sidecar-worker:
    image: ghcr.io/<you>/sidecar:1.0.0                  # same image, so api and worker versions can't drift
    command: ["sidecar", "worker"]
    read_only: true
    cap_drop: [ALL]
    environment:
      NVIDIA_DRIVER_CAPABILITIES: compute,utility,graphics   # graphics = Vulkan
      VRAM_BUDGET_MB: "8192"
    volumes:
      - /srv/nvme/photos:/library/external:ro
      - /srv/nvme/immich/library/<storage label>:/library/internal:ro
      - sidecar-cache:/cache
    tmpfs: [/tmp]
    deploy:
      resources:
        reservations:
          devices: [{ driver: nvidia, count: 1, capabilities: [gpu] }]
    networks: [sidecar]                                # no route to Immich or the internet

volumes:
  sidecar-cache: {}
  sidecar-state: {}

secrets:
  immich_admin_key: { file: ./immich_admin_key }

networks:
  immich: { external: true }
  sidecar: { internal: true }
```

```caddyfile
edit.example.com {
    reverse_proxy sidecar-api:8080
    # text/event-stream responses are flushed immediately.
    # Proxies arrive already compressed, so no `encode` directive.
}
```

### Publishing and licences

Sidecar is published as an Apache-2.0 Docker image on GitHub Container Registry, and reuses existing code wherever it fits, with credit. Apache-2.0 is as permissive as MIT and adds an explicit patent grant and the `NOTICE`-file convention for credits; MIT would also work. This is the common reading of these licences, not legal advice.

**What publishing changes.** A published image is distribution, so every licence of every component inside it applies. Everything below allows that under Apache-2.0; the obligations are notices, credit, and pointing to source for the two file-level copyleft pieces (LibRaw, and the X-Trans port derived from it). NVIDIA's driver libraries are never in the image — the container toolkit mounts them from the host at run time.

**Reused code**

| Stage | Project | Licence | How Sidecar uses it |
|---|---|---|---|
| RAW decoding | LibRaw (LibRaw LLC; builds on Dave Coffin's dcraw) | LGPL-2.1 or CDDL-1.0 — Sidecar takes CDDL-1.0 | Pinned source in `third_party/libraw`, statically linked through bindings adapted from `libraw-rs`. CDDL is file-level: LibRaw's files stay CDDL and their source is linked from the notices; Sidecar's own code is unaffected. LGPL-2.1 would oblige letting users relink a static binary |
| Bayer demosaic (RCD) | vkdt `demosaic/rcd_conv.comp`, `rcd_fill.comp` (Johannes Hanika) | BSD-2-Clause | Ported GLSL → WGSL. None of the vkdt files in `demosaic`, `hilite`, `denoise`, `deconv` or `filmcurv` carries a GPL marking |
| X-Trans demosaic (Markesteijn) | LibRaw `xtrans_demosaic.cpp` | CDDL-1.0 | Ported to WGSL; the ported file stays CDDL-1.0 with LibRaw's notice |
| Highlight reconstruction | vkdt `hilite` | BSD-2-Clause | Ported |
| Denoise, deconvolution | vkdt `denoise`, `deconv`; Starck & Murtagh (à-trous), Richardson 1972, Lucy 1974 | BSD-2-Clause; papers | Ported, checked against the papers |
| Tone map reference | vkdt `filmcurv` | BSD-2-Clause | Reference for the sigmoid |
| Gamut compression | ACES Reference Gamut Compression, `aces-aswf/aces-core`; also in OpenColorIO | Apache-2.0; BSD-3-Clause | Formulas and constants |
| CPU colour maths | `palette` crate; `colour-science` for test values | MIT OR Apache-2.0; BSD-3-Clause | OKLab and conversions in `crates/color`; temperature and tint checked against colour-science |
| XMP | `xmp_toolkit` (Adobe) on the XMP Toolkit SDK | MIT OR Apache-2.0; BSD-3-Clause | All sidecar reading and writing |
| Metadata copy | ExifTool (Phil Harvey) | Perl terms: Artistic or GPL | Separate process in the image; its licence ships alongside |
| JPEG, TIFF | `mozjpeg` crate; `tiff` crate | IJG; MIT | Encoding. The IJG licence requires the line "this software is based in part on the work of the Independent JPEG Group" in the notices |
| ICC profiles | Compact-ICC-Profiles (Clinton Ingram) | CC0-1.0 | Embedded in exports |
| Dither | Free blue-noise textures (Christoph Peters) | CC0-1.0 | View pass and 8-bit export |
| Struct layout | `encase`; `webgpu-utils` (Gregg Tavares) | MIT-0; MIT | Uniform buffer layouts |
| Parity tests | `deltae` crate; Playwright | MIT; Apache-2.0 | ΔE2000 comparisons |
| Server, GPU, web | axum, wgpu, naga, SvelteKit, Tailwind, shadcn-svelte, Bits UI | MIT or Apache-2.0 | Framework code |
| Lens correction (after V1) | lensfun models and database | LGPL-3.0 library; CC BY-SA 3.0 database | The distortion, CA and vignetting models are reimplemented from lensfun's documentation, so the library isn't linked. The database is downloaded into `/cache` on first use rather than shipped in the image, and credited |

**Porting vkdt shaders.** vkdt's modules are GLSL compute shaders, the closest existing match to WGSL. naga's GLSL front end produces a first WGSL draft once `#include`s are expanded; the draft is hand-cleaned, and the WGSL becomes the source of truth. Each port records its origin (next paragraph).

**Not used**, because their licences would bind the published image:

- darktable, RawTherapee and the original RCD repository — GPL-3.0. Reading their documentation is fine; copying or translating their code is not.
- `rawler` and `rawloader` — LGPL-2.1 Rust crates. Statically linked Rust makes LGPL's relinking requirement impractical.
- Immich's source — AGPL-3.0. Sidecar talks to its HTTP API and copies none of its code.
- AMaZE — only available as GPL-3.0 code, and left out.

**Credit.**

- `NOTICE` names the projects Sidecar builds on — vkdt, LibRaw and dcraw, Adobe's XMP Toolkit, ExifTool, mozjpeg and the IJG, ACES, Christoph Peters, Clinton Ingram, lensfun.
- `THIRD_PARTY_NOTICES.md` is generated in CI by `cargo-about` for Rust crates and a licence report for npm packages, plus a hand-kept section for ported shaders and data files. It's copied into the image at `/usr/share/doc/sidecar/` and shown in the app under *Settings → About → Credits*.
- Every ported file starts with its origin:

  ```wgsl
  // SPDX-License-Identifier: BSD-2-Clause
  // Ported from vkdt src/pipe/modules/demosaic/rcd_fill.comp @ <commit>
  // Copyright 2019 Johannes Hanika. WGSL port copyright Sidecar contributors.
  ```

- The README's *Built on* section links each project.

**Enforcement in CI**, so nothing incompatible slips in:

- `cargo-deny` allows only Apache-2.0, MIT, MIT-0, BSD-2-Clause, BSD-3-Clause, ISC, Zlib, Unicode-3.0, IJG and CC0-1.0, with CDDL-1.0 allowed for `libraw-sys` alone.
- An npm licence check applies the same allowlist to the web app.
- A script requires an `SPDX-License-Identifier` header on every file in `shaders/` and fails on GPL, LGPL or AGPL identifiers.
- Image builds attach an SBOM and provenance attestation, and label the image `org.opencontainers.image.licenses=Apache-2.0`.
- `CLAUDE.md` lists the allowed references and the projects not to copy from, so an implementing agent follows the same rules.

## 13. Build order

Each milestone ends usable on an iPhone; the riskiest integration comes first.

1. **Parity harness** — licence tooling in CI (`cargo-deny`, SPDX header check, `CLAUDE.md` rules) from the first commit, then `shaders/`, `crates/color`, wgpu headless, the Playwright runner, and `/debug/parity` on the phone render one synthetic linear image. Proves the single-source WGSL contract on Metal, Dawn and Vulkan before anything depends on it.
2. **Linear preview** — LibRaw with its built-in CPU demosaic (DHT) → proxy endpoint → WB, exposure, matrix and output transform in Safari. A RAW opens on the iPhone from a typed URL.
3. **Identity and sidecars** — `/open`, the iOS Shortcut, template-based resolution, Recent, XMP read–modify–write with ETags, drafts.
4. **Tone and color** — tone map and curve editor, OKLCh mixer, histogram, clip overlays, the iPhone layout, Safari gesture handling, slider behaviour.
5. **GPU ingest** — port vkdt's RCD and `hilite` and LibRaw's Markesteijn, add hot pixels; retire the CPU demosaic.
6. **Export** — jobs, SSE, tiling, encoding, metadata, versions, outbox, Immich upload and stacking with the JPEG as primary.
7. **Detail** — session buffers, loupe, denoise, deconvolution.
8. **Other entry points** — Android share target, desktop link paste, device-loss hardening.
9. **Release** — image build to `ghcr.io` with SBOM and notices, README *Built on* section, *Credits* screen.

After V1: lens correction, then crop and straighten, then local masks (§15).

## 14. Decisions

| Question | Decision | What it changed |
|---|---|---|
| iOS at launch? | Yes — the primary platform | Shortcut entry in milestone 3; Safari is the design target; on-device parity check; magnetic detent instead of haptics |
| Managed or external library? | External, with a strict date layout; the internal library uses the same template | Resolution by capture date and filename; draft 1's hash index dropped; external sidecars never move; internal ones are made known to Immich with an admin key |
| Re-export? | Keep every version | Version numbers, `-vN` file names, `sc:Exports` history, identical re-exports return the existing version |
| Stack primary? | The edited JPEG | The newest JPEG export becomes primary; TIFF versions join without taking it |
| Crop, lens correction, local masks? | Yes, after V1 | V1 ships the hooks in §15 so each arrives without migrating stored edits |
| Admin API key for internal-library sidecars? | Yes, provided | `job.create`-only key as a Docker secret; batched Sidecar *Missing* runs; a warning if the deprecated endpoint disappears |
| Publish? | Yes — Docker image on GitHub, permissive licence, reuse and credit existing code | Apache-2.0; one image for api and worker; vkdt, LibRaw, `xmp_toolkit` and others reused with notices; GPL and LGPL sources excluded and checked in CI |
| Immich version? | 3.1.0 | API calls taken from its source: hex checksum search, no `deviceAssetId` on upload, primary-based stack merging, the `sc:Sidecar` wrapper, internal-library sidecar discovery |

**Confirmed:**

- The Immich iOS app shares the original asset, pulled from the server — original bytes, original name, readable Date Taken.
- *Generate Hash* handles the largest RAW from the share sheet, and matches `sha1sum` on the server.
- Folder month names are English (`09.September`).
- Immich 3.1.0 API behaviour, from its source: checksum search, upload fields and duplicates, stack merging, sidecar discovery and writes.

**Still open:** nothing blocks implementation.

## 15. After V1: lens correction, crop, local masks

**Order.** Lens correction first, then crop and straighten, then masks. Lens correction redefines the image's geometry, and crop rectangles and mask shapes are stored in that geometry. Shipping it first means no stored crop or mask ever has to be migrated.

| Feature | Preview (phone) | Export (server) | Stored as |
|---|---|---|---|
| Lens correction — distortion, lateral CA, vignetting | A lens pass over the proxy, run when a lens setting changes: bicubic remap per channel and a vignetting gain, into the texture develop reads | Same WGSL, after denoise and before develop | `sc:Lens` — profile, per-correction amounts |
| Crop, straighten, keystone, turns | A projective map in the view pass; the histogram counts the output's pixels through it | The warp pass after develop, before resampling to `longEdge` | `sc:Geometry` — turns, mirror, angle, keystone, aspect, and the crop in the straightened plane |
| Mask layers — models' masks, brush (gradients and sky still to come) | Second bind group in the develop pass: each module's inputs blend by mask weight | Same shaders at full resolution | `sc:Masks` — the mask's name, strokes, and adjustments named like the global ones |

**Lens correction.** Profiles come from the file when it carries one — DNG opcodes (iPhone ProRAW and Lightroom's DNGs), Sony's and Fujifilm's own tables, each verified as the build log describes — otherwise from the lensfun database, matched on the lens ExifTool identifies. The models are reimplemented from lensfun's documentation, and the database (CC BY-SA 3.0) is downloaded on first use, not shipped (§12). Absent `sc:Lens` means off, so photos edited in V1 render unchanged; new edits default to on.

**Masks.**

- Weights: gradients (still to come) are analytic in the shader. Brush strokes — normalized points, radius, feather, flow — are rasterized by shared WGSL render passes into `r8unorm` masks, at proxy size on the phone and up to 4,096 px on the server (see the build log).
- Model masks (subject, skin, hair, clothes, depth; sky is still to come) run on the worker's GPU through `sidecar-ml`. A layer's `range` keeps a band of its mask's values — on depth, a band of distance. The sidecar stores only the mask's name; the raster lives in `/cache` and is recomputed if the cache is gone, so nothing binary lands beside the RAW. See the build log.
- Blending: `ev = global.ev + Σ wₖ · maskₖ.ev`, and likewise for WB multipliers, contrast, saturation and mixer values; the tone map's parameters blend toward the layer's, and each layer's curve applies after the global one by its weight. Up to 8 layers per photo, in a fixed-size uniform array.
- Display: the selected mask shows as desaturation outside it, which keeps red and blue reserved for clipping.

**Hooks that ship in V1** so all three arrive without migrations:

1. The view pass already applies a `geometry` matrix; crop and straighten compose into it.
2. Develop modules take values as arguments and never read the uniform buffer directly.
3. Loupe coordinates are normalized to the oriented image, not proxy pixels.
4. The XMP writer preserves unknown `sc:` elements, and a newer `ProcessVersion` opens read-only.
5. `EditParams` treats `lens`, `crop` and `masks` as optional, with absent meaning off.
