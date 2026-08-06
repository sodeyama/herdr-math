#!/usr/bin/env bash
#
# smoke-render-pipe: non-tty native render smoke (AT-3-802/803 helper).
#
# Pipes a bounded document to `tmath render -` and asserts the native stream
# completes successfully. Requires a built tmath binary (debug or release).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [ -n "${TMATH_BIN:-}" ]; then
  TMATH="$TMATH_BIN"
elif [ -x "$ROOT/target/release/tmath" ]; then
  TMATH="$ROOT/target/release/tmath"
elif [ -x "$ROOT/target/debug/tmath" ]; then
  TMATH="$ROOT/target/debug/tmath"
else
  (cd "$ROOT" && cargo build -p tmath >/dev/null)
  TMATH="$ROOT/target/debug/tmath"
fi

DOCUMENT=$'# Pipe smoke\n\nInline $E=mc^2$ and a list:\n\n- one\n- two\n'

OUTPUT="$(printf '%s' "$DOCUMENT" | "$TMATH" render - 2>&1)" || {
  echo "FAIL: tmath render - exited non-zero" >&2
  printf '%s\n' "$OUTPUT" >&2
  exit 1
}

if ! printf '%s\n' "$OUTPUT" | grep -q '^event=append id='; then
  echo "FAIL: no append event in output" >&2
  printf '%s\n' "$OUTPUT" >&2
  exit 1
fi

if ! printf '%s\n' "$OUTPUT" | grep -q '^event=done '; then
  echo "FAIL: no done event in output" >&2
  printf '%s\n' "$OUTPUT" >&2
  exit 1
fi

echo "PASS: native pipe render ($(printf '%s' "$DOCUMENT" | wc -c | tr -d ' ') byte document)"
