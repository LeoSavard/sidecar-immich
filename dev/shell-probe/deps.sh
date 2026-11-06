#!/usr/bin/env bash
# What Electron and the Wayland run need beyond the dev image (Debian trixie). The dev container is thrown
# away after each ./dev/run, so the experiment installs these per run rather than growing the image for it.
set -euo pipefail
command -v weston >/dev/null && ldconfig -p | grep -q libgtk-3 && exit 0
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
  libgtk-3-0t64 libnss3 libasound2t64 libgbm1 libxss1 libxtst6 weston >/dev/null
