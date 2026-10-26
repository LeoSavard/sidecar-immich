# The mobile module

Sidecar on the phone, as an optional part of the image. It lives in `mobile/`, is built into images with a
`-mobile` tag, and is served at **`/m`** next to the main app. The plan it starts on is in
[`research/pipeline-review-2026-10.md`](research/pipeline-review-2026-10.md) §7.

## What it does today (stage 0)

- **Choose a photo** from the phone's Photos (`accept="image/*"`) or its files (every RAW extension
  Sidecar knows, from `web/src/lib/formats.ts`).
- **A developed photo (JPEG, HEIC, PNG, WebP, AVIF) is decoded and developed on the phone.** The browser decodes
  it, converting its profile into Display P3 where the browser can. `mobile/src/local/decode.ts` turns it into the same
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
- **A RAW file, or a browser without WebGPU,** goes to the server the same way. Decoding RAW on the phone is
  stage 2.

## How it is put together

**It owns the phone's flow and nothing else.** `mobile/` is its own SvelteKit app (base path `/m`), and its
`$lib` *is* the main app's `web/src/lib` (`files.lib` in `mobile/vite.config.ts`). The GPU passes, the wasm
packer, the generated types, the controls and the api client are used as they are. What is in `mobile/`:

| File | What |
|---|---|
| `src/routes/+page.svelte` | The page: choose, develop, save, send |
| `src/local/decode.ts` | Browser decode → linear RGBA16F proxy |
| `src/local/export.ts` | Readback → JPEG; the share sheet or a download |
| `scripts/mobile-smoke.mjs` | End to end at a phone's size: decode, develop, Exposure, save, send, the server's copy of the edit |

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

`files.lib` is marked deprecated by SvelteKit in favour of a workspace package. It still works. If it is ever
removed, the move is to make `web/src/lib` a package both apps depend on, which is the same sharing done with more
ceremony.

## Building and running it

```bash
# once the main app's generated files exist (./dev/bootstrap, or `npm run generate` in web/)
cd mobile && npm ci && npm run build          # → mobile/build
```

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

**Smoke test:** `node scripts/mobile-smoke.mjs <url> <photo.jpg> [--swiftshader] [--shots <dir>]`, from
`mobile/`.

## What comes next

In the order the review gives:

1. **Stage 1 — the embedded preview** of a RAW for an instant first picture, while the server develops it.
2. **Stage 2 — RAW on the phone.** LibRaw and `shim.cpp` compiled to wasm with Emscripten, doing
   `open_file` + `unpack` + `sc_get_mosaic` only (CDDL-1.0, as the server uses it). `shaders/ingest/*` goes into
   `WEB_ENTRIES` so normalize, highlight reconstruction and RCD run in the browser from the same files. Proxies
   must bin before demosaicing to fit a phone's memory (review §6). The shim fixes (D1–D4) then apply on both
   sides at once.
3. **Stage 3 — offline.** Edits in OPFS, synced as XMP when a server is reachable; a full-resolution export
   on the phone, tiled; EXIF read in wasm (`kamadak-exif`, BSD-2-Clause) where ExifTool can't go.

Things to settle on a real phone rather than in Chromium:

- What the iOS Photos picker hands over for HEIC and ProRAW with `accept="image/*"` (WebKit has transcoded on
  pick, depending on the accept list).
- Whether `getImageData({ colorSpace: 'display-p3' })` is honoured, so the proxy keeps P3.
- How often Safari drops the GPU device when the tab is backgrounded. The page recovers by asking for the photo
  again; keeping the decoded proxy to re-upload would be better.
