# syntax=docker/dockerfile:1
# Sidecar — one image, two commands: `sidecar api` (public HTTP, sidecar writer) and `sidecar worker` (RAW, GPU).
# `--target ml` builds the same image plus `sidecar-ml` and its models, published with a `-ml` tag suffix.
# `--target mobile` (and `ml-mobile`) adds the mobile module at /m, published with a `-mobile` suffix
# (docs/MOBILE.md). Each optional layer is a target, so an image carries only what its operator chose.
# The models sidecar-ml runs, pinned by SHA-256, converted to ONNX that burn-onnx imports. Two need a Python
# tool each; `dev/ml-models` builds `ml-onnx` from here too, so there is one list of what the models are.
# The conversions are pinned by hash as well: the code in crates/ml/src/models/ is generated from exactly
# these files, and a converter that starts writing something else must stop the build, not ship weights the
# code can't load.
FROM scratch AS ml-sources
ADD --checksum=sha256:8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4 \
    https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx /w/yunet.onnx
ADD --checksum=sha256:5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333 \
    https://huggingface.co/onnx-community/BiRefNet_lite-ONNX/resolve/main/onnx/model.onnx /w/birefnet_lite_dynamic.onnx
ADD --checksum=sha256:c6748b1253a99067ef71f7e26ca71096cd449baefa8f101900ea23016507e0e0 \
    https://storage.googleapis.com/mediapipe-models/image_segmenter/selfie_multiclass_256x256/float32/latest/selfie_multiclass_256x256.tflite /w/multiclass.tflite
ADD --checksum=sha256:afb6a5c28f3b6bf1618c6e43f02073ef9dfdc70e937502d51603e57b0a1df10c \
    https://huggingface.co/onnx-community/depth-anything-v2-small/resolve/main/onnx/model.onnx /w/depth_dynamic.onnx

# BiRefNet lite, simplified at its fixed 1024² input: that resolves the dynamic shapes burn-onnx can't import.
FROM python:3.12-slim AS ml-birefnet
RUN pip install --no-cache-dir --root-user-action=ignore onnx==1.23.0 onnxsim==0.7.3
COPY --from=ml-sources /w/birefnet_lite_dynamic.onnx /w/
RUN cd /w && onnxsim birefnet_lite_dynamic.onnx birefnet.onnx --overwrite-input-shape 1,3,1024,1024 \
 && echo "d46e8e42a87b10478f3b7c710e1903cc45b4cd749de802ef0d53714b101cd952  birefnet.onnx" | sha256sum -c

# MediaPipe's multi-class model ships as TFLite.
FROM python:3.12-slim AS ml-multiclass
RUN pip install --no-cache-dir --root-user-action=ignore onnx==1.23.0 tf2onnx==1.17.0 tensorflow-cpu==2.21.0
COPY --from=ml-sources /w/multiclass.tflite /w/
COPY dev/ml/canonical_names.py /w/
# tf2onnx names folded constants in an order that changes between runs; renamed by first use, the conversion
# is the same file every time, which the committed generated code depends on.
RUN cd /w && python -m tf2onnx.convert --tflite multiclass.tflite --output converted.onnx --opset 17 \
 && python canonical_names.py converted.onnx multiclass.onnx \
 && echo "68cd929ca26e76743337685f155708672495032037277eee35af2fcaaf6a5f05  multiclass.onnx" | sha256sum -c

# Depth Anything V2 Small, simplified at its 518² input like BiRefNet. Only Small is Apache-2.0; Base and Large
# are CC-BY-NC.
FROM python:3.12-slim AS ml-depth
RUN pip install --no-cache-dir --root-user-action=ignore onnx==1.23.0 onnxsim==0.7.3
COPY --from=ml-sources /w/depth_dynamic.onnx /w/
RUN cd /w && onnxsim depth_dynamic.onnx depth.onnx --overwrite-input-shape 1,3,518,518 \
 && echo "f2f2a6917075c03c841dc24ca46a27fa8ddbbf86cb018506fd5b6254a1da1a0a  depth.onnx" | sha256sum -c

FROM scratch AS ml-onnx
COPY --from=ml-sources /w/yunet.onnx /
COPY --from=ml-birefnet /w/birefnet.onnx /
COPY --from=ml-multiclass /w/multiclass.onnx /
COPY --from=ml-depth /w/depth.onnx /

FROM rust:1-trixie AS build
RUN apt-get update && apt-get install -y --no-install-recommends zlib1g-dev libjpeg62-turbo-dev pkg-config \
 && rm -rf /var/lib/apt/lists/* && rustup target add wasm32-unknown-unknown
COPY --from=node:24-trixie-slim /usr/local/bin/node /usr/local/bin/node
COPY --from=node:24-trixie-slim /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target \
    cargo build --release -p sidecar -p sidecar-shaders \
 && cargo build --release -p sidecar-color-wasm --target wasm32-unknown-unknown \
 && cp target/release/sidecar /usr/local/bin/sidecar \
 && ./target/release/sidecar-shaders web/src/lib/gpu/wgsl \
 && cp target/wasm32-unknown-unknown/release/sidecar_color_wasm.wasm web/src/lib/color/sidecar_color.wasm
RUN --mount=type=cache,target=/root/.npm cd web && npm ci --no-audit --no-fund && npm run build

# The mobile module: its own SvelteKit app over the main app's `$lib`, so it builds here, after the main app has
# generated the WGSL and the colour wasm the two share.
FROM build AS build-mobile
RUN --mount=type=cache,target=/root/.npm cd mobile && npm ci --no-audit --no-fund && npm run build

# sidecar-ml and its weights. The import runs first and sidecar-ml is compiled after it, so the program is
# always built from the same conversion as the weights it ships with: weights are looked up by the names the
# generated code gives them. (Compiled the other way round, a conversion that numbered its constants
# differently from the committed code shipped weights that failed to load.)
FROM build AS build-ml
COPY --from=ml-onnx / /models/src/
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target \
    cargo build --release -p sidecar-ml --features import --bin sidecar-ml-import \
 && ./target/release/sidecar-ml-import /models \
 && cargo build --release -p sidecar-ml --features import --bin sidecar-ml \
 && cp target/release/sidecar-ml /usr/local/bin/sidecar-ml \
 && rm -r /models/src

FROM debian:trixie-slim AS runtime
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
# `/photos` exists in the image, owned by the user Sidecar runs as, so a deployment with no Immich can mount
# a named volume there and have the editing folder (DESIGN.md §3) be writable. Docker takes a fresh volume's
# ownership from the image's directory at that path; a path the image does not have becomes root-owned, which
# the uid-1000 process could not write to -- the same reason /cache and /state are created here.
RUN apt-get update && apt-get install -y --no-install-recommends \
      libvulkan1 libegl1 libxext6 libx11-6 libgomp1 libjpeg62-turbo zlib1g libimage-exiftool-perl ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 1000 --create-home sidecar \
 && mkdir -p /cache /state /photos && chown sidecar:sidecar /cache /state /photos

# Opt-in HEIC support, off in the published image. Every HEIF decoder worth having is LGPL (libheif), which
# an Apache-2.0 image can't link, and HEVC carries patent-pool royalties regardless of the decoder's copyright
# licence. So Sidecar never links one: it runs whatever decoder it finds as a separate program, the boundary
# NOTICE already documents for ExifTool. Building with `--build-arg HEIF=1` installs one and accepts both
# consequences for that build. See docs/ROADMAP.md, "Editing non-RAW images".
ARG HEIF=0
RUN if [ "$HEIF" = "1" ]; then \
      apt-get update && apt-get install -y --no-install-recommends libheif-examples \
   && rm -rf /var/lib/apt/lists/*; \
    fi
COPY --from=build /usr/local/bin/sidecar /usr/local/bin/sidecar
COPY --from=build /src/web/build /usr/share/sidecar/web
COPY LICENSE NOTICE THIRD_PARTY_NOTICES.md /usr/share/doc/sidecar/
COPY third_party/libraw/LICENSE.CDDL /usr/share/doc/sidecar/LibRaw-LICENSE.CDDL
COPY assets/icc/LICENSE /usr/share/doc/sidecar/Compact-ICC-Profiles-LICENSE
ENV NVIDIA_DRIVER_CAPABILITIES=compute,utility,graphics
USER sidecar
ENTRYPOINT ["sidecar"]
CMD ["api", "--config", "/etc/sidecar.toml"]

# The same image with the models, for subject masks, faces, hair and skin, on a GPU with the memory for them
# (about 4 GB). sidecar-ml runs as its own process, started for a task and gone when idle; the editor tools
# that ask for it are on the roadmap, so for now this image behaves like the plain one.
FROM runtime AS ml
COPY --from=build-ml /usr/local/bin/sidecar-ml /usr/local/bin/sidecar-ml
COPY --from=build-ml /models /usr/share/sidecar/models
LABEL org.opencontainers.image.description="A RAW editor beside Immich, with its machine-learning models"

# Either image plus the mobile module, served at /m: developing on the phone's own GPU, with the server as the
# fallback and the library. The api serves /m only where this directory exists, so the plain image has none.
FROM runtime AS mobile
COPY --from=build-mobile /src/mobile/build /usr/share/sidecar/mobile
LABEL org.opencontainers.image.description="A RAW editor beside Immich, with the mobile module"

FROM ml AS ml-mobile
COPY --from=build-mobile /src/mobile/build /usr/share/sidecar/mobile
LABEL org.opencontainers.image.description="A RAW editor beside Immich, with its machine-learning models and the mobile module"

# The default target is the image without any of them.
FROM runtime
