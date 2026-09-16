# Sidecar — working rules

Read `docs/DESIGN.md` before changing behaviour; it is the specification. Build and test only inside the dev
container: `./dev/run <command>` (GPU, Rust, Node and Chromium are there; nothing is installed on the host).

## Licensing — non-negotiable

Sidecar is published as an Apache-2.0 image. Every file you add must keep that possible.

- **Allowed to port or copy, with credit:** vkdt (BSD-2-Clause; check each file's header — a few vkdt files
  are GPL-3.0 and are off limits), LibRaw (CDDL-1.0; a port keeps the CDDL-1.0 header), aces-core
  (Apache-2.0), OpenColorIO (BSD-3-Clause), colour-science (BSD-3-Clause), any MIT / Apache-2.0 / BSD / ISC /
  Zlib / CC0 project. Published papers are always fine.
- **Never copy, translate or closely paraphrase code from:** darktable, RawTherapee, the original
  RCD-Demosaicing repository, rawler, rawloader, Immich's source (AGPL-3.0), or any GPL/LGPL/AGPL project.
  Reading their documentation is fine; reproducing their code from memory is not.
- Every WGSL file starts with `// SPDX-License-Identifier: <id>`; a ported file also names its origin, the
  upstream path and commit, and the original copyright. `cargo test -p sidecar-shaders` enforces the header.
- New Rust crates must pass `cargo deny check licenses` (see `deny.toml`). New npm packages must be MIT,
  Apache-2.0, BSD, ISC or similar.
- Add every new third-party project to `NOTICE` and the hand-kept section of `THIRD_PARTY_NOTICES.md`.

## Safety on the real server

- The photo libraries under `/mnt/volume/` are the user's. Read them freely; never write, rename or delete
  there during development. Tests use copies under `testdata/cache/`.
- Don't touch the running Immich containers or their data.

## Conventions

- Shaders: modules take values as arguments; only entry files (`shaders/client/*`, `shaders/server/*`) read
  bindings. Declare dependencies with `//! requires:`.
- Uniform layouts are mirrored by hand in Rust and verified against the WGSL by naga in tests.
- Parity: `web/scripts/parity-browser.mjs` + `sidecar-dev parity …`; mean ΔE2000 ≤ 0.5, p99 ≤ 2.
