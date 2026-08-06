#!/usr/bin/env bash
#
# smoke-install-no-node: AT-3-802 install without Node.js on PATH.
#
# Runs scripts/install.sh from a checkout inside a temporary HOME with an
# empty PATH prefix (no node/npm), then smoke-renders through the installed
# binary.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d)"
EMPTY_BIN="$TMP/empty-bin"
INSTALL_ROOT="$TMP/tmath-app"
BIN_HOME="$TMP/bin"

cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

mkdir -p "$EMPTY_BIN" "$BIN_HOME" "$INSTALL_ROOT/bin"

if ! command -v cargo >/dev/null; then
  echo "FAIL: cargo required for install smoke" >&2
  exit 1
fi

# Pre-build so install reuses the artifact (install always needs cargo).
if [ ! -x "$ROOT/target/release/tmath" ]; then
  (cd "$ROOT" && cargo build --release -p tmath)
fi

export HOME="$TMP/home"
export XDG_BIN_HOME="$BIN_HOME"
export TMATH_INSTALL_ROOT="$INSTALL_ROOT"
export TMATH_SKIP_SHELL_INTEGRATION=1
export TMATH_SKIP_TESTS=1
export TMATH_FORCE_REBUILD=0
export PATH="$EMPTY_BIN:$(dirname "$(command -v cargo)"):/usr/bin:/bin"

bash "$ROOT/scripts/install.sh"

INSTALLED="$INSTALL_ROOT/app/bin/tmath"
if [ ! -x "$INSTALLED" ]; then
  echo "FAIL: installed binary missing at $INSTALLED" >&2
  exit 1
fi

TMATH_BIN="$INSTALLED" "$ROOT/scripts/smoke-render-pipe.sh"
echo "PASS: install without Node.js and pipe render smoke"
