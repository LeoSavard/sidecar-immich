# Sourced by dev/run and dev/serve: who this checkout is, and where its build and state go in the cache volume.
#   SIDECAR_WORKTREE    "main" for the primary checkout, else the worktree's directory name
#   SIDECAR_TARGET_DIR  cargo's target directory inside the container (override to share one deliberately)
#   SIDECAR_STATE_DIR   the dev server's sessions and asset records
if [ "$(git -C "$here" rev-parse --path-format=absolute --git-dir)" = "$(git -C "$here" rev-parse --path-format=absolute --git-common-dir)" ]; then
  SIDECAR_WORKTREE=main
  : "${SIDECAR_TARGET_DIR:=/cache/target}"
  : "${SIDECAR_STATE_DIR:=/cache/sidecar-dev/state}"
else
  SIDECAR_WORKTREE="$(basename "$here" | tr -c 'A-Za-z0-9_.\n-' '-')"
  : "${SIDECAR_TARGET_DIR:=/cache/wt/$SIDECAR_WORKTREE/target}"
  : "${SIDECAR_STATE_DIR:=/cache/wt/$SIDECAR_WORKTREE/state}"
fi
# The primary checkout, where ./dev/ml-models leaves the model weights every worktree can share.
SIDECAR_MAIN_CHECKOUT="$(cd "$(git -C "$here" rev-parse --path-format=absolute --git-common-dir)/.." && pwd)"
