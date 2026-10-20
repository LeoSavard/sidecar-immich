#!/usr/bin/env bash
# Generate real, controlled Ultra HDR JPEGs (ISO 21496-1 / Adobe Gain Map format) to measure on an actual HDR
# screen with a colorimeter — the ground-truth check docs/ROADMAP.md's "HDR originals keep their gain map"
# needs before `sidecar_raw::gain_map` can implement a reconstruction formula, instead of guessing one.
#   ./dev/hdr-gainmap-probe/generate.sh [out-dir]      default: dev/hdr-gainmap-probe/out
#
# Builds Google's libultrahdr (dual MIT/Apache-2.0 — the reference encoder/decoder Android and Chrome use,
# https://github.com/google/libultrahdr) from source in a throwaway container, and uses its "encode scenario
# 4" (pre-compressed SDR + pre-compressed gain map + explicit metadata) to build each patch from values we
# choose completely, rather than deriving a gain map from a real HDR/SDR pair. The maths it applies, read
# from `lib/src/gainmapmath.cpp`'s `applyGain`, is the open, documented one — not Apple's undocumented own:
#
#   gain = pixelValue / 255                              (gamma != 1 would reshape this; we always use 1)
#   logBoost = log2(minContentBoost)*(1-gain) + log2(maxContentBoost)*gain
#   HDR = (SDR + offsetSdr) * exp2(logBoost) - offsetHdr
#
# which, at minContentBoost=1 and both offsets=0 (every scenario below), is just HDR = SDR * headroom^gain —
# a grounded version of the "looks good" guesses in docs/ROADMAP.md, using a real per-photo headroom instead
# of a fixed constant. Measuring these patches tells us whether an iPhone's Safari actually renders this
# documented formula faithfully; if it does, and Apple's own HEIC `XMP-HDRGainMap:HDRGainMapHeadroom` behaves
# like `maxContentBoost` here, that's the formula stage 2 needs — checked, not assumed.
#
# Needs Docker and ImageMagick (`imagemagick` in the throwaway container, not the host). Writes `manifest.json`
# (every patch's exact parameters and the formula's own predicted linear multiplier) beside the generated
# JPEGs — `probe.html` and `analyze.py` both read it.
set -euo pipefail
out="$(cd "$(dirname "$0")/.." && pwd)/hdr-gainmap-probe/${1:-out}"
mkdir -p "$out"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

# One line per real (non-calibration) patch: id, sdr 8-bit grey level, gain 0-1, minContentBoost, maxContentBoost.
# minContentBoost is 1.0 throughout (no darkening half), so maxContentBoost is directly "headroom".
cat > "$scratch/patches.tsv" <<'TSV'
gain00-grey128-hr3.325	128	0.00	1.0	3.325
gain25-grey128-hr3.325	128	0.25	1.0	3.325
gain50-grey128-hr3.325	128	0.50	1.0	3.325
gain75-grey128-hr3.325	128	0.75	1.0	3.325
gain100-grey128-hr3.325	128	1.00	1.0	3.325
gain100-grey128-hr1.5	128	1.00	1.0	1.5
gain100-grey128-hr2.0	128	1.00	1.0	2.0
gain100-grey128-hr5.0	128	1.00	1.0	5.0
gain100-grey32-hr3.325	32	1.00	1.0	3.325
gain100-grey224-hr3.325	224	1.00	1.0	3.325
TSV

cat > "$scratch/entrypoint.sh" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq cmake build-essential libjpeg62-turbo-dev ca-certificates git imagemagick >/dev/null 2>&1
git clone --depth 1 https://github.com/google/libultrahdr /scratch/libultrahdr >/dev/null 2>&1
cd /scratch/libultrahdr
mkdir build && cd build
cmake -DUHDR_BUILD_EXAMPLES=1 -DUHDR_BUILD_TESTS=0 -DCMAKE_BUILD_TYPE=Release .. >/scratch/build.log 2>&1
make -j"$(nproc)" >>/scratch/build.log 2>&1
app="$(pwd)/ultrahdr_app"

work=/scratch/work
mkdir -p "$work" /out
manifest=/out/manifest.json
echo '[' > "$manifest"
first=1

emit() {
  [ "$first" = 1 ] && first=0 || echo ',' >> "$manifest"
  cat >> "$manifest"
}

while IFS=$'\t' read -r id sdr gain minb maxb; do
  d="$work/$id"; mkdir -p "$d"
  # Plain `xc:gray(N)` already writes N on every channel in a normal 8-bit RGB JPEG — exactly what's wanted.
  # `-colorspace` converts pixel values (it's not just a tag), so it's deliberately not used here.
  magick -size 64x64 "xc:gray($sdr)" "$d/sdr.jpg"
  gain8=$(awk -v g="$gain" 'BEGIN{printf "%d", g*255+0.5}')
  magick -size 64x64 "xc:gray($gain8)" "$d/gain.jpg"
  cat > "$d/metadata.cfg" <<CFG
--maxContentBoost $maxb $maxb $maxb
--minContentBoost $minb $minb $minb
--gamma 1.0 1.0 1.0
--offsetSdr 0.0 0.0 0.0
--offsetHdr 0.0 0.0 0.0
--hdrCapacityMin $minb
--hdrCapacityMax $maxb
--useBaseColorSpace 1
CFG
  "$app" -m 0 -i "$d/sdr.jpg" -g "$d/gain.jpg" -f "$d/metadata.cfg" -z "/out/$id.jpg" >/dev/null 2>&1
  # `applyGain` (gainmapmath.cpp) works on linear light, not the gamma-encoded 8-bit value: the decoder sRGB-
  # decodes the base image first. sdr8/255 alone is the *encoded* fraction, not the linear one — using it
  # directly here would silently predict the wrong number for every boosted (gain > 0) patch.
  predicted=$(awk -v s="$sdr" -v g="$gain" -v mn="$minb" -v mx="$maxb" \
    'BEGIN{
       v = s / 255.0
       linear = (v <= 0.04045) ? v / 12.92 : ((v + 0.055) / 1.055) ^ 2.4
       boost = mn^(1-g) * mx^g
       printf "%.6f", linear * boost
     }')
  emit <<JSON
  {
    "id": "$id",
    "file": "$id.jpg",
    "label": "sdr=$sdr/255 gain=$gain minBoost=$minb maxBoost(headroom)=$maxb",
    "sdr8": $sdr,
    "gain": $gain,
    "minContentBoost": $minb,
    "maxContentBoost": $maxb,
    "predictedLinearHdr": $predicted
  }
JSON
done < /scratch/patches.tsv

# Calibration patches: plain flat sRGB colours, no gain map, nothing to predict beyond "this 8-bit value".
for cal in black:0,0,0 white:255,255,255 grey18:46,46,46 grey50:128,128,128 red:255,0,0 green:0,255,0 blue:0,0,255; do
  name="${cal%%:*}"; rgb="${cal#*:}"
  emit <<JSON
  {
    "id": "cal-$name",
    "calibration": true,
    "rgb": [${rgb//,/, }],
    "label": "calibration: $name ($rgb)"
  }
JSON
done

echo ']' >> "$manifest"
# The container runs as root, so /scratch (libultrahdr's clone and build) and /out are root-owned on the host
# too; hand both back to the invoking user or the outer script's cleanup trap can't remove /scratch.
chown -R "${1:-0}:${2:-0}" /out /scratch
SCRIPT
chmod +x "$scratch/entrypoint.sh"

uid="$(id -u)"; gid="$(id -g)"
docker run --rm --security-opt label=disable \
  -v "$scratch":/scratch \
  -v "$out":/out \
  sidecar-dev bash /scratch/entrypoint.sh "$uid" "$gid"

echo "Wrote $(find "$out" -name '*.jpg' | wc -l) patches and manifest.json to $out"
