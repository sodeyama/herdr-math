#!/usr/bin/env bash
#
# smoke-install-shell-integration: verifies the rc-editing block that
# scripts/install.sh appends to ~/.zshrc / ~/.bashrc is idempotent, replaces
# stale content in place, preserves unrelated lines, and honors
# TMATH_SKIP_SHELL_INTEGRATION=1. Runs against a temporary HOME only; never
# touches the real shell rc files. Covers AT-2-814.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

TMP_HOME="$(mktemp -d)"
cleanup() { rm -rf "$TMP_HOME"; }
trap cleanup EXIT

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

# Extract the install_shell_integration function and its invocation from
# install.sh so this test exercises the real logic, not a reimplementation.
SNIPPET="$TMP_HOME/shell-integration-snippet.sh"
awk '/^TMATH_SHELL_SNIPPET_PATH=/,/^fi$/' "$ROOT/scripts/install.sh" > "$SNIPPET"
if [ ! -s "$SNIPPET" ]; then
  echo "FAIL - could not extract install_shell_integration from install.sh"
  exit 1
fi

APP="$TMP_HOME/app"
mkdir -p "$APP/shell"
touch "$APP/shell/tmath-agent.sh"

cat > "$TMP_HOME/.zshrc" <<'EOF'
# existing user content before
export FOO=bar
EOF
cp "$TMP_HOME/.zshrc" "$TMP_HOME/.bashrc"

run_install_shell_integration() {
  ( HOME="$TMP_HOME" APP="$APP" source "$SNIPPET" )
}

# --- first run: adds exactly one marker block, preserves existing content --
run_install_shell_integration >/dev/null
MARK_COUNT="$(grep -c 'tmath shell integration >>>' "$TMP_HOME/.zshrc")"
check "first run adds one marker block (zshrc)" "$MARK_COUNT" 1
check "existing content preserved" "$(head -1 "$TMP_HOME/.zshrc")" "# existing user content before"
check "bashrc also got one marker block" \
  "$(grep -c 'tmath shell integration >>>' "$TMP_HOME/.bashrc")" 1

# --- second run with the same APP: still exactly one block (idempotent) ----
run_install_shell_integration >/dev/null
MARK_COUNT="$(grep -c 'tmath shell integration >>>' "$TMP_HOME/.zshrc")"
check "second run stays idempotent (still one block)" "$MARK_COUNT" 1

# --- stale block content is replaced in place, unrelated lines kept --------
sed -i '' 's#'"$APP"'/shell/tmath-agent.sh#/stale/path/tmath-agent.sh#g' "$TMP_HOME/.zshrc" 2>/dev/null \
  || sed -i 's#'"$APP"'/shell/tmath-agent.sh#/stale/path/tmath-agent.sh#g' "$TMP_HOME/.zshrc"
printf '\n# user content appended after\n' >> "$TMP_HOME/.zshrc"

run_install_shell_integration >/dev/null
MARK_COUNT="$(grep -c 'tmath shell integration >>>' "$TMP_HOME/.zshrc")"
check "stale block replaced, still one block" "$MARK_COUNT" 1
STALE_COUNT="$(grep -c '/stale/path/tmath-agent.sh' "$TMP_HOME/.zshrc" || true)"
check "stale path no longer present" "$STALE_COUNT" 0
check "content after the block survives replacement" \
  "$(grep -c '# user content appended after' "$TMP_HOME/.zshrc")" 1
check "content before the block still survives" \
  "$(grep -c '# existing user content before' "$TMP_HOME/.zshrc")" 1

# --- TMATH_SKIP_SHELL_INTEGRATION=1 leaves the rc files untouched ----------
cp "$TMP_HOME/.zshrc" "$TMP_HOME/.zshrc.before-skip"
( HOME="$TMP_HOME" APP="$APP" TMATH_SKIP_SHELL_INTEGRATION=1 source "$SNIPPET" ) >/dev/null
if diff -q "$TMP_HOME/.zshrc.before-skip" "$TMP_HOME/.zshrc" >/dev/null; then
  check "TMATH_SKIP_SHELL_INTEGRATION=1 leaves rc untouched" "unchanged" "unchanged"
else
  check "TMATH_SKIP_SHELL_INTEGRATION=1 leaves rc untouched" "changed" "unchanged"
fi

if [ "$pass" = 1 ]; then
  echo "PASS: shell rc integration is idempotent and replace-in-place per AT-2-814"
  exit 0
fi
exit 1
