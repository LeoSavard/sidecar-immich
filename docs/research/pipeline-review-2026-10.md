# Pipeline review — decoding, churn, duplication, and the road to mobile

*2026-10-26. A read of the development pipeline from file to proxy, against LibRaw's own source (the pinned
submodule, `b93f6e4`), the pull-request history (#1–#41) and published practice. Nothing here was measured on a
real photo: this session had no GPU, no Docker daemon and no `testdata/cache/`, so every finding is from reading
the code. Each one says how to confirm it.*

The short version:

- **Three places in the decode can render a file wrongly without an error**, all in how the LibRaw shim reads
  levels and colour: a per-position black pattern is averaged, a DNG's `CameraCalibration` and `AnalogBalance`
  are dropped, and the row stride ignored `raw_pitch` (fixed on this branch).
- **The X-Trans preview and export disagree in blown highlights.** The proxy gets the GPU highlight
  reconstruction and the export doesn't.
- **Developed files trust a substring for their colour space.** Anything that isn't "P3" is decoded as sRGB:
  Adobe RGB camera JPEGs, ProPhoto TIFFs, Rec.2020.
- **The biggest PR cluster (ten of forty-one) is the top end of the tone range.** White level, clip threshold,
  baseline exposure and shoulder were each fixed in a different place, because "what 1.0 means" is decided
  separately in each of three decode paths.
- **Mobile already works when a phone is pointed at a server.** Local decoding is reachable because the ingest
  is already WGSL. It is a staged job, not a rewrite, and it can be licence-clean.

## 1. Findings at a glance

| # | Finding | Where | Effect | Changes pixels when fixed | Effort |
|---|---|---|---|---|---|
| D1 | Black-level pattern averaged instead of mapped per CFA position | `shim.cpp` `sc_get_mosaic` | Shadow colour cast on DNGs with a 2×2 `BlackLevelRepeatDim` whose values differ | Yes, affected files only | S |
| D2 | DNG `CameraCalibration` / `AnalogBalance` dropped | `shim.cpp` `sc_get_info`, `raw/src/lib.rs` `camera_color` | Colour cast on camera-native DNGs whose calibration isn't identity | Yes, affected files only | S |
| D3 | Row stride from `raw_width`, not `raw_pitch` | `shim.cpp` | Sheared or garbage mosaic wherever a decoder pads rows | **Fixed here**; identical for every file that worked | — |
| D4 | No sanity check on `white > black` | `sc_get_mosaic`, `ingest.rs` | Division by ≤0, all-black frame instead of an error or fallback | No | XS |
| D5 | X-Trans: proxy reconstructs highlights, export clips them | `ingest.rs` vs `export.rs` `full_source` | Preview ≠ export in blown areas, breaking ARCHITECTURE's "one rule" | Yes (X-Trans exports) | M |
| D6 | Observed white from a single maximum sample | `ingest.rs` `white_obs`, `lib.rs` `saturation_threshold` | One hot pixel defeats the "saturates below declared" heuristic | Rarely | S |
| D7 | Three clip thresholds for one concept | `lib.rs` (99 % / 98 %), `ingest.rs` (98.5 % / 98 %), `linear_dng.rs` (99 %) | Clip coverage, and so highlight blending, differs by decode path | Marginally | S |
| D8 | Linear DNG reads one black and one white for all channels; ignores `BlackLevelDeltaH/V`, `DefaultCrop`, `ForwardMatrix`, `CameraCalibration` | `linear_dng.rs` | Small casts; uncropped edges | Yes | M |
| D9 | No colour matrix → `panic!` | `color/src/camera.rs` | Worker task fails; in the browser the wasm packer traps | **Fixed here** | — |
| D10 | ICC detection is `contains("P3")`, and no ICC means sRGB | `decoded.rs` `SourceSpace::from_profile` | Adobe RGB / ProPhoto / Rec.2020 sources desaturated, with the wrong tone curve | Yes, those files | M |
| D11 | Camera Adobe RGB JPEGs carry no ICC, only `InteropIndex = R03` | `decoded.rs` `exif` | Same as D10, for the most common case of it | Yes | XS |
| D12 | CMYK/YCCK JPEG, float TIFF and PNM `maxval` ≠ 255/65535 are unchecked | `decoded.rs` | Wrong colours or 64× dark instead of a clear refusal | No (refuse) / yes (support) | S |
| D13 | ProRAW `ProfileGainTableMap` not read | `linear_dng.rs` | ProRAW looks flatter than Apple's rendering (DNG 1.6 local tone map) | Optional stage | L |

The two fixes on this branch are D3 and D9. The rest are proposals. Every pixel-changing fix needs an
`INGEST_GENERATION` bump (`crates/worker/src/lib.rs`), a file that shows the problem copied under
`testdata/cache/library/`, and a parity run.

## 2. Where decoding can go wrong

### 2.1 The LibRaw shim (`crates/libraw-sys/shim/shim.cpp`)

**D1 — the black pattern.** `sc_get_mosaic` adds the *mean* of `cblack[6 …]` to every channel. LibRaw's
`cblack` layout is four per-channel values, then the pattern's height and width at `[4]` and `[5]`, then the
pattern itself (LibRaw's struct docs, as quoted on its forum). LibRaw folds a pattern of 2×2 or smaller into the
per-channel values in `adjust_bl()` (`src/utils/utils_libraw.cpp:468`), which runs inside `raw2image` and
`dcraw_process` but **not** after `unpack()`. That is the state the shim reads in. Its maintainer also says plainly
that blacks are not subtracted by `unpack()`. For a DNG with `BlackLevelRepeatDim = 2 2` and four different
values, the average is wrong for every channel by half the spread. The fix copies `adjust_bl`'s fold: for
`filters > 1000` and pattern dimensions ≤ 2, add `cblack[6 + (r % pr) * pc + c % pc]` to the colour at `(r, c)`.
The shader's `black` is already indexed by colour 0…3 with G2 = 3, so the mapping is exact. A 6×6 X-Trans
pattern can't be expressed in four numbers. If one turns up with uneven values, the honest options are a
per-position black texture or the LibRaw fallback.

**D2 — calibration.** The shim copies `dng_color[n].colormatrix` and `camera_color` uses it as XYZ→camera.
LibRaw itself builds `cam_xyz = AB · CC · CM` (`src/metadata/tiff.cpp:1756–1760`), which is what the DNG
specification defines. The per-unit `CameraCalibration` and the hardware `AnalogBalance` are both stored apart
from `ColorMatrix` so that a matrix can be swapped per model without losing them (colour-hdri's DNG notes). Both
are available (`dng_color[n].calibration` and `dng_levels.analogbalance`) and both are dropped. Adobe DNG
Converter output usually has identity CC, so a Nikon or Sony converted to DNG is unaffected. Camera-native DNGs
(Leica, Pentax, Ricoh, phones) are the exposure. Fix: pass CC and AB through `sc_info` and multiply in
`camera_color`. `linear_dng::color` needs the same (tags 50723/50724 and 50727). While there, the specification's
preferred path when `ForwardMatrix1/2` is present is the forward matrix (camera → XYZ D50) rather than the
inverted `ColorMatrix` (colour-hdri implements both). That is a rendering change and deserves its own parity case.

**D3 — row stride (fixed).** The mosaic was copied with `raw_width` as the stride. LibRaw's own `raw2image`
uses `raw_pitch / 2`, and `raw_pitch` is set separately by each decoder: RawSpeed returns its own pitch, and the
DNG SDK glue and several loaders set it independently (`src/decoders/unpack.cpp`, `src/integration/*`). For a
plain Bayer file `unpack()` sets `raw_pitch = raw_width * 2` (`unpack.cpp:341`), so this branch's change is
identical for every file that worked. It also refuses a crop that runs past the row instead of reading past the
buffer.

**D4 — levels sanity.** Nothing checks that `maximum > black`. `normalize.wgsl` divides by `white - black[c]`,
so a zero or bogus `maximum` gives a black frame and no error. ProRAW's "black" (#34) was a sibling of this
failure in `linear_dng.rs`. A guard in `RawFile::mosaic()` that returns `Err` turns it into the LibRaw fallback,
which already exists. The same function should refuse `n == 0` before `from_raw_parts`.

**Smaller shim notes.** `period` is 2 for any `filters ≠ 9`, so `filters == 1` (Leaf's 16×16) and four-colour
CYGM sensors take the Bayer path with the wrong pattern. `phase()` would reject most of these, but saying so
explicitly is cheaper than finding out. `dng_color` carries only two illuminants; DNG 1.6's third calibration
illuminant is ignored. That is fine for now and worth a comment.

### 2.2 GPU ingest (`crates/engine/src/ingest.rs`, `shaders/ingest/`)

**D5 — X-Trans preview vs export.** `Ingest::proxy` runs normalize → highlight pyramid → 3×3 bin for X-Trans.
`export::full_source` sends X-Trans to `develop_linear(false)`, which is LibRaw's Markesteijn with
`highlight = 0` (clip). The proxy and the export therefore differ in exactly the region highlight reconstruction
exists for. ROADMAP calls X-Trans export "slower, not broken", and that is true of speed but not of pixels.
Cheapest honest fix until the Markesteijn port exists: run the shared front half (normalize + highlight pyramid)
on the GPU, read the reconstructed CFA back, and hand it to LibRaw for the demosaic (`imgdata.rawdata.raw_image`
can be replaced before `dcraw_process`). Alternatively, gate highlight reconstruction off for X-Trans proxies so
both sides clip. Either way, add X-Trans to the parity set (`DSCF7611.RAF` is named in ROADMAP).

**D6 / D7 — what "clipped" means.** Three heuristics answer the same question:

| Path | Threshold |
|---|---|
| LibRaw RGB (`LinearImage::rgba`) | 99 % of declared, or 98 % of the observed maximum when that is ≥ 50 % |
| GPU mosaic (`Ingest::demosaic`) | 98.5 %, or 98 % of observed (capped at 98.5 %) when observed > 50 % |
| Linear DNG (`linear_dng::decode`) | 99 %, fixed |

The observed maximum is one sample over the whole frame, so a single stuck pixel at full scale pins it to
declared. LibRaw's own equivalent, `adjust_maximum_thr`, keys off `data_maximum` the same way and has the same
weakness. The more robust standard is a **per-channel histogram of the top of the range**: a real saturation
level shows as a spike there. Take the lowest bin above ~50 % of declared that holds a spike, per channel. One
`sidecar_raw::levels::clip_threshold(declared, histogram) -> [f32; 4]` used by all three paths retires D7, and
its result belongs in `Meta` so the browser can see what the server decided. RawTherapee and darktable ship
per-camera white-level tables instead, but both are GPL/LGPL data and off limits by CLAUDE.md. The histogram
needs no table.

**Memory.** `demosaic()` records every pass into one encoder, so every intermediate lives until the submit:
the R16 mosaic, R32F normalised, the RGBA32F highlight pyramid and its assembled copies, the R32F CFA, four
R32F RCD planes, an RGBA32F `rb` and the RGBA16F output. That is **about 60 bytes per sensor pixel**: ~1.4 GB of
VRAM for 24 MP, ~2.7 GB for 45 MP, ~3.7 GB for 61 MP. Three changes cut it without touching the algorithm:
`rb` only carries two channels (`rg32float`, −8 B/px); submitting after the highlight stage lets the pyramid go
(−~11 B/px); and **proxies don't need a full-resolution demosaic at all** (§6). This matters on the server
today (integrated GPUs, a big export beside a loupe session) and decides whether ingest can run on a phone.

**Hot pixels.** `normalize.wgsl` replaces a sample more than 4× its four same-colour neighbours. That is the
standard shape, as in darktable's hot-pixel module. Two notes: Bayer greens have
nearer same-colour neighbours on the diagonal than at ±2, so green hot pixels are judged against farther
samples than needed; and star fields will lose faint stars. Both are acceptable defaults. A comment saying so
would save the next person the investigation.

### 2.3 Linear DNG (`crates/raw/src/linear_dng.rs`)

The walker is careful about bounds and the ProRAW fixes (#34) are right: linearize before subtracting black, and
the stream's bit depth is not `BitsPerSample`. What it still assumes:

- **One black, one white.** `first(m, 50714)` and `first(m, 50717)`. The specification allows a black per
  sample and per repeat position (`BlackLevel` has `SamplesPerPixel × repeat` values), deltas per row and per
  column (`BlackLevelDeltaV`/`H`, 50715/50716), and a white per sample. Read the whole vectors; index black by
  channel.
- **No crop.** `DefaultCropOrigin`/`DefaultCropSize` (50719/50720) and `ActiveArea` (50829) are ignored, so a
  merge with a ragged edge keeps it.
- **Colour.** Same as D2: no `CameraCalibration`, `AnalogBalance` or `ForwardMatrix`. The empty-calibration case
  is D9: `color()` can return no calibrations at all, and `CameraColor::xyz_to_camera_at` used to panic on that.
  It now falls back to sRGB primaries, as the LibRaw path already did for an unknown camera.
- **ProRAW's local tone map (D13).** DNG 1.6's `ProfileGainTableMap` (0xCD2D) is the data Apple stores to
  reproduce its own local tone mapping, and ProRAW also carries semantic masks for people, skin and sky (Apple,
  WWDC 2021 session 10160). Applying it is optional by design and can be scaled or turned off (Sandofsky,
  *Understanding ProRAW*). No source found says which open converters apply it; in the pixls.us thread, darktable's
  contributors were still deciding where it belongs. Worth reading and exposing as a 0–100 % control later, not urgent.
- **Memory.** `decode` reads the whole file (`std::fs::read`) to decode tiles. A 306 MP merge is a lot of
  memory to hold just to find tile offsets. A `memmap2` (MIT/Apache) map, or seeking per tile, keeps the bounded
  footprint the module's doc comment promises.

### 2.4 Developed files (`crates/raw/src/decoded.rs`)

- **D10/D11 — colour spaces.** `from_profile` returns P3 when the profile description contains "P3" and sRGB
  for everything else, with the sRGB transfer curve always. That decodes Adobe RGB (gamma 2.2, wider green),
  ProPhoto (gamma 1.8, very wide) and Rec.2020 as if they were sRGB. Two levels of fix:
  1. **Cheap:** recognise Adobe RGB and ProPhoto by name, and read EXIF `ColorSpace = 0xFFFF` together with
     `InteropIndex = "R03"`. That pair is how a camera set to Adobe RGB marks a JPEG with *no* embedded profile
     (the DCF option-file rule). These are two primaries matrices and two transfer curves; `SourceSpace` grows
     two variants.
  2. **Right:** parse the embedded ICC and build the transform from it. **moxcms** (pure Rust, BSD-3-Clause OR
     Apache-2.0) and qcms (MIT) both qualify under `deny.toml`. V2.md's survey already recommends this shape
     for display: bake the transform once into a LUT or matrix, never per pixel.
- **D12 — refuse what isn't handled.** A four-component JPEG goes through `to_linear_rgba` as RGB plus alpha
  (whether zune-jpeg converts CMYK for you depends on its output colour-space setting; pin it). TIFF `F32` falls
  into "isn't supported", which is honest; float TIFFs from HDR merges are worth supporting since they are already
  linear. PNM `maxval` other than 255/65535 indexes the 65,536-entry LUT unscaled. Scale by `maxval` or refuse.
- **HEIC.** Decoding through an external program is the right licence boundary. Two loose ends: 10-bit HEIC
  with a PQ or HLG transfer is decoded as sRGB (iPhone stills are 8-bit SDR plus a gain map, so this is about
  other cameras); and the template is split on whitespace *before* substitution, which correctly keeps paths
  with spaces intact. Keep it that way, and a test would pin it.
- **Capture time.** `captured_unix` reads `DateTimeOriginal` as UTC. That matches LibRaw, so the paths sort
  together. `OffsetTimeOriginal` exists when a real instant is ever needed.

### 2.5 What is done well and should stay

`INGEST_GENERATION` separate from `ProcessVersion` is exactly right: proxies are a cache of a decode, not an
edit. Temp-then-rename for every cached file and a per-asset decode guard are both correct. Orientation is held
in one numbering (LibRaw's flip bits), with `pending_flip` naming the HEIC exception. The mirrored orientations
were fixed in generation 3. The linear-DNG walker bounds every read. These are the habits the fixes above should
copy.

## 3. What generated the most pull requests

Grouping #1–#41 by what they fixed or built:

| Theme | PRs | Count |
|---|---|---|
| **The top of the tone range**: white level, clipping, baseline exposure, highlights, shoulder | [#3](https://github.com/LeoSavard/sidecar-immich/pull/3), [#21](https://github.com/LeoSavard/sidecar-immich/pull/21), [#23](https://github.com/LeoSavard/sidecar-immich/pull/23), [#26](https://github.com/LeoSavard/sidecar-immich/pull/26), [#33](https://github.com/LeoSavard/sidecar-immich/pull/33), [#34](https://github.com/LeoSavard/sidecar-immich/pull/34), [#35](https://github.com/LeoSavard/sidecar-immich/pull/35), [#36](https://github.com/LeoSavard/sidecar-immich/pull/36), [#41](https://github.com/LeoSavard/sidecar-immich/pull/41) (+ the tone parts of [#24](https://github.com/LeoSavard/sidecar-immich/pull/24)) | 9–10 |
| Film (B&W, grain, developers, stocks, PROVIA) | [#10](https://github.com/LeoSavard/sidecar-immich/pull/10), [#16](https://github.com/LeoSavard/sidecar-immich/pull/16)–[#20](https://github.com/LeoSavard/sidecar-immich/pull/20), [#24](https://github.com/LeoSavard/sidecar-immich/pull/24) | 7 |
| Depth | [#1](https://github.com/LeoSavard/sidecar-immich/pull/1), [#13](https://github.com/LeoSavard/sidecar-immich/pull/13), [#15](https://github.com/LeoSavard/sidecar-immich/pull/15), [#22](https://github.com/LeoSavard/sidecar-immich/pull/22) | 4 |
| Editor structure and generated types | [#6](https://github.com/LeoSavard/sidecar-immich/pull/6), [#8](https://github.com/LeoSavard/sidecar-immich/pull/8), [#9](https://github.com/LeoSavard/sidecar-immich/pull/9), [#12](https://github.com/LeoSavard/sidecar-immich/pull/12), [#29](https://github.com/LeoSavard/sidecar-immich/pull/29) | 5 |
| Library, open and intake | [#7](https://github.com/LeoSavard/sidecar-immich/pull/7), [#28](https://github.com/LeoSavard/sidecar-immich/pull/28), [#32](https://github.com/LeoSavard/sidecar-immich/pull/32), [#39](https://github.com/LeoSavard/sidecar-immich/pull/39), [#40](https://github.com/LeoSavard/sidecar-immich/pull/40) | 5 |

The most-touched files outside docs (commits, all history): `web/src/routes/edit/[key]/+page.svelte` (36),
`crates/color/src/params.rs` (33), `edit-view.ts` and `pack.ts` (20 each), `xmp/tests.rs` (19),
`shaders/client/develop.wgsl` (18), `shaders/develop/params.wgsl` (16), and `worker/src/{lib,export}.rs` and
`color-wasm` (16 each). Separately, the HEIC double rotation and the mirrored orientations each cost an
`INGEST_GENERATION`.

**Why the top of the range keeps coming back.** Each of those PRs fixed one link in a chain that has no single
owner:

1. *Where is sensor white?* Decided three ways (§2.2 D7), and for ProRAW by a linearisation table (#34).
2. *What is clipped?* The same three heuristics.
3. *Where does middle grey sit?* `BASELINE_EV` as a stand-in, a file's `BaselineExposure` when it has one (#33,
   #36).
4. *What does the curve do above grey, and where does colour go as it nears white?* (#21, #35, #41)

The industry shape makes the first two **one ingest contract** and the last two **one display-transform
contract**. The ingest contract is darktable's *raw black/white point* stage, or the DNG model's linearise →
subtract black → divide by (white − black): a value of 1.0 means "the sensor saturated here", in every decode
path, before anything else runs. Exposure then places grey (Adobe's `BaselineExposure`, plus its "default black
render" for the toe). The view transform owns the shoulder and the path to white (filmic/sigmoid in darktable,
ACES 2's output transform with its gamut compression, already credited in NOTICE). The code is close to this. What's
missing is that (1) and (2) live in three files with three constants. A `SensorLevels { black: [f32; 4], white:
[f32; 4], clip: [f32; 4] }` computed once in `sidecar-raw` for all three sources, stored in `Meta` and
asserted by a test ("each source maps its declared white to 1.0 and its clip flag agrees"), would make the next
highlight PR a change in one place.

## 4. Industry practice for the problem areas, in brief

| Area | Common practice | Sidecar today | Recommendation |
|---|---|---|---|
| Black level | Per CFA position, plus row/column deltas (DNG); LibRaw `adjust_bl` folds ≤ 2×2 patterns | Averaged pattern | D1 |
| White level | Per channel; detect true saturation from data when declared is generous | One value; single-sample max | D6/D7 histogram |
| Camera colour | XYZ→camera = AB·CC·CM interpolated by the white point's CCT, iterated; ForwardMatrix preferred when present | CM only, interpolated and iterated | D2, then ForwardMatrix |
| Highlight reconstruction | Several methods in one module: clip, inpaint opposed, segmentation, guided Laplacians (darktable manual) | vkdt pyramid on Bayer and X-Trans proxies | Keep. It is the right method for a GPU. Fix D5 so export matches |
| Developed-file colour | ICC-driven via a CMS | Substring | D10/D11 with moxcms |
| Metadata | One long-lived ExifTool (`-stay_open True -@ -`), ~24× faster than a process per call in a published benchmark; or a native parser | A new Perl process per read: two for a linear DNG's ingest, more on export | `-stay_open` now; a Rust reader for the mobile build |
| Demosaic for previews | Bin or half-size when the output is ≤ half the sensor (LibRaw `half_size`; Sidecar's own X-Trans bin) | Full RCD, then downsample | §6 |

## 5. Readability and duplication

These are worth folding together, each with a test that pins the single version:

1. **Two decode dispatchers.** `decode_full` (`worker/src/lib.rs`) and `full_source_for` / `full_source`
   (`worker/src/export.rs`) each branch over developed / linear DNG / LibRaw mosaic / LibRaw RGB, and they have
   drifted: X-Trans goes to the GPU in one and to LibRaw in the other (D5), and one re-reads EXIF that the other
   takes from `Meta`. One `sidecar_raw::Source::open(path, meta) → Source` with `proxy(bound)` and `full()` is
   where D5's fix should live.
2. **`RawInfo` is built three times** (`RawFile::info`, `decoded::info`, `linear_dng::info`), each with the same
   `if rotated { h } else { w }` swap. A `RawInfo::new(sensor_size, flip, …)` keeps the orientation rule in one
   place. It is the rule that has already cost two cache generations.
3. **Three clip thresholds** (D7) and **two XYZ→sRGB matrices**: a literal in `raw/src/lib.rs::camera_color`,
   and one derived from `mat` in `decoded.rs` (and now `camera.rs`). Use the derived one.
4. **The sRGB transfer** is written in `decoded.rs::linear_lut`, `color/src/tone.rs`, `worker/src/export.rs` and
   twice in `devtools`. That's fine for WGSL, but on the Rust side there should be one function in
   `sidecar_color::tone`.
5. **`MosaicInput` is assembled field by field twice** (`worker/src/lib.rs:485`, `worker/src/export.rs:422`). One helper.
6. **The list of RAW extensions exists three times and disagrees.** `sidecar_template::RAW_EXTENSIONS` has 21.
   The home page's `accept` has 10 (no `nrw srf sr2 crw x3f 3fr iiq erf mef mos rwl`). The manifest's
   `share_target` has 11. Generate the web lists from the Rust constant in `npm run generate`, as the edit types
   already are, or at least have a test compare them.
7. **ExifTool is spawned from five call sites** (`raw/lib.rs`, `decoded.rs`, `gain_map.rs`, `worker/camera.rs`,
   `worker/export.rs`) with overlapping tag lists. One `Exif` service (a `-stay_open` process behind a mutex,
   one tag list per question) is both the speed-up and the seam a mobile build replaces.
8. **The editor page** (`+page.svelte`, 1,776 lines, about 70 functions, the most-changed file in the
   repository). ARCHITECTURE already names it as the next split. The natural seams are gestures (pointer, pinch,
   pan, hold), the render scheduler, brush strokes, and source loading (proxy, focus, fallback). Gestures first:
   that is the code mobile work will change.
9. **`params.rs` (2,553 lines, `pack_inner` 210)** and the hand-mirrored uniforms are V2's A4 and stay there.

## 6. Optimisations

| Change | Gain | Cost / risk |
|---|---|---|
| **Bin Bayer 2×2 for proxies when `long_edge ≤ sensor/2`** (as X-Trans already bins 3×3; LibRaw's `half_size` is the same idea) | ~4× less ingest work and memory per proxy; no demosaic artefacts at proxy scale | New `bayer_bin.wgsl` (Apache-2.0, trivial); proxy pixels change → `INGEST_GENERATION`; loupe and export keep RCD |
| Split `demosaic()` into two submits; `rb` as `rg32float` | −~19 B/px peak VRAM | None |
| ExifTool `-stay_open` | Perl start-up per call removed (one benchmark: 250 calls, 98 s → 4 s) | A long-lived child to supervise |
| `memmap2` (or per-tile reads) in `linear_dng::decode` | Bounded memory for panoramas, as documented | Small |
| Proxy transfer on phones: honour `navigator.connection.saveData` and cap at 1920 | ~45 % fewer bytes than 2560 | A setting |
| Proxy encoding: send clip coverage as a separate 8-bit plane, or RGB as `rgb9e5ufloat` | −12 % (8-bit clip plane) to −38 % (`rgb9e5` + clip plane) before compression | Measure ΔE against parity first; `rgb9e5` can't be a storage target |
| Recover from `device.lost` instead of asking for a reload: re-request the device, re-upload the kept proxy bytes | iOS drops GPU devices when Safari is backgrounded; today that is a reload | Keep the `ArrayBuffer`; rebuild passes |

## 7. Mobile: from "a phone pointed at a server" to local processing

### Where it stands

Already true:

- The PWA manifest and an Android share target.
- An iOS Shortcut.
- A device file picker that uploads to `POST /api/intake`.
- Pinch, pan, double-tap and press-and-hold.
- A 768 px first paint, then a proxy capped at 2,560 px on coarse pointers.
- Brotli and zstd on proxies.
- WebGPU in Safari 26 (iOS/iPadOS 26) and Chrome for Android since 121. Firefox on Android isn't
  confirmed in release builds; sources conflict.

### Gaps, in order of how soon a user hits them

1. **A shared photo that isn't in Immich goes nowhere.** The service worker hashes the file, redirects to
   `/open?sha1=…`, and throws the bytes away. In a standalone deployment (#40), or for a photo the phone hasn't
   backed up yet, `/open` finds nothing and the user lands on an empty pick page. Fix: the worker keeps the `File`
   (Cache Storage or IndexedDB, keyed by its SHA-1), and the pick page offers **Import this photo**, posting it to
   `/api/intake`. The iOS Shortcut gets a second variant that uploads the file instead of the hash.
2. **The picker.** Give two entry points: *Photo library* (`accept="image/*"`) and *Files* (the RAW extension
   list, generated per §5.6). WebKit transcodes on pick depending on the accept list: it has converted JPEG/PNG to
   HEIC when `image/heic` is listed (WebKit bug 303803 and related reports). How a ProRAW DNG comes out of the Photos
   picker isn't documented. Both need checking on a device, with a known file and its SHA-1 compared after upload.
   Android reportedly redacts GPS from images read through the system picker unless a native app holds
   `ACCESS_MEDIA_LOCATION` (a third-party plugin's docs; not confirmed by Google), so the server copy of an imported photo should keep the original's metadata from the
   file itself and never assume the location survived.
3. **Every pixel is decoded on the server**, which is the round trip local processing removes.

### A staged plan that stays licence-clean

**Stage 0: developed files decoded by the phone (no new dependencies).** `createImageBitmap(file,
{ colorSpaceConversion: 'none' })` (or WebCodecs `ImageDecoder`) gives the browser's own JPEG, PNG, WebP and AVIF
decoders, and in Safari HEIC as well. That is the OS's decoder, with no LGPL anywhere, which removes the HEIC
licence problem *on the device*. Upload the 8-bit result to a texture, linearise with `shaders/common/transfer.wgsl`
and develop it as `SourceKind::Developed`. Hash with SubtleCrypto (already done in the service worker) so the
asset key equals the server's SHA-1 and the edit can sync when there is a server. Limit: 8 bits per channel,
and the colour space comes from the ICC (D10 again: parse it in the existing colour wasm).

**Stage 1: the embedded preview for an instant first paint.** Every RAW carries a JPEG. Extracting it in the
browser (a small TIFF/CR3/RAF box walk, or LibRaw's `unpack_thumb` once stage 2 exists) shows the photo before
anything is decoded. V2.md's desktop survey asks for the same thing.

**Stage 2: RAW ingest on the device.** Two pieces:

- **The decoder:** LibRaw plus `shim.cpp`, compiled to wasm with Emscripten, doing only `open_file` + `unpack` +
  `sc_get_mosaic`, so no CPU demosaic and a small module. LibRaw is dual-licensed LGPL-2.1 / CDDL-1.0. Built
  under CDDL, as the server already uses it, it fits the Apache-2.0 image with the same notice obligations.
  Existing builds show it works in a Web Worker (`@colorhythm/libraw-wasm` exposes the raw buffer, per-channel
  black and linear maxima; MIT wrapper over LGPL/CDDL LibRaw). Build our own from the pinned submodule rather than
  depending on one, so the shim's fixes (D1–D4) apply in both places.
- **The pipeline:** `shaders/ingest/*` is already WGSL. Adding normalize, the highlight pyramid, RCD and the bins
  to `WEB_ENTRIES` runs the **same files** in the browser. That is the project's one rule applied to ingest,
  and the parity harness can hold it as it does develop. Budget memory with the bin-first proxy path from §6:
  a 12 MP Bayer RAW at ~60 B/px (ProRAW is a linear DNG and skips this) is ~730 MB of intermediates; binning first brings the proxy path to roughly 200 MB.
  Check `maxBufferSize` and `maxStorageTextureDimension` on real devices before relying on either.

**Stage 3: offline and local-only.** Edits in OPFS beside a cached proxy, XMP synced when a server is
reachable, export rendered on the device tile by tile (the detail passes already work on tiles for the loupe).
Metadata without ExifTool: `kamadak-exif` (BSD-2-Clause, pure Rust, reads TIFF/JPEG/HEIF and some RAW) in the
wasm build for reading. Writing XMP is already Sidecar's own code. This is also V2's "standalone phone app"
question answered from the web side first, without PhotoKit.

### Ease of use on a phone

The code already has the gestures. What it lacks is a phone in the test loop. ROADMAP notes iPhone parity is
unverified, and `editor-smoke.mjs --narrow` runs Chromium at a narrow width, not Safari on a phone. Before
redesigning anything, add one smoke run against Playwright's WebKit with a phone viewport and `hasTouch`, and
screenshot each section. Then measure on a real device: time to first paint, time to full proxy, and whether a
backgrounded tab survives (`device.lost`, §6).

## 8. Proposed order of work

1. **Small, no pixel change:** D4 guard; D12 refusals; the extension lists generated from Rust (§5.6); share-target
   keep-and-import (§7.1); `device.lost` recovery.
2. **Decode correctness, one `INGEST_GENERATION` bump for the lot:** D1, D2 (+ `linear_dng` CC/AB/per-channel
   levels), D7's single threshold, D11's Adobe RGB marker. Each needs a file that shows it, under `testdata/`.
3. **Structure:** one decode dispatcher (§5.1) carrying the D5 fix; `RawInfo::new`; ExifTool `-stay_open` behind
   one service.
4. **Proxy cost:** bin-first proxies, the two-submit split, and the transfer-size settings.
5. **Mobile stage 0 → 2**, with WebKit in the smoke test from the start.
6. ICC via moxcms (D10) and, later, ProRAW's gain table map (D13) as an optional control.

## 9. What changed on this branch

- `crates/color/src/camera.rs`: a camera colour with no calibration resolves with sRGB primaries instead of
  panicking (D9), with a test. No pixel changes for any file that opened before.
- `crates/libraw-sys/shim/shim.cpp`: the mosaic copy uses `raw_pitch` (D3) and refuses a crop wider than the row.
  This is identical for every file that worked. It was syntax-checked against the LibRaw headers but **not compiled
  into a running build here**, so the next `./dev/run cargo test --workspace` is its first real build.

## Sources

- LibRaw source at the pinned submodule: `src/utils/utils_libraw.cpp` (`adjust_bl`), `src/metadata/tiff.cpp`
  (the `AB·CC·CM` product), `src/decoders/unpack.cpp` and `src/integration/*` (`raw_pitch`).
- LibRaw forum on `cblack` layout and black subtraction: [Q about BlackLevels and Maximum values](https://www.libraw.org/node/2564), [Problem with dng file from Pentax K5-II](https://www.libraw.org/node/2485).
- DNG colour model and tags: [colour-hdri `models.dng`](https://colour-hdri.readthedocs.io/en/develop/_modules/colour_hdri/models/dng.html), [camera_space_to_XYZ_matrix](https://colour-hdri.readthedocs.io/en/v0.1.7/generated/colour_hdri.camera_space_to_XYZ_matrix.html), [Constrained Color Carrier (arXiv 2609.18361)](https://arxiv.org/pdf/2609.18361).
- Highlight reconstruction methods: [darktable manual, highlight reconstruction](https://docs.darktable.org/usermanual/development/en/module-reference/processing-modules/highlight-reconstruction/).
- ProRAW and DNG 1.6: [Apple WWDC21 10160](https://developer.apple.com/videos/play/wwdc2021/10160/), [pixls.us thread on DNG 1.6 tags](https://discuss.pixls.us/t/looking-for-samples-smartphone-dngs-with-embedded-gainmap/27296?page=3), [Library of Congress, ProRAW](https://loc.gov/preservation/digital/formats/fdd/fdd000628.shtml).
- ICC in Rust: [moxcms on docs.rs](https://docs.rs/crate/moxcms/0.5.0).
- ExifTool `-stay_open`: [ExifTool forum benchmark](https://exiftool.org/forum/index.php?msg=6121).
- WebGPU availability: [WebGPU (Wikipedia)](https://en.wikipedia.org/wiki/WebGPU), [WebGPU in iOS 26](https://appdevelopermagazine.com/webgpu-in-ios-26/).
- Mobile pickers: [WebKit bug 303803](https://bugs.webkit.org/show_bug.cgi?id=303803), [WebKit bug 292350](https://bugs.webkit.org/show_bug.cgi?id=292350), [Apple forums 743037](https://developer.apple.com/forums/thread/743037), [Android photo picker and location](https://www.androidauthority.com/android-photo-picker-location-3683074/).
- LibRaw in the browser: [@colorhythm/libraw-wasm](https://github.com/colorhythm/libraw-wasm).
- EXIF in Rust: [kamadak-exif](https://www.lib.rs/crates/kamadak-exif).
