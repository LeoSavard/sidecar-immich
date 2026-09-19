# Installing Sidecar

Sidecar runs as two containers — `sidecar-api` and `sidecar-worker` — from one image. There are two ways to
get that image, depending on whether your library has HEIC photos in it.

## Option 1: the published image

`ghcr.io/leosavard/sidecar-immich` is what CI builds and publishes from every tagged release. It edits every
RAW format LibRaw reads, plus JPEG, PNG and TIFF — not HEIC. See "Why no HEIC" below for why that's
deliberate, not an oversight.

1. Copy [`deploy/compose.yaml`](../deploy/compose.yaml) and write a `sidecar.toml` — see
   [`deploy/remote-proxy/sidecar.toml`](../deploy/remote-proxy/sidecar.toml) for a full worked example.
2. `docker compose up -d`, then put HTTPS in front of port 8080 (your reverse proxy, a Cloudflare Tunnel, or
   `tailscale serve`).
3. Sign in with an Immich API key scoped to `user.read, asset.read, asset.view, asset.upload, asset.update,
   stack.create`.
4. On iPhone, create the [Edit in Sidecar shortcut](../ios/README.md). On Android, install Sidecar as an app
   and use Share.

You can also open a photo that isn't on the server yet: choose or drag one onto Sidecar's home page and it is
uploaded to your Immich library first, then opened like any other. Exports go to Immich, to the device you're
editing on, or both — on a phone that's the share sheet, ending in *Save Image*.

### The `user:` of the api container has to be able to write

Sidecar stores an edit in an XMP file *beside the original*, so `sidecar-api` needs write access to the folders
your photos are in — that's the one thing the compose file's `user: "1000:1000"` is there for, and the comment
"the owner of your photo files" is meant literally.

Immich's own internal library (`UPLOAD_LOCATION`) is a common place to get this wrong: Immich runs as root by
default, so it writes that tree as `root:root` with mode 755, and a `sidecar-api` running as uid 1000 can read
every photo in it and save nothing. The symptom is that editing works normally and the first save fails.
Sidecar now says so when the photo opens rather than after the edit, but the fix is on your side — either give
that account write access to the tree (`chown -R 1000:1000`, or a group both accounts are in), or run
`sidecar-api` as whatever user owns it. `sidecar-worker` stays read-only; it never writes to a library.

Your browser doesn't need WebGPU to use Sidecar: Safari 26 and current Chromium edit live in the browser;
anything else — Firefox, older Safari — falls back to the server developing each change and sending back a
JPEG. Slower per change, same result, no separate setup.

## Option 2: build locally, with HEIC

If your library is mostly iPhone photos, build your own image with a HEIF decoder installed:

```bash
git clone --recurse-submodules https://github.com/LeoSavard/sidecar-immich.git
cd sidecar-immich
docker build --build-arg HEIF=1 -t sidecar:local .
```

Then point `deploy/compose.yaml`'s `image:` at `sidecar:local` instead of the `ghcr.io` tag, and follow steps
1–4 above.

### Why no HEIC in the published image

Every capable HEIF decoder is LGPL (`libheif`), and Sidecar's published image is Apache-2.0 — linking an
LGPL library into it, even dynamically, isn't something this project does. HEVC decoding also carries
patent-pool royalty exposure, independent of that copyright question. So Sidecar never links a HEIF decoder:
it runs one as a separate program if it finds one installed, the same arrangement the image already uses for
ExifTool. `--build-arg HEIF=1` is what installs one; the published image doesn't set that flag, so it ships
with none and reports HEIC files as unsupported until you build your own.

Once you do, HEIC files decode and edit like any other photo — white balance and tone become relative
adjustments around the file's own colour, since a HEIC has already been developed by whatever wrote it and
there's no sensor data left to recover a blown highlight from.

## Everything else

[`docs/DESIGN.md`](DESIGN.md) is the specification. [`docs/ROADMAP.md`](ROADMAP.md) is what's missing or
known wrong.
