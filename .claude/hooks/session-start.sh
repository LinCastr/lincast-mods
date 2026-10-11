#!/bin/bash
# Cloud sessions only: install the system libraries the build needs and prefetch crates.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

# keyring (sync-secret-service) links against libdbus.
if ! pkg-config --exists dbus-1 2>/dev/null; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get install -y -q libdbus-1-dev pkg-config >/dev/null 2>&1 \
    || { apt-get update -q >/dev/null && apt-get install -y -q libdbus-1-dev pkg-config >/dev/null; }
fi

cd "$CLAUDE_PROJECT_DIR"
cargo fetch --quiet
