# Architecture — a map for working on Sidecar

[`DESIGN.md`](DESIGN.md) is the specification and says *why*; this page says *where*. Read it before your
first change, whether you're a person or an agent picking the work up cold.

## The one rule

Every develop module is written once, in WGSL under `shaders/`, and runs in the browser (WebGPU) and on the
server (wgpu on Vulkan). What the phone previews is what export renders. Parity tests hold it to mean ΔE2000
≤ 0.5. Anything that changes pixels changes both sides at once, or it's a bug.

## Crates

```
sidecar (the binary: `api`, `worker`, `dev`)
├── sidecar-api      HTTP, sessions, /open and resolve, the XMP writer, exports + SSE, Immich client
│   ├── sidecar-xmp      sc:Sidecar inside XMP, the edit history tree
│   ├── sidecar-template storage-template rendering, date/name lookup
│   └── sidecar-color
└── sidecar-worker   decode, proxies, loupe, preview, export, masks (spawns sidecar-ml)
    ├── sidecar-engine   wgpu device and passes: ingest, develop, detail, lens, warp, brush, focus
    ├── sidecar-raw      LibRaw wrapper (sidecar-libraw-sys), JPEG/PNG/TIFF/HEIC, JPEG XL DNGs, lens tables
    ├── sidecar-xmp
    └── sidecar-color

sidecar-color        the hub: EditParams, white balance, camera matrices, curve LUTs, lens model, geometry,
                     and `pack()` — EditParams → the develop uniform. Also compiled to wasm (sidecar-color-wasm)
                     so the browser packs uniforms with the same code.
sidecar-shaders      embeds shaders/ and composes `//! requires:`; its binary writes the browser's WGSL
sidecar-ml           its own program (burn on Vulkan), started by the worker on demand; not a library dep
sidecar-devtools     sidecar-dev: parity fixtures, lens checks, ad hoc develops
```

`sidecar-color` feeds six crates, so a change there rebuilds most of the workspace — about 2 s under
`./dev/serve`'s profile, 20 s in release.

## Requests

```
browser ──/open?…──▶ api: resolve (Immich checksum → date layout → name search) → admit → 303 /edit/{assetKey}
browser ──/api/assets/{key}/meta, proxy, focus, masks──▶ api ──/v1/…──▶ worker (decode once, cache in /cache)
browser: develop.wgsl on the proxy, every slider move; view.wgsl draws it; histogram.wgsl counts it
browser ──PUT …/edit {edit, base, checkpoint, session}──▶ api → sidecar-xmp → <original>.xmp (history node)
browser ──POST /api/exports──▶ api ──/v1/render──▶ worker (full res, same WGSL) → outbox → Immich upload + stack
no WebGPU: POST …/preview → worker develops the proxy with the same shaders → JPEG
```

`assetKey` is the SHA-1 of the original's bytes. The edit lives in the XMP next to the original, found by
*path* (`DSC_1234.NEF.xmp`, then `DSC_1234.xmp`).

## Where to change things

| To change… | Look in |
|---|---|
| A develop control (slider) | the checklist below |
| What a develop stage does to pixels | `shaders/develop/*.wgsl`, called from `shaders/client/develop.wgsl` |
| Decode, demosaic, highlight reconstruction | `crates/raw`, `shaders/ingest/`, `crates/engine/src/ingest.rs` |
| Sharpening, noise reduction | `shaders/detail/`, `crates/engine/src/detail.rs` (server only: loupe, export) |
| Crop, straighten, keystone | `crates/color/src/geometry.rs`, `shaders/server/warp.wgsl`, `web/src/lib/ui/CropOverlay.svelte` |
| Lens correction | `crates/color/src/lens.rs`, `crates/raw/src/{lens,lensfun}.rs`, `shaders/lens/` |
| Masks | `shaders/develop/masks.wgsl`, `crates/worker/src/{masks,ml}.rs`, `crates/ml`, `web/src/lib/masks.ts` |
| What's stored in the sidecar | `crates/xmp` — it serializes `EditParams` generically, field names in PascalCase |
| Edit history and versions | `crates/xmp/src/history.rs`, `web/src/lib/ui/HistoryPanel.svelte` |
| Export, versions, stacking | `crates/api/src/export.rs`, `crates/worker/src/export.rs` |
| The editor UI | `web/src/routes/edit/[key]/+page.svelte`, `web/src/lib/ui/`, `web/src/lib/state/edit-view.ts` |

### Adding a develop control

The global Saturation slider (PR #4: one commit for the core, one for the UI) is the worked example. In order:

1. `crates/color/src/params.rs` — the field on `EditParams` (or a sub-struct), its default, and
   `#[serde(skip_serializing_if = …)]` at the default so existing edits keep their ETag.
2. If the shader needs it: the `DevelopParams` (or `MaskParams`) field — use padding before growing the struct —
   set in `pack()`, mirrored in `shaders/develop/params.wgsl`, and added to the `uniform_layout_matches_wgsl`
   test's offset list. That test, run by naga, is what catches a layout mismatch.
3. The stage in `shaders/develop/` taking the value as an argument; only `shaders/client/develop.wgsl` reads
   the uniform. Results at the default must be bit-identical to before.
4. Tests in `params.rs`: the packing scale, and that the field is omitted at its default.
5. `npm run types` in `web/` (part of `npm run generate`) regenerates `web/src/lib/generated/` from the Rust
   types; commit the result — CI fails on drift. A field serde leaves out at its default also needs
   `#[cfg_attr(feature = "ts", ts(as = "Option<…>", optional))]`, so the TypeScript says it can be absent.
6. `web/src/lib/state/edit-view.ts` — a `CONTROLS` entry (history summaries) and the right `PICKS` section
   ("use its …" in History, and what a copied look carries).
7. The slider in the editor's section snippet.
8. If old edits would render differently, bump `PROCESS_VERSION` and migrate in `sidecar-xmp` — both where
   the current edit is read (`SidecarDoc::edit`) and where history rebuilds one (`History::edit_of`).
   Purely additive fields don't need a bump.
9. A parity case if the stage is new (`crates/devtools/src/parity.rs`).

## Working on it

- **Build and test inside the dev container**: `./dev/run <command>`. `./dev/bootstrap` makes a fresh clone or
  worktree buildable (submodule, dev image, npm packages, generated WGSL and wasm).
- **Run it**: `./dev/serve` picks a free port from 8091 and prints the URL; `./dev/serve --web` adds Vite's dev
  server, so Svelte and TypeScript edits show as you save. Shader or `sidecar-color` changes need
  `./dev/run bash -c 'cd web && npm run generate'` (a second or two), then a reload.
- **Worktrees are isolated**: each gets its own cargo target directory, server state and container
  (`sidecar-serve-<worktree>`). sccache shares compiled dependencies between them.
- **Fast checks**: `./dev/run cargo check -p <crate>`, `./dev/run cargo test -p <crate>`,
  `./dev/run bash -c 'cd web && npm run check'`.

### Testing pitfalls

- **Fixtures**: put copies under `testdata/cache/library/` (never touch `/mnt/storage/`). Use a *new file
  name* for different content: the sidecar is found by path, so overwriting `a.nef` with other bytes hands the
  new photo the old one's edit. Byte-identical copies are the same asset (same `assetKey`).
- **Headless Chromium has no GPU adapter**, so the editor runs in server-preview fallback: the photo is an
  `<img>`, and the histogram and overlays are hidden. That's expected, not a bug.
- **The editor renders each control twice** at desktop widths (tab bar and accordion). Pick the visible one.
- **Sliders** are `role="slider"` with `aria-valuenow`; arrow keys step (Shift ×10); clicking the value opens
  an input for an exact number — more reliable than dragging.
- **Edits persist** in the server state across runs of the same worktree: set absolute values in tests rather
  than stepping from wherever the last run left them.

## Decisions and their records

Specification changes go in `DESIGN.md`, with what the code actually does in its build log at the top.
Known gaps go in `ROADMAP.md`. Licensing rules are in `CLAUDE.md` and enforced by `cargo deny` and the
shader header test.
