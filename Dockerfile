# Sidecar — one image, two commands: `sidecar api` (public HTTP, sidecar writer) and `sidecar worker` (RAW, GPU).
FROM rust:1-trixie AS build
RUN apt-get update && apt-get install -y --no-install-recommends zlib1g-dev libjpeg62-turbo-dev pkg-config \
 && rm -rf /var/lib/apt/lists/* && rustup target add wasm32-unknown-unknown
COPY --from=node:24-trixie-slim /usr/local/bin/node /usr/local/bin/node
COPY --from=node:24-trixie-slim /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target \
    cargo build --release -p sidecar -p sidecar-devtools \
 && cargo build --release -p sidecar-color-wasm --target wasm32-unknown-unknown \
 && cp target/release/sidecar /usr/local/bin/sidecar \
 && ./target/release/sidecar-dev shaders web/src/lib/gpu/wgsl \
 && cp target/wasm32-unknown-unknown/release/sidecar_color_wasm.wasm web/src/lib/color/sidecar_color.wasm
RUN --mount=type=cache,target=/root/.npm cd web && npm ci --no-audit --no-fund && npm run build

FROM debian:trixie-slim
ARG VERSION=dev
ARG REVISION=unknown
# `image.source` is what links the package to its repository on GHCR, including its visibility.
LABEL org.opencontainers.image.title="Sidecar" \
      org.opencontainers.image.description="A RAW editor beside Immich" \
      org.opencontainers.image.source="https://github.com/LeoSavard/sidecar-immich" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.revision="${REVISION}" \
      org.opencontainers.image.version="${VERSION}"
# libEGL, libXext and libX11 are needed by NVIDIA's Vulkan driver inside containers.
RUN apt-get update && apt-get install -y --no-install-recommends \
      libvulkan1 libegl1 libxext6 libx11-6 libgomp1 libjpeg62-turbo zlib1g libimage-exiftool-perl ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 1000 --create-home sidecar \
 && mkdir -p /cache /state && chown sidecar:sidecar /cache /state
COPY --from=build /usr/local/bin/sidecar /usr/local/bin/sidecar
COPY --from=build /src/web/build /usr/share/sidecar/web
COPY LICENSE NOTICE THIRD_PARTY_NOTICES.md /usr/share/doc/sidecar/
COPY third_party/libraw/LICENSE.CDDL /usr/share/doc/sidecar/LibRaw-LICENSE.CDDL
COPY assets/icc/LICENSE /usr/share/doc/sidecar/Compact-ICC-Profiles-LICENSE
ENV NVIDIA_DRIVER_CAPABILITIES=compute,utility,graphics
USER sidecar
ENTRYPOINT ["sidecar"]
CMD ["api", "--config", "/etc/sidecar.toml"]
