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
RUN npx -y playwright@1 install --with-deps chromium
RUN rustup component add rustfmt clippy && rustup target add wasm32-unknown-unknown
ENV CARGO_TARGET_DIR=/cache/target CARGO_HOME=/cache/cargo NPM_CONFIG_CACHE=/cache/npm
WORKDIR /src
