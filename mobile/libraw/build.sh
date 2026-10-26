#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# LibRaw and the server's shim, built for the browser with Emscripten (docs/MOBILE.md, stage 2). Writes
# mobile/static/libraw/sidecar_libraw.{js,wasm}, which are build products and not committed. The module serves them
# at /m/libraw/, and the editor's RAW worker (web/src/lib/local/raw.worker.ts) loads them from there at run time.
#
# The object list is LibRaw's own (LIB_OBJECTS in third_party/libraw/Makefile.dist), as crates/libraw-sys/build.rs
# reads it for the server. LibRaw is used under CDDL-1.0, as the server uses it. Needs `emcc` on PATH: the
# Dockerfile installs a pinned emsdk; locally, `source <emsdk>/emsdk_env.sh` first.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
libraw="$root/third_party/libraw"
shim="$root/crates/libraw-sys/shim"
out="$root/mobile/static/libraw"
obj="${SIDECAR_LIBRAW_WASM_OBJ:-$root/mobile/libraw/.obj}"
mkdir -p "$out" "$obj"

objects=$(sed -n '/^LIB_OBJECTS=/,/^$/p' "$libraw/Makefile.dist" | tr ' \\' '\n\n' | sed -n 's#^object/\(.*\)\.o$#\1#p')
[ "$(echo "$objects" | wc -l)" -gt 50 ] || { echo "unexpected LibRaw object list" >&2; exit 1; }

# zlib (deflate DNGs) and libjpeg (lossy DNGs, some Kodak) are left out for now: without them LibRaw reports
# those files unsupported, and the page hands them to the server.
cflags=(-O3 -fwasm-exceptions -I"$libraw" -I"$shim" -DLIBRAW_NOTHREADS -w)

sources=()
for o in $objects; do sources+=("$(find "$libraw/src" -name "$o.cpp" | head -1)"); done
sources+=("$shim/shim.cpp" "$here/wasm_api.cpp")

# Compile in parallel, each source to its own object.
printf '%s\n' "${sources[@]}" | xargs -P "$(nproc)" -I{} sh -c \
  'f="$1"; shift; o="'"$obj"'/$(basename "$f" .cpp).o"; [ "$o" -nt "$f" ] || em++ "$@" -c "$f" -o "$o"' _ {} "${cflags[@]}"

exports='["_scw_open","_scw_strerror","_scw_info","_scw_mosaic","_scw_mosaic_data","_scw_close","_malloc","_free"]'
em++ -O3 -fwasm-exceptions "$obj"/*.o -o "$out/sidecar_libraw.js" \
  -sMODULARIZE=1 -sEXPORT_ES6=1 -sEXPORT_NAME=createLibRaw -sENVIRONMENT=web,worker \
  -sALLOW_MEMORY_GROWTH=1 -sINITIAL_MEMORY=64MB -sMAXIMUM_MEMORY=4GB -sFILESYSTEM=0 \
  -sEXPORTED_FUNCTIONS="$exports" -sEXPORTED_RUNTIME_METHODS='["HEAPU8","HEAPU16","UTF8ToString"]'
# Its types are web/src/lib/local/sidecar_libraw.d.ts; the file itself is served as it is, never bundled.
ls -l "$out"
