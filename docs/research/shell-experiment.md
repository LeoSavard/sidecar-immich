# The shell experiment: WebGPU in Electron on Linux, and colour through wgpu

*Run 2026-11-06 on the dev server, by V2's rule that this is "an experiment, not a search". The probe and
how to rerun it are in [`dev/shell-probe/`](../../dev/shell-probe/).*

V2's "Measurements to run" put two questions first, each gating the desktop target's architecture:

1. **Does Electron actually give WebGPU on Linux?** — `enable-unsafe-webgpu` plus Vulkan, on AMD/Mesa and on
   NVIDIA.
2. **Does wgpu expose Wayland `color-management-v1` surfaces or HDR swapchain formats?** — "the single most
   load-bearing unknown".

## What this machine could and couldn't test

A headless server: an NVIDIA RTX A4000 (driver 615.78), Mesa's software Vulkan (llvmpipe), no display, no AMD
or Intel GPU. Electron ran in the dev container (Debian trixie) in a *real window* — under Xvfb for X11, under
Weston's headless backend for Wayland — not in Chromium's headless mode, because a desktop shell has to show
its canvas, which the CI parity run never does. So:

- **Settled here:** NVIDIA under X11, including drawing to the window and Sidecar's parity fixtures.
- **Indicative only:** Wayland, through a headless Weston (GL renderer on the NVIDIA GPU) in a container
  without the display device.
- **Not tested:** AMD and Intel hardware, a real desktop compositor (KWin, Mutter), macOS, Windows. The probe
  is written to be rerun on a desktop for exactly these (`dev/shell-probe/README.md`).

## 1. Electron and WebGPU on Linux

| Run | Electron (Chromium) | Platform | Flags | WebGPU adapter | Reaches the window |
|---|---|---|---|---|---|
| x11-default | 44.9.3 (152) | X11 | none | none | — |
| x11-unsafe | 44.9.3 (152) | X11 | `enable-unsafe-webgpu` | SwiftShader (CPU fallback) | no: device lost |
| x11-vulkan, …-angle | 44.9.3 (152) | X11 | + Vulkan, `ignore-gpu-blocklist` | SwiftShader | no |
| headless-vulkan | 44.9.3 (152) | headless | the CI parity flags | none | — |
| *Playwright Chromium* | *(153)* | *headless* | *the CI parity flags* | ***NVIDIA Ampere*** | *(headless)* |
| **e45-x11-default** | 45.0.0 (156) | X11 | none | none | — |
| **e45-x11-vulkan** | 45.0.0 (156) | X11 | `enable-unsafe-webgpu`, `enable-features=Vulkan`, `use-vulkan=native`, `ignore-gpu-blocklist` | **NVIDIA Ampere** | **yes: (200, 40, 40) read back exactly** |
| **e45-x11-inapp** | 45.0.0 (156) | X11 | the same, set by the app (`app.commandLine`) | **NVIDIA Ampere** | **yes** |
| e45-wayland-vulkan | 45.0.0 (156) | Wayland | the same + `ozone-platform=wayland` | none | — |
| e45-wayland-gl-vulkan | 45.0.0 (156) | Wayland, Weston on the GPU | the same | NVIDIA Ampere | no: device lost on `requestDevice` |
| e45-wayland-novk | 45.0.0 (156) | Wayland, Weston on the GPU | without `enable-features=Vulkan` | SwiftShader | no |

**Parity inside the Electron 45 window, X11, NVIDIA: 21 of 21 cases pass**, worst max ΔE2000 0.099
(`dev-staining-layers`) against the gate of mean ≤ 0.5 and p99 ≤ 2 — the same numbers Chromium gives in CI.
The develop pipeline runs in Electron exactly as it does natively.

What the X11 window's WebGPU offers, in the configurations the editor uses: `shader-f16` and
`float32-filterable`; a canvas configured as sRGB, as Display P3, as `rgba16float` Display P3, and with
`toneMapping: extended` — all accepted. Accepted is not the same as shown correctly: Xvfb has no colour
management, so this says nothing about what a calibrated display would show (below).

**Findings**

- **Yes, on X11, with the flags — and the app can set them.** Without flags there is no adapter at all;
  with them, set either on the command line or by the app before it is ready, Electron 45 gets the NVIDIA GPU,
  draws to its window and passes parity. That is the property V2 wanted from Electron: the app, not the user,
  turns WebGPU on.
- **The Chromium version decides it.** Electron 44 (Chromium 152) gets no hardware adapter with any flags;
  Electron 45 (Chromium 156) and Playwright's Chromium 153 do. A shell that ships its own pinned Chromium
  controls this; "your browser" does not — a user's Chromium a few versions old simply doesn't work.
- **Native Wayland is out, by Chromium's own statement.** Its log says it outright:
  `'--ozone-platform=wayland' is not compatible with Vulkan. Consider switching to '--ozone-platform=x11' or
  disabling Vulkan`. Without the Vulkan feature it falls back to SwiftShader. (The headless Weston adds its own
  failures — no display device in the container — but the incompatibility message is Chromium's, not the
  test's.) So an Electron Sidecar on a Wayland desktop runs as an **X11 client under XWayland**, which is what
  Electron did by default until recently, forced with `--ozone-platform=x11`.
- **What XWayland costs is colour management.** Under X11 there is no `color-management-v1`; Chromium on
  Wayland does look for `wp_color_management_surface_v1` (its log notes the test compositor lacks it), but
  that is the path Vulkan is incompatible with. Whether Chromium on X11 still converts a Display P3 canvas
  for the screen's ICC profile (the `_ICC_PROFILE` atom) was not tested here and needs a desktop.

## 2. wgpu and colour on Linux

Read from wgpu 30.0.1 — the version Sidecar builds — and its tracker:

- **Wide-gamut and HDR swapchains are exposed.** `wgpu_types::SurfaceColorSpace` has `Srgb`,
  `ExtendedSrgbLinear` (scRGB), `DisplayP3`, `ExtendedSrgb`, `ExtendedDisplayP3`, `Bt2100Pq` (HDR10) and
  `Bt2100Hlg`, chosen per surface configuration, with `Auto` reproducing the old behaviour.
- **On Wayland, Vulkan's window-system layer carries them to the compositor.** gfx-rs/wgpu#10545 (opened
  2026-10 by someone building a photo editor on wgpu and winit) reports that on Mesa/Wayland the WSI attaches
  an image description for the chosen colour space — with perceptual intent.
- **What is missing is the application owning that description.** A photo editor wants to declare its own
  (linear Rec.2020, a reference luminance) and to choose relative-colorimetric intent, which proofing needs.
  Vulkan's `VK_COLOR_SPACE_PASS_THROUGH_EXT` leaves the description to the application; wgpu 30 filters it
  out. PR gfx-rs/wgpu#10546 maps it (tested by its author on KWin with Intel/Mesa and NVIDIA 610), open and
  awaiting review since 2026-10.

So the answer to V2's most load-bearing question is **yes for the colour spaces, not yet for full control**:
a native wgpu surface can present Display P3 and HDR on a colour-managed Wayland desktop today, and can own
its description and intent once #10546 or something like it lands. Not verified here: the Mesa side, and
NVIDIA's WSI behaviour, which needs a desktop.

## What it means for the shell

The two routes V2 ranked now look like this:

| | Electron (or CEF, or a bundled Chromium) | Hybrid: thin shell + native wgpu canvas |
|---|---|---|
| Runs the editor as it is | **Yes**: the same web app, parity 21/21 | No: the canvas side is new code (the WGSL is shared) |
| WebGPU on Linux | Yes on X11/XWayland with app-set flags, given a recent enough Chromium; not native Wayland | Not needed: wgpu straight to Vulkan |
| Colour on Linux | sRGB / P3 canvas tagging; no Wayland colour management under XWayland | P3 and HDR swapchains through Wayland colour management now; full control with wgpu pass-through |
| Size (Flatpak) | about +250 MB on a ~100 MB core | the core, plus a small shell |
| Untested | AMD, Intel, a real compositor, `_ICC_PROFILE` on X11 | the whole canvas |

Neither is a reason to stop. Electron is the short path to a desktop app that behaves exactly like today's
editor; the native canvas is the only path to Wayland colour management and HDR. They also compose: the
shell's window can come first, and the image canvas can move to a native surface later without the rest of
the UI changing. The decision is the owner's.

## Windows and macOS (a desk estimate, 2026-11-06 — nothing below was run)

**The renderer is the cheap part.** wgpu targets Metal and DX12 (or Vulkan) with the same WGSL; the work is
proving it, by running the parity harness against each backend on real hardware, since Metal and DX12 differ
in f16 and filtering details. Windows CI runners can run DX12 in software (WARP); whether macOS runners
expose Metal to a job is unchecked.

**The native dependencies are where the effort is**, read from the code:

| | macOS | Windows |
|---|---|---|
| LibRaw (`libraw-sys`, `cc`) | builds; OpenMP needs `libomp`, since Apple clang has none and the build asks for `-fopenmp` and `gomp` | MSVC has OpenMP; zlib and libjpeg vendored or static rather than system |
| ExifTool (8 call sites) | Perl ships with macOS but is deprecated: bundle it | a standalone `exiftool.exe` exists: bundle it |
| HEIC (`heif-dec`, a separate program for licence reasons) | use ImageIO instead — no licence question and a better decoder | bundle `heif-dec`; Windows' own decoder needs the paid HEVC extension. The HEVC pool question in V2 stays open |
| ML (burn on wgpu) | Metal | DX12 / Vulkan |
| Platform code | a few `cfg(unix)` permission lines | the same |

**The window and the web UI.** macOS is the easiest of the three: a Metal-backed view under a WKWebView is an
ordinary view hierarchy, and macOS is colour-managed system-wide — wgpu already sets the `CAMetalLayer` colour
space for Display P3, extended P3 and EDR (its 2026 Metal colour-space fixes show it in use). That closes, on
paper, the survey's "macOS ColorSync and EDR were not researched". Windows is more work: WebView2 over a DX12
swapchain hits the airspace problem (a child HWND cannot be transparent over another surface), which
WebView2's composition-hosting mode solves at some complexity; colour is good once done — render scRGB or
HDR10 and let Windows' colour management convert, which is Microsoft's own advice. WebView2 also supports
WebGPU, so Windows alone could keep today's in-page rendering and skip a native canvas.

**Packaging.** macOS: an `.app` in a `.dmg` or a Homebrew cask; the Apple Developer Program (~$99/year) plus
notarisation, or Gatekeeper blocks it; outside the App Store a sidecar beside the original needs nothing
special, inside it the sandbox's related-items mechanism exists for exactly sidecar files. Windows: MSIX or
MSI and `winget`; signing is the hard part — unsigned means SmartScreen warnings, Azure's cheap signing service
reportedly needs an organisation with a verifiable multi-year history in a limited set of countries, and a
traditional certificate costs a few hundred dollars a year with reputation that builds slowly.

**Rough effort, a judgement:** macOS 2–4 weeks plus $99/year; Windows 3–6 weeks plus a signing arrangement.
macOS can be as seamless as Linux once signed; Windows can come close, gated on signing rather than code.

## Where this leaves the desktop target

**Parked by the owner, 2026-11-06:** they have no personal need for desktop integration. It stays on the
roadmap, to be picked up if the need arises; this page and `dev/shell-probe/` are where it restarts.
