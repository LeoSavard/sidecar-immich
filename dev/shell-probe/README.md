# Shell probe

V2's shell experiment (`docs/research/shell-experiment.md`): does Electron give WebGPU on Linux, in a real
window, and does Sidecar's develop pipeline render the same there? Not part of the app and not shipped.

```bash
# In the dev container (installs GTK and Weston for that run; the container is thrown away after):
./dev/run bash -c 'dev/shell-probe/deps.sh && dev/shell-probe/run.sh <name> file:///src/dev/shell-probe/probe.html \
    enable-unsafe-webgpu,enable-features=Vulkan,use-vulkan=native,ignore-gpu-blocklist'
python3 dev/shell-probe/summary.py            # one block per result in testdata/cache/shell-probe/
```

The third argument is the Chromium switches the *app* sets (`app.commandLine`); `CHROMIUM_FLAGS="--…"` passes
them on the command line instead. With no display, `run.sh` starts Xvfb; with `WAYLAND_DISPLAY` set and
`--ozone-platform=wayland` it uses that compositor.

**On a desktop** (outside the container — AMD, Intel, a real KWin or Mutter session are what the server could
not test): `npm install` here, then `./run.sh <name> file://$PWD/probe.html <switches>` uses your display.

**Parity in the window:** serve `web/build` with `vite preview` on port 4179, then
`run.sh <name> http://127.0.0.1:4179/debug/parity <switches> --parity-out=<dir>`, with `<dir>` under
`testdata/cache/` (git ignores it). Copy the `*.native.f16` renders from `sidecar-dev parity native` into
`<dir>`, and run `sidecar-dev parity compare web/static/parity <dir>`.
