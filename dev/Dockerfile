# Development toolchain: Rust, Node, Vulkan (NVIDIA via CDI, lavapipe fallback), Chromium for WebGPU parity tests.
FROM rust:1-trixie
RUN apt-get update && apt-get install -y --no-install-recommends \
      libvulkan1 vulkan-tools mesa-vulkan-drivers libegl1 libxext6 libx11-6 \
      libimage-exiftool-perl zlib1g-dev libjpeg62-turbo-dev pkg-config ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=node:24-trixie-slim /usr/local/bin/node /usr/local/bin/node
COPY --from=node:24-trixie-slim /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm \
 && ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx \
 && node --version && npm --version
# The browser build must match the playwright version in web/package-lock.json; bump them together.
RUN npx -y playwright@1.63.0 install --with-deps chromium
RUN rustup component add rustfmt clippy && rustup target add wasm32-unknown-unknown
# sccache (Apache-2.0; dev image only, never shipped): each worktree gets its own target directory (dev/run),
# and this makes the first build in a new one mostly cache hits — Rust dependencies and LibRaw's C++ alike.
ARG SCCACHE=0.18.0
RUN set -e; arch="$(uname -m)"; \
    case "$arch" in \
      x86_64)  sum=45f1447fbe231e3037bde351ef70677dd212216c8d62ae7ca409fecc4d6acc89 ;; \
      aarch64) sum=2b3284d5da3b46a47dc4229e75bb7b88ac4aa99c8d754fb7d2f84997e5a4354a ;; \
    esac; \
    f="sccache-v${SCCACHE}-${arch}-unknown-linux-musl"; \
    curl -fsSL -o /tmp/s.tgz "https://github.com/mozilla/sccache/releases/download/v${SCCACHE}/${f}.tar.gz"; \
    echo "$sum  /tmp/s.tgz" | sha256sum -c -; \
    tar -xzf /tmp/s.tgz -C /tmp && install -m 755 "/tmp/${f}/sccache" /usr/local/bin/sccache && rm -rf /tmp/s.tgz "/tmp/${f}"
ENV CARGO_TARGET_DIR=/cache/target CARGO_HOME=/cache/cargo NPM_CONFIG_CACHE=/cache/npm \
    RUSTC_WRAPPER=sccache SCCACHE_DIR=/cache/sccache SCCACHE_CACHE_SIZE=40G
WORKDIR /src
