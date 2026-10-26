# The mobile module

Sidecar on the phone, as an optional part of the image. It lives in `mobile/`, is built into images with a
`-mobile` tag, and is served at **`/m`** next to the main app. The plan it starts on is in
[`research/pipeline-review-2026-10.md`](research/pipeline-review-2026-10.md) §7.

## What it does today (stages 0, 1 and 2)

- **Choose a photo** from the phone's Photos (`accept="image/*"`) or its files (every RAW extension
  Sidecar knows, from `web/src/lib/formats.ts`).
- **A developed photo (JPEG, HEIC, PNG, WebP, AVIF) is decoded and developed on the phone.** The browser decodes
  it, converting its profile into Display P3 where the browser can; a camera's Adobe RGB JPEG, which has no profile
  and says so only in EXIF, is read as Adobe RGB, as the server reads it. `mobile/src/local/decode.ts` turns it into the same
  linear RGBA16F proxy the server would send, and the editor's own `DevelopPass`, `ViewPass`, colour packer and
  Light / Colour / White balance sections develop it. Nothing is uploaded. HEIC is decoded by the phone's
  system decoder, so the server's licence boundary for HEIC (CLAUDE.md) doesn't come into it.
- **Save a copy:** the edit rendered in sRGB, read back and encoded as a JPEG on the phone. It goes to the share
  sheet where there is one (Save to Photos on an iPhone), and is a download otherwise. It is saved at the size it
  was edited at (2,560 px long edge at most).
- **Open in Sidecar:** to the full editor on the server. An untouched photo goes the way a share does: it is kept
  on the phone and opened by checksum, so a photo already in the library opens as itself, and the pick page offers to
  import one that isn't. An edited photo is imported (`POST /api/intake`), and the phone's edit is saved as its
  first version, so the work carries over.
- **A RAW file shows the camera's own preview at once** (stage 1): the JPEG the camera embedded in the RAW,
  found and shown by the phone (`mobile/src/local/preview.ts`), the right way up, with nothing uploaded. It is
  labelled as the camera's rendering, not Sidecar's.
- **…and then develops the RAW itself** (stage 2): LibRaw, compiled to WebAssembly, unpacks the sensor data in a
  Web Worker, and the server's own ingest shaders turn it into the proxy on the phone's GPU. The editor then opens
  on it with a RAW's controls (Highlight recovery among them), still with nothing uploaded. An sRAW or linear
  DNG, a frame larger than the GPU's textures, more than 26 megapixels on a phone, or any failure leaves the
  preview up with the reason, and *Open it in Sidecar* takes the RAW to the server the way a share does.
- **A browser without WebGPU** goes to the server the same way.

## How it is put together

**It owns the phone's flow and nothing else.** `mobile/` is its own SvelteKit app (base path `/m`), and its
`$lib` *is* the main app's `web/src/lib` (`files.lib` in `mobile/vite.config.ts`). The GPU passes, the wasm
packer, the generated types, the controls and the api client are used as they are. What is in `mobile/`:

| File | What |
|---|---|
| `src/routes/+page.svelte` | The page: choose, develop, save, send |
| `src/local/decode.ts` | Browser decode → linear RGBA16F proxy |
| `src/local/export.ts` | Readback → JPEG; the share sheet or a download |
| `src/local/preview.ts` | A RAW's embedded JPEG: found by its own structure, oriented, scaled |
| `src/local/raw.ts`, `raw.worker.ts` | LibRaw in a Web Worker: the mosaic and the file's metadata |
| `libraw/build.sh`, `libraw/wasm_api.cpp` | LibRaw and the server's shim built for the browser; JSON out, so no struct is mirrored |
| `scripts/mobile-smoke.mjs` | End to end at a phone's size: decode, develop, Exposure, save, send, the server's copy of the edit; with `--raw`, the preview shown with nothing sent |
| `scripts/preview-check.mjs` | `preview.ts` against ExifTool on RAW files: the same bytes, the same orientation |
| `scripts/raw-parity.mjs` | A RAW decoded on the phone against the same RAW decoded by the server: colour model and proxy |

This is deliberate. The project's one rule is that a develop stage is written once (ARCHITECTURE.md), and a
mobile app with its own copy of the pipeline would break it the first time either side changed. What the module
decouples is **packaging and pace**: it ships in its own image layer and can change its flow without touching the
editor page, the most-changed file in the repository. The price is that a change to the shared `$lib` can break
it, so CI type-checks and builds it in the job that has the generated files.

Two things in the shared code exist because of it, and are worth knowing:

- `sidecar_color::camera::SourceSpace` moved from `sidecar-raw` into `sidecar-color`, with a wasm export
  (`sc_source_color`, `ColorPacker.sourceColor`). A photo the phone decoded gets exactly the colour model the
  worker gives the same JPEG.
- `web/src/lib/color/transfer.ts` (sRGB curve, half floats), `formats.ts` (extensions, checked against Rust by a
  `sidecar-template` test), `shared.ts` and `intake.ts` are shared with the main app.
- For stage 2: `sidecar_color::camera::LibrawColor` is the one mapping from what LibRaw reads to a `CameraColor`,
  used by the server's `sidecar-raw` and by the browser (`sc_camera_color`, `ColorPacker.cameraColor`);
  `web/src/lib/gpu/ingest.ts` runs `shaders/ingest/*` (now in `WEB_ENTRIES`) as `crates/engine/src/ingest.rs` does
  — **change one, change the other**; and `requestDevice` asks for the adapter's texture and buffer limits.

`files.lib` is marked deprecated by SvelteKit in favour of a workspace package. It still works. If it is ever
removed, the move is to make `web/src/lib` a package both apps depend on, which is the same sharing done with more
ceremony.

## The embedded preview (stage 1)

Every RAW format Sidecar opens stores its preview as an ordinary JPEG somewhere: in a TIFF IFD (NEF, CR2, ARW,
DNG, PEF), as a tag's value (RW2), behind a fixed header (RAF), in an ISO BMFF box (CR3). So rather than one parser
per format, `preview.ts` finds JPEG streams by their own structure:

- Each candidate is walked **marker segment by marker segment**, which steps over the thumbnail nested inside a
  preview's EXIF block.
- It is kept only if it is a **Huffman-coded picture** (SOF0–2). Lossless JPEG (SOF3), which CR2 and DNG use for
  the sensor data itself, is not a preview.
- The **largest** candidate wins.
- **Orientation:** the browser applies a JPEG's own EXIF orientation as it decodes. Where the JPEG states none,
  which is the usual case for a NEF or CR2 preview, the RAW's own orientation applies: TIFF IFD0, or the TIFF block
  in a CR3's `CMT1` box.
- The picture is drawn onto a canvas at most 2,560 px on its long side: the editing proxy's size, and well inside
  iOS's limit on canvas area.

**How it was checked.** `scripts/preview-check.mjs` compares every file against ExifTool, which reads each format
with its own maker-specific parser: the bytes shown must be **identical** to the largest image ExifTool extracts,
and the orientation must agree. It passes on:
- 13 files: real CR2s from rawpy's test set (one an sRAW, one rotated 180°, one 5616 × 3744) and real NEFs from
  NASA's ISS photographs, found in 4–30 ms;
- ExifTool's own sample CR2, CR3, DNG, RAF, NEF and RW2, whose previews are stripped, so they test the "no
  preview" answer;
- a DNG and an RW2 that ExifTool wrote a preview into, in each format's own place.

In Chromium at phone size, the shown preview matches ExifTool's, turned and scaled independently with PIL, to
**0.8 / 255** for the 180° CR2 (23.8 / 255 had it not been turned) and about 2 / 255 for the scaled ones. A
5616 × 3744 preview is on screen in 1.7 s on this machine's CPU, a 1936 × 1288 one in 0.4–0.8 s.

**Not yet checked on real files:** CR3, RAF, ARW, ORF, PEF, SRW previews. The approach doesn't depend on the format,
but no real file of those was at hand. They want a run of `preview-check.mjs` over a folder of them.

## RAW on the phone (stage 2)

**The decoder** is LibRaw from `third_party/libraw` and the server's `crates/libraw-sys/shim/shim.cpp`, compiled
with Emscripten 6.0.13 (`mobile/libraw/build.sh`): 630 KB of wasm, 255 KB gzipped, with C++ exceptions as wasm
exceptions. `shim.cpp` gained `sc_open_buffer`, since a browser has no paths; `wasm_api.cpp` hands the metadata
and mosaic back as JSON. Built without zlib and libjpeg for now, so deflate and lossy DNGs go to the server.

**The pipeline** after it is the server's, pass for pass: `IngestPass.proxy` in `$lib/gpu/ingest.ts` is
`Ingest::proxy` then `RgbaF32::oriented` — the same composed WGSL, crop phase, highlight pyramid, RCD (or the
X-Trans bin), clip threshold and downsample — and WebGPU error scopes turn a GPU that runs out of memory into a
message instead of a black picture. The colour model is `LibrawColor::camera_color`, the server's own.

**How it was checked**, on real CR2 and NEF files (rawpy's test set) against a real `sidecar dev` whose worker
ingests on llvmpipe, with Chromium's WebGPU on SwiftShader:

- *The wasm build is the native one:* `wasm_api.cpp` built natively and as wasm reads all five files to the same
  metadata and the same FNV-1a hash over every mosaic sample (21–26 million each).
- *The phone's proxy is the server's* (`scripts/raw-parity.mjs`): for four files with a mosaic, the colour model is
  identical and the proxy differs by **0.06–0.10 % of the signal on average, at most 2 × 10⁻³** — half-float
  rounding between two GPUs — at the same size. The sRAW is declined, as it should be. A truncated CR2 matches
  too, NaNs and all; those NaNs are the server's own (the review's D14).
- *The developed picture is the server's:* the phone's saved copy of a NEF at default settings against the
  server's `/preview` render of it differs by 2.3 / 255 at full size, 1.0 at a quarter, 0.2 at a sixteenth, with
  no bias in any channel (0.01–0.03 / 255): JPEG and dither noise, not development.
- *The flow:* `scripts/mobile-smoke.mjs --raw` — preview at once, then the RAW developed on the phone, RAW
  controls offered, a picture on screen, nothing sent to the server.

**What it costs.** On SwiftShader (a CPU standing in for a GPU) a 12–21 MP RAW takes 27–55 s; a phone's GPU should
be one to two orders faster, and that is to be measured. Memory is the real limit: the ingest holds about 60 bytes
per sensor pixel (review §6), so the page declines more than 26 MP on a phone until proxies bin before
demosaicing.

## Building and running it

```bash
# once the main app's generated files exist (./dev/bootstrap, or `npm run generate` in web/)
source <emsdk>/emsdk_env.sh                   # Emscripten 6.0.13, as the Dockerfile and CI pin it
cd mobile && npm run libraw                   # → src/local/libraw/ (LibRaw for the browser; ~30 s)
npm ci && npm run build                       # → mobile/build
```

The dev container has no Emscripten yet; the Dockerfile's `build-mobile` stage and CI install it themselves.

The api serves `server.mobile_root` (default `/usr/share/sidecar/mobile`) at `/m` **only when that directory has an
`index.html`**. In development, point it at the build:

```toml
[server]
mobile_root = "/src/mobile/build"
```

`npm run dev` in `mobile/` runs Vite's dev server; set `SIDECAR_API` to a running Sidecar to proxy `/api` and
`/open`, as `dev/serve --web` does for the main app.

**Images.** `--target mobile` is the plain image plus the module; `--target ml-mobile` is the `-ml` image plus the
module. The image workflow publishes them as `vX.Y.Z-mobile` and `vX.Y.Z-ml-mobile` (and `latest-…`). Each is
one `COPY` on top of an image already built, so they cost the module's npm build and little else. Moving an
existing deployment over is a tag change: `ghcr.io/…/sidecar-immich:latest` → `:latest-mobile`.

**Smoke test:** `node scripts/mobile-smoke.mjs <url> <photo.jpg> [--raw <file>] [--swiftshader] [--shots <dir>]`,
from `mobile/`. **RAW parity:** `node scripts/raw-parity.mjs <url> <RAW>… [--swiftshader] [--dump <dir>]`.

## What comes next

In the order the review gives:

1. ~~Stage 1 — the embedded preview~~ — done (below).
2. **Stage 2 — RAW on the phone:** done (above). What is left of it: **bin before demosaicing** for proxies,
   on both sides at once (review §6), which lifts the 26 MP limit and cuts the time; zlib and libjpeg in the wasm
   build for deflate and lossy DNGs; measuring it on a real phone. The shim fixes (D1–D4) now apply on both sides
   at once.
3. **Stage 3 — offline.** Edits in OPFS, synced as XMP when a server is reachable; a full-resolution export
   on the phone, tiled; EXIF read in wasm (`kamadak-exif`, BSD-2-Clause) where ExifTool can't go.

Things to settle on a real phone rather than in Chromium:

- What the iOS Photos picker hands over for HEIC and ProRAW with `accept="image/*"` (WebKit has transcoded on
  pick, depending on the accept list).
- Whether `getImageData({ colorSpace: 'display-p3' })` is honoured, so the proxy keeps P3.
- How often Safari drops the GPU device when the tab is backgrounded. The page recovers by asking for the photo
  again; keeping the decoded proxy to re-upload would be better.
