# Sidecar

A RAW editor beside [Immich](https://immich.app). Share a photo from the Immich app, edit it on the GPU in your
browser, and your server renders every version back into Immich, stacked on the original.

- **Edits live beside your photos** as XMP sidecars (`DSC_1234.NEF.xmp`), not in a database. Other tools' fields are kept.
- **Preview and export run the same shaders** (WGSL): WebGPU in the browser, wgpu on Vulkan on the server.
- **GPU RAW pipeline:** highlight reconstruction, RCD demosaicing, wavelet noise reduction, Richardson–Lucy sharpening.
- **Also edits JPEG, PNG and TIFF**, and HEIC in a locally built image — see [Installing Sidecar](docs/INSTALL.md).
- **Every export is kept** as a numbered version (`DSC_1234-v2.jpg`), and the newest JPEG leads the stack in Immich.
- **You edit in the space you export to** — sRGB or Display P3 — so the viewport is a soft proof, with clipping
  warnings, focus peaking and an autofocus-point marker over it.

Status: early. See [`docs/DESIGN.md`](docs/DESIGN.md) for the specification and the build log, and
[`docs/ROADMAP.md`](docs/ROADMAP.md) for what's missing or known wrong.

## Requirements

- Immich 3.x, and the photo libraries mounted on the same machine
- An NVIDIA GPU with the container toolkit (Vulkan), for the worker
- HTTPS in front of Sidecar — browsers only offer WebGPU on secure pages. Use your reverse proxy, a Cloudflare Tunnel,
  or `tailscale serve`.
- Any modern browser. Safari 26 (iOS 26) and current Chromium edit live over WebGPU; anything else — Firefox,
  older Safari — falls back to a server-rendered preview automatically.

## Run it

1. Copy [`deploy/compose.yaml`](deploy/compose.yaml) and write a `sidecar.toml` (see
   [`deploy/remote-proxy/sidecar.toml`](deploy/remote-proxy/sidecar.toml) for a full example).
2. `docker compose up -d`, then put HTTPS in front of port 8080.
3. Sign in with an Immich API key with `user.read, asset.read, asset.view, asset.upload, asset.update, stack.create`.
4. On iPhone, create the [Edit in Sidecar shortcut](ios/README.md). On Android, install Sidecar as an app and use Share.

See [Installing Sidecar](docs/INSTALL.md) for a build option that adds HEIC support.

## Dependencies

LibRaw is vendored as a submodule, so clone with it — the build fails without it:

```bash
git clone --recurse-submodules https://github.com/LeoSavard/sidecar-immich.git
```

For a clone you already have: `git submodule update --init`.

**To build:** a Rust toolchain (edition 2024) with the `wasm32-unknown-unknown` target, Node 24, a C/C++
compiler for LibRaw, `pkg-config`, zlib and libjpeg headers, a Vulkan loader and driver, and ExifTool.
[`dev/Dockerfile`](dev/Dockerfile) pins the exact set, and `./dev/run` builds inside it, so none of this has to
be installed on the host.

**To run the image:** a Vulkan loader with a working ICD — for NVIDIA, the container toolkit's CDI device —
plus `libegl1`, `libxext6` and `libx11-6`, without which NVIDIA's ICD fails to initialise, and ExifTool for
copying metadata into exports.

**Rust:** wgpu and naga 30 (GPU, WGSL), axum with tower-http and tokio (server), reqwest (Immich), the Adobe
XMP Toolkit (sidecars), jpeg-encoder, tiff and png (encoders), palette (colour), chacha20poly1305 (sealing
API keys at rest), plus serde, toml, anyhow, tracing, sha1, hex, base64, bytemuck and half.

**Web:** SvelteKit 2 with Svelte 5 and Tailwind v4, built by Vite; Playwright drives Chromium for the parity
harness only.

Every third-party project, its licence and what it is used for is listed in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) and [`NOTICE`](NOTICE); `cargo deny check licenses`
enforces the allowlist in CI.

## Develop

Everything builds and runs in a container with the GPU: `./dev/run <command>`, `./dev/serve` for a local server.

## Built on

- [LibRaw](https://www.libraw.org) (CDDL-1.0), which builds on dcraw by Dave Coffin
- [vkdt](https://github.com/hanatos/vkdt) by Johannes Hanika (BSD-2-Clause): demosaic and highlight reconstruction shaders
- [wgpu and naga](https://wgpu.rs), [Adobe XMP Toolkit](https://github.com/adobe/xmp-toolkit-rs), [ExifTool](https://exiftool.org)
- [ACES reference gamut compression](https://github.com/aces-aswf/aces-core), [OKLab](https://bottosson.github.io/posts/oklab/)
- [Compact-ICC-Profiles](https://github.com/saucecontrol/Compact-ICC-Profiles) by Clinton Ingram (CC0)
- SvelteKit, Tailwind CSS, axum, jpeg-encoder, and the crates listed in `THIRD_PARTY_NOTICES.md`

Licensed under Apache-2.0. See [NOTICE](NOTICE) and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
