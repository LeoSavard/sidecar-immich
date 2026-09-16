# Sidecar

A RAW editor beside [Immich](https://immich.app). Share a photo from the Immich app, edit it on the GPU in your
browser, and your server renders every version back into Immich, stacked on the original.

- **Edits live beside your photos** as XMP sidecars (`DSC_1234.NEF.xmp`), not in a database. Other tools' fields are kept.
- **Preview and export run the same shaders** (WGSL): WebGPU in the browser, wgpu on Vulkan on the server.
- **GPU RAW pipeline:** highlight reconstruction, RCD demosaicing, wavelet noise reduction, Richardson–Lucy sharpening.
- **Every export is kept** as a numbered version (`DSC_1234-v2.jpg`), and the newest JPEG leads the stack in Immich.

Status: early. See [`docs/DESIGN.md`](docs/DESIGN.md) for the specification and the build log.

## Requirements

- Immich 3.x, and the photo libraries mounted on the same machine
- An NVIDIA GPU with the container toolkit (Vulkan), for the worker
- HTTPS in front of Sidecar — browsers only offer WebGPU on secure pages. Use your reverse proxy, a Cloudflare Tunnel,
  or `tailscale serve`.
- A browser with WebGPU: Safari 26 (iOS 26), or a current Chromium

## Run it

1. Copy [`deploy/compose.yaml`](deploy/compose.yaml) and write a `sidecar.toml` (see
   [`deploy/remote-proxy/sidecar.toml`](deploy/remote-proxy/sidecar.toml) for a full example).
2. `docker compose up -d`, then put HTTPS in front of port 8080.
3. Sign in with an Immich API key with `user.read, asset.read, asset.view, asset.upload, asset.update, stack.create`.
4. On iPhone, create the [Edit in Sidecar shortcut](ios/README.md). On Android, install Sidecar as an app and use Share.

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
