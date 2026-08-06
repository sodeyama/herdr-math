#!/usr/bin/env bash
#
# smoke-footprint: AT-3-801 release binary size and dynamic-link audit.
#
# Builds (or reuses) target/release/tmath, asserts the artifact is ≤ 60 MiB,
# and rejects unexpected Node/browser dynamic dependencies.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="$ROOT/target/release/tmath"
MAX_BYTES=$((60 * 1024 * 1024))

fail() {
  echo "FAIL - $1" >&2
  exit 1
}

if [ ! -x "$BINARY" ]; then
  echo "building release binary…"
  (cd "$ROOT" && cargo build --release -p tmath)
fi

if [ ! -x "$BINARY" ]; then
  fail "release binary missing at $BINARY"
fi

if command -v stat >/dev/null 2>&1; then
  if stat -f%z "$BINARY" >/dev/null 2>&1; then
    SIZE="$(stat -f%z "$BINARY")"
  else
    SIZE="$(stat -c%s "$BINARY")"
  fi
else
  fail "stat unavailable"
fi

if [ "$SIZE" -gt "$MAX_BYTES" ]; then
  fail "binary size ${SIZE} bytes exceeds ${MAX_BYTES} byte (60 MiB) cap"
fi

case "$(uname -s)" in
  Darwin)
    DEPS="$(otool -L "$BINARY")"
    if printf '%s\n' "$DEPS" | grep -qiE 'node|chromium|playwright'; then
      fail "unexpected dynamic dependency:\n$DEPS"
    fi
    ;;
  Linux)
    if command -v ldd >/dev/null 2>&1; then
      DEPS="$(ldd "$BINARY" 2>&1 || true)"
      if printf '%s\n' "$DEPS" | grep -qiE 'node|chromium|playwright'; then
        fail "unexpected dynamic dependency:\n$DEPS"
      fi
    fi
    ;;
esac

HUMAN="$(awk -v s="$SIZE" 'BEGIN { printf "%.1f MiB", s / (1024 * 1024) }')"
echo "PASS: release binary ${HUMAN} (${SIZE} bytes), dynamic deps clean"
