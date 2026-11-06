#!/usr/bin/env bash
# V2's shell experiment: run the probe (or Sidecar's parity page) in Electron, in a real window.
#   dev/shell-probe/run.sh <name> <url> [switches] [extra main.js args]
# On a desktop it uses your display; with none (a server), it starts Xvfb. Results land in
# testdata/cache/shell-probe/<name>.json. Inside the dev container: ./dev/run dev/shell-probe/run.sh …
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
name="$1"; url="$2"; switches="${3:-}"; shift $(( $# < 3 ? $# : 3 ))
out="$here/../../testdata/cache/shell-probe"
mkdir -p "$out"
cd "$here"
[ -d node_modules/electron ] || npm install --no-audit --no-fund >/dev/null
[ -f node_modules/electron/path.txt ] || node node_modules/electron/install.js
if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ]; then
  # Xvfb picks a free display and says which, so runs back to back never race for one.
  exec 3< <(Xvfb -displayfd 1 -screen 0 1280x1024x24 -nolisten tcp 2>/dev/null)
  read -r -t 10 display <&3
  trap 'pkill -f "Xvfb -displayfd" 2>/dev/null' EXIT
  export DISPLAY=":$display"
fi
# --no-sandbox only because the dev container runs as root; a shipped app keeps the sandbox.
rm -f "$out/$name.json"
timeout 300 ./node_modules/.bin/electron --no-sandbox --enable-logging=stderr ${CHROMIUM_FLAGS:-} main.js --url="$url" --out="$out/$name.json" \
  --switches="$switches" "$@" >"$out/$name.log" 2>&1 || echo "electron exited with $?" >>"$out/$name.log"
[ -f "$out/$name.json" ] && echo "wrote $out/$name.json" || { echo "no result: Electron did not finish" >&2; exit 1; }
