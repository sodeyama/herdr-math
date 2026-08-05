#!/usr/bin/env bash
#
# smoke-agent-allowlist: end-to-end check of `tmath agent-enable` /
# `agent-disable` / `agent-allowed` against a temporary HOME, so the real
# allowlist file is never touched. Covers AT-2-812 (directory matching,
# including subdirectory match and sibling-directory rejection) and AT-2-813
# (enable/disable idempotency).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMATH="$ROOT/target/debug/tmath"

if [ ! -x "$TMATH" ]; then
  (cd "$ROOT" && cargo build --workspace >/dev/null)
fi

TMP_HOME="$(mktemp -d)"
cleanup() { rm -rf "$TMP_HOME"; }
trap cleanup EXIT

TARGET_DIR="$TMP_HOME/proj"
SUBDIR="$TARGET_DIR/src/deep"
SIBLING_DIR="$TMP_HOME/proj2"
mkdir -p "$SUBDIR" "$SIBLING_DIR"

run_tmath() {
  HOME="$TMP_HOME" "$TMATH" "$@"
}

pass=1
check() {
  local desc="$1" got="$2" want="$3"
  if [ "$got" = "$want" ]; then
    echo "ok   - $desc"
  else
    echo "FAIL - $desc (got $got, want $want)"
    pass=0
  fi
}

# --- AT-2-812: not allowed before enable -------------------------------------
set +e
run_tmath agent-allowed "$TARGET_DIR" >/dev/null 2>&1
check "denied before enable" "$?" 1
set -e

# --- enable, then exact match, subdirectory match, sibling rejection --------
run_tmath agent-enable "$TARGET_DIR" >/dev/null

set +e
run_tmath agent-allowed "$TARGET_DIR" >/dev/null 2>&1
check "allowed: exact directory" "$?" 0

run_tmath agent-allowed "$SUBDIR" >/dev/null 2>&1
check "allowed: subdirectory" "$?" 0

run_tmath agent-allowed "$SIBLING_DIR" >/dev/null 2>&1
check "denied: sibling directory with name prefix" "$?" 1
set -e

# agent-allowed must stay silent on both streams (hot path, called on every
# wrapped launch).
OUT="$(run_tmath agent-allowed "$TARGET_DIR" 2>&1 || true)"
check "agent-allowed prints nothing" "${#OUT}" 0

# --- AT-2-813: enable is idempotent (no duplicate allowlist entries) --------
run_tmath agent-enable "$TARGET_DIR" >/dev/null
ENTRY_COUNT="$(grep -c -F "$(cd "$TARGET_DIR" && pwd)" "$TMP_HOME/.config/tmath/agent-allowlist")"
check "enable is idempotent (one entry)" "$ENTRY_COUNT" 1

# --- AT-2-813: disable removes only the target, is a no-op when absent -----
run_tmath agent-enable "$SIBLING_DIR" >/dev/null
run_tmath agent-disable "$TARGET_DIR" >/dev/null

set +e
run_tmath agent-allowed "$TARGET_DIR" >/dev/null 2>&1
check "denied after disable" "$?" 1
run_tmath agent-allowed "$SIBLING_DIR" >/dev/null 2>&1
check "sibling entry untouched by disable" "$?" 0

run_tmath agent-disable "$TARGET_DIR" >/dev/null 2>&1
check "disabling an unregistered directory is a no-op success" "$?" 0
set -e

# --- AT-R-201/202: wrapper distinguishes policy from breakage ----------------
WRAPPER_SH="$ROOT/scripts/shell/tmath-agent.sh"
STUB_DIR="$TMP_HOME/wrapper-stub-bin"
mkdir -p "$STUB_DIR"

write_stub_tmath() {
  local exit_code="$1"
  printf '%s\n' '#!/bin/sh' "exit $exit_code" > "$STUB_DIR/tmath"
  chmod 755 "$STUB_DIR/tmath"
}

write_stub_tmath 137
STDERR_FILE="$TMP_HOME/wrapper-137.stderr"
set +e
OUT="$(PATH="$STUB_DIR:$PATH" bash -c "source '$WRAPPER_SH' && __tmath_wrap_agent /bin/echo hello world" 2>"$STDERR_FILE")"
WRAPPER_RC=$?
set -e

check "AT-R-202: wrapped stdout unchanged (stub exit 137)" "$OUT" "hello world"
check "AT-R-202: wrapper rc equals wrapped command (stub exit 137)" "$WRAPPER_RC" 0
STDERR_137="$(cat "$STDERR_FILE")"
STDERR_137_LINES="$([ -z "$STDERR_137" ] && echo 0 || printf '%s\n' "$STDERR_137" | wc -l | tr -d ' ')"
check "AT-R-202: stderr is exactly one line (stub exit 137)" "$STDERR_137_LINES" 1
STDERR_137_MATCH="$(printf '%s\n' "$STDERR_137" | grep -c 'agent-allowed failed (exit 137)' || true)"
check "AT-R-202: stderr matches failure line (stub exit 137)" "$STDERR_137_MATCH" 1

write_stub_tmath 1
STDERR_FILE="$TMP_HOME/wrapper-1.stderr"
set +e
OUT="$(PATH="$STUB_DIR:$PATH" bash -c "source '$WRAPPER_SH' && __tmath_wrap_agent /bin/echo hello world" 2>"$STDERR_FILE")"
WRAPPER_RC=$?
set -e

check "AT-R-201: silent passthrough stdout (stub exit 1)" "$OUT" "hello world"
check "AT-R-201: silent passthrough rc (stub exit 1)" "$WRAPPER_RC" 0
STDERR_1="$(cat "$STDERR_FILE")"
check "AT-R-201: silent passthrough stderr empty (stub exit 1)" "${#STDERR_1}" 0

if [ "$pass" = 1 ]; then
  echo "PASS: allowlist enable/disable/allowed behave per AT-2-812/AT-2-813; wrapper per AT-R-201/AT-R-202"
  exit 0
fi
exit 1
