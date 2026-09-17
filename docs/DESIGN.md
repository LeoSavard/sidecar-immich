# Sidecar — system design

*Sidecar* is a working name. Status: draft 2, 2026-09-14, with a build log kept current (last entry 2026-09-16); Immich API details checked against the source of Immich 3.1.0, the pinned version. Revised for iPhone as the primary platform, an external library with a strict date layout, versioned exports with the JPEG leading the stack, and crop / lens correction / local masks after V1.
Source: the architecture plan (PWA entry, XMP sidecars, server-side demosaic, WebGPU preview, SSE export telemetry, Carbon/shadcn/HIG interface references).

## Implementation notes (build log)

Where the code differs from the text below, the code and these notes win.

- **Immich checksums:** Immich hashes file contents only for its internal library. External-library assets get `sha1("path:" + originalPath)` as their checksum, so checksum search can't find them. Resolution therefore relies on the date layout and filename search for external RAWs, confirms candidates by hashing the local files, and finds the Immich asset by `originalPath`.
- **Immich is 3.2.0** on the server (floating `v3` tag). The API behaviour relied on is unchanged from 3.1.0. `GET /api/users/me` needs `user.read`, so the sign-in key needs `user.read, asset.read, asset.view, asset.upload, asset.update, stack.create`.
- **Uniform layouts** are mirrored by hand in `#[repr(C)]` Rust structs and checked against the WGSL by parsing it with naga in tests. `encase` 0.12 dropped glam support and `webgpu-utils` isn't needed: the browser packs uniforms with the same Rust code compiled to WebAssembly (`crates/color-wasm`).
- **Vulkan in containers** also needs `libegl1`, `libxext6` and `libx11-6` in the image; NVIDIA's ICD fails to initialise without libEGL.
- **Proxies (milestone 2)** use LibRaw's half-size mode (2×2 binning, no interpolation): 0.3–0.5 s for a 45 MP NEF, ~2 s for a compressed RAF. The GPU demosaic of milestone 5 replaces this.
- **XMP field names** follow the JSON parameters in PascalCase inside `sc:Sidecar/sc:Edit` (`sc:Ev`, `sc:TemperatureK`, …). The namespace URI is `https://sidecar.ls-tech.ca/ns/1.0/` and must never change.
- **Library root:** the external library's template starts at `/mnt/storage/Photography/Library`.
- **Known limitation:** Lightroom-merged panorama DNGs use a compression LibRaw 0.22 can't decode; Sidecar reports them as unsupported.
- **GPU ingest (milestone 5):** vkdt's highlight pyramid and RCD are ported to WGSL; RCD is split into five full-texture passes instead of vkdt's shared-memory tiles. X-Trans proxies use 3×3 binning on the GPU; full-resolution X-Trans (export, loupe) still uses LibRaw's Markesteijn on the CPU — a WGSL port of LibRaw's code is the remaining step. LibRaw is built with OpenMP (a Fuji RAF opens in 0.2 s instead of 1.7 s).
- **Export (milestone 6):** JPEG through `jpeg-encoder` (pure Rust, MIT/Apache + IJG) rather than mozjpeg. The worker streams NDJSON progress to the api, which relays it as SSE with `Last-Event-ID` replay. The outbox is reached through worker endpoints, so the api keeps `/cache` read-only.
- **Detail (milestone 7):** denoise and sharpening are implemented from the published methods (à-trous wavelets with square-root variance stabilization and a per-image noise estimate; Richardson–Lucy on luminance), not ported — vkdt's denoise depends on per-camera noise profiles. The worker keeps the last two full-resolution buffers for loupe tiles.
- **Internal library** roots can set `per_user = true`: Immich keeps a folder per user (storage label or id) before the template.
- **Entry points (milestone 8):** Android share target hashes in the service worker and opens `/open?sha1=…`; desktop pastes an Immich link, resolved through `GET /api/assets/{id}`.
- **Development:** `./dev/serve` runs api + worker on `127.0.0.1:8090`, the real library read-only, and a writable test copy under `testdata/cache/library`.
- **First live run (2026-09-15):** share → resolve → develop → sidecar → export → upload → stack, against the real external library. The Immich *admin* key is scoped without `asset.read`, so stacks can't be checked from the CLI — only in the Immich app.
- **Output space is a user setting** (§11), not a guess from `matchMedia`. It drives the develop uniform, the canvas, the loupe request and the export default together; the export sheet still overrides it for one export. The editor previewing in P3 while exports defaulted to sRGB was a real trap: saturated reds fit on screen and clipped in the file.
- **Tint direction:** the slider names what happens to the *image*, as every editor does. Positive tint moves the assumed illuminant toward green so the photo goes magenta. It shipped inverted and is fixed; `positive_tint_moves_the_illuminant_toward_green` pins it.
- **Tone curve, measured:** the logistic base curve starts shouldering immediately above middle grey — log-log slope 0.80 at +1 EV, 0.57 at +2 EV, 0.40 at +2.5 EV. Raising exposure therefore costs upper-midtone texture (0.73× the local contrast of the unedited render in one real case, measured as mean |∇ log Y|). The underlying constraint is headroom: sRGB holds 2.47 stops above grey while the default `whiteEv` asks the curve to fit 4.5. Lowering Whites is the working answer; replacing the curve is on the roadmap, with a candidate patch measured and rejected for regressing shadow contrast.
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
```

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
- **Concurrency.** ETag = SHA-256 of the canonicalized edit parameters. It excludes `sc:Exports`, so a finishing export never makes the editor's next save conflict. `PUT` requires `If-Match`; `412` returns the current state so the client can offer *Keep mine* / *Use theirs*.
- **`sc:ProcessVersion`** pins the math. A pipeline change that alters output bumps it; older sidecars keep rendering with the older path until the user updates them. A sidecar with a newer ProcessVersion than the running app opens read-only with "Update Sidecar to edit this photo" — a phone can hold an old cached copy of the app.
- **`sc:SourceSHA1`** ties the sidecar to its original and catches a sidecar left behind by a rename.
- **Immich writes too.** When a description, date, location, rating or tag changes in Immich, it writes those fields into `<original>.xmp` — the same file — through exiftool, which preserves unknown namespaces. Tagging alone creates the file, so many RAWs will already have one. Sidecar's writer re-checks the file's size and modification time just before its rename and starts over if Immich wrote in between. A test edits a rating in Immich and asserts `sc:Sidecar` survives.
- **Drafts.** Slider state autosaves to IndexedDB (per device, 500 ms debounce). It commits to XMP on *Done*, on *Export*, and on `visibilitychange → hidden` via `fetch(…, { keepalive: true })`. Drafts also cover Safari's edge-swipe back gesture: swiping forward returns to the edit intact.

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

On first open, cached by `assetKey + ProcessVersion`:

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
  c = P.workToDisp * c;                     //     Rec.2020 → sRGB / P3 primaries, soft clip
  return vec4f(c, 1.0);
}
```

Module functions take their values as arguments and never read the uniform buffer themselves, so local masks (§15) can call them with per-pixel blended values. Module bypasses compile as pipeline `override` constants, so disabled modules cost nothing.

1. **White balance.** The `color` crate (compiled to WASM) turns (Temperature K, Tint) into CIE xy on the Planckian locus offset along the isotemperature line → XYZ white → camera neutral through the XYZ→camera matrix interpolated for that temperature → multipliers = 1/neutral, normalized so G = 1. For *As shot* it runs the inverse, iterating on `cam_mul` to place the sliders.
2. **Exposure.** `2^EV`, computed on the CPU. Values above 1.0 pass through untouched.
   **Highlight blend.** Where alpha > 0, blend toward the pixel's luminance-preserving neutral by `alpha × strength`, suppressing the magenta cast WB multipliers put on reconstructed highlights.
3. **Color space.** Camera → XYZ (inverse of the interpolated XYZ → camera matrix, normalized so camera white lands on D65) then XYZ → Rec.2020, recomputed whenever WB changes. Gamut compression (distance-based, in the style of the ACES reference gamut compression) keeps saturated lights from producing negative channels.
4. **Tone map.** Luminance Y (Rec.2020 weights) → `log2(Y / 0.18)` mapped from [BlackEV, WhiteEV] to [0, 1] → sigmoid (contrast, pivot) → user curve LUT → decode to Y′ → `rgb × Y′ / Y`, which keeps channel ratios and therefore hue. The curve is Fritsch–Carlson monotone cubic over display-encoded values, baked into 4,096 entries in an `r32float` 1D texture and interpolated manually (`r32float` isn't filterable without an optional feature). Shadows / Midtones / Highlights sliders in the UI move curve control points — the same spline.
5. **Color mixer.** Linear → OKLab → LCh. Bands: red, orange, yellow, green, aqua, blue, purple, magenta. Raised-cosine weights span neighbouring band centres and sum to 1. `h += Σ wᵢ·hueᵢ`, `C ×= 1 + Σ wᵢ·satᵢ`, `L += Σ wᵢ·lumᵢ · smoothstep(0.02, 0.08, C)` so neutrals stay put. Back to linear.

Uniform buffer:

```wgsl
struct DevelopParams {            // offset  size
  wbMul      : vec3f,             //    0     12   camera-space multipliers, G = 1
  exposure   : f32,               //   12      4   2^EV
  camToWork  : mat3x3f,           //   16     48
  workToDisp : mat3x3f,           //   64     48
  tone       : vec4f,             //  112     16   blackEv, whiteEv, contrast, pivot
  hlStrength : f32,               //  128      4
  _pad0      : f32,               //  132      4
  _pad1      : vec2f,             //  136      8
  mixer      : array<vec4f, 8>,   //  144    128   per band: hue, sat, lum, unused
}                                 //  total 272 bytes
```

**Struct layouts.** The 272-byte table above documents the layout; code never hard-codes it. `encase` (MIT-0) derives it on the Rust side and `webgpu-utils` (MIT) reads it from the WGSL source in the browser, so a field added to the struct can't silently shift the offsets.

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

**Focus peaking** shares the pass and the same principle — judge what the pixels actually hold:

```wgsl
let step = max(1.0 / vec2f(textureDimensions(developed)), vec2f(V.uv_from_px[0][0], V.uv_from_px[1][1]));
if (focus_energy(uv, step) >= V.peak_t) { return vec4f(PEAK_COLOUR, 1.0); }   // amber; red and blue stay clipping
```

`focus_energy` is the central-difference gradient of encoded luminance. **This is a stopgap:** measured on the proxy it reports edge contrast rather than sharpness, and the real measure belongs at full resolution on the server, shipped as a layer — see the roadmap. The step is the larger of one texel and one canvas pixel, so zooming out averages detail away instead of aliasing downscaling noise into false peaks. It reads the proxy, so at fit zoom it reports composition-scale sharpness; critical focus means zooming in or opening the loupe.

## 9. Export

Every export is kept. Each distinct edit that gets exported becomes a numbered version, and the newest JPEG leads the photo's stack in Immich.

1. **Commit** — `PUT /api/assets/{key}/edit` with `If-Match` → XMP written → new ETag.
2. **Create job** — `POST /api/exports` with `Idempotency-Key` (one UUID per Export tap) and `{ assetKey, editEtag, format, colorSpace, quality, longEdge }`.
   - If `sc:Exports` already holds this ETag with these settings, the answer is `200 { version, immichAssetId }` — nothing renders twice.
   - Otherwise the api assigns the version under a per-photo lock and answers `202 { jobId, version }`. The job is pinned to that ETag; later edits don't touch it. Retrying with the same key returns the same job.
3. **Stream** — `new EventSource('/api/exports/' + jobId + '/events')`.
4. **Render** — worker, full resolution, tiled.
5. **Upload and stack** — api.

**Versions and file names.** A new edit ETag takes the next version number; the same edit exported in another format or size reuses its number.

| Export | File name |
|---|---|
| First exported edit, JPEG | `DSC_1234-v1.jpg` |
| Same edit, 16-bit TIFF | `DSC_1234-v1.tif` |
| Same edit, JPEG at 2,048 px | `DSC_1234-v1-2048px.jpg` |
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
4. Append the version to `sc:Exports` in the RAW's sidecar.
5. Delete the outbox entry. On restart the api retries leftover outbox entries with backoff, so an Immich outage never loses a finished render.

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
| POST | `/api/exports` | `202` new job, or `200` existing version; `Idempotency-Key` |
| GET | `/api/exports/{id}/events` | `text/event-stream` |
| GET | `/api/exports/{id}` | Status snapshot |
| DELETE | `/api/exports/{id}` | Cancel |

**Auth.** The user signs in once per browser (Safari, Home Screen app, desktop) with an Immich API key holding `asset.read`, `asset.view` (thumbnails in the picker), `asset.upload`, `asset.update` and `stack.create` — stack creation in 3.1.0 checks `asset.update` on each asset. The admin key for internal-library sidecar discovery (§4) is separate, holds only `job.create`, and is read from a Docker secret. It's stored server-side, encrypted at rest; the browser gets an HttpOnly, `SameSite=Lax` session cookie — `Lax` so the cookie comes along on the top-level navigation from the Shortcut. Every resolve goes through that key, so nobody opens a photo Immich wouldn't show them.

```ts
type Band = 'red' | 'orange' | 'yellow' | 'green' | 'aqua' | 'blue' | 'purple' | 'magenta';

interface EditParams {
  processVersion: 1;
  whiteBalance: { mode: 'asShot' | 'custom'; temperatureK: number; tint: number };   // 2,000–50,000 K; −150…+150
  exposure:     { ev: number; highlightBlend: number };                              // −5…+5 EV; 0…1
  tone:         { blackEv: number; whiteEv: number; contrast: number;
                  curve: Array<[x: number, y: number]> };                            // display-encoded 0…1
  mixer:        Record<Band, { hue: number; saturation: number; luminance: number }>; // −100…+100
  detail:       { sharpen: { amount: number; radius: number; iterations: number };
                  denoise: { luminance: number; chroma: number } };
  // After V1, optional: lens?, crop?, masks? — absent means off (§15)
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
    api/              axum · open/resolve · XMP writer · versions · jobs · SSE · Immich client
    worker/           LibRaw FFI · wgpu · ingest · loupe · export · tiling
    color/            WB/temperature, DNG matrix interpolation, spline → LUT (native + wasm32)
    libraw-sys/       bindings to third_party/libraw, adapted from libraw-rs (MIT OR Apache-2.0)
    xmp/              sc:Sidecar schema on top of xmp_toolkit
    template/         storage-template rendering and date/name lookup
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
| Lens correction — distortion, lateral CA, vignetting | Vignetting as a gain before WB; distortion and CA as a coordinate remap when sampling `sourceTex`, from a displacement texture | Same WGSL, after denoise and before develop | `sc:Lens` — profile, per-correction amounts |
| Crop and straighten | Composed into the view pass `geometry` matrix; the histogram counts only pixels inside the crop | Before resampling to `longEdge` | `sc:Crop` — angle and edges, normalized to the lens-corrected image. An existing Immich crop on the RAW can be offered once as a starting point. |
| Local masks — linear and radial gradients, brush, subject/sky | Second bind group in the develop pass: each module's inputs blend by mask weight | Same shaders at full resolution | `sc:Masks` — shapes plus adjustments named like the global ones |

**Lens correction.** Profiles come from DNG opcodes when the file carries them (iPhone ProRAW does), otherwise from the lensfun database, matched on the EXIF lens model. The models are reimplemented from lensfun's documentation, and the database (CC BY-SA 3.0) is downloaded on first use, not shipped (§12). Absent `sc:Lens` means off, so photos edited in V1 render unchanged; new edits default to on.

**Masks.**

- Weights: gradients are analytic in the shader. Brush strokes — normalized points, radius, feather, flow — are rasterized by a WGSL compute pass into an `r8unorm` texture array, at proxy size on the phone and full size on the server, so both sides share the rasterizer.
- Subject and sky masks run on the RTX card in the worker. The sidecar stores their inputs (model version, tap points); the raster lives in `/cache` and is recomputed if the cache is gone, so nothing binary lands beside the RAW.
- Blending: `ev = global.ev + Σ wₖ · maskₖ.ev`, and likewise for WB multipliers, contrast, saturation and mixer values. Up to 8 masks per photo in the first release, in a fixed-size uniform array.
- Display: the selected mask shows as desaturation outside it, which keeps red and blue reserved for clipping.

**Hooks that ship in V1** so all three arrive without migrations:

1. The view pass already applies a `geometry` matrix; crop and straighten compose into it.
2. Develop modules take values as arguments and never read the uniform buffer directly.
3. Loupe coordinates are normalized to the oriented image, not proxy pixels.
4. The XMP writer preserves unknown `sc:` elements, and a newer `ProcessVersion` opens read-only.
5. `EditParams` treats `lens`, `crop` and `masks` as optional, with absent meaning off.
